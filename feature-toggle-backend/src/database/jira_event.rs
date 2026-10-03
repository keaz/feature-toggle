//! The Jira event log (`jira_integration_events`, JI-15): every inbound event
//! an integration accepted, with what the rule engine did.

use chrono::{DateTime, Utc};
use mockall::automock;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use uuid::Uuid;

use crate::Error;
use crate::database::handle_error;
use crate::database::jira_outbound_job::{
    JiraOutboundJobRepositoryTx, NewOutboundJob, jira_outbound_job_repository_tx,
};

const EVENT_COLUMNS: &str = "id, integration_id, received_at, issue_key, jira_status, jira_actor, \
     delivery_hash, results, unknown_environments, unknown_features, ignored, error";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct JiraEventRow {
    pub id: Uuid,
    pub integration_id: Uuid,
    pub received_at: DateTime<Utc>,
    pub issue_key: Option<String>,
    pub jira_status: Option<String>,
    pub jira_actor: Option<JsonValue>,
    pub delivery_hash: Option<String>,
    pub results: JsonValue,
    pub unknown_environments: Vec<String>,
    pub unknown_features: Vec<String>,
    /// Why the event did nothing on purpose, e.g. "no status change".
    pub ignored: Option<String>,
    /// Why the event could not be processed (bad body, database error).
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewJiraEvent {
    /// Set by the caller (`Uuid::new_v4()`), so a job can name the event before it exists.
    pub id: Uuid,
    pub integration_id: Uuid,
    pub issue_key: Option<String>,
    pub jira_status: Option<String>,
    pub jira_actor: Option<JsonValue>,
    pub delivery_hash: Option<String>,
    pub results: JsonValue,
    pub unknown_environments: Vec<String>,
    pub unknown_features: Vec<String>,
    pub ignored: Option<String>,
    pub error: Option<String>,
}

#[automock]
#[async_trait::async_trait]
pub trait JiraEventRepository: Send + Sync {
    async fn insert(&self, event: NewJiraEvent) -> Result<JiraEventRow, Error>;
    /// Stores the event and enqueues `jobs` in one transaction. A job whose
    /// `dedupe_key` is taken is skipped; it does not fail the call.
    async fn insert_with_jobs(
        &self,
        event: NewJiraEvent,
        jobs: Vec<NewOutboundJob>,
    ) -> Result<JiraEventRow, Error>;
    /// The newest event of the integration with `delivery_hash` received at or
    /// after `since` that was processed without an error.
    async fn find_recent_delivery(
        &self,
        integration_id: Uuid,
        delivery_hash: &str,
        since: DateTime<Utc>,
    ) -> Result<Option<JiraEventRow>, Error>;
    /// Events of the integration, newest first, and their total.
    async fn list(
        &self,
        integration_id: Uuid,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<JiraEventRow>, i64), Error>;
    /// Deletes events received before `cutoff`; returns how many.
    async fn delete_older_than(&self, cutoff: DateTime<Utc>) -> Result<u64, Error>;

    fn clone_box(&self) -> Box<dyn JiraEventRepository>;
}

impl Clone for Box<dyn JiraEventRepository> {
    fn clone(&self) -> Box<dyn JiraEventRepository> {
        self.clone_box()
    }
}

pub fn jira_event_repository(pool: PgPool) -> Box<dyn JiraEventRepository> {
    Box::new(JiraEventRepositoryImpl { pool })
}

#[derive(Clone)]
struct JiraEventRepositoryImpl {
    pool: PgPool,
}

async fn insert_event(
    conn: &mut sqlx::PgConnection,
    event: NewJiraEvent,
) -> Result<JiraEventRow, Error> {
    let result = sqlx::query_as::<_, JiraEventRow>(&format!(
        "INSERT INTO jira_integration_events (id, integration_id, issue_key, jira_status, \
         jira_actor, delivery_hash, results, unknown_environments, unknown_features, ignored, \
         error) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) RETURNING {EVENT_COLUMNS}"
    ))
    .bind(event.id)
    .bind(event.integration_id)
    .bind(event.issue_key)
    .bind(event.jira_status)
    .bind(event.jira_actor)
    .bind(event.delivery_hash)
    .bind(event.results)
    .bind(event.unknown_environments)
    .bind(event.unknown_features)
    .bind(event.ignored)
    .bind(event.error)
    .fetch_one(conn)
    .await;
    handle_error(Some(event.integration_id), result)
}

#[async_trait::async_trait]
impl JiraEventRepository for JiraEventRepositoryImpl {
    async fn insert(&self, event: NewJiraEvent) -> Result<JiraEventRow, Error> {
        let integration_id = event.integration_id;
        let mut conn = handle_error(Some(integration_id), self.pool.acquire().await)?;
        insert_event(&mut conn, event).await
    }

    async fn insert_with_jobs(
        &self,
        event: NewJiraEvent,
        jobs: Vec<NewOutboundJob>,
    ) -> Result<JiraEventRow, Error> {
        let integration_id = event.integration_id;
        let mut tx = handle_error(Some(integration_id), self.pool.begin().await)?;
        let row = insert_event(&mut tx, event).await?;
        let outbox = jira_outbound_job_repository_tx(self.pool.clone());
        for job in jobs {
            outbox.enqueue_tx(&mut tx, job).await?;
        }
        handle_error(Some(integration_id), tx.commit().await)?;
        Ok(row)
    }

    async fn find_recent_delivery(
        &self,
        integration_id: Uuid,
        delivery_hash: &str,
        since: DateTime<Utc>,
    ) -> Result<Option<JiraEventRow>, Error> {
        let result = sqlx::query_as::<_, JiraEventRow>(&format!(
            "SELECT {EVENT_COLUMNS} FROM jira_integration_events \
             WHERE integration_id = $1 AND delivery_hash = $2 AND received_at >= $3 \
               AND error IS NULL \
             ORDER BY received_at DESC LIMIT 1"
        ))
        .bind(integration_id)
        .bind(delivery_hash)
        .bind(since)
        .fetch_optional(&self.pool)
        .await;
        handle_error(Some(integration_id), result)
    }

    async fn list(
        &self,
        integration_id: Uuid,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<JiraEventRow>, i64), Error> {
        let rows = sqlx::query_as::<_, JiraEventRow>(&format!(
            "SELECT {EVENT_COLUMNS} FROM jira_integration_events WHERE integration_id = $1 \
             ORDER BY received_at DESC, id DESC OFFSET $2 LIMIT $3"
        ))
        .bind(integration_id)
        .bind(offset)
        .bind(limit)
        .fetch_all(&self.pool)
        .await;
        let rows = handle_error(Some(integration_id), rows)?;
        let total = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM jira_integration_events WHERE integration_id = $1",
        )
        .bind(integration_id)
        .fetch_one(&self.pool)
        .await;
        Ok((rows, handle_error(Some(integration_id), total)?))
    }

    async fn delete_older_than(&self, cutoff: DateTime<Utc>) -> Result<u64, Error> {
        let result = sqlx::query("DELETE FROM jira_integration_events WHERE received_at < $1")
            .bind(cutoff)
            .execute(&self.pool)
            .await;
        Ok(handle_error(None, result)?.rows_affected())
    }

    fn clone_box(&self) -> Box<dyn JiraEventRepository> {
        Box::new(self.clone())
    }
}
