use crate::Error;
use crate::database::approval::{
    ApprovalRepository, ApprovalRepositoryTx, CreateApprovalRequestInput, CreateApprovalVoteInput,
    approval_repository_tx,
};
use crate::database::entity::{
    ApprovalPolicy, ApprovalRequest, ApprovalStatus, ApprovalVote, ApprovalVoteValue,
    Feature as DbFeature, FeaturePipelineStage, FeatureType, LogicOperator, SENTINEL_UUID,
    VariantSelectionMode, VariantValueType,
};
use crate::database::feature::{
    FeatureConfigSnapshot, FeatureRepository, FeatureRepositoryTx, FeatureSnapshotMetadata,
    FeatureStageSnapshot, FeatureVariantSnapshot, RuleConditionSnapshot, RuleGroupSnapshot,
    StageCriterionSnapshot, VariantAllocationSnapshot, diff_entries_to_json,
    diff_feature_snapshots, feature_repository_tx,
};
use crate::database::role::RoleRepository;
use crate::judgment::service::JudgmentService;
use crate::judgment::{JudgmentKind, SubjectType, approval_risk};
use crate::logic::environment::EnvironmentLogic;
use crate::model::ID;
use chrono::Utc;
use feature_toggle_shared::constants::StageStatus;
use log::warn;
use mockall::automock;
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::broadcast;
use uuid::Uuid;

pub(crate) fn status_requires_interception(status: &str) -> bool {
    matches!(status, "DEPLOYMENT_REQUESTED" | "ROLLBACK_REQUESTED")
}

pub(crate) fn policy_applies(policy: &ApprovalPolicy, env_id: Uuid, env_type: &str) -> bool {
    if !policy.enabled {
        return false;
    }

    match policy.applies_to.as_str() {
        "all" => true,
        "production_only" => env_type.eq_ignore_ascii_case("production"),
        "specific_environments" => policy
            .environment_ids
            .as_ref()
            .map(|ids| ids.contains(&env_id))
            .unwrap_or(false),
        _ => false,
    }
}

fn policy_scope_rank(policy: &ApprovalPolicy, env_id: Uuid, env_type: &str) -> u8 {
    match policy.applies_to.as_str() {
        "specific_environments"
            if policy
                .environment_ids
                .as_ref()
                .map(|ids| ids.contains(&env_id))
                .unwrap_or(false) =>
        {
            3
        }
        "production_only" if env_type.eq_ignore_ascii_case("production") => 2,
        "all" => 1,
        _ => 0,
    }
}

#[derive(Clone)]
pub struct ApprovalRequestEvent {
    pub request: ApprovalRequest,
    pub team_id: Uuid,
    pub votes: Vec<ApprovalVote>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApprovalPolicyPreviewOutcome {
    NotRequired,
    RequiresApproval,
    AutoApproved,
    Blocked,
}

impl ApprovalPolicyPreviewOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            ApprovalPolicyPreviewOutcome::NotRequired => "not_required",
            ApprovalPolicyPreviewOutcome::RequiresApproval => "requires_approval",
            ApprovalPolicyPreviewOutcome::AutoApproved => "auto_approved",
            ApprovalPolicyPreviewOutcome::Blocked => "blocked",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ApprovalPolicyPreview {
    pub change_type: String,
    pub requested_status: Option<String>,
    pub environment_id: Uuid,
    pub outcome: ApprovalPolicyPreviewOutcome,
    pub policy: Option<ApprovalPolicy>,
    pub eligible_approvers_count: Option<i64>,
    pub warnings: Vec<String>,
    pub reason: String,
}

#[automock]
#[async_trait::async_trait]
pub trait ApprovalLogic: Send + Sync {
    /// Return Some(request) when an approval gate is configured for this stage change.
    async fn maybe_create_stage_change_request(
        &self,
        feature: &DbFeature,
        stage: &FeaturePipelineStage,
        next_status: &str,
        requested_by: Uuid,
    ) -> Result<Option<ApprovalRequest>, Error>;

    async fn approve_request(
        &self,
        request_id: Uuid,
        approver_id: Uuid,
        comment: Option<String>,
    ) -> Result<ApprovalRequest, Error>;

    async fn reject_request(
        &self,
        request_id: Uuid,
        approver_id: Uuid,
        comment: Option<String>,
    ) -> Result<ApprovalRequest, Error>;

    async fn cancel_request(
        &self,
        request_id: Uuid,
        cancelled_by: Uuid,
    ) -> Result<ApprovalRequest, Error>;

    async fn get_request(&self, request_id: Uuid) -> Result<ApprovalRequest, Error>;

    async fn list_requests_for_team(
        &self,
        team_id: Option<Uuid>,
        statuses: Option<Vec<ApprovalStatus>>,
        page_number: Option<i32>,
        page_size: Option<i32>,
    ) -> Result<(Vec<ApprovalRequest>, i64), Error>;

    async fn auto_approve_request(
        &self,
        request: ApprovalRequest,
    ) -> Result<ApprovalRequest, Error>;

    async fn preview_stage_change_policy(
        &self,
        team_id: Uuid,
        environment_id: Uuid,
        requested_status: &str,
    ) -> Result<ApprovalPolicyPreview, Error>;

    fn clone_box(&self) -> Box<dyn ApprovalLogic>;
}

impl Clone for Box<dyn ApprovalLogic> {
    fn clone(&self) -> Box<dyn ApprovalLogic> {
        self.clone_box()
    }
}

pub fn approval_logic(
    approval_repository: Box<dyn ApprovalRepository>,
    feature_repository: Box<dyn FeatureRepository>,
    environment_logic: Box<dyn EnvironmentLogic>,
    role_repository: Box<dyn RoleRepository>,
    approval_events_tx: broadcast::Sender<ApprovalRequestEvent>,
    feature_updates_tx: broadcast::Sender<crate::grpc::pb::FeatureUpdate>,
) -> Box<dyn ApprovalLogic> {
    approval_logic_with_notifications(
        approval_repository,
        feature_repository,
        environment_logic,
        role_repository,
        approval_events_tx,
        feature_updates_tx,
        None,
        None,
    )
}

pub fn approval_logic_with_notifications(
    approval_repository: Box<dyn ApprovalRepository>,
    feature_repository: Box<dyn FeatureRepository>,
    environment_logic: Box<dyn EnvironmentLogic>,
    role_repository: Box<dyn RoleRepository>,
    approval_events_tx: broadcast::Sender<ApprovalRequestEvent>,
    feature_updates_tx: broadcast::Sender<crate::grpc::pb::FeatureUpdate>,
    notification_logic: Option<Box<dyn crate::logic::notification::NotificationLogic>>,
    judgments: Option<Arc<JudgmentService>>,
) -> Box<dyn ApprovalLogic> {
    Box::new(ApprovalLogicImpl {
        db_pool: None,
        approval_repository,
        feature_repository,
        environment_logic,
        role_repository,
        approval_events_tx,
        feature_updates_tx,
        notification_logic,
        judgments,
    })
}

pub fn approval_logic_with_pool(
    db_pool: PgPool,
    approval_repository: Box<dyn ApprovalRepository>,
    feature_repository: Box<dyn FeatureRepository>,
    environment_logic: Box<dyn EnvironmentLogic>,
    role_repository: Box<dyn RoleRepository>,
    approval_events_tx: broadcast::Sender<ApprovalRequestEvent>,
    feature_updates_tx: broadcast::Sender<crate::grpc::pb::FeatureUpdate>,
) -> Box<dyn ApprovalLogic> {
    approval_logic_with_pool_and_notifications(
        db_pool,
        approval_repository,
        feature_repository,
        environment_logic,
        role_repository,
        approval_events_tx,
        feature_updates_tx,
        None,
        None,
    )
}

pub fn approval_logic_with_pool_and_notifications(
    db_pool: PgPool,
    approval_repository: Box<dyn ApprovalRepository>,
    feature_repository: Box<dyn FeatureRepository>,
    environment_logic: Box<dyn EnvironmentLogic>,
    role_repository: Box<dyn RoleRepository>,
    approval_events_tx: broadcast::Sender<ApprovalRequestEvent>,
    feature_updates_tx: broadcast::Sender<crate::grpc::pb::FeatureUpdate>,
    notification_logic: Option<Box<dyn crate::logic::notification::NotificationLogic>>,
    judgments: Option<Arc<JudgmentService>>,
) -> Box<dyn ApprovalLogic> {
    Box::new(ApprovalLogicImpl {
        db_pool: Some(db_pool),
        approval_repository,
        feature_repository,
        environment_logic,
        role_repository,
        approval_events_tx,
        feature_updates_tx,
        notification_logic,
        judgments,
    })
}

#[derive(Clone)]
struct ApprovalLogicImpl {
    db_pool: Option<PgPool>,
    approval_repository: Box<dyn ApprovalRepository>,
    feature_repository: Box<dyn FeatureRepository>,
    environment_logic: Box<dyn EnvironmentLogic>,
    role_repository: Box<dyn RoleRepository>,
    approval_events_tx: broadcast::Sender<ApprovalRequestEvent>,
    feature_updates_tx: broadcast::Sender<crate::grpc::pb::FeatureUpdate>,
    notification_logic: Option<Box<dyn crate::logic::notification::NotificationLogic>>,
    /// Present only when TypeSafe judgments are available (`TYPESAFE_API_KEY` set).
    judgments: Option<Arc<JudgmentService>>,
}

#[derive(Clone, Debug)]
struct ApprovalRoutingDecision {
    eligible_approver_ids: Vec<Uuid>,
    reason: String,
}

impl ApprovalLogicImpl {
    fn dispatch_notification(&self, event: crate::logic::notification::NotificationEvent) {
        if let Some(logic) = &self.notification_logic {
            crate::logic::notification::spawn_notification_dispatch(logic.clone_box(), event);
        }
    }

    async fn notify_edge_servers(&self, feature_id: Uuid) {
        if let Ok(db_feature) = self.feature_repository.get_feature_by_id(feature_id).await
            && let Ok(full) = crate::broadcast::map_db_feature_to_full_for_broadcast(
                self.feature_repository.as_ref(),
                db_feature,
            )
            .await
        {
            let _ = self
                .feature_updates_tx
                .send(crate::grpc::pb::FeatureUpdate {
                    message_id: uuid::Uuid::new_v4().to_string(),
                    action: crate::grpc::pb::feature_update::Action::Upsert as i32,
                    feature: Some(full),
                    feature_key: String::new(),
                    error: String::new(),
                });
        }
    }

    /// Queues the background AI risk assessment of a new request. It never
    /// fails the request. Every policy mode except `off` assesses;
    /// `gate_auto_approve` and `require_extra_approver` act on a finished
    /// assessment (see `database::approval` and `ApprovalRiskHandler::apply`).
    /// Returns whether an assessment was queued.
    async fn submit_risk_assessment(
        &self,
        feature: &DbFeature,
        stage: &FeaturePipelineStage,
        policy: &ApprovalPolicy,
        request: &ApprovalRequest,
    ) -> bool {
        let Some(judgments) = &self.judgments else {
            return false;
        };
        if policy.ai_risk_mode == "off"
            || !judgments
                .team_enabled(feature.team_id, JudgmentKind::ApprovalRisk.feature())
                .await
        {
            return false;
        }

        let environment = match self
            .environment_logic
            .get_environment_by_id(ID::from(stage.environment_id))
            .await
        {
            Ok(environment) => environment,
            Err(err) => {
                warn!(
                    "Skipping AI risk assessment of approval request {}: {err}",
                    request.id
                );
                return false;
            }
        };
        let input =
            approval_risk::build_input(feature, stage, &environment, &request.change_payload);
        match judgments
            .submit(
                feature.team_id,
                JudgmentKind::ApprovalRisk,
                SubjectType::ApprovalRequest,
                request.id,
                input,
            )
            .await
        {
            Ok(_) => true,
            Err(err) => {
                warn!(
                    "Could not queue AI risk assessment of approval request {}: {err}",
                    request.id
                );
                false
            }
        }
    }

    async fn get_applicable_policy(
        &self,
        team_id: Uuid,
        environment_id: Uuid,
    ) -> Result<Option<ApprovalPolicy>, Error> {
        let env = self
            .environment_logic
            .get_environment_by_id(ID::from(environment_id))
            .await?;

        let policies = self
            .approval_repository
            .list_policies_for_team(team_id)
            .await?;
        let mut applicable: Vec<ApprovalPolicy> = policies
            .into_iter()
            .filter(|policy| {
                policy_applies(
                    policy,
                    environment_id,
                    env.environment_type.as_str(), // Check environment type instead of name
                )
            })
            .collect();

        if applicable.is_empty() {
            return Ok(None);
        }

        applicable.sort_by(|left, right| {
            let left_scope = policy_scope_rank(left, environment_id, env.environment_type.as_str());
            let right_scope =
                policy_scope_rank(right, environment_id, env.environment_type.as_str());
            right_scope
                .cmp(&left_scope)
                .then_with(|| {
                    right
                        .auto_approve_after_hours
                        .is_none()
                        .cmp(&left.auto_approve_after_hours.is_none())
                })
                .then_with(|| right.created_at.cmp(&left.created_at))
        });

        Ok(applicable.into_iter().next())
    }

    async fn count_eligible_approvers(
        &self,
        policy: &ApprovalPolicy,
    ) -> Result<Option<i64>, Error> {
        let Some(_) = &self.db_pool else {
            return Ok(None);
        };

        let routing = self.resolve_approval_routing(policy, None).await?;
        Ok(Some(routing.eligible_approver_ids.len() as i64))
    }

    fn routing_reason(policy: &ApprovalPolicy) -> String {
        match (
            policy.approver_user_ids.is_empty(),
            policy.approver_role_ids.is_empty(),
            policy.fallback_to_roles,
        ) {
            (false, false, true) => "Explicit approvers plus role fallback".to_string(),
            (false, _, false) => "Explicit approvers only".to_string(),
            (true, false, _) => "Role-based approvers".to_string(),
            _ => "No approver routing source configured".to_string(),
        }
    }

    /// Resolves who may vote on requests under `policy`. `requester_id` is excluded
    /// from the result: nobody approves their own request, so counting them would let
    /// a request be created that can never reach its required approvals.
    async fn resolve_approval_routing(
        &self,
        policy: &ApprovalPolicy,
        requester_id: Option<Uuid>,
    ) -> Result<ApprovalRoutingDecision, Error> {
        let Some(pool) = &self.db_pool else {
            return Ok(ApprovalRoutingDecision {
                eligible_approver_ids: Vec::new(),
                reason: "Routing not resolved without database pool; legacy role checks apply"
                    .to_string(),
            });
        };

        let eligible_approver_ids = sqlx::query_scalar::<_, Vec<Uuid>>(
            r#"
            SELECT COALESCE(ARRAY_AGG(DISTINCT u.id ORDER BY u.id), '{}'::uuid[])
            FROM users u
            WHERE (
                  EXISTS (
                      SELECT 1 FROM user_teams ut
                      WHERE ut.user_id = u.id AND ut.team_id = $1
                  )
                  -- A system client's shadow user is not a team member, but it
                  -- belongs to its client's team while the client is active.
                  OR EXISTS (
                      SELECT 1 FROM system_clients sc
                      WHERE sc.id = u.id
                        AND sc.team_id = $1
                        AND sc.enabled = TRUE
                        AND sc.expires_at > NOW()
                  )
              )
              AND u.enabled = TRUE
              AND ($5::uuid IS NULL OR u.id <> $5)
              AND EXISTS (
                  SELECT 1
                  FROM user_roles approver_ur
                  JOIN roles approver_role ON approver_role.id = approver_ur.role_id
                  WHERE approver_ur.user_id = u.id
                    AND LOWER(approver_role.name) = LOWER('Approver')
              )
              AND (
                  (cardinality($3::uuid[]) > 0 AND u.id = ANY($3))
                  OR (
                      ($4::boolean OR cardinality($3::uuid[]) = 0)
                      AND cardinality($2::uuid[]) > 0
                      AND EXISTS (
                          SELECT 1
                          FROM user_roles policy_ur
                          WHERE policy_ur.user_id = u.id
                            AND policy_ur.role_id = ANY($2)
                      )
                  )
              )
            "#,
        )
        .bind(policy.team_id)
        .bind(&policy.approver_role_ids)
        .bind(&policy.approver_user_ids)
        .bind(policy.fallback_to_roles)
        .bind(requester_id)
        .fetch_one(pool)
        .await
        .map_err(Error::DatabaseError)?;

        Ok(ApprovalRoutingDecision {
            eligible_approver_ids,
            reason: Self::routing_reason(policy),
        })
    }

    async fn ensure_request_has_eligible_approvers(
        &self,
        policy: &ApprovalPolicy,
        routing: &ApprovalRoutingDecision,
    ) -> Result<(), Error> {
        if self.db_pool.is_none() {
            return Ok(());
        }

        let eligible_count = routing.eligible_approver_ids.len() as i32;
        if eligible_count < policy.required_approvers {
            return Err(Error::InvalidInput(format!(
                "Approval request cannot be created: only {eligible_count} eligible approver(s) found for {} required approval(s)",
                policy.required_approvers
            )));
        }

        Ok(())
    }

    async fn user_can_admin_override(&self, approver_id: Uuid) -> Result<bool, Error> {
        if let Some(pool) = &self.db_pool {
            let is_admin = sqlx::query_scalar::<_, bool>(
                r#"
                SELECT is_admin
                FROM users
                WHERE id = $1
                  AND enabled = TRUE
                "#,
            )
            .bind(approver_id)
            .fetch_optional(pool)
            .await
            .map_err(Error::DatabaseError)?
            .unwrap_or(false);

            if is_admin {
                return Ok(true);
            }
        }

        self.role_repository
            .user_has_role(approver_id, "Team Admin")
            .await
    }

    async fn ensure_user_can_vote(
        &self,
        request: &ApprovalRequest,
        policy: &ApprovalPolicy,
        approver_id: Uuid,
        vote: &ApprovalVoteValue,
    ) -> Result<(), Error> {
        // The requester can reject their own request, but never approve it, and an
        // admin override does not lift that restriction.
        if *vote == ApprovalVoteValue::Approve && request.requested_by == approver_id {
            return Err(Error::SelfApprovalNotAllowed);
        }

        if request.admin_override_enabled
            && policy.allow_admin_override
            && self.user_can_admin_override(approver_id).await?
        {
            return Ok(());
        }

        let has_approver_role = self
            .role_repository
            .user_has_role(approver_id, "Approver")
            .await?;

        if !has_approver_role {
            return Err(Error::InvalidInput(
                "User does not have 'Approver' role required to vote on approval requests".into(),
            ));
        }

        if !request.eligible_approver_ids.is_empty() {
            // Routing excludes the requester from the eligible set, but they keep the
            // right to reject their own request.
            let rejecting_own_request =
                *vote == ApprovalVoteValue::Reject && request.requested_by == approver_id;
            if rejecting_own_request || request.eligible_approver_ids.contains(&approver_id) {
                return Ok(());
            }

            return Err(Error::InvalidInput(
                "User is not an eligible approver for this approval request".into(),
            ));
        }

        let user_roles = self.role_repository.get_user_roles(approver_id).await?;
        let user_role_ids: Vec<Uuid> = user_roles.iter().map(|r| r.id).collect();
        let has_policy_role = policy
            .approver_role_ids
            .iter()
            .any(|policy_role_id| user_role_ids.contains(policy_role_id));

        if !has_policy_role {
            return Err(Error::InvalidInput(
                "User does not have any of the required roles specified in this approval policy"
                    .into(),
            ));
        }

        Ok(())
    }

    async fn publish_event(&self, request: &ApprovalRequest, team_id: Uuid) -> Result<(), Error> {
        let votes = self
            .approval_repository
            .list_votes_for_request(request.id)
            .await
            .unwrap_or_default();
        let _ = self.approval_events_tx.send(ApprovalRequestEvent {
            request: request.clone(),
            team_id,
            votes,
        });
        Ok(())
    }

    async fn policy_team_id(&self, policy_id: Uuid) -> Result<Uuid, Error> {
        let policy = self
            .approval_repository
            .get_policy_by_id(policy_id)
            .await?
            .ok_or(Error::NotFound(policy_id))?;
        Ok(policy.team_id)
    }

    fn stage_change_request_kind(request: &ApprovalRequest) -> Option<&'static str> {
        if request.change_type != "stage_change" {
            return None;
        }

        let next_status = request
            .change_payload
            .get("next_status")
            .and_then(|value| value.as_str())?;

        match next_status {
            "DEPLOYMENT_REQUESTED" => Some("deployment"),
            "ROLLBACK_REQUESTED" => Some("rollback"),
            _ => None,
        }
    }

    fn request_environment_id(request: &ApprovalRequest) -> Option<Uuid> {
        request.environment_id.or_else(|| {
            request
                .change_payload
                .get("environment_id")
                .and_then(|value| value.as_str())
                .and_then(|value| Uuid::parse_str(value).ok())
        })
    }

    async fn resolve_environment_name(&self, request: &ApprovalRequest) -> Option<String> {
        let environment_id = Self::request_environment_id(request)?;
        self.environment_logic
            .get_environment_by_id(ID::from(environment_id))
            .await
            .ok()
            .map(|environment| environment.name)
    }

    fn feature_type_to_string(feature_type: &FeatureType) -> String {
        match feature_type {
            FeatureType::Simple => "Simple".to_string(),
            FeatureType::Contextual => "Contextual".to_string(),
        }
    }

    fn variant_value_type_to_string(value_type: VariantValueType) -> String {
        match value_type {
            VariantValueType::String => "string".to_string(),
            VariantValueType::Number => "number".to_string(),
            VariantValueType::Boolean => "boolean".to_string(),
            VariantValueType::Json => "json".to_string(),
        }
    }

    fn variant_selection_mode_to_string(mode: VariantSelectionMode) -> String {
        match mode {
            VariantSelectionMode::SpecificVariant => "SPECIFIC_VARIANT".to_string(),
            VariantSelectionMode::WeightedSplit => "WEIGHTED_SPLIT".to_string(),
        }
    }

    fn logic_operator_to_string(operator: LogicOperator) -> String {
        match operator {
            LogicOperator::Or => "OR".to_string(),
            LogicOperator::And => "AND".to_string(),
        }
    }

    async fn build_approval_snapshot(
        &self,
        feature: &DbFeature,
        fallback_stage: &FeaturePipelineStage,
    ) -> Result<FeatureConfigSnapshot, Error> {
        let mut dependencies = feature
            .dependencies
            .iter()
            .map(|dependency| dependency.depends_on_id)
            .collect::<Vec<_>>();
        dependencies.sort();

        let mut stages = self
            .feature_repository
            .get_feature_stages(feature.id)
            .await
            .unwrap_or_else(|_| vec![fallback_stage.clone()]);
        if !stages.iter().any(|stage| stage.id == fallback_stage.id) {
            stages.push(fallback_stage.clone());
        }
        stages.sort_by_key(|stage| (stage.order_index, stage.id));

        let stage_snapshots = stages
            .iter()
            .map(|stage| FeatureStageSnapshot {
                id: stage.id,
                environment_id: stage.environment_id,
                order_index: stage.order_index,
                parent_stage_id: stage.parent_stage_id,
                position: stage.position.clone(),
                status: stage.status.clone(),
                enabled: stage.enabled,
            })
            .collect::<Vec<_>>();

        let mut variants = self
            .feature_repository
            .get_feature_variants(feature.id)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|variant| FeatureVariantSnapshot {
                control: variant.control,
                value: variant.value,
                value_type: Self::variant_value_type_to_string(variant.value_type),
                description: variant.description,
            })
            .collect::<Vec<_>>();
        variants.sort_by(|left, right| left.control.cmp(&right.control));

        let mut criteria = Vec::new();
        for stage in &stages {
            let stage_criteria = self
                .feature_repository
                .get_stage_criteria(stage.id)
                .await
                .unwrap_or_default();
            for criterion in stage_criteria {
                let mut rule_groups = criterion
                    .rule_groups
                    .into_iter()
                    .map(|group| {
                        let mut conditions = group
                            .conditions
                            .into_iter()
                            .map(|condition| RuleConditionSnapshot {
                                id: condition.id,
                                context_key: condition.context_key,
                                operator: condition.operator,
                                value: condition.value,
                                order_index: condition.order_index,
                            })
                            .collect::<Vec<_>>();
                        conditions.sort_by_key(|condition| (condition.order_index, condition.id));

                        RuleGroupSnapshot {
                            id: group.id,
                            logic_operator: Self::logic_operator_to_string(group.logic_operator),
                            conditions,
                        }
                    })
                    .collect::<Vec<_>>();
                rule_groups.sort_by_key(|group| group.id);

                let mut allocations = criterion
                    .variant_allocations
                    .into_iter()
                    .map(|allocation| VariantAllocationSnapshot {
                        variant_control: allocation.variant_control,
                        weight: allocation.weight,
                    })
                    .collect::<Vec<_>>();
                allocations.sort_by(|left, right| left.variant_control.cmp(&right.variant_control));

                criteria.push(StageCriterionSnapshot {
                    id: criterion.id,
                    stage_id: criterion.stage_id,
                    priority: criterion.priority,
                    variant_selection_mode: Self::variant_selection_mode_to_string(
                        criterion.variant_selection_mode,
                    ),
                    selected_variant_control: criterion.selected_variant_control,
                    variant_allocations: allocations,
                    rule_groups,
                });
            }
        }
        criteria.sort_by_key(|criterion| (criterion.stage_id, criterion.priority, criterion.id));

        Ok(FeatureConfigSnapshot {
            schema_version: 1,
            feature: FeatureSnapshotMetadata {
                id: feature.id,
                team_id: feature.team_id,
                key: feature.key.clone(),
                description: feature.description.clone(),
                feature_type: Self::feature_type_to_string(&feature.feature_type),
                enabled: feature.active,
                created_at: feature.created_at,
                kill_switch_enabled: feature.kill_switch_enabled,
                kill_switch_activated_at: feature.kill_switch_activated_at,
                rollback_scheduled_at: feature.rollback_scheduled_at,
                emergency_override_reason: None,
                emergency_override_expires_at: None,
                emergency_override_actor_id: None,
                emergency_override_applied_at: None,
                lifecycle_stage: feature.lifecycle_stage.clone(),
                owner: feature.owner.clone(),
                purpose: feature.purpose.clone(),
                reference_url: feature.reference_url.clone(),
                expires_at: feature.expires_at,
                cleanup_reason: feature.cleanup_reason.clone(),
                tags: feature.tags.clone(),
                archived_at: feature.archived_at,
                deprecated_at: feature.deprecated_at,
                deprecation_notice: feature.deprecation_notice.clone(),
            },
            dependencies,
            stages: stage_snapshots,
            variants,
            criteria,
        })
    }

    fn add_marker(markers: &mut Vec<String>, marker: &str) {
        if !markers.iter().any(|existing| existing == marker) {
            markers.push(marker.to_string());
        }
    }

    fn stage_change_risk_markers(
        policy: &ApprovalPolicy,
        stage: &FeaturePipelineStage,
        next_status: &str,
    ) -> Vec<String> {
        let mut markers = Vec::new();
        if policy.applies_to == "production_only"
            || stage.position.eq_ignore_ascii_case("production")
        {
            Self::add_marker(&mut markers, "production-impact");
        }
        if next_status.contains("ROLLBACK") {
            Self::add_marker(&mut markers, "emergency-action");
        }
        markers
    }

    async fn build_stage_change_snapshot_payload(
        &self,
        feature: &DbFeature,
        stage: &FeaturePipelineStage,
        next_status: &str,
        after_status: &str,
        policy: &ApprovalPolicy,
    ) -> Result<(JsonValue, JsonValue, JsonValue, Vec<String>), Error> {
        let before_snapshot = self.build_approval_snapshot(feature, stage).await?;
        let mut after_snapshot = before_snapshot.clone();
        if let Some(target_stage) = after_snapshot
            .stages
            .iter_mut()
            .find(|snapshot_stage| snapshot_stage.id == stage.id)
        {
            target_stage.status = after_status.to_string();
        }

        let before = serde_json::to_value(before_snapshot).map_err(|e| {
            Error::InvalidInput(format!("Failed to serialize approval snapshot: {e}"))
        })?;
        let after = serde_json::to_value(after_snapshot).map_err(|e| {
            Error::InvalidInput(format!("Failed to serialize approval snapshot: {e}"))
        })?;
        let diff = diff_entries_to_json(&diff_feature_snapshots(&before, &after));
        let risk_markers = Self::stage_change_risk_markers(policy, stage, next_status);

        Ok((before, after, diff, risk_markers))
    }

    async fn build_stage_change_blast_radius(
        &self,
        feature: &DbFeature,
        stage: &FeaturePipelineStage,
        next_status: &str,
    ) -> Result<JsonValue, Error> {
        let environment = self
            .environment_logic
            .get_environment_by_id(ID::from(stage.environment_id))
            .await?;
        let production = environment
            .environment_type
            .eq_ignore_ascii_case("production");
        let dependency_count = feature.dependencies.len() as i64;
        let evaluation_volume_7d = feature.evaluation_count_7d;
        let mut affected_clients = 0_i64;
        let mut affected_contexts = 0_i64;

        if let Some(pool) = &self.db_pool {
            affected_clients = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM clients WHERE team_id = $1 AND environment_id = $2",
            )
            .bind(feature.team_id)
            .bind(stage.environment_id)
            .fetch_one(pool)
            .await
            .map_err(Error::DatabaseError)?;

            affected_contexts =
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM contexts WHERE team_id = $1")
                    .bind(feature.team_id)
                    .fetch_one(pool)
                    .await
                    .map_err(Error::DatabaseError)?;
        }

        let mut score = 0;
        let mut risk_markers = Vec::new();
        if production {
            score += 3;
            risk_markers.push("production-impact");
        }
        if dependency_count >= 3 {
            score += 2;
            risk_markers.push("dependency-heavy");
        }
        if evaluation_volume_7d >= 10_000 {
            score += 2;
            risk_markers.push("high-traffic");
        }
        if next_status.contains("ROLLBACK") {
            score += 1;
            risk_markers.push("rollback-impact");
        }

        let risk_level = if score >= 5 {
            "high"
        } else if score >= 2 {
            "medium"
        } else {
            "low"
        };

        let warnings = if evaluation_volume_7d == 0 {
            vec!["No recent traffic data is available; impact estimate may be incomplete"]
        } else {
            Vec::new()
        };

        Ok(serde_json::json!({
            "riskLevel": risk_level,
            "summary": format!(
                "{risk_level} risk: 1 environment, {affected_clients} client(s), {affected_contexts} context(s), {dependency_count} dependent feature link(s), {evaluation_volume_7d} evaluations in 7d"
            ),
            "affectedEnvironments": [{
                "id": stage.environment_id.to_string(),
                "name": environment.name,
                "environmentType": environment.environment_type,
            }],
            "affectedClients": affected_clients,
            "affectedContexts": affected_contexts,
            "dependencyCount": dependency_count,
            "evaluationVolume7d": evaluation_volume_7d,
            "warnings": warnings,
            "riskMarkers": risk_markers,
        }))
    }

    async fn resolve_approver_name(&self, actor_id: Option<Uuid>) -> Option<String> {
        let approver_id = actor_id?;

        if let Some(pool) = &self.db_pool {
            let user_repo = crate::database::user::user_repository(pool.clone());
            if let Ok(user) = user_repo.get_user_by_id(approver_id).await {
                let full_name = format!("{} {}", user.first_name.trim(), user.last_name.trim())
                    .trim()
                    .to_string();
                if !full_name.is_empty() {
                    return Some(full_name);
                }
                if !user.username.trim().is_empty() {
                    return Some(user.username);
                }
            }
        }

        Some(approver_id.to_string())
    }

    async fn dispatch_stage_change_approved_notification(
        &self,
        request: &ApprovalRequest,
        team_id: Uuid,
        actor_id: Option<Uuid>,
    ) {
        let Some(kind) = Self::stage_change_request_kind(request) else {
            return;
        };

        let feature_key = self
            .feature_repository
            .get_feature_by_id(request.feature_id)
            .await
            .map(|feature| feature.key)
            .unwrap_or_else(|_| request.feature_id.to_string());

        let approver_name = self.resolve_approver_name(actor_id).await;
        let environment_id = Self::request_environment_id(request);
        let environment_name = self.resolve_environment_name(request).await;
        let was_auto_approved = matches!(request.status, ApprovalStatus::AutoApproved);
        let approval_verb = if was_auto_approved {
            "auto-approved"
        } else {
            "approved"
        };

        let subject = match environment_name.as_deref() {
            Some(environment_name) => {
                format!(
                    "Feature {kind} request {approval_verb} in {environment_name}: {feature_key}"
                )
            }
            None => format!("Feature {kind} request {approval_verb}: {feature_key}"),
        };

        let message = match (approver_name.as_deref(), environment_name.as_deref()) {
            (Some(approver_name), Some(environment_name)) => format!(
                "{approver_name} approved the {kind} request for feature '{feature_key}' in environment '{environment_name}'."
            ),
            (Some(approver_name), None) => {
                format!("{approver_name} approved the {kind} request for feature '{feature_key}'.")
            }
            (None, Some(environment_name)) if was_auto_approved => format!(
                "The {kind} request for feature '{feature_key}' was auto-approved in environment '{environment_name}'."
            ),
            (None, None) if was_auto_approved => {
                format!("The {kind} request for feature '{feature_key}' was auto-approved.")
            }
            (None, Some(environment_name)) => format!(
                "A {kind} request was approved for feature '{feature_key}' in environment '{environment_name}'."
            ),
            (None, None) => {
                format!("A {kind} request was approved for feature '{feature_key}'.")
            }
        };

        self.dispatch_notification(crate::logic::notification::NotificationEvent {
            notification_type: crate::logic::notification::NOTIFICATION_TYPE_STAGE_CHANGE_APPROVED
                .to_string(),
            team_id: Some(team_id),
            actor_id,
            recipient_user_ids: None,
            subject,
            message,
            metadata: Some(serde_json::json!({
                "approval_request_id": request.id.to_string(),
                "feature_id": request.feature_id.to_string(),
                "feature_key": feature_key,
                "team_id": team_id.to_string(),
                "environment_id": environment_id.map(|id| id.to_string()),
                "environment_name": environment_name,
                "approved_by": approver_name,
                "auto_approved": was_auto_approved,
                "status": format!("{:?}", request.status),
            })),
        });
    }

    async fn apply_vote(
        &self,
        request_id: Uuid,
        approver_id: Uuid,
        vote: ApprovalVoteValue,
        comment: Option<String>,
    ) -> Result<ApprovalRequest, Error> {
        let request = self
            .approval_repository
            .get_request_by_id(request_id)
            .await?
            .ok_or(Error::NotFound(request_id))?;
        if !matches!(request.status, ApprovalStatus::Pending) {
            return Err(Error::InvalidInput("Request is already resolved".into()));
        }

        let policy = self
            .approval_repository
            .get_policy_by_id(request.policy_id)
            .await?
            .ok_or(Error::NotFound(request.policy_id))?;
        let team_id = policy.team_id;

        self.ensure_user_can_vote(&request, &policy, approver_id, &vote)
            .await?;

        let updated = self
            .approval_repository
            .add_vote(
                CreateApprovalVoteInput {
                    request_id,
                    approver_id,
                    vote,
                    comment,
                },
                request
                    .required_approvers_override
                    .unwrap_or(policy.required_approvers),
            )
            .await?;

        self.publish_event(&updated, team_id).await?;

        if matches!(updated.status, ApprovalStatus::Approved) {
            if let Err(exec_err) = self.execute_change(&updated, approver_id).await {
                // Put the request back into pending so approvers can retry after fixing errors.
                let _ = self
                    .approval_repository
                    .update_request_status(request_id, ApprovalStatus::Pending, None)
                    .await;
                return Err(exec_err);
            }

            let final_request = self
                .approval_repository
                .update_request_status(request_id, ApprovalStatus::Approved, Some(Utc::now()))
                .await?;
            self.publish_event(&final_request, team_id).await?;

            // Notify edge servers about the feature update after approval
            self.notify_edge_servers(updated.feature_id).await;

            self.dispatch_stage_change_approved_notification(
                &final_request,
                team_id,
                Some(approver_id),
            )
            .await;

            return Ok(final_request);
        }

        if matches!(updated.status, ApprovalStatus::Rejected) {
            if let Err(exec_err) = self.execute_change(&updated, approver_id).await {
                let _ = self
                    .approval_repository
                    .update_request_status(request_id, ApprovalStatus::Pending, None)
                    .await;
                return Err(exec_err);
            }

            let final_request = self
                .approval_repository
                .update_request_status(request_id, ApprovalStatus::Rejected, None)
                .await?;
            self.publish_event(&final_request, team_id).await?;

            // Notify edge servers about the feature update after rejection
            self.notify_edge_servers(updated.feature_id).await;

            return Ok(final_request);
        }

        Ok(updated)
    }

    async fn apply_vote_tx(
        &self,
        request_id: Uuid,
        approver_id: Uuid,
        vote: ApprovalVoteValue,
        comment: Option<String>,
    ) -> Result<ApprovalRequest, Error> {
        let request = self
            .approval_repository
            .get_request_by_id(request_id)
            .await?
            .ok_or(Error::NotFound(request_id))?;
        if !matches!(request.status, ApprovalStatus::Pending) {
            return Err(Error::InvalidInput("Request is already resolved".into()));
        }

        let policy = self
            .approval_repository
            .get_policy_by_id(request.policy_id)
            .await?
            .ok_or(Error::NotFound(request.policy_id))?;
        let team_id = policy.team_id;

        self.ensure_user_can_vote(&request, &policy, approver_id, &vote)
            .await?;

        let pool = self
            .db_pool
            .as_ref()
            .ok_or_else(|| Error::InvalidInput("Transaction pool not configured".into()))?;
        let mut tx = pool.begin().await.map_err(Error::DatabaseError)?;
        let approval_repo_tx = approval_repository_tx(pool.clone());
        let feature_repo_tx = feature_repository_tx(pool.clone());

        let updated = approval_repo_tx
            .add_vote_tx(
                &mut tx,
                CreateApprovalVoteInput {
                    request_id,
                    approver_id,
                    vote,
                    comment,
                },
                request
                    .required_approvers_override
                    .unwrap_or(policy.required_approvers),
            )
            .await?;

        if matches!(updated.status, ApprovalStatus::Approved) {
            if let Err(exec_err) = self
                .execute_change_tx(&feature_repo_tx, &mut tx, &updated, approver_id)
                .await
            {
                let pending = approval_repo_tx
                    .update_request_status_tx(&mut tx, request_id, ApprovalStatus::Pending, None)
                    .await?;
                tx.commit().await.map_err(Error::DatabaseError)?;
                self.publish_event(&pending, team_id).await?;
                return Err(exec_err);
            }

            let final_request = approval_repo_tx
                .update_request_status_tx(
                    &mut tx,
                    request_id,
                    ApprovalStatus::Approved,
                    Some(Utc::now()),
                )
                .await?;
            tx.commit().await.map_err(Error::DatabaseError)?;

            self.publish_event(&updated, team_id).await?;
            self.publish_event(&final_request, team_id).await?;
            self.notify_edge_servers(updated.feature_id).await;

            self.dispatch_stage_change_approved_notification(
                &final_request,
                team_id,
                Some(approver_id),
            )
            .await;

            return Ok(final_request);
        }

        if matches!(updated.status, ApprovalStatus::Rejected) {
            if let Err(exec_err) = self
                .execute_change_tx(&feature_repo_tx, &mut tx, &updated, approver_id)
                .await
            {
                let pending = approval_repo_tx
                    .update_request_status_tx(&mut tx, request_id, ApprovalStatus::Pending, None)
                    .await?;
                tx.commit().await.map_err(Error::DatabaseError)?;
                self.publish_event(&pending, team_id).await?;
                return Err(exec_err);
            }

            let final_request = approval_repo_tx
                .update_request_status_tx(&mut tx, request_id, ApprovalStatus::Rejected, None)
                .await?;
            tx.commit().await.map_err(Error::DatabaseError)?;

            self.publish_event(&updated, team_id).await?;
            self.publish_event(&final_request, team_id).await?;
            self.notify_edge_servers(updated.feature_id).await;

            return Ok(final_request);
        }

        tx.commit().await.map_err(Error::DatabaseError)?;
        self.publish_event(&updated, team_id).await?;
        Ok(updated)
    }

    async fn execute_change(&self, request: &ApprovalRequest, actor_id: Uuid) -> Result<(), Error> {
        if request.change_type != "stage_change" {
            return Ok(());
        }

        let stage_id = request
            .change_payload
            .get("stage_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::InvalidInput("Missing stage_id in change_payload".into()))
            .and_then(|s| Uuid::parse_str(s).map_err(|e| Error::InvalidInput(e.to_string())))?;

        let next_status = request
            .change_payload
            .get("next_status")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::InvalidInput("Missing next_status in change_payload".into()))?;

        let approval_target_status = request
            .change_payload
            .get("approval_target_status")
            .and_then(|v| v.as_str());
        let rejection_target_status = request
            .change_payload
            .get("rejection_target_status")
            .and_then(|v| v.as_str());

        let final_status = match request.status {
            ApprovalStatus::Approved => approval_target_status.unwrap_or(next_status),
            ApprovalStatus::Rejected => rejection_target_status.unwrap_or(next_status),
            _ => return Ok(()),
        };

        self.feature_repository
            .approve_or_reject_stage_change(stage_id, final_status, actor_id)
            .await?;

        Ok(())
    }

    async fn execute_change_tx<R>(
        &self,
        feature_repo: &R,
        conn: &mut sqlx::PgConnection,
        request: &ApprovalRequest,
        actor_id: Uuid,
    ) -> Result<(), Error>
    where
        R: FeatureRepositoryTx,
    {
        if request.change_type != "stage_change" {
            return Ok(());
        }

        let stage_id = request
            .change_payload
            .get("stage_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::InvalidInput("Missing stage_id in change_payload".into()))
            .and_then(|s| Uuid::parse_str(s).map_err(|e| Error::InvalidInput(e.to_string())))?;

        let next_status = request
            .change_payload
            .get("next_status")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::InvalidInput("Missing next_status in change_payload".into()))?;

        let approval_target_status = request
            .change_payload
            .get("approval_target_status")
            .and_then(|v| v.as_str());
        let rejection_target_status = request
            .change_payload
            .get("rejection_target_status")
            .and_then(|v| v.as_str());

        let final_status = match request.status {
            ApprovalStatus::Approved => approval_target_status.unwrap_or(next_status),
            ApprovalStatus::Rejected => rejection_target_status.unwrap_or(next_status),
            _ => return Ok(()),
        };

        feature_repo
            .approve_or_reject_stage_change_tx(conn, stage_id, final_status, actor_id)
            .await?;

        Ok(())
    }
}

#[async_trait::async_trait]
impl ApprovalLogic for ApprovalLogicImpl {
    async fn maybe_create_stage_change_request(
        &self,
        feature: &DbFeature,
        stage: &FeaturePipelineStage,
        next_status: &str,
        requested_by: Uuid,
    ) -> Result<Option<ApprovalRequest>, Error> {
        if !status_requires_interception(next_status) {
            return Ok(None);
        }

        let Some(policy) = self
            .get_applicable_policy(feature.team_id, stage.environment_id)
            .await?
        else {
            return Ok(None);
        };

        let approval_target_status = match next_status {
            "DEPLOYMENT_REQUESTED" => StageStatus::DeploymentApproved.as_str(),
            "ROLLBACK_REQUESTED" => StageStatus::RollbackApproved.as_str(),
            other => other,
        };
        let rejection_target_status = match next_status {
            "DEPLOYMENT_REQUESTED" => StageStatus::DeploymentRejected.as_str(),
            "ROLLBACK_REQUESTED" => StageStatus::RollbackRejected.as_str(),
            other => other,
        };
        let after_status = approval_target_status;
        let snapshot_id = Uuid::new_v4();
        let (before_snapshot, after_snapshot, diff, risk_markers) = self
            .build_stage_change_snapshot_payload(feature, stage, next_status, after_status, &policy)
            .await?;
        let blast_radius = self
            .build_stage_change_blast_radius(feature, stage, next_status)
            .await?;
        let mut risk_markers = risk_markers;
        if let Some(markers) = blast_radius
            .get("riskMarkers")
            .and_then(|value| value.as_array())
        {
            for marker in markers.iter().filter_map(|value| value.as_str()) {
                Self::add_marker(&mut risk_markers, marker);
            }
        }
        let routing = self
            .resolve_approval_routing(&policy, Some(requested_by))
            .await?;
        self.ensure_request_has_eligible_approvers(&policy, &routing)
            .await?;
        let eligible_approver_id_strings = routing
            .eligible_approver_ids
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>();
        let approver_user_id_strings = policy
            .approver_user_ids
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>();
        let approver_role_id_strings = policy
            .approver_role_ids
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>();

        let change_payload = serde_json::json!({
            "stage_id": stage.id.to_string(),
            "next_status": next_status,
            "approval_target_status": approval_target_status,
            "rejection_target_status": rejection_target_status,
            "previous_status": stage.status,
            "feature_id": feature.id.to_string(),
            "environment_id": stage.environment_id.to_string(),
            "before": { "status": stage.status },
            "after": { "status": after_status },
            "snapshot_id": snapshot_id.to_string(),
            "snapshot_schema_version": 1,
            "before_snapshot": before_snapshot,
            "after_snapshot": after_snapshot,
            "diff": diff,
            "risk_markers": risk_markers,
            "blast_radius": blast_radius,
            "policy": {
                "id": policy.id.to_string(),
                "name": policy.name.clone(),
                "applies_to": policy.applies_to.clone(),
                "required_approvers": policy.required_approvers,
                "approver_role_ids": approver_role_id_strings,
                "approver_user_ids": approver_user_id_strings,
                "allow_admin_override": policy.allow_admin_override,
                "fallback_to_roles": policy.fallback_to_roles,
                "auto_approve_after_hours": policy.auto_approve_after_hours,
            },
            "routing": {
                "eligible_approver_ids": eligible_approver_id_strings,
                "reason": routing.reason.clone(),
                "admin_override_enabled": policy.allow_admin_override,
                "fallback_to_roles": policy.fallback_to_roles,
            },
            "links": {
                "feature": format!("/features/{}/edit", feature.id),
                "metrics": format!("/dashboard/metrics?featureId={}", feature.id),
                "audit_history": format!("/features/{}/edit?tab=history", feature.id),
            },
        });

        let request = self
            .approval_repository
            .create_request(CreateApprovalRequestInput {
                policy_id: policy.id,
                feature_id: feature.id,
                environment_id: Some(stage.environment_id),
                change_type: "stage_change".into(),
                change_payload,
                change_description: Some(format!(
                    "Stage {} -> {} for feature {}",
                    stage.status, next_status, feature.key
                )),
                requested_by,
                eligible_approver_ids: routing.eligible_approver_ids.clone(),
                routing_reason: Some(routing.reason.clone()),
                admin_override_enabled: policy.allow_admin_override,
            })
            .await?;

        // Notify subscribers about the newly created request so dashboards/badges update immediately.
        self.publish_event(&request, feature.team_id).await?;

        // After the event, so the AI settings reads and the judgment write do
        // not delay it. Once an assessment is queued, a second event lets
        // streams show it as pending.
        if self
            .submit_risk_assessment(feature, stage, &policy, &request)
            .await
        {
            self.publish_event(&request, feature.team_id).await?;
        }

        Ok(Some(request))
    }

    async fn approve_request(
        &self,
        request_id: Uuid,
        approver_id: Uuid,
        comment: Option<String>,
    ) -> Result<ApprovalRequest, Error> {
        if self.db_pool.is_some() {
            self.apply_vote_tx(request_id, approver_id, ApprovalVoteValue::Approve, comment)
                .await
        } else {
            self.apply_vote(request_id, approver_id, ApprovalVoteValue::Approve, comment)
                .await
        }
    }

    async fn reject_request(
        &self,
        request_id: Uuid,
        approver_id: Uuid,
        comment: Option<String>,
    ) -> Result<ApprovalRequest, Error> {
        if self.db_pool.is_some() {
            self.apply_vote_tx(request_id, approver_id, ApprovalVoteValue::Reject, comment)
                .await
        } else {
            self.apply_vote(request_id, approver_id, ApprovalVoteValue::Reject, comment)
                .await
        }
    }

    async fn cancel_request(
        &self,
        request_id: Uuid,
        _cancelled_by: Uuid,
    ) -> Result<ApprovalRequest, Error> {
        if let Some(pool) = &self.db_pool {
            let existing = self
                .approval_repository
                .get_request_by_id(request_id)
                .await?
                .ok_or(Error::NotFound(request_id))?;
            let team_id = self.policy_team_id(existing.policy_id).await?;

            let stage_reset: Option<(Uuid, String)> = if existing.change_type == "stage_change" {
                let stage_id = existing
                    .change_payload
                    .get("stage_id")
                    .and_then(|v| v.as_str())
                    .and_then(|s| Uuid::parse_str(s).ok());
                let previous_status = existing
                    .change_payload
                    .get("previous_status")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                match (stage_id, previous_status) {
                    (Some(id), Some(status)) => Some((id, status)),
                    _ => None,
                }
            } else {
                None
            };

            let mut tx = pool.begin().await.map_err(Error::DatabaseError)?;
            let approval_repo_tx = approval_repository_tx(pool.clone());
            let feature_repo_tx = feature_repository_tx(pool.clone());

            let updated = approval_repo_tx
                .update_request_status_tx(&mut tx, request_id, ApprovalStatus::Cancelled, None)
                .await?;

            if let Some((stage_id, status)) = stage_reset {
                let _ = feature_repo_tx
                    .reset_stage_status_tx(&mut tx, stage_id, status.as_str())
                    .await;
            }

            tx.commit().await.map_err(Error::DatabaseError)?;
            self.publish_event(&updated, team_id).await?;
            return Ok(updated);
        }

        let existing = self
            .approval_repository
            .get_request_by_id(request_id)
            .await?
            .ok_or(Error::NotFound(request_id))?;
        let team_id = self.policy_team_id(existing.policy_id).await?;

        let stage_reset: Option<(Uuid, String)> = if existing.change_type == "stage_change" {
            let stage_id = existing
                .change_payload
                .get("stage_id")
                .and_then(|v| v.as_str())
                .and_then(|s| Uuid::parse_str(s).ok());
            let previous_status = existing
                .change_payload
                .get("previous_status")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            match (stage_id, previous_status) {
                (Some(id), Some(status)) => Some((id, status)),
                _ => None,
            }
        } else {
            None
        };

        let updated = self
            .approval_repository
            .update_request_status(request_id, ApprovalStatus::Cancelled, None)
            .await?;

        if let Some((stage_id, status)) = stage_reset {
            let _ = self
                .feature_repository
                .reset_stage_status(stage_id, status.as_str())
                .await;
        }

        self.publish_event(&updated, team_id).await?;
        Ok(updated)
    }

    async fn get_request(&self, request_id: Uuid) -> Result<ApprovalRequest, Error> {
        self.approval_repository
            .get_request_by_id(request_id)
            .await?
            .ok_or(Error::NotFound(request_id))
    }

    async fn list_requests_for_team(
        &self,
        team_id: Option<Uuid>,
        statuses: Option<Vec<ApprovalStatus>>,
        page_number: Option<i32>,
        page_size: Option<i32>,
    ) -> Result<(Vec<ApprovalRequest>, i64), Error> {
        self.approval_repository
            .list_requests_for_team(team_id, statuses, page_number, page_size)
            .await
    }

    async fn auto_approve_request(
        &self,
        request: ApprovalRequest,
    ) -> Result<ApprovalRequest, Error> {
        if let Some(pool) = &self.db_pool {
            let team_id = self.policy_team_id(request.policy_id).await?;
            let mut tx = pool.begin().await.map_err(Error::DatabaseError)?;
            let approval_repo_tx = approval_repository_tx(pool.clone());
            let feature_repo_tx = feature_repository_tx(pool.clone());

            self.execute_change_tx(&feature_repo_tx, &mut tx, &request, SENTINEL_UUID)
                .await?;

            let updated = approval_repo_tx
                .update_request_status_tx(
                    &mut tx,
                    request.id,
                    ApprovalStatus::AutoApproved,
                    Some(Utc::now()),
                )
                .await?;
            tx.commit().await.map_err(Error::DatabaseError)?;
            self.publish_event(&updated, team_id).await?;
            self.notify_edge_servers(request.feature_id).await;

            self.dispatch_stage_change_approved_notification(&updated, team_id, None)
                .await;

            return Ok(updated);
        }

        let team_id = self.policy_team_id(request.policy_id).await?;
        self.execute_change(&request, SENTINEL_UUID).await?;
        let updated = self
            .approval_repository
            .update_request_status(request.id, ApprovalStatus::AutoApproved, Some(Utc::now()))
            .await?;
        self.publish_event(&updated, team_id).await?;

        // Notify edge servers about the feature update after auto-approval
        self.notify_edge_servers(request.feature_id).await;

        self.dispatch_stage_change_approved_notification(&updated, team_id, None)
            .await;

        Ok(updated)
    }

    async fn preview_stage_change_policy(
        &self,
        team_id: Uuid,
        environment_id: Uuid,
        requested_status: &str,
    ) -> Result<ApprovalPolicyPreview, Error> {
        if !status_requires_interception(requested_status) {
            return Ok(ApprovalPolicyPreview {
                change_type: "stage_change".to_string(),
                requested_status: Some(requested_status.to_string()),
                environment_id,
                outcome: ApprovalPolicyPreviewOutcome::NotRequired,
                policy: None,
                eligible_approvers_count: None,
                warnings: Vec::new(),
                reason: format!("{requested_status} does not require approval interception"),
            });
        }

        let Some(policy) = self.get_applicable_policy(team_id, environment_id).await? else {
            return Ok(ApprovalPolicyPreview {
                change_type: "stage_change".to_string(),
                requested_status: Some(requested_status.to_string()),
                environment_id,
                outcome: ApprovalPolicyPreviewOutcome::NotRequired,
                policy: None,
                eligible_approvers_count: None,
                warnings: Vec::new(),
                reason: "No enabled approval policy matches this environment".to_string(),
            });
        };

        let eligible_approvers_count = self.count_eligible_approvers(&policy).await?;
        let missing_approvers = eligible_approvers_count
            .map(|count| count < i64::from(policy.required_approvers))
            .unwrap_or(false);

        let mut warnings = Vec::new();
        if missing_approvers {
            warnings.push(format!(
                "Only {} eligible approver(s) found for {} required approval(s)",
                eligible_approvers_count.unwrap_or_default(),
                policy.required_approvers
            ));
        }

        let outcome = if missing_approvers {
            ApprovalPolicyPreviewOutcome::Blocked
        } else if policy.auto_approve_after_hours.is_some() {
            ApprovalPolicyPreviewOutcome::AutoApproved
        } else {
            ApprovalPolicyPreviewOutcome::RequiresApproval
        };

        let reason = match outcome {
            ApprovalPolicyPreviewOutcome::Blocked => {
                "Matched policy cannot be satisfied with current approvers".to_string()
            }
            ApprovalPolicyPreviewOutcome::AutoApproved => format!(
                "Matched policy will auto-approve after {} hour(s) if still pending",
                policy.auto_approve_after_hours.unwrap_or_default()
            ),
            ApprovalPolicyPreviewOutcome::RequiresApproval => {
                "Matched policy requires manual approval".to_string()
            }
            ApprovalPolicyPreviewOutcome::NotRequired => unreachable!(),
        };

        Ok(ApprovalPolicyPreview {
            change_type: "stage_change".to_string(),
            requested_status: Some(requested_status.to_string()),
            environment_id,
            outcome,
            policy: Some(policy),
            eligible_approvers_count,
            warnings,
            reason,
        })
    }

    fn clone_box(&self) -> Box<dyn ApprovalLogic> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::approval::MockApprovalRepository;
    use crate::database::entity::{FeatureType, Role};
    use crate::database::feature::MockFeatureRepository;
    use crate::database::role::MockRoleRepository;
    use crate::logic::environment::MockEnvironmentLogic;
    use crate::model::Environment;
    use chrono::Utc;
    use tokio::sync::mpsc;
    use tokio::time::{Duration, timeout};

    #[derive(Clone)]
    struct RecordingNotificationLogic {
        sender: mpsc::UnboundedSender<String>,
    }

    #[async_trait::async_trait]
    impl crate::logic::notification::NotificationLogic for RecordingNotificationLogic {
        async fn get_settings(
            &self,
        ) -> Result<crate::logic::notification::NotificationSettingsView, Error> {
            Err(Error::InvalidInput("unused_in_test".to_string()))
        }

        async fn update_channel_config(
            &self,
            _input: crate::logic::notification::UpdateNotificationChannelConfigInput,
        ) -> Result<crate::logic::notification::NotificationChannelConfigView, Error> {
            Err(Error::InvalidInput("unused_in_test".to_string()))
        }

        async fn update_preference(
            &self,
            _input: crate::logic::notification::UpdateNotificationPreferenceInput,
        ) -> Result<crate::logic::notification::NotificationPreferenceView, Error> {
            Err(Error::InvalidInput("unused_in_test".to_string()))
        }

        async fn dispatch_event(
            &self,
            event: crate::logic::notification::NotificationEvent,
        ) -> Result<(), Error> {
            let _ = self.sender.send(event.notification_type);
            Ok(())
        }

        fn clone_box(&self) -> Box<dyn crate::logic::notification::NotificationLogic> {
            Box::new(self.clone())
        }
    }

    fn policy_for_test(
        id: Uuid,
        team_id: Uuid,
        name: &str,
        applies_to: &str,
        environment_ids: Option<Vec<Uuid>>,
    ) -> ApprovalPolicy {
        ApprovalPolicy {
            id,
            team_id,
            name: name.to_string(),
            description: None,
            applies_to: applies_to.to_string(),
            environment_ids,
            required_approvers: 1,
            approver_role_ids: vec![Uuid::new_v4()],
            approver_user_ids: Vec::new(),
            allow_admin_override: false,
            fallback_to_roles: true,
            auto_approve_after_hours: None,
            enabled: true,
            created_at: Utc::now(),
            ai_risk_mode: "advisory".to_string(),
        }
    }

    #[tokio::test]
    async fn preview_stage_change_policy_uses_policy_precedence() {
        let team_id = Uuid::new_v4();
        let production_env_id = Uuid::new_v4();
        let staging_env_id = Uuid::new_v4();
        let qa_env_id = Uuid::new_v4();
        let global_policy_id = Uuid::new_v4();
        let production_policy_id = Uuid::new_v4();
        let staging_policy_id = Uuid::new_v4();

        let policies = vec![
            policy_for_test(global_policy_id, team_id, "Global", "all", None),
            policy_for_test(
                production_policy_id,
                team_id,
                "Production",
                "production_only",
                None,
            ),
            policy_for_test(
                staging_policy_id,
                team_id,
                "Staging",
                "specific_environments",
                Some(vec![staging_env_id]),
            ),
        ];

        let mut approval_repo = MockApprovalRepository::new();
        approval_repo
            .expect_list_policies_for_team()
            .with(mockall::predicate::eq(team_id))
            .times(3)
            .returning(move |_| Ok(policies.clone()));

        let mut env_logic = MockEnvironmentLogic::new();
        env_logic
            .expect_get_environment_by_id()
            .times(3)
            .returning(move |id| {
                let env_id = Uuid::try_from(id).expect("environment id");
                let environment_type = if env_id == production_env_id {
                    "Production"
                } else if env_id == staging_env_id {
                    "Staging"
                } else {
                    "Development"
                };
                Ok(Environment {
                    id: ID::from(env_id),
                    name: environment_type.to_string(),
                    active: true,
                    team_id: ID::from(team_id),
                    environment_type: environment_type.to_string(),
                })
            });

        let (approval_events_tx, _) = tokio::sync::broadcast::channel(4);
        let (feature_updates_tx, _) = tokio::sync::broadcast::channel(4);
        let logic = ApprovalLogicImpl {
            db_pool: None,
            approval_repository: Box::new(approval_repo),
            feature_repository: Box::new(MockFeatureRepository::new()),
            environment_logic: Box::new(env_logic),
            role_repository: Box::new(MockRoleRepository::new()),
            approval_events_tx,
            feature_updates_tx,
            notification_logic: None,
            judgments: None,
        };

        let production_preview = logic
            .preview_stage_change_policy(team_id, production_env_id, "DEPLOYMENT_REQUESTED")
            .await
            .expect("production preview");
        let staging_preview = logic
            .preview_stage_change_policy(team_id, staging_env_id, "DEPLOYMENT_REQUESTED")
            .await
            .expect("staging preview");
        let qa_preview = logic
            .preview_stage_change_policy(team_id, qa_env_id, "DEPLOYMENT_REQUESTED")
            .await
            .expect("qa preview");

        assert_eq!(
            production_preview.policy.map(|policy| policy.id),
            Some(production_policy_id)
        );
        assert_eq!(
            staging_preview.policy.map(|policy| policy.id),
            Some(staging_policy_id)
        );
        assert_eq!(
            qa_preview.policy.map(|policy| policy.id),
            Some(global_policy_id)
        );
        assert_eq!(
            production_preview.outcome,
            ApprovalPolicyPreviewOutcome::RequiresApproval
        );
    }

    #[tokio::test]
    async fn test_approve_request_success_with_valid_roles() {
        let mut approval_repo = MockApprovalRepository::new();
        let mut role_repo = MockRoleRepository::new();
        let feature_repo = MockFeatureRepository::new();
        let env_logic = MockEnvironmentLogic::new();

        let request_id = Uuid::new_v4();
        let approver_id = Uuid::new_v4();
        let policy_id = Uuid::new_v4();
        let team_id = Uuid::new_v4();
        let senior_engineer_role_id = Uuid::new_v4();

        // Mock the request
        let request = ApprovalRequest {
            id: request_id,
            policy_id,
            feature_id: Uuid::new_v4(),
            environment_id: Some(Uuid::new_v4()),
            change_type: "stage_change".into(),
            change_payload: serde_json::json!({}),
            change_description: None,
            requested_by: Uuid::new_v4(),
            eligible_approver_ids: Vec::new(),
            routing_reason: None,
            admin_override_enabled: false,
            status: ApprovalStatus::Pending,
            approved_count: 0,
            rejected_count: 0,
            executed_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            required_approvers_override: None,
        };

        // Mock the policy - requires "Senior Engineer" role
        let policy = ApprovalPolicy {
            id: policy_id,
            team_id,
            name: "Production Approval".into(),
            description: None,
            applies_to: "all".into(),
            environment_ids: None,
            required_approvers: 1,
            approver_role_ids: vec![senior_engineer_role_id],
            approver_user_ids: Vec::new(),
            allow_admin_override: false,
            fallback_to_roles: true,
            auto_approve_after_hours: None,
            enabled: true,
            created_at: Utc::now(),
            ai_risk_mode: "advisory".to_string(),
        };

        // Mock user has "Approver" system role
        role_repo
            .expect_user_has_role()
            .with(
                mockall::predicate::eq(approver_id),
                mockall::predicate::eq("Approver"),
            )
            .times(1)
            .returning(|_, _| Ok(true));

        // Mock user has "Senior Engineer" role
        role_repo
            .expect_get_user_roles()
            .with(mockall::predicate::eq(approver_id))
            .times(1)
            .returning(move |_| {
                Ok(vec![Role {
                    id: senior_engineer_role_id,
                    name: "Senior Engineer".into(),
                    description: "Senior engineering role".into(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                }])
            });

        let request_clone = request.clone();
        approval_repo
            .expect_get_request_by_id()
            .with(mockall::predicate::eq(request_id))
            .times(1)
            .returning(move |_| Ok(Some(request_clone.clone())));

        approval_repo
            .expect_get_policy_by_id()
            .with(mockall::predicate::eq(policy_id))
            .times(1)
            .returning(move |_| Ok(Some(policy.clone())));

        approval_repo
            .expect_add_vote()
            .times(1)
            .returning(move |_, _| Ok(request.clone()));

        approval_repo
            .expect_list_votes_for_request()
            .times(1)
            .returning(move |_| Ok(vec![]));

        role_repo.expect_clone_box().returning(|| {
            let mut mock = MockRoleRepository::new();
            mock.expect_clone_box()
                .returning(|| Box::new(MockRoleRepository::new()));
            Box::new(mock)
        });

        let (tx, _rx) = tokio::sync::broadcast::channel(10);
        let (updates_tx, _updates_rx) = tokio::sync::broadcast::channel(10);
        let logic = approval_logic(
            Box::new(approval_repo),
            Box::new(feature_repo),
            Box::new(env_logic),
            Box::new(role_repo),
            tx,
            updates_tx,
        );

        let result = logic.approve_request(request_id, approver_id, None).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_dispatch_stage_change_approved_notification_uses_injected_notifier() {
        let mut feature_repo = MockFeatureRepository::new();
        let mut env_logic = MockEnvironmentLogic::new();
        let approval_repo = MockApprovalRepository::new();
        let role_repo = MockRoleRepository::new();
        let team_id = Uuid::new_v4();
        let feature_id = Uuid::new_v4();
        let environment_id = Uuid::new_v4();

        feature_repo
            .expect_get_feature_by_id()
            .with(mockall::predicate::eq(feature_id))
            .times(1)
            .returning(move |_| {
                Ok(DbFeature {
                    id: feature_id,
                    team_id,
                    key: "feature-a".to_string(),
                    description: Some("feature".to_string()),
                    feature_type: FeatureType::Simple,
                    active: true,
                    kill_switch_enabled: false,
                    kill_switch_activated_at: None,
                    rollback_scheduled_at: None,
                    emergency_override_reason: None,
                    emergency_override_expires_at: None,
                    emergency_override_actor_id: None,
                    emergency_override_applied_at: None,
                    deprecated_at: None,
                    deprecation_notice: None,
                    lifecycle_stage: "Active".to_string(),
                    owner: None,
                    purpose: None,
                    reference_url: None,
                    expires_at: None,
                    cleanup_reason: None,
                    tags: vec![],
                    archived_at: None,
                    created_at: Utc::now(),
                    dependencies: vec![],
                    last_evaluated_at: None,
                    evaluation_count_7d: 0,
                    evaluation_count_30d: 0,
                    evaluation_count_90d: 0,
                    flag_kind: None,
                    flag_kind_confidence: None,
                    flag_kind_source: None,
                })
            });

        env_logic
            .expect_get_environment_by_id()
            .with(mockall::predicate::eq(ID::from(environment_id)))
            .times(1)
            .returning(move |_| {
                Ok(Environment {
                    id: ID::from(environment_id),
                    name: "Production".to_string(),
                    active: true,
                    team_id: ID::from(team_id),
                    environment_type: "Production".to_string(),
                })
            });

        let (approval_events_tx, _) = tokio::sync::broadcast::channel(4);
        let (feature_updates_tx, _) = tokio::sync::broadcast::channel(4);
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let logic = ApprovalLogicImpl {
            db_pool: Some(sqlx::PgPool::connect_lazy("postgres://unused").expect("lazy pool")),
            approval_repository: Box::new(approval_repo),
            feature_repository: Box::new(feature_repo),
            environment_logic: Box::new(env_logic),
            role_repository: Box::new(role_repo),
            approval_events_tx,
            feature_updates_tx,
            notification_logic: Some(Box::new(RecordingNotificationLogic { sender })),
            judgments: None,
        };

        let request = ApprovalRequest {
            id: Uuid::new_v4(),
            policy_id: Uuid::new_v4(),
            feature_id,
            environment_id: Some(environment_id),
            change_type: "stage_change".into(),
            change_payload: serde_json::json!({
                "next_status": "DEPLOYMENT_REQUESTED"
            }),
            change_description: None,
            requested_by: Uuid::new_v4(),
            eligible_approver_ids: Vec::new(),
            routing_reason: None,
            admin_override_enabled: false,
            status: ApprovalStatus::Approved,
            approved_count: 1,
            rejected_count: 0,
            executed_at: Some(Utc::now()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            required_approvers_override: None,
        };

        logic
            .dispatch_stage_change_approved_notification(&request, team_id, None)
            .await;

        let notification_type = timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("notification task should complete")
            .expect("notification channel should receive an event");
        assert_eq!(
            notification_type,
            crate::logic::notification::NOTIFICATION_TYPE_STAGE_CHANGE_APPROVED
        );
    }

    // Note: test_approve_request_fails_without_approver_role was removed due to mockall limitations
    // with complex clone_box scenarios. This authorization check is covered by integration tests.

    #[tokio::test]
    async fn test_approve_request_fails_without_policy_role() {
        let mut approval_repo = MockApprovalRepository::new();
        let mut role_repo = MockRoleRepository::new();
        let feature_repo = MockFeatureRepository::new();
        let env_logic = MockEnvironmentLogic::new();

        let request_id = Uuid::new_v4();
        let approver_id = Uuid::new_v4();
        let policy_id = Uuid::new_v4();
        let team_id = Uuid::new_v4();
        let senior_engineer_role_id = Uuid::new_v4();
        let junior_engineer_role_id = Uuid::new_v4();

        let request = ApprovalRequest {
            id: request_id,
            policy_id,
            feature_id: Uuid::new_v4(),
            environment_id: Some(Uuid::new_v4()),
            change_type: "stage_change".into(),
            change_payload: serde_json::json!({}),
            change_description: None,
            requested_by: Uuid::new_v4(),
            eligible_approver_ids: Vec::new(),
            routing_reason: None,
            admin_override_enabled: false,
            status: ApprovalStatus::Pending,
            approved_count: 0,
            rejected_count: 0,
            executed_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            required_approvers_override: None,
        };

        // Policy requires "Senior Engineer" role
        let policy = ApprovalPolicy {
            id: policy_id,
            team_id,
            name: "Production Approval".into(),
            description: None,
            applies_to: "all".into(),
            environment_ids: None,
            required_approvers: 1,
            approver_role_ids: vec![senior_engineer_role_id],
            approver_user_ids: Vec::new(),
            allow_admin_override: false,
            fallback_to_roles: true,
            auto_approve_after_hours: None,
            enabled: true,
            created_at: Utc::now(),
            ai_risk_mode: "advisory".to_string(),
        };

        approval_repo
            .expect_get_request_by_id()
            .with(mockall::predicate::eq(request_id))
            .times(1)
            .returning(move |_| Ok(Some(request.clone())));

        approval_repo
            .expect_get_policy_by_id()
            .with(mockall::predicate::eq(policy_id))
            .times(1)
            .returning(move |_| Ok(Some(policy.clone())));

        // User has "Approver" system role
        role_repo
            .expect_user_has_role()
            .with(
                mockall::predicate::eq(approver_id),
                mockall::predicate::eq("Approver"),
            )
            .times(1)
            .returning(|_, _| Ok(true));

        // But user only has "Junior Engineer" role, NOT "Senior Engineer"
        role_repo
            .expect_get_user_roles()
            .with(mockall::predicate::eq(approver_id))
            .times(1)
            .returning(move |_| {
                Ok(vec![Role {
                    id: junior_engineer_role_id,
                    name: "Junior Engineer".into(),
                    description: "Junior engineering role".into(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                }])
            });

        role_repo.expect_clone_box().returning(|| {
            let mut mock = MockRoleRepository::new();
            mock.expect_clone_box()
                .returning(|| Box::new(MockRoleRepository::new()));
            Box::new(mock)
        });

        let (tx, _rx) = tokio::sync::broadcast::channel(10);
        let (updates_tx, _updates_rx) = tokio::sync::broadcast::channel(10);
        let logic = approval_logic(
            Box::new(approval_repo),
            Box::new(feature_repo),
            Box::new(env_logic),
            Box::new(role_repo),
            tx,
            updates_tx,
        );

        let result = logic.approve_request(request_id, approver_id, None).await;

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("does not have any of the required roles")
        );
    }

    #[tokio::test]
    async fn test_approve_request_fails_when_not_in_frozen_eligible_set() {
        let mut approval_repo = MockApprovalRepository::new();
        let mut role_repo = MockRoleRepository::new();
        let feature_repo = MockFeatureRepository::new();
        let env_logic = MockEnvironmentLogic::new();

        let request_id = Uuid::new_v4();
        let approver_id = Uuid::new_v4();
        let eligible_approver_id = Uuid::new_v4();
        let policy_id = Uuid::new_v4();
        let team_id = Uuid::new_v4();
        let senior_engineer_role_id = Uuid::new_v4();

        let request = ApprovalRequest {
            id: request_id,
            policy_id,
            feature_id: Uuid::new_v4(),
            environment_id: Some(Uuid::new_v4()),
            change_type: "stage_change".into(),
            change_payload: serde_json::json!({}),
            change_description: None,
            requested_by: Uuid::new_v4(),
            eligible_approver_ids: vec![eligible_approver_id],
            routing_reason: Some("Explicit approvers only".into()),
            admin_override_enabled: false,
            status: ApprovalStatus::Pending,
            approved_count: 0,
            rejected_count: 0,
            executed_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            required_approvers_override: None,
        };

        let policy = ApprovalPolicy {
            id: policy_id,
            team_id,
            name: "Production Approval".into(),
            description: None,
            applies_to: "all".into(),
            environment_ids: None,
            required_approvers: 1,
            approver_role_ids: vec![senior_engineer_role_id],
            approver_user_ids: vec![eligible_approver_id],
            allow_admin_override: false,
            fallback_to_roles: false,
            auto_approve_after_hours: None,
            enabled: true,
            created_at: Utc::now(),
            ai_risk_mode: "advisory".to_string(),
        };

        approval_repo
            .expect_get_request_by_id()
            .with(mockall::predicate::eq(request_id))
            .times(1)
            .returning(move |_| Ok(Some(request.clone())));

        approval_repo
            .expect_get_policy_by_id()
            .with(mockall::predicate::eq(policy_id))
            .times(1)
            .returning(move |_| Ok(Some(policy.clone())));

        role_repo
            .expect_user_has_role()
            .with(
                mockall::predicate::eq(approver_id),
                mockall::predicate::eq("Approver"),
            )
            .times(1)
            .returning(|_, _| Ok(true));

        let (tx, _rx) = tokio::sync::broadcast::channel(10);
        let (updates_tx, _updates_rx) = tokio::sync::broadcast::channel(10);
        let logic = approval_logic(
            Box::new(approval_repo),
            Box::new(feature_repo),
            Box::new(env_logic),
            Box::new(role_repo),
            tx,
            updates_tx,
        );

        let result = logic.approve_request(request_id, approver_id, None).await;

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not an eligible approver")
        );
    }

    #[tokio::test]
    async fn test_maybe_create_stage_change_request_emits_event() {
        let mut approval_repo = MockApprovalRepository::new();
        let mut feature_repo = MockFeatureRepository::new();
        let mut env_logic = MockEnvironmentLogic::new();
        let role_repo = MockRoleRepository::new();

        let team_id = Uuid::new_v4();
        let environment_id = Uuid::new_v4();
        let stage_id = Uuid::new_v4();
        let policy_id = Uuid::new_v4();
        let request_id = Uuid::new_v4();
        let requested_by = Uuid::new_v4();

        let feature = DbFeature {
            id: Uuid::new_v4(),
            key: "checkout_new".into(),
            description: Some("New checkout flow".into()),
            feature_type: FeatureType::Simple,
            team_id,
            active: true,
            created_at: Utc::now(),
            kill_switch_enabled: false,
            kill_switch_activated_at: None,
            rollback_scheduled_at: None,
            emergency_override_reason: None,
            emergency_override_expires_at: None,
            emergency_override_actor_id: None,
            emergency_override_applied_at: None,
            lifecycle_stage: "active".into(),
            owner: None,
            purpose: None,
            reference_url: None,
            expires_at: None,
            cleanup_reason: None,
            tags: vec![],
            archived_at: None,
            deprecated_at: None,
            deprecation_notice: None,
            last_evaluated_at: None,
            evaluation_count_7d: 0,
            evaluation_count_30d: 0,
            evaluation_count_90d: 0,
            dependencies: vec![],
            flag_kind: None,
            flag_kind_confidence: None,
            flag_kind_source: None,
        };

        let stage = FeaturePipelineStage {
            id: stage_id,
            feature_id: feature.id,
            environment_id,
            order_index: 0,
            parent_stage_id: None,
            position: "production".into(),
            enabled: false,
            status: "NOT_DEPLOYED".into(),
        };

        let policy = ApprovalPolicy {
            id: policy_id,
            team_id,
            name: "Prod approvals".into(),
            description: None,
            applies_to: "production_only".into(),
            environment_ids: None,
            required_approvers: 1,
            approver_role_ids: vec![Uuid::new_v4()],
            approver_user_ids: Vec::new(),
            allow_admin_override: false,
            fallback_to_roles: true,
            auto_approve_after_hours: None,
            enabled: true,
            created_at: Utc::now(),
            ai_risk_mode: "advisory".to_string(),
        };

        let created_request = ApprovalRequest {
            id: request_id,
            policy_id,
            feature_id: feature.id,
            environment_id: Some(environment_id),
            change_type: "stage_change".into(),
            change_payload: serde_json::json!({
                "stage_id": stage_id.to_string(),
                "next_status": "DEPLOYMENT_REQUESTED"
            }),
            change_description: Some("Stage NOT_DEPLOYED -> DEPLOYMENT_REQUESTED".into()),
            requested_by,
            eligible_approver_ids: Vec::new(),
            routing_reason: None,
            admin_override_enabled: false,
            status: ApprovalStatus::Pending,
            approved_count: 0,
            rejected_count: 0,
            executed_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            required_approvers_override: None,
        };

        env_logic
            .expect_get_environment_by_id()
            .with(mockall::predicate::eq(ID::from(environment_id)))
            .returning(move |_| {
                Ok(Environment {
                    id: ID::from(environment_id),
                    name: "Production".into(),
                    active: true,
                    team_id: ID::from(team_id),
                    environment_type: "Production".into(),
                })
            });

        approval_repo
            .expect_list_policies_for_team()
            .with(mockall::predicate::eq(team_id))
            .return_once(move |_| Ok(vec![policy.clone()]));

        let stage_for_snapshot = stage.clone();
        feature_repo
            .expect_get_feature_stages()
            .with(mockall::predicate::eq(feature.id))
            .times(1)
            .return_once(move |_| Ok(vec![stage_for_snapshot.clone()]));
        feature_repo
            .expect_get_feature_variants()
            .with(mockall::predicate::eq(feature.id))
            .times(1)
            .returning(|_| Ok(vec![]));
        feature_repo
            .expect_get_stage_criteria()
            .with(mockall::predicate::eq(stage_id))
            .times(1)
            .returning(|_| Ok(vec![]));

        approval_repo
            .expect_create_request()
            .times(1)
            .return_once(move |input| {
                assert_eq!(input.policy_id, policy_id);
                assert_eq!(input.feature_id, created_request.feature_id);
                assert_eq!(input.environment_id, Some(environment_id));
                assert_eq!(input.change_type, "stage_change");
                assert!(input.change_payload["snapshot_id"].as_str().is_some());
                assert_eq!(
                    input.change_payload["before_snapshot"]["stages"][0]["status"],
                    "NOT_DEPLOYED"
                );
                assert_eq!(
                    input.change_payload["after_snapshot"]["stages"][0]["status"],
                    "DEPLOYMENT_APPROVED"
                );
                assert_eq!(input.change_payload["policy"]["name"], "Prod approvals");
                assert!(
                    input.change_payload["diff"]
                        .as_array()
                        .is_some_and(|entries| !entries.is_empty())
                );
                Ok(created_request.clone())
            });

        approval_repo
            .expect_list_votes_for_request()
            .times(1)
            .returning(move |_| Ok(vec![]));

        let (tx, mut rx) = tokio::sync::broadcast::channel(8);
        let (updates_tx, _updates_rx) = tokio::sync::broadcast::channel(8);
        let logic = approval_logic(
            Box::new(approval_repo),
            Box::new(feature_repo),
            Box::new(env_logic),
            Box::new(role_repo),
            tx,
            updates_tx,
        );

        let result = logic
            .maybe_create_stage_change_request(
                &feature,
                &stage,
                "DEPLOYMENT_REQUESTED",
                requested_by,
            )
            .await
            .unwrap();

        assert!(result.is_some());
        let event = rx.recv().await.expect("event should be published");
        assert_eq!(event.request.id, request_id);
        assert_eq!(event.team_id, team_id);
    }

    #[tokio::test]
    async fn test_approve_request_publishes_events_on_status_change() {
        let mut approval_repo = MockApprovalRepository::new();
        let mut role_repo = MockRoleRepository::new();
        let mut feature_repo = MockFeatureRepository::new();
        let env_logic = MockEnvironmentLogic::new();

        let request_id = Uuid::new_v4();
        let policy_id = Uuid::new_v4();
        let stage_id = Uuid::new_v4();
        let team_id = Uuid::new_v4();
        let approver_id = Uuid::new_v4();
        let required_role_id = Uuid::new_v4();

        let pending_request = ApprovalRequest {
            id: request_id,
            policy_id,
            feature_id: Uuid::new_v4(),
            environment_id: Some(Uuid::new_v4()),
            change_type: "stage_change".into(),
            change_payload: serde_json::json!({
                "stage_id": stage_id.to_string(),
                "next_status": "DEPLOYED",
                "approval_target_status": "DEPLOYED"
            }),
            change_description: Some("Promote to prod".into()),
            requested_by: Uuid::new_v4(),
            eligible_approver_ids: Vec::new(),
            routing_reason: None,
            admin_override_enabled: false,
            status: ApprovalStatus::Pending,
            approved_count: 0,
            rejected_count: 0,
            executed_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            required_approvers_override: None,
        };

        let policy = ApprovalPolicy {
            id: policy_id,
            team_id,
            name: "Prod approvals".into(),
            description: None,
            applies_to: "all".into(),
            environment_ids: None,
            required_approvers: 1,
            approver_role_ids: vec![required_role_id],
            approver_user_ids: Vec::new(),
            allow_admin_override: false,
            fallback_to_roles: true,
            auto_approve_after_hours: None,
            enabled: true,
            created_at: Utc::now(),
            ai_risk_mode: "advisory".to_string(),
        };

        let approved_request = ApprovalRequest {
            status: ApprovalStatus::Approved,
            approved_count: 1,
            updated_at: Utc::now(),
            ..pending_request.clone()
        };

        let final_request = ApprovalRequest {
            executed_at: Some(Utc::now()),
            ..approved_request.clone()
        };

        // Extract feature_id before moving pending_request
        let feature_id_for_notify = pending_request.feature_id;

        approval_repo
            .expect_get_request_by_id()
            .with(mockall::predicate::eq(request_id))
            .times(1)
            .returning(move |_| Ok(Some(pending_request.clone())));

        approval_repo
            .expect_get_policy_by_id()
            .with(mockall::predicate::eq(policy_id))
            .times(1)
            .returning(move |_| Ok(Some(policy.clone())));

        role_repo
            .expect_user_has_role()
            .with(
                mockall::predicate::eq(approver_id),
                mockall::predicate::eq("Approver"),
            )
            .times(1)
            .returning(|_, _| Ok(true));

        role_repo
            .expect_get_user_roles()
            .with(mockall::predicate::eq(approver_id))
            .times(1)
            .returning(move |_| {
                Ok(vec![Role {
                    id: required_role_id,
                    name: "Release Manager".into(),
                    description: "Can approve prod".into(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                }])
            });

        approval_repo
            .expect_add_vote()
            .times(1)
            .returning(move |_, _| Ok(approved_request.clone()));

        approval_repo
            .expect_list_votes_for_request()
            .times(2)
            .returning(move |_| Ok(vec![]));

        feature_repo
            .expect_approve_or_reject_stage_change()
            .with(
                mockall::predicate::eq(stage_id),
                mockall::predicate::eq("DEPLOYED"),
                mockall::predicate::eq(approver_id),
            )
            .times(1)
            .returning(|_, _, _| Ok(true));

        // Add expectation for notify_edge_servers calling get_feature_by_id
        feature_repo
            .expect_get_feature_by_id()
            .with(mockall::predicate::eq(feature_id_for_notify))
            .times(1)
            .returning(move |_| {
                Ok(DbFeature {
                    id: feature_id_for_notify,
                    key: "test_feature".into(),
                    description: Some("Test feature".into()),
                    feature_type: FeatureType::Simple,
                    team_id: Uuid::new_v4(),
                    active: true,
                    created_at: Utc::now(),
                    kill_switch_enabled: false,
                    kill_switch_activated_at: None,
                    rollback_scheduled_at: None,
                    emergency_override_reason: None,
                    emergency_override_expires_at: None,
                    emergency_override_actor_id: None,
                    emergency_override_applied_at: None,
                    lifecycle_stage: "active".into(),
                    owner: None,
                    purpose: None,
                    reference_url: None,
                    expires_at: None,
                    cleanup_reason: None,
                    tags: vec![],
                    archived_at: None,
                    deprecated_at: None,
                    deprecation_notice: None,
                    last_evaluated_at: None,
                    evaluation_count_7d: 0,
                    evaluation_count_30d: 0,
                    evaluation_count_90d: 0,
                    dependencies: vec![],
                    flag_kind: None,
                    flag_kind_confidence: None,
                    flag_kind_source: None,
                })
            });

        // The broadcast mapping loads child rows with batched calls.
        feature_repo
            .expect_get_feature_stages_batch()
            .times(..=1)
            .returning(|_| Ok(std::collections::HashMap::new()));

        // Variants of the feature (empty here)
        feature_repo
            .expect_get_feature_variants_batch()
            .times(..=1)
            .returning(|_| Ok(std::collections::HashMap::new()));

        approval_repo
            .expect_update_request_status()
            .times(1)
            .returning(move |_, _, _| Ok(final_request.clone()));

        role_repo.expect_clone_box().returning(|| {
            let mut mock = MockRoleRepository::new();
            mock.expect_clone_box()
                .returning(|| Box::new(MockRoleRepository::new()));
            Box::new(mock)
        });

        let (tx, mut rx) = tokio::sync::broadcast::channel(8);
        let (updates_tx, _updates_rx) = tokio::sync::broadcast::channel(8);
        let logic = approval_logic(
            Box::new(approval_repo),
            Box::new(feature_repo),
            Box::new(env_logic),
            Box::new(role_repo),
            tx,
            updates_tx,
        );

        let updated = logic
            .approve_request(request_id, approver_id, Some("looks good".into()))
            .await
            .unwrap();

        assert_eq!(updated.status, ApprovalStatus::Approved);

        // First event is after vote, second after final status update
        let first_event = rx.recv().await.expect("first event missing");
        assert_eq!(first_event.request.id, request_id);
        assert_eq!(first_event.request.status, ApprovalStatus::Approved);

        let second_event = rx.recv().await.expect("second event missing");
        assert_eq!(second_event.request.id, request_id);
        assert_eq!(second_event.request.status, ApprovalStatus::Approved);
        assert!(second_event.request.executed_at.is_some());
        assert_eq!(first_event.team_id, team_id);
        assert_eq!(second_event.team_id, team_id);
    }
}

#[cfg(test)]
mod ai_risk_trigger_tests {
    use super::*;
    use crate::database::ai::{
        AiJudgment, MockAiJudgmentRepository, MockTeamAiSettingsRepository, StoredTeamAiSettings,
        TeamAiSettings,
    };
    use crate::database::approval::MockApprovalRepository;
    use crate::database::entity::FeatureType;
    use crate::database::feature::MockFeatureRepository;
    use crate::database::role::MockRoleRepository;
    use crate::judgment::JudgmentKind;
    use crate::judgment::client::MockJudgmentClient;
    use crate::logic::environment::MockEnvironmentLogic;
    use crate::model::Environment;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn judgment_service(
        team_setting_on: bool,
        submit_fails: bool,
        submits: Arc<AtomicUsize>,
    ) -> Arc<JudgmentService> {
        judgment_service_with_probe(team_setting_on, submit_fails, submits, Arc::new(|| {}))
    }

    /// Like `judgment_service`; `probe` runs at the start of every upsert.
    fn judgment_service_with_probe(
        team_setting_on: bool,
        submit_fails: bool,
        submits: Arc<AtomicUsize>,
        probe: Arc<dyn Fn() + Send + Sync>,
    ) -> Arc<JudgmentService> {
        let mut settings = MockTeamAiSettingsRepository::new();
        settings.expect_get().returning(move |_| {
            Ok(StoredTeamAiSettings {
                settings: TeamAiSettings {
                    approval_risk: team_setting_on,
                    ..TeamAiSettings::default()
                },
                ..StoredTeamAiSettings::default()
            })
        });
        let mut repo = MockAiJudgmentRepository::new();
        repo.expect_upsert_pending().returning(move |new| {
            probe();
            submits.fetch_add(1, Ordering::SeqCst);
            assert_eq!(new.kind, JudgmentKind::ApprovalRisk);
            assert_eq!(new.input["feature"]["key"], "checkout_new");
            if submit_fails {
                return Err(Error::InvalidInput("db down".into()));
            }
            Ok(AiJudgment {
                id: Uuid::new_v4(),
                team_id: new.team_id,
                subject_type: new.subject_type.as_str().into(),
                subject_id: new.subject_id,
                kind: new.kind.as_str().into(),
                status: "pending".into(),
                attempts: 1,
                input: new.input,
                input_hash: new.input_hash,
                model: None,
                raw_answers: None,
                derived: None,
                input_tokens: None,
                error: None,
                created_at: Utc::now(),
                completed_at: None,
            })
        });
        // The background run has no handler registered in this test and is not
        // the subject here; it only needs to be able to record a failure.
        repo.expect_mark_failed().returning(|_, _, _| Ok(true));
        Arc::new(JudgmentService::new(
            Arc::new(MockJudgmentClient::new()),
            Box::new(repo),
            Box::new(settings),
        ))
    }

    async fn create_request(
        ai_risk_mode: &str,
        judgments: Option<Arc<JudgmentService>>,
    ) -> Result<Option<ApprovalRequest>, Error> {
        let (tx, _rx) = tokio::sync::broadcast::channel(8);
        create_request_with_events(ai_risk_mode, judgments, tx).await
    }

    async fn create_request_with_events(
        ai_risk_mode: &str,
        judgments: Option<Arc<JudgmentService>>,
        tx: tokio::sync::broadcast::Sender<ApprovalRequestEvent>,
    ) -> Result<Option<ApprovalRequest>, Error> {
        let mut approval_repo = MockApprovalRepository::new();
        let mut feature_repo = MockFeatureRepository::new();
        let mut env_logic = MockEnvironmentLogic::new();

        let team_id = Uuid::new_v4();
        let environment_id = Uuid::new_v4();
        let policy_id = Uuid::new_v4();
        let requested_by = Uuid::new_v4();
        let feature = DbFeature {
            id: Uuid::new_v4(),
            key: "checkout_new".into(),
            description: Some("New checkout flow".into()),
            feature_type: FeatureType::Simple,
            team_id,
            active: true,
            created_at: Utc::now(),
            kill_switch_enabled: false,
            kill_switch_activated_at: None,
            rollback_scheduled_at: None,
            emergency_override_reason: None,
            emergency_override_expires_at: None,
            emergency_override_actor_id: None,
            emergency_override_applied_at: None,
            lifecycle_stage: "active".into(),
            owner: None,
            purpose: None,
            reference_url: None,
            expires_at: None,
            cleanup_reason: None,
            tags: vec![],
            archived_at: None,
            deprecated_at: None,
            deprecation_notice: None,
            last_evaluated_at: None,
            evaluation_count_7d: 0,
            evaluation_count_30d: 0,
            evaluation_count_90d: 0,
            dependencies: vec![],
            flag_kind: None,
            flag_kind_confidence: None,
            flag_kind_source: None,
        };
        let stage = FeaturePipelineStage {
            id: Uuid::new_v4(),
            feature_id: feature.id,
            environment_id,
            order_index: 0,
            parent_stage_id: None,
            position: "production".into(),
            enabled: false,
            status: "NOT_DEPLOYED".into(),
        };
        let policy = ApprovalPolicy {
            id: policy_id,
            team_id,
            name: "Prod approvals".into(),
            description: None,
            applies_to: "production_only".into(),
            environment_ids: None,
            required_approvers: 1,
            approver_role_ids: vec![Uuid::new_v4()],
            approver_user_ids: Vec::new(),
            allow_admin_override: false,
            fallback_to_roles: true,
            auto_approve_after_hours: None,
            enabled: true,
            created_at: Utc::now(),
            ai_risk_mode: ai_risk_mode.to_string(),
        };
        let created = ApprovalRequest {
            id: Uuid::new_v4(),
            policy_id,
            feature_id: feature.id,
            environment_id: Some(environment_id),
            change_type: "stage_change".into(),
            change_payload: serde_json::json!({ "next_status": "DEPLOYMENT_REQUESTED" }),
            change_description: None,
            requested_by,
            eligible_approver_ids: Vec::new(),
            routing_reason: None,
            admin_override_enabled: false,
            status: ApprovalStatus::Pending,
            approved_count: 0,
            rejected_count: 0,
            executed_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            required_approvers_override: None,
        };

        env_logic
            .expect_get_environment_by_id()
            .returning(move |_| {
                Ok(Environment {
                    id: ID::from(environment_id),
                    name: "Production".into(),
                    active: true,
                    team_id: ID::from(team_id),
                    environment_type: "Production".into(),
                })
            });
        approval_repo
            .expect_list_policies_for_team()
            .return_once(move |_| Ok(vec![policy]));
        let stage_for_snapshot = stage.clone();
        feature_repo
            .expect_get_feature_stages()
            .return_once(move |_| Ok(vec![stage_for_snapshot]));
        feature_repo
            .expect_get_feature_variants()
            .returning(|_| Ok(vec![]));
        feature_repo
            .expect_get_stage_criteria()
            .returning(|_| Ok(vec![]));
        approval_repo
            .expect_create_request()
            .return_once(move |_| Ok(created));
        approval_repo
            .expect_list_votes_for_request()
            .returning(|_| Ok(vec![]));

        let (updates_tx, _updates_rx) = tokio::sync::broadcast::channel(8);
        let logic = approval_logic_with_notifications(
            Box::new(approval_repo),
            Box::new(feature_repo),
            Box::new(env_logic),
            Box::new(MockRoleRepository::new()),
            tx,
            updates_tx,
            None,
            judgments,
        );
        logic
            .maybe_create_stage_change_request(
                &feature,
                &stage,
                "DEPLOYMENT_REQUESTED",
                requested_by,
            )
            .await
    }

    #[tokio::test]
    async fn advisory_policy_with_team_setting_on_submits_once() {
        let submits = Arc::new(AtomicUsize::new(0));
        let service = judgment_service(true, false, submits.clone());

        let created = create_request("advisory", Some(service)).await.unwrap();

        assert!(created.is_some());
        assert_eq!(submits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn other_enabled_modes_also_submit_until_enforcement_exists() {
        for mode in ["gate_auto_approve", "require_extra_approver"] {
            let submits = Arc::new(AtomicUsize::new(0));
            let service = judgment_service(true, false, submits.clone());

            create_request(mode, Some(service)).await.unwrap();

            assert_eq!(submits.load(Ordering::SeqCst), 1, "{mode}");
        }
    }

    /// The request event is not delayed by the assessment: it goes out
    /// first, and a second event follows once the assessment is queued, so
    /// streams show it as pending.
    #[tokio::test]
    async fn the_request_event_is_published_before_the_assessment_is_queued() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(8);
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (probe_tx, probe_seen) = (tx.clone(), seen.clone());
        let probe: Arc<dyn Fn() + Send + Sync> =
            Arc::new(move || probe_seen.lock().unwrap().push(probe_tx.len()));
        let submits = Arc::new(AtomicUsize::new(0));
        let service = judgment_service_with_probe(true, false, submits.clone(), probe);

        let created = create_request_with_events("advisory", Some(service), tx)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(submits.load(Ordering::SeqCst), 1);
        assert_eq!(
            seen.lock().unwrap().clone(),
            vec![1],
            "one event was already published when the assessment was queued"
        );
        assert_eq!(rx.recv().await.unwrap().request.id, created.id);
        assert_eq!(rx.recv().await.unwrap().request.id, created.id);
        assert!(rx.try_recv().is_err(), "exactly two events");
    }

    #[tokio::test]
    async fn no_second_event_when_nothing_is_queued() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(8);
        let submits = Arc::new(AtomicUsize::new(0));
        let service = judgment_service(false, false, submits.clone());

        create_request_with_events("advisory", Some(service), tx)
            .await
            .unwrap();

        assert!(rx.recv().await.is_ok());
        assert!(rx.try_recv().is_err(), "exactly one event");
    }

    #[tokio::test]
    async fn off_mode_does_not_submit() {
        let submits = Arc::new(AtomicUsize::new(0));
        let service = judgment_service(true, false, submits.clone());

        let created = create_request("off", Some(service)).await.unwrap();

        assert!(created.is_some());
        assert_eq!(submits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn team_setting_off_does_not_submit() {
        let submits = Arc::new(AtomicUsize::new(0));
        let service = judgment_service(false, false, submits.clone());

        let created = create_request("advisory", Some(service)).await.unwrap();

        assert!(created.is_some());
        assert_eq!(submits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn missing_service_creates_the_request_unchanged() {
        let created = create_request("advisory", None).await.unwrap();

        assert!(created.is_some());
    }

    #[tokio::test]
    async fn submit_failure_does_not_fail_request_creation() {
        let submits = Arc::new(AtomicUsize::new(0));
        let service = judgment_service(true, true, submits.clone());

        let created = create_request("advisory", Some(service)).await.unwrap();

        assert!(created.is_some());
        assert_eq!(submits.load(Ordering::SeqCst), 1);
    }

    /// Casts one approve vote with a policy needing `policy_required` approvals
    /// and checks that `add_vote` receives `expected_required`. The vote leaves
    /// the request pending, so no change is executed.
    async fn vote_passes_required(
        policy_required: i32,
        request_override: Option<i32>,
        admin_override: bool,
        expected_required: i32,
    ) {
        let request_id = Uuid::new_v4();
        let policy_id = Uuid::new_v4();
        let approver_id = Uuid::new_v4();
        let pending = ApprovalRequest {
            id: request_id,
            policy_id,
            feature_id: Uuid::new_v4(),
            environment_id: Some(Uuid::new_v4()),
            change_type: "stage_change".into(),
            change_payload: serde_json::json!({}),
            change_description: None,
            requested_by: Uuid::new_v4(),
            eligible_approver_ids: vec![approver_id],
            routing_reason: None,
            admin_override_enabled: admin_override,
            status: ApprovalStatus::Pending,
            approved_count: 0,
            rejected_count: 0,
            executed_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            required_approvers_override: request_override,
        };
        let policy = ApprovalPolicy {
            id: policy_id,
            team_id: Uuid::new_v4(),
            name: "Prod approvals".into(),
            description: None,
            applies_to: "all".into(),
            environment_ids: None,
            required_approvers: policy_required,
            approver_role_ids: vec![],
            approver_user_ids: vec![approver_id],
            allow_admin_override: admin_override,
            fallback_to_roles: false,
            auto_approve_after_hours: None,
            enabled: true,
            created_at: Utc::now(),
            ai_risk_mode: "require_extra_approver".into(),
        };

        let mut approval_repo = MockApprovalRepository::new();
        let stored = pending.clone();
        approval_repo
            .expect_get_request_by_id()
            .returning(move |_| Ok(Some(stored.clone())));
        approval_repo
            .expect_get_policy_by_id()
            .returning(move |_| Ok(Some(policy.clone())));
        let after_vote = ApprovalRequest {
            approved_count: 1,
            ..pending.clone()
        };
        approval_repo
            .expect_add_vote()
            .withf(move |_, required| *required == expected_required)
            .times(1)
            .returning(move |_, _| Ok(after_vote.clone()));
        approval_repo
            .expect_list_votes_for_request()
            .returning(|_| Ok(vec![]));

        let mut role_repo = MockRoleRepository::new();
        // An admin override lets the approver skip the Approver role and routing checks.
        role_repo.expect_user_has_role().returning(move |_, role| {
            Ok(admin_override && role == "Team Admin" || role == "Approver")
        });
        role_repo.expect_clone_box().returning(|| {
            let mut mock = MockRoleRepository::new();
            mock.expect_clone_box()
                .returning(|| Box::new(MockRoleRepository::new()));
            Box::new(mock)
        });

        let (tx, _rx) = tokio::sync::broadcast::channel(8);
        let (updates_tx, _updates_rx) = tokio::sync::broadcast::channel(8);
        let logic = approval_logic(
            Box::new(approval_repo),
            Box::new(MockFeatureRepository::new()),
            Box::new(MockEnvironmentLogic::new()),
            Box::new(role_repo),
            tx,
            updates_tx,
        );

        let updated = logic
            .approve_request(request_id, approver_id, None)
            .await
            .unwrap();
        assert_eq!(updated.status, ApprovalStatus::Pending);
    }

    #[tokio::test]
    async fn vote_counts_against_the_request_override_when_set() {
        vote_passes_required(1, Some(2), false, 2).await;
    }

    #[tokio::test]
    async fn vote_counts_against_the_policy_without_an_override() {
        vote_passes_required(1, None, false, 1).await;
    }

    #[tokio::test]
    async fn admin_override_does_not_lower_the_overridden_requirement() {
        vote_passes_required(1, Some(2), true, 2).await;
    }
}
