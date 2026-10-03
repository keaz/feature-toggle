//! The Jira write-back outbox (`jira_outbound_jobs`, JI-42). Capture code enqueues
//! jobs; the sender claims, sends and settles them.

use chrono::{DateTime, Utc};
use mockall::automock;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::Error;
use crate::database::entity::OutboundJobRow;
use crate::database::handle_error;

const JOB_COLUMNS: &str = "id, integration_id, issue_key, feature_id, kind, payload, dedupe_key, \
     status, attempts, next_attempt_at, last_error, created_at, sent_at";

/// What a job sends to Jira.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboundKind {
    Comment,
    RemoteLink,
    RemoteLinkDelete,
}

impl OutboundKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Comment => "comment",
            Self::RemoteLink => "remote_link",
            Self::RemoteLinkDelete => "remote_link_delete",
        }
    }

    pub fn parse(kind: &str) -> Option<Self> {
        match kind {
            "comment" => Some(Self::Comment),
            "remote_link" => Some(Self::RemoteLink),
            "remote_link_delete" => Some(Self::RemoteLinkDelete),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewOutboundJob {
    pub integration_id: Uuid,
    pub issue_key: String,
    pub feature_id: Option<Uuid>,
    pub kind: OutboundKind,
    pub payload: serde_json::Value,
    pub dedupe_key: String,
}

#[automock]
#[async_trait::async_trait]
pub trait JiraOutboundJobRepository: Send + Sync {
    /// Inserts a pending job. `false` when `dedupe_key` exists or a pending remote
    /// link for the same integration, issue and feature already covers it.
    async fn enqueue(&self, job: NewOutboundJob) -> Result<bool, Error>;
    /// Leases up to `limit` due jobs of active integrations (enabled, write-back on,
    /// not paused) by pushing their `next_attempt_at` out by `lease`. A job is not
    /// claimed while an earlier pending job of the same issue waits for a retry.
    async fn claim_due(
        &self,
        limit: i64,
        lease: chrono::Duration,
    ) -> Result<Vec<OutboundJobRow>, Error>;
    /// The `mark_*` calls change a job only while it is `pending`: a job cancelled
    /// meanwhile stays `dead`.
    /// Marks the job `sent`; `note` is kept in `last_error` (for example "skipped: ...").
    async fn mark_sent(&self, id: Uuid, note: Option<String>) -> Result<(), Error>;
    async fn mark_retry(
        &self,
        id: Uuid,
        attempts: i32,
        next_attempt_at: DateTime<Utc>,
        error: String,
    ) -> Result<(), Error>;
    async fn mark_dead(&self, id: Uuid, attempts: i32, error: String) -> Result<(), Error>;
    /// Whether the job is still `pending` (not cancelled since it was claimed).
    async fn is_pending(&self, id: Uuid) -> Result<bool, Error>;
    /// Makes pending jobs due again at `next_attempt_at`, without counting an attempt.
    async fn release(&self, ids: &[Uuid], next_attempt_at: DateTime<Utc>) -> Result<(), Error>;
    /// Jobs of the integration, newest first, and their total.
    async fn list(
        &self,
        integration_id: Uuid,
        status: Option<String>,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<OutboundJobRow>, i64), Error>;
    /// `dead` to `pending` with `attempts = 0`. `None` when the job is not dead or
    /// does not belong to the integration.
    async fn retry_dead(
        &self,
        integration_id: Uuid,
        id: Uuid,
    ) -> Result<Option<OutboundJobRow>, Error>;
    /// Deletes `sent` jobs sent before `sent_before` and `dead` jobs created before
    /// `dead_before`. Pending jobs stay.
    async fn delete_finished_before(
        &self,
        sent_before: DateTime<Utc>,
        dead_before: DateTime<Utc>,
    ) -> Result<u64, Error>;
}

#[async_trait::async_trait]
pub trait JiraOutboundJobRepositoryTx {
    /// Like [`JiraOutboundJobRepository::enqueue`], inside the caller's transaction.
    async fn enqueue_tx(&self, conn: &mut PgConnection, job: NewOutboundJob)
    -> Result<bool, Error>;
    /// Sets the integration's pending jobs to `dead` with `last_error = reason`.
    async fn cancel_pending_tx(
        &self,
        conn: &mut PgConnection,
        integration_id: Uuid,
        reason: &str,
    ) -> Result<u64, Error>;
}

pub fn jira_outbound_job_repository(pool: PgPool) -> Box<dyn JiraOutboundJobRepository> {
    Box::new(JiraOutboundJobRepositoryImpl { pool })
}

pub fn jira_outbound_job_repository_tx(pool: PgPool) -> JiraOutboundJobRepositoryImpl {
    JiraOutboundJobRepositoryImpl { pool }
}

#[derive(Clone)]
pub struct JiraOutboundJobRepositoryImpl {
    pool: PgPool,
}

impl JiraOutboundJobRepositoryImpl {
    async fn enqueue_conn(conn: &mut PgConnection, job: NewOutboundJob) -> Result<bool, Error> {
        // `clock_timestamp()`, not the transaction's `now()`: jobs enqueued in one
        // transaction keep their insert order.
        // No conflict target: both the `dedupe_key` constraint and the partial unique
        // index on pending remote links make a duplicate a no-op.
        let result = sqlx::query(
            "INSERT INTO jira_outbound_jobs (integration_id, issue_key, feature_id, kind, \
             payload, dedupe_key, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, clock_timestamp()) ON CONFLICT DO NOTHING",
        )
        .bind(job.integration_id)
        .bind(job.issue_key)
        .bind(job.feature_id)
        .bind(job.kind.as_str())
        .bind(job.payload)
        .bind(job.dedupe_key)
        .execute(&mut *conn)
        .await;
        Ok(handle_error(None, result)?.rows_affected() > 0)
    }
}

#[async_trait::async_trait]
impl JiraOutboundJobRepository for JiraOutboundJobRepositoryImpl {
    async fn enqueue(&self, job: NewOutboundJob) -> Result<bool, Error> {
        let mut conn = handle_error(None, self.pool.acquire().await)?;
        Self::enqueue_conn(&mut conn, job).await
    }

    async fn claim_due(
        &self,
        limit: i64,
        lease: chrono::Duration,
    ) -> Result<Vec<OutboundJobRow>, Error> {
        let result = sqlx::query_as::<_, OutboundJobRow>(&format!(
            "UPDATE jira_outbound_jobs SET next_attempt_at = now() + make_interval(secs => $2) \
             WHERE id IN ( \
               SELECT j.id FROM jira_outbound_jobs j \
               JOIN jira_integrations i ON i.id = j.integration_id \
               WHERE j.status = 'pending' AND j.next_attempt_at <= now() \
                 AND i.enabled AND i.writeback_enabled AND i.writeback_paused_reason IS NULL \
                 AND NOT EXISTS ( \
                   SELECT 1 FROM jira_outbound_jobs e \
                   WHERE e.integration_id = j.integration_id AND e.issue_key = j.issue_key \
                     AND e.status = 'pending' AND e.next_attempt_at > now() \
                     AND (e.created_at, e.id) < (j.created_at, j.id)) \
               ORDER BY j.created_at, j.id LIMIT $1 FOR UPDATE OF j SKIP LOCKED) \
             RETURNING {JOB_COLUMNS}"
        ))
        .bind(limit)
        .bind(lease.num_milliseconds() as f64 / 1000.0)
        .fetch_all(&self.pool)
        .await;
        handle_error(None, result)
    }

    async fn mark_sent(&self, id: Uuid, note: Option<String>) -> Result<(), Error> {
        let result = sqlx::query(
            "UPDATE jira_outbound_jobs SET status = 'sent', sent_at = now(), last_error = $2 \
             WHERE id = $1 AND status = 'pending'",
        )
        .bind(id)
        .bind(note)
        .execute(&self.pool)
        .await;
        handle_error(Some(id), result).map(|_| ())
    }

    async fn mark_retry(
        &self,
        id: Uuid,
        attempts: i32,
        next_attempt_at: DateTime<Utc>,
        error: String,
    ) -> Result<(), Error> {
        let result = sqlx::query(
            "UPDATE jira_outbound_jobs SET status = 'pending', attempts = $2, \
             next_attempt_at = $3, last_error = $4 WHERE id = $1 AND status = 'pending'",
        )
        .bind(id)
        .bind(attempts)
        .bind(next_attempt_at)
        .bind(error)
        .execute(&self.pool)
        .await;
        handle_error(Some(id), result).map(|_| ())
    }

    async fn mark_dead(&self, id: Uuid, attempts: i32, error: String) -> Result<(), Error> {
        let result = sqlx::query(
            "UPDATE jira_outbound_jobs SET status = 'dead', attempts = $2, last_error = $3 \
             WHERE id = $1 AND status = 'pending'",
        )
        .bind(id)
        .bind(attempts)
        .bind(error)
        .execute(&self.pool)
        .await;
        handle_error(Some(id), result).map(|_| ())
    }

    async fn is_pending(&self, id: Uuid) -> Result<bool, Error> {
        let result = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM jira_outbound_jobs WHERE id = $1 AND status = 'pending')",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await;
        handle_error(Some(id), result)
    }

    async fn release(&self, ids: &[Uuid], next_attempt_at: DateTime<Utc>) -> Result<(), Error> {
        if ids.is_empty() {
            return Ok(());
        }
        let result = sqlx::query(
            "UPDATE jira_outbound_jobs SET next_attempt_at = $2 \
             WHERE id = ANY($1) AND status = 'pending'",
        )
        .bind(ids)
        .bind(next_attempt_at)
        .execute(&self.pool)
        .await;
        handle_error(None, result).map(|_| ())
    }

    async fn list(
        &self,
        integration_id: Uuid,
        status: Option<String>,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<OutboundJobRow>, i64), Error> {
        let rows = sqlx::query_as::<_, OutboundJobRow>(&format!(
            "SELECT {JOB_COLUMNS} FROM jira_outbound_jobs \
             WHERE integration_id = $1 AND ($2::text IS NULL OR status = $2) \
             ORDER BY created_at DESC, id DESC OFFSET $3 LIMIT $4"
        ))
        .bind(integration_id)
        .bind(&status)
        .bind(offset)
        .bind(limit)
        .fetch_all(&self.pool)
        .await;
        let rows = handle_error(Some(integration_id), rows)?;
        let total = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM jira_outbound_jobs \
             WHERE integration_id = $1 AND ($2::text IS NULL OR status = $2)",
        )
        .bind(integration_id)
        .bind(&status)
        .fetch_one(&self.pool)
        .await;
        Ok((rows, handle_error(Some(integration_id), total)?))
    }

    async fn retry_dead(
        &self,
        integration_id: Uuid,
        id: Uuid,
    ) -> Result<Option<OutboundJobRow>, Error> {
        let result = sqlx::query_as::<_, OutboundJobRow>(&format!(
            "UPDATE jira_outbound_jobs SET status = 'pending', attempts = 0, \
             next_attempt_at = now(), sent_at = NULL \
             WHERE id = $1 AND integration_id = $2 AND status = 'dead' RETURNING {JOB_COLUMNS}"
        ))
        .bind(id)
        .bind(integration_id)
        .fetch_optional(&self.pool)
        .await;
        handle_error(Some(id), result)
    }

    async fn delete_finished_before(
        &self,
        sent_before: DateTime<Utc>,
        dead_before: DateTime<Utc>,
    ) -> Result<u64, Error> {
        let result = sqlx::query(
            "DELETE FROM jira_outbound_jobs \
             WHERE (status = 'sent' AND sent_at < $1) OR (status = 'dead' AND created_at < $2)",
        )
        .bind(sent_before)
        .bind(dead_before)
        .execute(&self.pool)
        .await;
        Ok(handle_error(None, result)?.rows_affected())
    }
}

#[async_trait::async_trait]
impl JiraOutboundJobRepositoryTx for JiraOutboundJobRepositoryImpl {
    async fn enqueue_tx(
        &self,
        conn: &mut PgConnection,
        job: NewOutboundJob,
    ) -> Result<bool, Error> {
        Self::enqueue_conn(conn, job).await
    }

    async fn cancel_pending_tx(
        &self,
        conn: &mut PgConnection,
        integration_id: Uuid,
        reason: &str,
    ) -> Result<u64, Error> {
        let result = sqlx::query(
            "UPDATE jira_outbound_jobs SET status = 'dead', last_error = $2 \
             WHERE integration_id = $1 AND status = 'pending'",
        )
        .bind(integration_id)
        .bind(reason)
        .execute(&mut *conn)
        .await;
        Ok(handle_error(Some(integration_id), result)?.rows_affected())
    }
}
