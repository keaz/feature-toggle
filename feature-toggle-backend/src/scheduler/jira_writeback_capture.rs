//! Source 2 of the Jira write-back outbox (JI-43): reads `activity_log` with a
//! cursor and enqueues comment and remote link jobs for every change of a linked
//! feature. Source 1 (the inbound event comment) is in `rest/jira_events.rs`.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use chrono::{DateTime, Utc};
use log::{error, warn};
use sqlx::{Acquire, PgConnection, PgPool};
use tokio::time;
use uuid::Uuid;

use crate::Error;
use crate::database::activity_log::{
    ActivityLogRepository, ActivityLogRow, activity_log_repository,
};
use crate::database::external_link::{ExternalLinkRepository, external_link_repository};
use crate::database::jira_integration::{JiraIntegrationRepository, jira_integration_repository};
use crate::database::jira_outbound_job::{
    JiraOutboundJobRepositoryTx, jira_outbound_job_repository_tx,
};
use crate::database::user::{UserRepository, user_repository};
use crate::logic::jira_capture::{
    CAPTURE_TYPES, CaptureContext, WritebackTarget, is_capturable, is_link_row,
    link_row_issue_keys, plan_jobs, row_feature_id,
};

const BATCH_SIZE: i64 = 500;
/// Rows that committed late are still read: the window starts this long before the cursor.
const OVERLAP_SECONDS: i64 = 60;
/// Rows younger than this are left for the next tick.
const LAG_SECONDS: i64 = 2;

/// What the capture knows about a feature's team and links, for one run.
#[derive(Clone)]
struct FeatureInfo {
    team_id: Uuid,
    key: String,
    jira_keys: Vec<String>,
}

/// Per-run caches. Nothing outlives a run.
#[derive(Default)]
struct Caches {
    features: HashMap<Uuid, Option<FeatureInfo>>,
    targets: HashMap<Uuid, Vec<WritebackTarget>>,
    names: HashMap<Uuid, Option<String>>,
    jira_actor_ids: Option<HashSet<Uuid>>,
}

pub struct JiraWritebackCapture {
    pool: PgPool,
    activity: Box<dyn ActivityLogRepository>,
    links: Box<dyn ExternalLinkRepository>,
    integrations: Box<dyn JiraIntegrationRepository>,
    users: Box<dyn UserRepository>,
    interval: Duration,
    lag: chrono::Duration,
}

impl JiraWritebackCapture {
    pub fn new(pool: PgPool, interval: Duration) -> Self {
        Self {
            activity: activity_log_repository(pool.clone()),
            links: external_link_repository(pool.clone()),
            integrations: jira_integration_repository(pool.clone()),
            users: user_repository(pool.clone()),
            pool,
            interval,
            lag: chrono::Duration::seconds(LAG_SECONDS),
        }
    }

    /// Reads rows up to `lag` before now (tests use zero).
    pub fn with_lag(mut self, lag: chrono::Duration) -> Self {
        self.lag = lag;
        self
    }

    pub async fn run(self) {
        let mut ticker = time::interval(self.interval);
        ticker.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            if let Err(err) = self.run_once().await {
                error!("Jira write-back capture failed: {err}");
            }
        }
    }

    /// Enqueues the jobs of the activity rows since the cursor; returns how many
    /// jobs were offered to the outbox (a duplicate counts, it is a no-op there).
    pub async fn run_once(&self) -> Result<usize, Error> {
        let mut conn = self.pool.acquire().await.map_err(Error::DatabaseError)?;
        let Some(cursor) = read_cursor(&mut conn).await? else {
            // First run: start at now, no backfill of older activity.
            sqlx::query(
                "INSERT INTO jira_writeback_cursor (id, last_created_at) VALUES (TRUE, now()) \
                 ON CONFLICT DO NOTHING",
            )
            .execute(&mut *conn)
            .await
            .map_err(Error::DatabaseError)?;
            return Ok(0);
        };
        drop(conn);

        // The database clock, so app/DB clock skew cannot eat the overlap.
        let to: DateTime<Utc> = sqlx::query_scalar("SELECT now() - make_interval(secs => $1)")
            .bind(self.lag.num_milliseconds() as f64 / 1000.0)
            .fetch_one(&self.pool)
            .await
            .map_err(Error::DatabaseError)?;
        let from = cursor - chrono::Duration::seconds(OVERLAP_SECONDS);
        let mut after: Option<(DateTime<Utc>, Uuid)> = None;
        let mut caches = Caches::default();
        let mut offered = 0;
        loop {
            let rows = self
                .activity
                .list_window_after(CAPTURE_TYPES, from, to, after, BATCH_SIZE)
                .await
                .map_err(Error::DatabaseError)?;
            let Some(last) = rows.last() else {
                break;
            };
            // Keyset paging: the next batch starts after the last row of this one.
            after = Some((last.created_at, last.id));
            let newest = last.created_at;
            let full = rows.len() as i64 >= BATCH_SIZE;

            let mut planned = Vec::new();
            for row in rows.iter().filter(|row| is_capturable(row)) {
                planned.push((row.id, self.jobs_for(row, &mut caches).await?));
            }
            let mut tx = self.pool.begin().await.map_err(Error::DatabaseError)?;
            let outbox = jira_outbound_job_repository_tx(self.pool.clone());
            for (row_id, jobs) in planned {
                // One savepoint per row: a row whose jobs cannot be stored is skipped
                // and must not block the cursor.
                let mut savepoint = (&mut tx).begin().await.map_err(Error::DatabaseError)?;
                let mut count = 0;
                let mut failure = None;
                for job in jobs {
                    match outbox.enqueue_tx(&mut savepoint, job).await {
                        Ok(_) => count += 1,
                        Err(err) => {
                            failure = Some(err);
                            break;
                        }
                    }
                }
                match failure {
                    None => {
                        savepoint.commit().await.map_err(Error::DatabaseError)?;
                        offered += count;
                    }
                    Some(err) => {
                        warn!(
                            "Jira write-back capture skipped activity row {row_id}: {}",
                            error_kind(&err)
                        );
                        savepoint.rollback().await.map_err(Error::DatabaseError)?;
                    }
                }
            }
            // Never moves back: a window of only overlap rows leaves the cursor.
            sqlx::query(
                "UPDATE jira_writeback_cursor SET last_created_at = GREATEST(last_created_at, $1)",
            )
            .bind(newest)
            .execute(&mut *tx)
            .await
            .map_err(Error::DatabaseError)?;
            tx.commit().await.map_err(Error::DatabaseError)?;

            if !full {
                break;
            }
        }
        Ok(offered)
    }

    async fn jobs_for(
        &self,
        row: &ActivityLogRow,
        caches: &mut Caches,
    ) -> Result<Vec<crate::database::jira_outbound_job::NewOutboundJob>, Error> {
        let Some(feature_id) = row_feature_id(row) else {
            return Ok(Vec::new());
        };
        let removed = row.activity_type == "external_link_removed";
        let info = match self.feature_info(feature_id, caches).await? {
            Some(info) => info,
            // A removed link outlives its feature only in the row's metadata.
            None if removed => match removed_link_info(row) {
                Some(info) => info,
                None => return Ok(Vec::new()),
            },
            None => return Ok(Vec::new()),
        };
        let issue_keys = if is_link_row(row) {
            link_row_issue_keys(row)
        } else {
            info.jira_keys.clone()
        };
        if issue_keys.is_empty() {
            return Ok(Vec::new());
        }
        let integrations = self.targets(info.team_id, caches).await?;
        if integrations.is_empty() {
            return Ok(Vec::new());
        }
        let jira_actor_ids = self.jira_actor_ids(caches).await?;
        let actor_name = match (row.actor_name.as_deref(), row.actor_id) {
            (None | Some(""), Some(actor_id)) => self.user_name(actor_id, caches).await,
            _ => None,
        };
        let ctx = CaptureContext {
            feature_key: info.key,
            issue_keys,
            integrations,
            jira_actor_ids,
            actor_name,
        };
        Ok(plan_jobs(row, &ctx))
    }

    async fn feature_info(
        &self,
        feature_id: Uuid,
        caches: &mut Caches,
    ) -> Result<Option<FeatureInfo>, Error> {
        if let Some(cached) = caches.features.get(&feature_id) {
            return Ok(cached.clone());
        }
        let info = match self.links.feature_scope(feature_id).await? {
            Some(scope) => {
                let jira_keys = self
                    .links
                    .list_for_feature(feature_id)
                    .await?
                    .into_iter()
                    .filter(|link| link.system == "jira")
                    .map(|link| link.external_key)
                    .collect();
                Some(FeatureInfo {
                    team_id: scope.team_id,
                    key: scope.key,
                    jira_keys,
                })
            }
            None => None,
        };
        caches.features.insert(feature_id, info.clone());
        Ok(info)
    }

    /// Enabled integrations of the team with write-back on.
    async fn targets(
        &self,
        team_id: Uuid,
        caches: &mut Caches,
    ) -> Result<Vec<WritebackTarget>, Error> {
        if let Some(cached) = caches.targets.get(&team_id) {
            return Ok(cached.clone());
        }
        let targets: Vec<WritebackTarget> = self
            .integrations
            .list_for_team(team_id)
            .await?
            .into_iter()
            .filter(|integration| integration.enabled && integration.writeback_enabled)
            .map(|integration| WritebackTarget {
                integration_id: integration.id,
                comments: integration.writeback_comments,
                remote_link: integration.writeback_remote_link,
            })
            .collect();
        caches.targets.insert(team_id, targets.clone());
        Ok(targets)
    }

    /// `actor_user_id` of every Jira integration, any team, enabled or not.
    async fn jira_actor_ids(&self, caches: &mut Caches) -> Result<HashSet<Uuid>, Error> {
        if let Some(cached) = &caches.jira_actor_ids {
            return Ok(cached.clone());
        }
        let ids: Vec<Uuid> = sqlx::query_scalar("SELECT actor_user_id FROM jira_integrations")
            .fetch_all(&self.pool)
            .await
            .map_err(Error::DatabaseError)?;
        let ids: HashSet<Uuid> = ids.into_iter().collect();
        caches.jira_actor_ids = Some(ids.clone());
        Ok(ids)
    }

    async fn user_name(&self, user_id: Uuid, caches: &mut Caches) -> Option<String> {
        if let Some(cached) = caches.names.get(&user_id) {
            return cached.clone();
        }
        let name = match self.users.get_user_by_id(user_id).await {
            Ok(user) => {
                let full = format!("{} {}", user.first_name.trim(), user.last_name.trim());
                let full = full.trim();
                if !full.is_empty() {
                    Some(full.to_string())
                } else if !user.username.trim().is_empty() {
                    Some(user.username.trim().to_string())
                } else {
                    None
                }
            }
            Err(_) => None,
        };
        caches.names.insert(user_id, name.clone());
        name
    }
}

/// Team and key of a removed link whose feature is gone, from the row's metadata.
fn removed_link_info(row: &ActivityLogRow) -> Option<FeatureInfo> {
    let metadata = row.metadata.as_ref()?;
    let team_id = Uuid::parse_str(metadata.get("team_id")?.as_str()?).ok()?;
    let key = metadata.get("feature_key")?.as_str()?.to_string();
    Some(FeatureInfo {
        team_id,
        key,
        jira_keys: Vec::new(),
    })
}

async fn read_cursor(conn: &mut PgConnection) -> Result<Option<DateTime<Utc>>, Error> {
    sqlx::query_scalar("SELECT last_created_at FROM jira_writeback_cursor")
        .fetch_optional(conn)
        .await
        .map_err(Error::DatabaseError)
}

/// What failed, without any value from the row.
fn error_kind(err: &Error) -> String {
    match err {
        Error::DatabaseError(sqlx::Error::Database(db)) => {
            format!("database error {}", db.code().unwrap_or_default())
        }
        Error::DatabaseError(_) => "database error".to_string(),
        _ => "error".to_string(),
    }
}
