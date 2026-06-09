use crate::database::activity_log::{ActivityLogRepository, CreateActivityLog};
use crate::logic::ActorContext;
use crate::logic::feature::FeatureLogic;
use crate::model::ID;
use crate::rest::operational_safety::{
    ScheduledChangeRow, ScheduledChangeStatus, claim_due_scheduled_changes,
    mark_scheduled_change_status, scheduled_change_hits_freeze, stage_request_from_status,
};
use log::{info, warn};
use sqlx::PgPool;
use std::time::Duration;
use tokio::time;

#[derive(Debug, thiserror::Error)]
pub enum ScheduledChangeSchedulerError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("feature operation failed: {0}")]
    Feature(#[from] crate::Error),
    #[error("invalid scheduled change: {0}")]
    Invalid(String),
}

pub struct ScheduledChangeScheduler {
    pool: PgPool,
    feature_logic: Box<dyn FeatureLogic>,
    activity_log_repository: Box<dyn ActivityLogRepository>,
    interval: Duration,
}

impl ScheduledChangeScheduler {
    pub fn new(
        pool: PgPool,
        feature_logic: Box<dyn FeatureLogic>,
        activity_log_repository: Box<dyn ActivityLogRepository>,
        interval: Duration,
    ) -> Self {
        Self {
            pool,
            feature_logic,
            activity_log_repository,
            interval,
        }
    }

    pub async fn start(self) {
        let mut ticker = time::interval(self.interval);
        loop {
            ticker.tick().await;
            match self.run_once(25).await {
                Ok(processed) => {
                    if processed > 0 {
                        info!("Scheduled feature changes processed {} item(s)", processed);
                    }
                }
                Err(err) => {
                    warn!(
                        "Scheduled feature change scheduler encountered an error: {}",
                        err
                    );
                }
            }
        }
    }

    pub async fn run_once(&self, limit: i64) -> Result<usize, ScheduledChangeSchedulerError> {
        let due = claim_due_scheduled_changes(&self.pool, limit).await?;
        let mut processed = 0usize;

        for change in due {
            processed += 1;
            if let Err(err) = self.execute_change(change.clone()).await {
                warn!("Scheduled feature change {} failed: {}", change.id, err);
                let _ = mark_scheduled_change_status(
                    &self.pool,
                    change.id,
                    ScheduledChangeStatus::Failed,
                    None,
                    Some(&err.to_string()),
                )
                .await;
            }
        }

        Ok(processed)
    }

    async fn execute_change(
        &self,
        change: ScheduledChangeRow,
    ) -> Result<(), ScheduledChangeSchedulerError> {
        if let Some(window) = scheduled_change_hits_freeze(&self.pool, &change).await? {
            let message = format!("Blocked by active freeze window '{}'", window.name);
            mark_scheduled_change_status(
                &self.pool,
                change.id,
                ScheduledChangeStatus::Blocked,
                None,
                Some(&message),
            )
            .await?;
            self.log_execution(&change, "scheduled_change_blocked", &message)
                .await?;
            return Ok(());
        }

        match change.action.as_str() {
            "ENABLE_FEATURE" => {
                self.feature_logic
                    .emergency_enable_feature(
                        ID::from(change.feature_id),
                        change.reason.clone(),
                        self.actor_context(&change),
                    )
                    .await?;
            }
            "DISABLE_FEATURE" => {
                self.feature_logic
                    .emergency_disable_feature(
                        ID::from(change.feature_id),
                        None,
                        change.reason.clone(),
                        None,
                        self.actor_context(&change),
                    )
                    .await?;
            }
            "STAGE_CHANGE" => {
                let stage_id = change.stage_id.ok_or_else(|| {
                    ScheduledChangeSchedulerError::Invalid("stage_id missing".to_string())
                })?;
                let requested_status = change.requested_status.as_deref().ok_or_else(|| {
                    ScheduledChangeSchedulerError::Invalid("requested_status missing".to_string())
                })?;
                let request = stage_request_from_status(requested_status).ok_or_else(|| {
                    ScheduledChangeSchedulerError::Invalid(format!(
                        "unsupported requested_status {requested_status}"
                    ))
                })?;
                let user_id = change.requested_by.unwrap_or_else(uuid::Uuid::nil);
                self.feature_logic
                    .request_stage_change(ID::from(stage_id), request, user_id)
                    .await?;
            }
            "ARCHIVE_FEATURE" => {
                sqlx::query(
                    r#"
                    UPDATE features
                    SET lifecycle_stage = 'archived',
                        archived_at = COALESCE(archived_at, NOW())
                    WHERE id = $1
                    "#,
                )
                .bind(change.feature_id)
                .execute(&self.pool)
                .await?;
            }
            other => {
                return Err(ScheduledChangeSchedulerError::Invalid(format!(
                    "unsupported action {other}"
                )));
            }
        }

        mark_scheduled_change_status(
            &self.pool,
            change.id,
            ScheduledChangeStatus::Executed,
            Some("Scheduled change executed"),
            None,
        )
        .await?;
        self.log_execution(
            &change,
            "scheduled_change_executed",
            "Scheduled change executed",
        )
        .await?;
        Ok(())
    }

    fn actor_context(&self, change: &ScheduledChangeRow) -> Option<ActorContext> {
        change
            .requested_by
            .map(|id| ActorContext::new(id, "scheduled-change".to_string()))
    }

    async fn log_execution(
        &self,
        change: &ScheduledChangeRow,
        activity_type: &str,
        description: &str,
    ) -> Result<(), ScheduledChangeSchedulerError> {
        self.activity_log_repository
            .create_activity(CreateActivityLog {
                activity_type: activity_type.to_string(),
                entity_type: "feature".to_string(),
                entity_id: change.feature_id.to_string(),
                actor_id: change.requested_by,
                actor_name: Some("scheduled-change".to_string()),
                description: description.to_string(),
                metadata: Some(serde_json::json!({
                    "scheduled_change_id": change.id.to_string(),
                    "team_id": change.team_id.to_string(),
                    "feature_id": change.feature_id.to_string(),
                    "stage_id": change.stage_id.map(|id| id.to_string()),
                    "environment_id": change.environment_id.map(|id| id.to_string()),
                    "action": change.action.clone(),
                    "requested_status": change.requested_status.clone(),
                    "scheduled_at": change.scheduled_at.to_rfc3339(),
                    "status": activity_type,
                })),
            })
            .await?;
        Ok(())
    }
}
