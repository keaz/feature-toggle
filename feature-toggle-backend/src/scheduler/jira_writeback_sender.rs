//! Sends the Jira write-back outbox (`jira_outbound_jobs`, JI-42): claims due jobs,
//! calls Jira with the integration's credential, retries with backoff and pauses an
//! integration when Jira rejects its credential.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use chrono::Utc;
use log::warn;
use sqlx::PgPool;
use tokio::time;
use uuid::Uuid;

use crate::Error;
use crate::config::JiraConfig;
use crate::database::entity::OutboundJobRow;
use crate::database::environment::{EnvironmentRepository, environment_repository};
use crate::database::external_link::{ExternalLinkRepository, external_link_repository};
use crate::database::feature::{FeatureRepository, feature_repository};
use crate::database::jira_integration::{JiraIntegrationRepository, jira_integration_repository};
use crate::database::jira_outbound_job::{
    JiraOutboundJobRepository, OutboundKind, jira_outbound_job_repository,
};
use crate::logic::jira_client::{JiraClient, JiraResponse, JiraTransportError, client_for};
use crate::logic::jira_writeback::{
    Outcome, classify, comment_body, error_note, remote_link_body, remote_link_global_id,
    remote_link_title,
};

const BATCH_SIZE: i64 = 50;
/// A job claimed by a tick that crashed becomes due again after this long.
const LEASE_MINUTES: i64 = 5;

/// Jobs settled by one [`JiraWritebackSender::run_once`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SendCounts {
    pub sent: u64,
    pub retried: u64,
    pub dead: u64,
    pub paused: u64,
}

/// How a job ended up, before it is stored.
enum Step {
    /// Nothing to send; the job is `sent` with this note.
    Skipped(&'static str),
    /// The job cannot ever succeed.
    Dead(&'static str),
    /// Jira's answer, or a transport failure.
    Called(Result<JiraResponse, JiraTransportError>),
}

/// What a tick needs of an integration: a ready client, or why there is none.
enum Access {
    Ready(JiraClient),
    /// Not sendable now. `Some(reason)` pauses the integration, `None` just releases
    /// the jobs (the integration changed after the claim).
    Blocked(Option<String>),
}

pub struct JiraWritebackSender {
    jobs: Box<dyn JiraOutboundJobRepository>,
    integrations: Box<dyn JiraIntegrationRepository>,
    links: Box<dyn ExternalLinkRepository>,
    features: Box<dyn FeatureRepository>,
    environments: Box<dyn EnvironmentRepository>,
    jira_config: JiraConfig,
    ui_base_url: Option<String>,
    interval: Duration,
}

impl JiraWritebackSender {
    pub fn new(
        pool: PgPool,
        jira_config: JiraConfig,
        ui_base_url: Option<String>,
        interval: Duration,
    ) -> Self {
        Self {
            jobs: jira_outbound_job_repository(pool.clone()),
            integrations: jira_integration_repository(pool.clone()),
            links: external_link_repository(pool.clone()),
            features: feature_repository(pool.clone()),
            environments: environment_repository(pool),
            jira_config,
            ui_base_url,
            interval,
        }
    }

    pub async fn run(self) {
        let mut ticker = time::interval(self.interval);
        ticker.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            self.run_once().await;
        }
    }

    /// Claims due jobs and sends them. Within one issue, jobs go out in `created_at`
    /// order, and a retry holds back the rest of the issue's jobs.
    pub async fn run_once(&self) -> SendCounts {
        let mut counts = SendCounts::default();
        let mut claimed = match self
            .jobs
            .claim_due(BATCH_SIZE, chrono::Duration::minutes(LEASE_MINUTES))
            .await
        {
            Ok(claimed) => claimed,
            Err(err) => {
                warn!("Jira write-back sender could not claim jobs: {err}");
                return counts;
            }
        };
        claimed.sort_by_key(|job| (job.created_at, job.id));
        let mut groups: BTreeMap<(Uuid, String), Vec<OutboundJobRow>> = BTreeMap::new();
        for job in claimed {
            groups
                .entry((job.integration_id, job.issue_key.clone()))
                .or_default()
                .push(job);
        }

        // One credential decryption per integration per tick; nothing outlives it.
        let mut access: HashMap<Uuid, Access> = HashMap::new();
        for ((integration_id, _), jobs) in groups {
            if let Entry::Vacant(slot) = access.entry(integration_id) {
                slot.insert(self.access_for(integration_id).await);
            }
            let Access::Ready(client) = &access[&integration_id] else {
                if let Access::Blocked(Some(reason)) = &access[&integration_id] {
                    self.pause(integration_id, reason.clone()).await;
                    counts.paused += 1;
                    // Pause once: later groups of the integration only release.
                    access.insert(integration_id, Access::Blocked(None));
                }
                self.release(&jobs, Utc::now()).await;
                continue;
            };
            let mut remaining = jobs.into_iter();
            while let Some(job) = remaining.next() {
                match self.send(client, &job).await {
                    Settled::Sent => counts.sent += 1,
                    Settled::Dead => counts.dead += 1,
                    Settled::Retry(next_attempt_at) => {
                        counts.retried += 1;
                        let rest: Vec<_> = remaining.by_ref().collect();
                        self.release(&rest, next_attempt_at).await;
                        break;
                    }
                    Settled::Pause(reason) => {
                        counts.dead += 1;
                        counts.paused += 1;
                        self.pause(integration_id, reason).await;
                        let rest: Vec<_> = remaining.by_ref().collect();
                        self.release(&rest, Utc::now()).await;
                        access.insert(integration_id, Access::Blocked(None));
                        break;
                    }
                }
            }
        }
        counts
    }

    async fn access_for(&self, integration_id: Uuid) -> Access {
        let row = match self.integrations.get(integration_id).await {
            Ok(Some(row)) => row,
            Ok(None) => return Access::Blocked(None),
            Err(err) => {
                warn!("Jira write-back sender could not load integration {integration_id}: {err}");
                return Access::Blocked(None);
            }
        };
        if !row.enabled || !row.writeback_enabled || row.writeback_paused_reason.is_some() {
            return Access::Blocked(None);
        }
        match client_for(&row, &self.jira_config) {
            Ok(Some(client)) => Access::Ready(client),
            Ok(None) => Access::Blocked(Some("write-back is not configured".to_string())),
            Err(Error::InvalidInput(message)) if message.contains("cannot be decrypted") => {
                Access::Blocked(Some("credential cannot be decrypted".to_string()))
            }
            Err(Error::InvalidInput(message)) => Access::Blocked(Some(message)),
            Err(err) => {
                warn!(
                    "Jira write-back sender could not build a client for {integration_id}: {err}"
                );
                Access::Blocked(None)
            }
        }
    }

    /// Its own UPDATE of `writeback_paused_reason`: a concurrent settings change
    /// cannot undo it.
    async fn pause(&self, integration_id: Uuid, reason: String) {
        if let Err(err) = self
            .integrations
            .set_writeback_paused(integration_id, Some(reason))
            .await
        {
            warn!("Jira write-back sender could not pause integration {integration_id}: {err}");
        }
    }

    async fn release(&self, jobs: &[OutboundJobRow], next_attempt_at: chrono::DateTime<Utc>) {
        let ids: Vec<Uuid> = jobs.iter().map(|job| job.id).collect();
        if let Err(err) = self.jobs.release(&ids, next_attempt_at).await {
            warn!("Jira write-back sender could not release jobs: {err}");
        }
    }

    async fn send(&self, client: &JiraClient, job: &OutboundJobRow) -> Settled {
        let kind = OutboundKind::parse(&job.kind);
        let step = match kind {
            None => Step::Dead("unknown job kind"),
            Some(kind) => self.step(client, job, kind).await,
        };
        let attempts_done = job.attempts + 1;
        let store = |result: Result<(), Error>| {
            if let Err(err) = result {
                warn!(
                    "Jira write-back sender could not store the result of job {}: {err}",
                    job.id
                );
            }
        };
        let result = match step {
            Step::Skipped(note) => {
                store(self.jobs.mark_sent(job.id, Some(note.to_string())).await);
                return Settled::Sent;
            }
            Step::Dead(reason) => {
                store(
                    self.jobs
                        .mark_dead(job.id, attempts_done, reason.to_string())
                        .await,
                );
                return Settled::Dead;
            }
            Step::Called(result) => result,
        };
        let note = match &result {
            Ok(response) => error_note(response, &client.secrets()),
            Err(err) => format!("transport error: {err}"),
        };
        // Status only: no URL, no headers, no body.
        let status = result.as_ref().map(|r| r.status).ok();
        let outcome = classify(
            kind.unwrap_or(OutboundKind::Comment),
            &result,
            attempts_done,
        );
        match outcome {
            Outcome::Sent => {
                store(self.jobs.mark_sent(job.id, None).await);
                Settled::Sent
            }
            Outcome::Retry { delay } => {
                let next_attempt_at = Utc::now() + delay;
                warn!(
                    "Jira write-back job {} (integration {}, issue {}) will retry, status {:?}",
                    job.id, job.integration_id, job.issue_key, status
                );
                store(
                    self.jobs
                        .mark_retry(job.id, attempts_done, next_attempt_at, note)
                        .await,
                );
                Settled::Retry(next_attempt_at)
            }
            Outcome::Dead => {
                warn!(
                    "Jira write-back job {} (integration {}, issue {}) is dead, status {:?}",
                    job.id, job.integration_id, job.issue_key, status
                );
                store(self.jobs.mark_dead(job.id, attempts_done, note).await);
                Settled::Dead
            }
            Outcome::PauseIntegration => {
                warn!(
                    "Jira write-back job {} (integration {}, issue {}) paused the integration, status {:?}",
                    job.id, job.integration_id, job.issue_key, status
                );
                store(self.jobs.mark_dead(job.id, attempts_done, note).await);
                Settled::Pause(format!(
                    "Jira returned {} at {}",
                    status.unwrap_or_default(),
                    Utc::now().to_rfc3339()
                ))
            }
        }
    }

    async fn step(&self, client: &JiraClient, job: &OutboundJobRow, kind: OutboundKind) -> Step {
        match kind {
            OutboundKind::Comment => {
                let lines: Option<Vec<String>> = job.payload["lines"].as_array().map(|lines| {
                    lines
                        .iter()
                        .filter_map(|line| line.as_str().map(str::to_string))
                        .collect()
                });
                match lines {
                    Some(lines) if !lines.is_empty() => Step::Called(
                        client
                            .add_comment(&job.issue_key, &comment_body(client.edition(), &lines))
                            .await,
                    ),
                    _ => Step::Dead("comment has no lines"),
                }
            }
            OutboundKind::RemoteLink => self.remote_link_step(client, job).await,
            OutboundKind::RemoteLinkDelete => {
                let feature_id = job.payload["featureId"]
                    .as_str()
                    .and_then(|id| Uuid::parse_str(id).ok());
                match feature_id {
                    Some(feature_id) => Step::Called(
                        client
                            .delete_remote_link(&job.issue_key, &remote_link_global_id(feature_id))
                            .await,
                    ),
                    None => Step::Dead("remote link delete has no featureId"),
                }
            }
        }
    }

    async fn remote_link_step(&self, client: &JiraClient, job: &OutboundJobRow) -> Step {
        let Some(feature_id) = job.feature_id else {
            return Step::Skipped("skipped: feature deleted");
        };
        // A database failure while preparing is retried like a Jira outage.
        let internal = || Step::Called(Err(JiraTransportError("internal error".to_string())));
        let scope = match self.links.feature_scope(feature_id).await {
            Ok(Some(scope)) => scope,
            Ok(None) => return Step::Skipped("skipped: feature deleted"),
            Err(_) => return internal(),
        };
        let linked = match self.links.list_for_feature(feature_id).await {
            Ok(links) => links.iter().any(|link| {
                link.system == "jira" && link.external_key.eq_ignore_ascii_case(&job.issue_key)
            }),
            Err(_) => return internal(),
        };
        if !linked {
            return Step::Skipped("skipped: issue no longer linked");
        }
        let Some(ui_base_url) = self.ui_base_url.as_deref() else {
            return Step::Dead("ui base URL is not configured");
        };
        let mut stages = match self.features.get_feature_stages(feature_id).await {
            Ok(stages) => stages,
            Err(_) => return internal(),
        };
        stages.sort_by_key(|stage| stage.order_index);
        let mut pairs = Vec::with_capacity(stages.len());
        for stage in stages {
            match self
                .environments
                .get_environment_by_id(stage.environment_id)
                .await
            {
                Ok(environment) => pairs.push((environment.name, stage.status)),
                Err(Error::NotFound(_)) => {}
                Err(_) => return internal(),
            }
        }
        let title = remote_link_title(&scope.key, &pairs);
        let url = format!("{ui_base_url}/features/{feature_id}");
        Step::Called(
            client
                .put_remote_link(&job.issue_key, &remote_link_body(feature_id, &title, &url))
                .await,
        )
    }
}

/// What happened to one job.
enum Settled {
    Sent,
    Dead,
    /// Stored for a retry at this time.
    Retry(chrono::DateTime<Utc>),
    /// The job is dead and the integration pauses for this reason.
    Pause(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::activity_log::activity_log_repository;
    use crate::database::init_pg_pool;
    use crate::database::jira_integration::jira_integration_repository_tx;
    use crate::database::jira_outbound_job::NewOutboundJob;
    use crate::logic::ActorContext;
    use crate::logic::jira_integration_tx::{JiraIntegrationInput, create_jira_integration_in_tx};
    use crate::logic::secret_box;
    use base64::Engine;
    use serde_json::json;
    use serial_test::serial;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const SEED_ADMIN_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const UI: &str = "https://flux.example.com";

    /// The encryption key is read once per process. Tests set a random one.
    fn ensure_encryption_key() {
        static SET: std::sync::Once = std::sync::Once::new();
        SET.call_once(|| {
            if std::env::var(secret_box::ENCRYPTION_KEY_ENV).is_err() {
                let mut key = Uuid::new_v4().as_bytes().to_vec();
                key.extend_from_slice(Uuid::new_v4().as_bytes());
                // SAFETY: runs once, before any sender test reads the variable.
                unsafe {
                    std::env::set_var(
                        secret_box::ENCRYPTION_KEY_ENV,
                        base64::engine::general_purpose::STANDARD.encode(key),
                    )
                };
            }
        });
    }

    struct Fixture {
        pool: PgPool,
        jira: MockServer,
        team_id: Uuid,
        integration_id: Uuid,
        shadow_user: Uuid,
        token: String,
        qa: Uuid,
        prod: Uuid,
    }

    impl Fixture {
        /// `cloud`: Cloud (Basic) instead of Data Center (Bearer).
        async fn new(cloud: bool) -> Self {
            ensure_encryption_key();
            let pool = init_pg_pool().await;
            let jira = MockServer::start().await;
            let team_id = Uuid::new_v4();
            sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, 'sender')")
                .bind(team_id)
                .bind(format!("jira-sender-{team_id}"))
                .execute(&pool)
                .await
                .expect("insert team");
            let mut envs = Vec::new();
            for name in ["QA", "Production"] {
                let id = Uuid::new_v4();
                sqlx::query(
                    "INSERT INTO environments (id, name, active, team_id, environment_type) \
                     VALUES ($1, $2, TRUE, $3, 'Development')",
                )
                .bind(id)
                .bind(name)
                .bind(team_id)
                .execute(&pool)
                .await
                .expect("insert environment");
                envs.push(id);
            }
            let repo = jira_integration_repository_tx(pool.clone());
            let activity = activity_log_repository(pool.clone());
            let mut tx = pool.begin().await.expect("begin");
            let created = create_jira_integration_in_tx(
                &mut tx,
                &repo,
                activity.as_ref(),
                team_id,
                JiraIntegrationInput {
                    name: "sender".to_string(),
                    jira_base_url: Some(jira.uri()),
                    environment_field: "labels".to_string(),
                    environment_aliases: Default::default(),
                    jira_approved_environment_ids: vec![],
                    feature_key_field: None,
                    enabled: true,
                },
                ActorContext::new(Uuid::parse_str(SEED_ADMIN_ID).unwrap(), "admin".to_string()),
            )
            .await
            .expect("create integration");
            tx.commit().await.expect("commit");
            let integration_id = created.integration.id;
            let token = format!("tok-{}", Uuid::new_v4());
            let sealed = secret_box::encrypt_with_aad(&token, integration_id.as_bytes())
                .expect("seal credential");
            sqlx::query(
                "UPDATE jira_integrations SET writeback_enabled = TRUE, jira_auth_kind = $2, \
                 jira_account_email = $3, jira_credential_enc = $4 WHERE id = $1",
            )
            .bind(integration_id)
            .bind(if cloud { "cloud_basic" } else { "dc_pat" })
            .bind(cloud.then_some("me@example.com"))
            .bind(sealed)
            .execute(&pool)
            .await
            .expect("configure write-back");
            Self {
                pool,
                jira,
                team_id,
                integration_id,
                shadow_user: created.integration.actor_user_id,
                token,
                qa: envs[0],
                prod: envs[1],
            }
        }

        fn sender(&self) -> JiraWritebackSender {
            self.sender_with_ui(Some(UI.to_string()))
        }

        fn sender_with_ui(&self, ui: Option<String>) -> JiraWritebackSender {
            JiraWritebackSender::new(
                self.pool.clone(),
                JiraConfig {
                    allow_insecure_http: true,
                    ui_base_url: None,
                },
                ui,
                Duration::from_secs(5),
            )
        }

        async fn enqueue(
            &self,
            issue: &str,
            kind: OutboundKind,
            payload: serde_json::Value,
        ) -> Uuid {
            self.enqueue_for(issue, kind, None, payload).await
        }

        async fn enqueue_for(
            &self,
            issue: &str,
            kind: OutboundKind,
            feature_id: Option<Uuid>,
            payload: serde_json::Value,
        ) -> Uuid {
            let id = Uuid::new_v4();
            let queued = jira_outbound_job_repository(self.pool.clone())
                .enqueue(NewOutboundJob {
                    integration_id: self.integration_id,
                    issue_key: issue.to_string(),
                    feature_id,
                    kind,
                    payload,
                    dedupe_key: format!("test:{id}"),
                })
                .await
                .expect("enqueue");
            assert!(queued);
            sqlx::query_scalar("SELECT id FROM jira_outbound_jobs WHERE dedupe_key = $1")
                .bind(format!("test:{id}"))
                .fetch_one(&self.pool)
                .await
                .expect("job id")
        }

        async fn comment(&self, issue: &str, text: &str) -> Uuid {
            self.enqueue(issue, OutboundKind::Comment, json!({"lines": [text]}))
                .await
        }

        async fn job(&self, id: Uuid) -> OutboundJobRow {
            let (rows, _) = jira_outbound_job_repository(self.pool.clone())
                .list(self.integration_id, None, 0, 200)
                .await
                .expect("list");
            rows.into_iter().find(|row| row.id == id).expect("job")
        }

        async fn paused_reason(&self) -> Option<String> {
            sqlx::query_scalar(
                "SELECT writeback_paused_reason FROM jira_integrations WHERE id = $1",
            )
            .bind(self.integration_id)
            .fetch_one(&self.pool)
            .await
            .expect("paused reason")
        }

        /// A feature in both environments, linked to `issue`.
        async fn feature(&self, issue: &str) -> (Uuid, String) {
            let id = Uuid::new_v4();
            let key = format!("sender-flag-{id}");
            sqlx::query(
                "INSERT INTO features (id, key, feature_type, team_id) VALUES ($1, $2, 'Simple', $3)",
            )
            .bind(id)
            .bind(&key)
            .bind(self.team_id)
            .execute(&self.pool)
            .await
            .expect("insert feature");
            for (index, env) in [self.qa, self.prod].into_iter().enumerate() {
                sqlx::query(
                    "INSERT INTO features_pipeline_stages (id, feature_id, environment_id, \
                     order_index, position, status) VALUES ($1, $2, $3, $4, '{}', 'NOT_DEPLOYED')",
                )
                .bind(Uuid::new_v4())
                .bind(id)
                .bind(env)
                .bind(index as i32)
                .execute(&self.pool)
                .await
                .expect("insert stage");
            }
            sqlx::query(
                "INSERT INTO feature_external_links (id, feature_id, system, external_key) \
                 VALUES ($1, $2, 'jira', $3)",
            )
            .bind(Uuid::new_v4())
            .bind(id)
            .bind(issue)
            .execute(&self.pool)
            .await
            .expect("link feature");
            (id, key)
        }

        async fn cleanup(self) {
            sqlx::query("DELETE FROM teams WHERE id = $1")
                .bind(self.team_id)
                .execute(&self.pool)
                .await
                .expect("delete team");
            sqlx::query("DELETE FROM users WHERE id = $1")
                .bind(self.shadow_user)
                .execute(&self.pool)
                .await
                .expect("delete shadow user");
        }
    }

    async fn requests(server: &MockServer) -> Vec<wiremock::Request> {
        server.received_requests().await.unwrap_or_default()
    }

    #[tokio::test]
    #[serial]
    async fn comment_job_is_sent_and_marked_sent() {
        let fx = Fixture::new(false).await;
        Mock::given(method("POST"))
            .and(path("/rest/api/2/issue/PROJ-1/comment"))
            .respond_with(ResponseTemplate::new(201).set_body_string("{}"))
            .expect(1)
            .mount(&fx.jira)
            .await;
        let id = fx.comment("PROJ-1", "hello").await;

        let counts = fx.sender().run_once().await;
        assert_eq!(
            counts,
            SendCounts {
                sent: 1,
                ..Default::default()
            }
        );
        let job = fx.job(id).await;
        assert_eq!(job.status, "sent");
        assert!(job.sent_at.is_some());
        let sent = requests(&fx.jira).await;
        assert_eq!(
            sent[0].body_json::<serde_json::Value>().unwrap(),
            json!({"body": "hello"})
        );

        fx.cleanup().await;
    }

    #[tokio::test]
    #[serial]
    async fn jobs_of_one_issue_are_sent_in_created_at_order() {
        let fx = Fixture::new(false).await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&fx.jira)
            .await;
        for text in ["first", "second", "third"] {
            fx.comment("PROJ-1", text).await;
        }
        let counts = fx.sender().run_once().await;
        assert_eq!(counts.sent, 3);
        let bodies: Vec<_> = requests(&fx.jira)
            .await
            .iter()
            .map(|r| r.body_json::<serde_json::Value>().unwrap()["body"].clone())
            .collect();
        assert_eq!(
            bodies,
            vec![json!("first"), json!("second"), json!("third")]
        );

        fx.cleanup().await;
    }

    #[tokio::test]
    #[serial]
    async fn server_error_schedules_a_retry_with_backoff() {
        let fx = Fixture::new(false).await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&fx.jira)
            .await;
        let id = fx.comment("PROJ-1", "hello").await;

        let counts = fx.sender().run_once().await;
        assert_eq!(
            counts,
            SendCounts {
                retried: 1,
                ..Default::default()
            }
        );
        let job = fx.job(id).await;
        assert_eq!(job.status, "pending");
        assert_eq!(job.attempts, 1);
        let wait = job.next_attempt_at - Utc::now();
        assert!(
            wait > chrono::Duration::seconds(25) && wait <= chrono::Duration::seconds(30),
            "{wait}"
        );
        assert_eq!(job.last_error.as_deref(), Some("500: boom"));

        fx.cleanup().await;
    }

    #[tokio::test]
    #[serial]
    async fn rate_limited_job_honours_retry_after() {
        let fx = Fixture::new(false).await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "120"))
            .mount(&fx.jira)
            .await;
        let id = fx.comment("PROJ-1", "hello").await;

        fx.sender().run_once().await;
        let job = fx.job(id).await;
        assert_eq!(job.attempts, 1);
        let wait = job.next_attempt_at - Utc::now();
        assert!(
            wait > chrono::Duration::seconds(115) && wait <= chrono::Duration::seconds(120),
            "{wait}"
        );

        fx.cleanup().await;
    }

    #[tokio::test]
    #[serial]
    async fn unauthorized_pauses_the_integration_and_keeps_other_jobs_pending() {
        let fx = Fixture::new(false).await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401).set_body_string("nope"))
            .mount(&fx.jira)
            .await;
        let first = fx.comment("PROJ-1", "one").await;
        let same_issue = fx.comment("PROJ-1", "two").await;
        let other_issue = fx.comment("PROJ-2", "three").await;

        let counts = fx.sender().run_once().await;
        assert_eq!(
            counts,
            SendCounts {
                dead: 1,
                paused: 1,
                ..Default::default()
            }
        );
        assert_eq!(
            requests(&fx.jira).await.len(),
            1,
            "one call, not one per job"
        );
        assert_eq!(fx.job(first).await.status, "dead");
        assert_eq!(fx.job(same_issue).await.status, "pending");
        assert_eq!(fx.job(other_issue).await.status, "pending");
        assert_eq!(fx.job(same_issue).await.attempts, 0);
        let reason = fx.paused_reason().await.expect("paused");
        assert!(reason.starts_with("Jira returned 401 at "), "{reason}");
        chrono::DateTime::parse_from_rfc3339(reason.trim_start_matches("Jira returned 401 at "))
            .expect("RFC 3339 time");

        // Paused: the next tick sends nothing.
        assert_eq!(fx.sender().run_once().await, SendCounts::default());
        assert_eq!(requests(&fx.jira).await.len(), 1);

        fx.cleanup().await;
    }

    #[tokio::test]
    #[serial]
    async fn redirect_is_not_followed() {
        let fx = Fixture::new(false).await;
        let elsewhere = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", format!("{}/steal", elsewhere.uri()).as_str()),
            )
            .mount(&fx.jira)
            .await;
        let id = fx.comment("PROJ-1", "hello").await;

        let counts = fx.sender().run_once().await;
        assert_eq!(counts.dead, 1);
        let job = fx.job(id).await;
        assert_eq!(job.status, "dead");
        assert!(job.last_error.unwrap().starts_with("302:"));
        assert!(
            requests(&elsewhere).await.is_empty(),
            "the redirect was followed"
        );

        fx.cleanup().await;
    }

    #[tokio::test]
    #[serial]
    async fn last_error_never_contains_the_credential() {
        let fx = Fixture::new(true).await;
        let basic = base64::engine::general_purpose::STANDARD
            .encode(format!("me@example.com:{}", fx.token));
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_string(format!(
                "bad request. Authorization: Basic {basic} (token {})",
                fx.token
            )))
            .mount(&fx.jira)
            .await;
        let id = fx.comment("PROJ-1", "hello").await;

        fx.sender().run_once().await;
        let job = fx.job(id).await;
        assert_eq!(job.status, "dead");
        let error = job.last_error.expect("last_error");
        assert!(error.starts_with("400: bad request."), "{error}");
        assert!(!error.contains(&fx.token), "{error}");
        assert!(!error.contains(&basic), "{error}");

        fx.cleanup().await;
    }

    #[tokio::test]
    #[serial]
    async fn remote_link_for_missing_feature_is_dropped() {
        let fx = Fixture::new(false).await;
        let (feature_id, _) = fx.feature("PROJ-1").await;
        let id = fx
            .enqueue_for(
                "PROJ-1",
                OutboundKind::RemoteLink,
                Some(feature_id),
                json!({}),
            )
            .await;
        sqlx::query("DELETE FROM features WHERE id = $1")
            .bind(feature_id)
            .execute(&fx.pool)
            .await
            .expect("delete feature");
        assert!(fx.job(id).await.feature_id.is_none(), "FK sets NULL");

        let counts = fx.sender().run_once().await;
        assert_eq!(counts.sent, 1);
        let job = fx.job(id).await;
        assert_eq!(job.status, "sent");
        assert_eq!(job.last_error.as_deref(), Some("skipped: feature deleted"));
        assert!(requests(&fx.jira).await.is_empty());

        fx.cleanup().await;
    }

    #[tokio::test]
    #[serial]
    async fn remote_link_for_an_unlinked_issue_is_dropped() {
        let fx = Fixture::new(false).await;
        let (feature_id, _) = fx.feature("PROJ-1").await;
        sqlx::query("DELETE FROM feature_external_links WHERE feature_id = $1")
            .bind(feature_id)
            .execute(&fx.pool)
            .await
            .expect("unlink");
        let id = fx
            .enqueue_for(
                "PROJ-1",
                OutboundKind::RemoteLink,
                Some(feature_id),
                json!({}),
            )
            .await;

        fx.sender().run_once().await;
        let job = fx.job(id).await;
        assert_eq!(job.status, "sent");
        assert_eq!(
            job.last_error.as_deref(),
            Some("skipped: issue no longer linked")
        );
        assert!(requests(&fx.jira).await.is_empty());

        fx.cleanup().await;
    }

    #[tokio::test]
    #[serial]
    async fn remote_link_without_ui_base_url_is_dead() {
        let fx = Fixture::new(false).await;
        let (feature_id, _) = fx.feature("PROJ-1").await;
        let id = fx
            .enqueue_for(
                "PROJ-1",
                OutboundKind::RemoteLink,
                Some(feature_id),
                json!({}),
            )
            .await;

        fx.sender_with_ui(None).run_once().await;
        let job = fx.job(id).await;
        assert_eq!(job.status, "dead");
        assert_eq!(
            job.last_error.as_deref(),
            Some("ui base URL is not configured")
        );

        fx.cleanup().await;
    }

    #[tokio::test]
    #[serial]
    async fn remote_link_title_reflects_current_stage_status() {
        let fx = Fixture::new(false).await;
        let (feature_id, key) = fx.feature("PROJ-1").await;
        sqlx::query(
            "UPDATE features_pipeline_stages SET status = 'DEPLOYED' \
             WHERE feature_id = $1 AND environment_id = $2",
        )
        .bind(feature_id)
        .bind(fx.qa)
        .execute(&fx.pool)
        .await
        .expect("deploy qa");
        Mock::given(method("POST"))
            .and(path("/rest/api/2/issue/PROJ-1/remotelink"))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&fx.jira)
            .await;
        let id = fx
            .enqueue_for(
                "PROJ-1",
                OutboundKind::RemoteLink,
                Some(feature_id),
                json!({}),
            )
            .await;

        let counts = fx.sender().run_once().await;
        assert_eq!(counts.sent, 1);
        assert_eq!(fx.job(id).await.status, "sent");
        let body = requests(&fx.jira).await[0]
            .body_json::<serde_json::Value>()
            .unwrap();
        assert_eq!(
            body["object"]["title"],
            format!("FluxGate: {key} · QA DEPLOYED · Production NOT_DEPLOYED")
        );
        assert_eq!(body["object"]["url"], format!("{UI}/features/{feature_id}"));
        assert_eq!(body["globalId"], format!("fluxgate:feature:{feature_id}"));

        fx.cleanup().await;
    }

    #[tokio::test]
    #[serial]
    async fn remote_link_delete_uses_the_feature_id_from_the_payload() {
        let fx = Fixture::new(false).await;
        let feature_id = Uuid::new_v4();
        Mock::given(method("DELETE"))
            .and(path("/rest/api/2/issue/PROJ-1/remotelink"))
            .and(wiremock::matchers::query_param(
                "globalId",
                format!("fluxgate:feature:{feature_id}").as_str(),
            ))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&fx.jira)
            .await;
        let id = fx
            .enqueue(
                "PROJ-1",
                OutboundKind::RemoteLinkDelete,
                json!({"featureId": feature_id.to_string()}),
            )
            .await;

        fx.sender().run_once().await;
        assert_eq!(fx.job(id).await.status, "sent", "404 on delete is success");

        fx.cleanup().await;
    }

    #[tokio::test]
    #[serial]
    async fn a_retry_holds_back_later_jobs_of_the_same_issue() {
        let fx = Fixture::new(false).await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&fx.jira)
            .await;
        let first = fx.comment("PROJ-1", "one").await;
        let second = fx.comment("PROJ-1", "two").await;

        let counts = fx.sender().run_once().await;
        assert_eq!(
            counts,
            SendCounts {
                retried: 1,
                ..Default::default()
            }
        );
        assert_eq!(
            requests(&fx.jira).await.len(),
            1,
            "the second job was not sent"
        );
        let (first, second) = (fx.job(first).await, fx.job(second).await);
        assert_eq!(second.status, "pending");
        assert_eq!(second.attempts, 0);
        assert_eq!(second.next_attempt_at, first.next_attempt_at);

        fx.cleanup().await;
    }

    #[tokio::test]
    #[serial]
    async fn sixth_failure_is_dead() {
        let fx = Fixture::new(false).await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503).set_body_string("down"))
            .mount(&fx.jira)
            .await;
        let id = fx.comment("PROJ-1", "hello").await;
        sqlx::query("UPDATE jira_outbound_jobs SET attempts = 5 WHERE id = $1")
            .bind(id)
            .execute(&fx.pool)
            .await
            .expect("set attempts");

        let counts = fx.sender().run_once().await;
        assert_eq!(
            counts,
            SendCounts {
                dead: 1,
                ..Default::default()
            }
        );
        let job = fx.job(id).await;
        assert_eq!(job.status, "dead");
        assert_eq!(job.attempts, 6);
        assert_eq!(job.last_error.as_deref(), Some("503: down"));

        fx.cleanup().await;
    }

    #[tokio::test]
    #[serial]
    async fn undecryptable_credential_pauses_without_calling_jira() {
        let fx = Fixture::new(false).await;
        let id = fx.comment("PROJ-1", "hello").await;
        // Sealed for another integration id: decryption fails.
        let wrong = secret_box::encrypt_with_aad("tok", Uuid::new_v4().as_bytes()).unwrap();
        sqlx::query("UPDATE jira_integrations SET jira_credential_enc = $2 WHERE id = $1")
            .bind(fx.integration_id)
            .bind(wrong)
            .execute(&fx.pool)
            .await
            .expect("break credential");

        let counts = fx.sender().run_once().await;
        assert_eq!(
            counts,
            SendCounts {
                paused: 1,
                ..Default::default()
            }
        );
        assert_eq!(
            fx.paused_reason().await.as_deref(),
            Some("credential cannot be decrypted")
        );
        assert_eq!(fx.job(id).await.status, "pending");
        assert!(requests(&fx.jira).await.is_empty());

        fx.cleanup().await;
    }
}
