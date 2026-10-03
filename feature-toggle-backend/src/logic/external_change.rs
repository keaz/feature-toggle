//! Stage changes on behalf of an external system (design §3.8, JI-14). The
//! Jira rule engine (JI-15) is the only caller: it requests, approves, deploys
//! or rolls back one feature in one environment. Every change goes through the
//! same paths as a person's (state machine, freeze windows, dependencies,
//! approval requests, edge broadcast, activity). Where the environment trusts
//! the external system, it closes the approval request itself
//! (`approval_source = 'jira'`); it never votes.

use chrono::Utc;
use mockall::automock;
use serde::Serialize;
use sqlx::PgPool;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::Error;
use crate::database::activity_log::{ActivityLogRepository, CreateActivityLog};
use crate::database::entity::FeaturePipelineStage;
use crate::database::feature::FeatureRepository;
use crate::grpc::pb::FeatureUpdate;
use crate::logic::approval::{ApprovalLogic, ExternalApproval, ExternalApprovalResult};
use crate::logic::feature::{FeatureLogic, StageChangeRequestType};
use crate::model::{ID, StageChangeMeta};
use crate::utils::activity_logger::entity_types;

/// Refusal when the environment does not trust the external system to approve.
pub const REFUSED_NOT_TRUSTED: &str = "environment not approved by Jira";
/// Refusal for `deploy` on a stage that is not approved (decision J14).
pub const REFUSED_NOT_APPROVED: &str = "not approved";

/// Refusal when a feature has no stage in the environment.
pub const REFUSED_NO_STAGE: &str = "feature has no stage in this environment";
/// Refusal when the approval request was closed (for example by a vote) or
/// the stage changed while the external approval ran.
pub const REFUSED_ALREADY_RESOLVED: &str = "approval request already resolved";
/// Activity row when an active freeze window blocks a change (same type as
/// the REST freeze check).
pub const FREEZE_BLOCKED: &str = "freeze_blocked";

/// Who acted in the external system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExternalActor {
    /// `jira`; also the `approval_source` of requests it approves.
    pub system: String,
    pub account_id: Option<String>,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ExternalChangeContext {
    /// The integration's shadow user: requester and actor of every change.
    pub actor_user_id: Uuid,
    /// Issue key, stored as the request's `externalRef`.
    pub external_ref: String,
    /// Status in the external system that triggered the change.
    pub external_status: String,
    /// Request reason, for example "Jira status 'Done'".
    pub reason: String,
    pub external_actor: ExternalActor,
    /// The environment trusts the external system as an approver (J12).
    pub trusted_approval: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ExternalOutcome {
    Applied {
        from: String,
        to: String,
        approval_request_id: Option<Uuid>,
    },
    NoOp {
        status: String,
    },
    /// The change is not allowed now (state, trust, freeze, dependencies).
    /// Infrastructure failures are `Err` instead.
    Refused {
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalAction {
    Request,
    Approve,
    Deploy,
    Rollback,
}

/// One stage change the plan runs, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// `DEPLOYMENT_REQUESTED` through the normal request path.
    RequestDeployment,
    /// Approve the pending deployment as the external system.
    ApproveDeployment,
    /// `DEPLOYED` through the normal request path.
    Deploy,
    /// `ROLLBACK_REQUESTED` through the normal request path.
    RequestRollback,
    /// Approve the pending rollback as the external system.
    ApproveRollback,
    /// `ROLLBACKED` through the normal request path.
    ExecuteRollback,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Plan {
    Run(Vec<Step>),
    NoOp,
    Refuse(String),
}

/// The state table of design §3.8: what `action` does to a stage in `status`.
pub(crate) fn plan(action: ExternalAction, status: &str, trusted: bool) -> Plan {
    use Step::*;

    let status = status.to_uppercase();
    let not_deployed = matches!(
        status.as_str(),
        "NOT_DEPLOYED" | "DEPLOYMENT_REJECTED" | "ROLLBACKED"
    );
    // A rejected rollback leaves the stage deployed.
    let deployed = matches!(status.as_str(), "DEPLOYED" | "ROLLBACK_REJECTED");

    match action {
        ExternalAction::Request if not_deployed => Plan::Run(vec![RequestDeployment]),
        ExternalAction::Request => Plan::NoOp,

        ExternalAction::Approve if !trusted => Plan::Refuse(REFUSED_NOT_TRUSTED.to_string()),
        ExternalAction::Approve if not_deployed => {
            Plan::Run(vec![RequestDeployment, ApproveDeployment])
        }
        ExternalAction::Approve if status == "DEPLOYMENT_REQUESTED" => {
            Plan::Run(vec![ApproveDeployment])
        }
        ExternalAction::Approve => Plan::NoOp,

        ExternalAction::Deploy if status == "DEPLOYMENT_APPROVED" => Plan::Run(vec![Deploy]),
        ExternalAction::Deploy if deployed => Plan::NoOp,
        ExternalAction::Deploy => Plan::Refuse(REFUSED_NOT_APPROVED.to_string()),

        ExternalAction::Rollback if deployed && trusted => {
            Plan::Run(vec![RequestRollback, ApproveRollback, ExecuteRollback])
        }
        ExternalAction::Rollback if deployed => Plan::Run(vec![RequestRollback]),
        ExternalAction::Rollback if trusted && status == "ROLLBACK_REQUESTED" => {
            Plan::Run(vec![ApproveRollback, ExecuteRollback])
        }
        ExternalAction::Rollback if trusted && status == "ROLLBACK_APPROVED" => {
            Plan::Run(vec![ExecuteRollback])
        }
        ExternalAction::Rollback => Plan::NoOp,
    }
}

#[automock]
#[async_trait::async_trait]
pub trait ExternalChangeLogic: Send + Sync {
    /// Applies `action` to the feature's stage in `environment_id` on behalf
    /// of the external system, following the state table of design §3.8.
    async fn apply_external_action(
        &self,
        feature_id: Uuid,
        environment_id: Uuid,
        action: ExternalAction,
        ctx: &ExternalChangeContext,
    ) -> Result<ExternalOutcome, Error>;

    fn clone_box(&self) -> Box<dyn ExternalChangeLogic>;
}

impl Clone for Box<dyn ExternalChangeLogic> {
    fn clone(&self) -> Box<dyn ExternalChangeLogic> {
        self.clone_box()
    }
}

/// `approval_logic` needs a database pool (`approval_logic_with_pool*`), and
/// `feature_logic` should hold the same approval logic, as in `lib.rs::run`.
pub fn external_change_logic(
    pool: PgPool,
    feature_logic: Box<dyn FeatureLogic>,
    approval_logic: Box<dyn ApprovalLogic>,
    feature_repository: Box<dyn FeatureRepository>,
    activity_log_repository: Box<dyn ActivityLogRepository>,
    feature_updates_tx: broadcast::Sender<FeatureUpdate>,
) -> Box<dyn ExternalChangeLogic> {
    Box::new(ExternalChangeLogicImpl {
        pool,
        feature_logic,
        approval_logic,
        feature_repository,
        activity_log_repository,
        feature_updates_tx,
    })
}

#[derive(Clone)]
struct ExternalChangeLogicImpl {
    pool: PgPool,
    feature_logic: Box<dyn FeatureLogic>,
    approval_logic: Box<dyn ApprovalLogic>,
    feature_repository: Box<dyn FeatureRepository>,
    activity_log_repository: Box<dyn ActivityLogRepository>,
    feature_updates_tx: broadcast::Sender<FeatureUpdate>,
}

/// Why a step did not apply.
enum StepError {
    Refused(String),
    Failed(Error),
}

impl From<Error> for StepError {
    /// Only infrastructure errors fail the call; any other error is a reason
    /// the change is not allowed now.
    fn from(err: Error) -> Self {
        match err {
            Error::DatabaseError(_) => StepError::Failed(err),
            Error::InvalidInput(message) => StepError::Refused(message),
            other => StepError::Refused(other.to_string()),
        }
    }
}

/// "Jira" for `jira`; other systems as given.
fn system_label(system: &str) -> String {
    if system.eq_ignore_ascii_case("jira") {
        "Jira".to_string()
    } else {
        system.to_string()
    }
}

fn actor_name(actor: &ExternalActor) -> String {
    match &actor.display_name {
        Some(name) => format!("{} ({name})", system_label(&actor.system)),
        None => system_label(&actor.system),
    }
}

impl ExternalChangeLogicImpl {
    async fn stage_in(
        &self,
        feature_id: Uuid,
        environment_id: Uuid,
    ) -> Result<Option<FeaturePipelineStage>, Error> {
        match self.feature_repository.get_feature_stages(feature_id).await {
            Ok(stages) => Ok(stages
                .into_iter()
                .find(|stage| stage.environment_id == environment_id)),
            Err(Error::NotFound(_)) => Ok(None),
            Err(err) => Err(err),
        }
    }

    async fn current_status(&self, stage_id: Uuid) -> Result<String, Error> {
        Ok(self
            .feature_repository
            .get_stage_by_id(stage_id)
            .await?
            .ok_or(Error::NotFound(stage_id))?
            .status
            .to_uppercase())
    }

    /// The freeze window that blocks a change now, after logging the attempt.
    /// No override: an external system never overrides a freeze.
    async fn blocking_freeze(
        &self,
        feature_id: Uuid,
        environment_id: Uuid,
        ctx: &ExternalChangeContext,
    ) -> Result<Option<String>, Error> {
        let feature = self
            .feature_repository
            .get_feature_by_id(feature_id)
            .await?;
        let Some(window) = crate::rest::operational_safety::active_freeze_for_environment(
            &self.pool,
            feature.team_id,
            environment_id,
            Utc::now(),
        )
        .await
        .map_err(Error::DatabaseError)?
        else {
            return Ok(None);
        };
        self.activity_log_repository
            .create_activity(CreateActivityLog {
                activity_type: FREEZE_BLOCKED.to_string(),
                entity_type: entity_types::FEATURE.to_string(),
                entity_id: feature_id.to_string(),
                actor_id: Some(ctx.actor_user_id),
                actor_name: Some(actor_name(&ctx.external_actor)),
                description: format!(
                    "Change blocked for feature '{}' during freeze '{}'",
                    feature.key, window.name
                ),
                metadata: Some(serde_json::json!({
                    "team_id": feature.team_id.to_string(),
                    "feature_id": feature_id.to_string(),
                    "feature_key": feature.key,
                    "environment_id": environment_id.to_string(),
                    "freeze_window_id": window.id.to_string(),
                    "freeze_window_name": window.name,
                    "external_ref": ctx.external_ref,
                    "external_status": ctx.external_status,
                })),
            })
            .await
            .map_err(Error::DatabaseError)?;
        Ok(Some(format!("freeze window {}", window.name)))
    }

    /// Runs one step. Returns the approval request it created or closed.
    async fn run_step(
        &self,
        step: Step,
        stage_id: Uuid,
        ctx: &ExternalChangeContext,
    ) -> Result<Option<Uuid>, StepError> {
        let request = match step {
            Step::RequestDeployment => StageChangeRequestType::DeploymentRequested,
            Step::Deploy => StageChangeRequestType::Deployed,
            Step::RequestRollback => StageChangeRequestType::RollbackRequested,
            Step::ExecuteRollback => StageChangeRequestType::Rollbacked,
            Step::ApproveDeployment | Step::ApproveRollback => {
                return self.approve(stage_id, ctx).await;
            }
        };
        let meta = StageChangeMeta {
            external_ref: Some(ctx.external_ref.clone()),
            reason: Some(ctx.reason.clone()),
        };
        let feature = self
            .feature_logic
            .request_stage_change(ID::from(stage_id), request, ctx.actor_user_id, meta)
            .await?;
        Ok(feature
            .pending_approval_request_id
            .and_then(|id| Uuid::try_from(id).ok()))
    }

    async fn approve(
        &self,
        stage_id: Uuid,
        ctx: &ExternalChangeContext,
    ) -> Result<Option<Uuid>, StepError> {
        let actor = &ctx.external_actor;
        let approval = ExternalApproval {
            actor_user_id: ctx.actor_user_id,
            actor_name: actor_name(actor),
            source: actor.system.to_lowercase(),
            approver: serde_json::json!({
                "system": actor.system,
                "account_id": actor.account_id,
                "display_name": actor.display_name,
                "issue_key": ctx.external_ref,
                "status": ctx.external_status,
            }),
            metadata: serde_json::json!({
                "issue_key": ctx.external_ref,
                "external_status": ctx.external_status,
                "external_actor": actor,
            }),
        };
        match self
            .approval_logic
            .approve_stage_change_externally(stage_id, approval)
            .await?
        {
            ExternalApprovalResult::Approved {
                approval_request_id,
            } => Ok(approval_request_id),
            ExternalApprovalResult::AlreadyResolved => {
                Err(StepError::Refused(REFUSED_ALREADY_RESOLVED.to_string()))
            }
        }
    }

    /// Sends the feature to edge servers. The request path (unlike the REST
    /// handlers) does not broadcast.
    async fn broadcast(&self, feature_id: Uuid) {
        let Ok(feature) = self.feature_repository.get_feature_by_id(feature_id).await else {
            return;
        };
        if let Ok(full) = crate::broadcast::map_db_feature_to_full_for_broadcast(
            self.feature_repository.as_ref(),
            feature,
        )
        .await
        {
            let _ = self.feature_updates_tx.send(FeatureUpdate {
                message_id: Uuid::new_v4().to_string(),
                action: crate::grpc::pb::feature_update::Action::Upsert as i32,
                feature: Some(full),
                feature_key: String::new(),
                error: String::new(),
            });
        }
    }
}

#[async_trait::async_trait]
impl ExternalChangeLogic for ExternalChangeLogicImpl {
    async fn apply_external_action(
        &self,
        feature_id: Uuid,
        environment_id: Uuid,
        action: ExternalAction,
        ctx: &ExternalChangeContext,
    ) -> Result<ExternalOutcome, Error> {
        let Some(stage) = self.stage_in(feature_id, environment_id).await? else {
            return Ok(ExternalOutcome::Refused {
                reason: REFUSED_NO_STAGE.to_string(),
            });
        };
        let from = stage.status.to_uppercase();
        let steps = match plan(action, &from, ctx.trusted_approval) {
            Plan::NoOp => return Ok(ExternalOutcome::NoOp { status: from }),
            Plan::Refuse(reason) => return Ok(ExternalOutcome::Refused { reason }),
            Plan::Run(steps) => steps,
        };
        if let Some(reason) = self
            .blocking_freeze(feature_id, environment_id, ctx)
            .await?
        {
            return Ok(ExternalOutcome::Refused { reason });
        }

        let mut approval_request_id = None;
        let mut applied = false;
        for step in steps {
            match self.run_step(step, stage.id, ctx).await {
                Ok(request_id) => {
                    approval_request_id = request_id.or(approval_request_id);
                    applied = true;
                }
                Err(StepError::Failed(err)) => return Err(err),
                Err(StepError::Refused(reason)) if !applied => {
                    return Ok(ExternalOutcome::Refused { reason });
                }
                Err(StepError::Refused(reason)) => {
                    // An earlier step applied: say where the stage stopped.
                    self.broadcast(feature_id).await;
                    let now = self.current_status(stage.id).await?;
                    return Ok(ExternalOutcome::Refused {
                        reason: format!("{reason} (stage now {now})"),
                    });
                }
            }
        }

        self.broadcast(feature_id).await;
        Ok(ExternalOutcome::Applied {
            from,
            to: self.current_status(stage.id).await?,
            approval_request_id,
        })
    }

    fn clone_box(&self) -> Box<dyn ExternalChangeLogic> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ExternalAction::{Approve, Deploy, Request, Rollback};
    use Step::*;

    const NOT_DEPLOYED: &str = "NOT_DEPLOYED";
    const DEPLOYMENT_REQUESTED: &str = "DEPLOYMENT_REQUESTED";
    const DEPLOYMENT_APPROVED: &str = "DEPLOYMENT_APPROVED";
    const DEPLOYMENT_REJECTED: &str = "DEPLOYMENT_REJECTED";
    const DEPLOYED: &str = "DEPLOYED";
    const ROLLBACK_REQUESTED: &str = "ROLLBACK_REQUESTED";
    const ROLLBACK_APPROVED: &str = "ROLLBACK_APPROVED";
    const ROLLBACK_REJECTED: &str = "ROLLBACK_REJECTED";
    const ROLLBACKED: &str = "ROLLBACKED";

    const ALL: [&str; 9] = [
        NOT_DEPLOYED,
        DEPLOYMENT_REQUESTED,
        DEPLOYMENT_APPROVED,
        DEPLOYMENT_REJECTED,
        DEPLOYED,
        ROLLBACK_REQUESTED,
        ROLLBACK_APPROVED,
        ROLLBACK_REJECTED,
        ROLLBACKED,
    ];

    fn run(steps: &[Step]) -> Plan {
        Plan::Run(steps.to_vec())
    }

    fn refuse(reason: &str) -> Plan {
        Plan::Refuse(reason.to_string())
    }

    /// Expected plan for every (action, status, trusted) combination.
    fn expected(action: ExternalAction, status: &str, trusted: bool) -> Plan {
        match (action, status) {
            (Request, NOT_DEPLOYED | DEPLOYMENT_REJECTED | ROLLBACKED) => run(&[RequestDeployment]),
            (Request, _) => Plan::NoOp,

            (Approve, _) if !trusted => refuse(REFUSED_NOT_TRUSTED),
            (Approve, NOT_DEPLOYED | DEPLOYMENT_REJECTED | ROLLBACKED) => {
                run(&[RequestDeployment, ApproveDeployment])
            }
            (Approve, DEPLOYMENT_REQUESTED) => run(&[ApproveDeployment]),
            (Approve, _) => Plan::NoOp,

            (Deploy, DEPLOYMENT_APPROVED) => run(&[Step::Deploy]),
            // Deployed, or deployed with a rejected rollback.
            (Deploy, DEPLOYED | ROLLBACK_REJECTED) => Plan::NoOp,
            (Deploy, _) => refuse(REFUSED_NOT_APPROVED),

            (Rollback, DEPLOYED | ROLLBACK_REJECTED) if trusted => {
                run(&[RequestRollback, ApproveRollback, ExecuteRollback])
            }
            (Rollback, DEPLOYED | ROLLBACK_REJECTED) => run(&[RequestRollback]),
            (Rollback, ROLLBACK_REQUESTED) if trusted => run(&[ApproveRollback, ExecuteRollback]),
            (Rollback, ROLLBACK_APPROVED) if trusted => run(&[ExecuteRollback]),
            (Rollback, _) => Plan::NoOp,
        }
    }

    #[test]
    fn every_action_status_and_trust_follows_the_state_table() {
        for action in [Request, Approve, Deploy, Rollback] {
            for status in ALL {
                for trusted in [false, true] {
                    assert_eq!(
                        plan(action, status, trusted),
                        expected(action, status, trusted),
                        "{action:?} from {status}, trusted = {trusted}"
                    );
                }
            }
        }
    }

    #[test]
    fn spot_checks_of_the_design_table() {
        assert_eq!(
            plan(Approve, NOT_DEPLOYED, false),
            refuse(REFUSED_NOT_TRUSTED)
        );
        assert_eq!(
            plan(Approve, NOT_DEPLOYED, true),
            run(&[RequestDeployment, ApproveDeployment])
        );
        assert_eq!(
            plan(Deploy, NOT_DEPLOYED, true),
            refuse(REFUSED_NOT_APPROVED)
        );
        assert_eq!(
            plan(Deploy, DEPLOYMENT_REQUESTED, true),
            refuse(REFUSED_NOT_APPROVED)
        );
        assert_eq!(plan(Deploy, DEPLOYED, false), Plan::NoOp);
        assert_eq!(plan(Rollback, ROLLBACK_REQUESTED, false), Plan::NoOp);
        assert_eq!(plan(Rollback, NOT_DEPLOYED, true), Plan::NoOp);
    }

    #[test]
    fn status_is_compared_ignoring_case() {
        assert_eq!(
            plan(Deploy, "deployment_approved", false),
            run(&[Step::Deploy])
        );
    }
}
