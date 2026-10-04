use crate::database::entity::VariantValueType as DbVariantValueType;
use crate::logic::feature::StageChangeRequestType;
use crate::model::{
    FeatureType as ModelFeatureType, FlagKind, FlagKindSource,
    LifecycleStage as ModelLifecycleStage, StageChangeMeta,
    VariantValueType as ModelVariantValueType,
};
use crate::rest::environment::EnvironmentResponse;
use crate::rest::pagination::PageMeta;
use crate::rest::pipeline::CreateRelationshipRequest;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FeatureType {
    Simple,
    Contextual,
}

impl From<ModelFeatureType> for FeatureType {
    fn from(value: ModelFeatureType) -> Self {
        match value {
            ModelFeatureType::Simple => FeatureType::Simple,
            ModelFeatureType::Contextual => FeatureType::Contextual,
        }
    }
}

impl From<FeatureType> for ModelFeatureType {
    fn from(value: FeatureType) -> Self {
        match value {
            FeatureType::Simple => ModelFeatureType::Simple,
            FeatureType::Contextual => ModelFeatureType::Contextual,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LifecycleStage {
    Draft,
    Active,
    Deprecated,
    Archived,
}

impl From<ModelLifecycleStage> for LifecycleStage {
    fn from(value: ModelLifecycleStage) -> Self {
        match value {
            ModelLifecycleStage::Draft => LifecycleStage::Draft,
            ModelLifecycleStage::Active => LifecycleStage::Active,
            ModelLifecycleStage::Deprecated => LifecycleStage::Deprecated,
            ModelLifecycleStage::Archived => LifecycleStage::Archived,
        }
    }
}

impl From<LifecycleStage> for ModelLifecycleStage {
    fn from(value: LifecycleStage) -> Self {
        match value {
            LifecycleStage::Draft => ModelLifecycleStage::Draft,
            LifecycleStage::Active => ModelLifecycleStage::Active,
            LifecycleStage::Deprecated => ModelLifecycleStage::Deprecated,
            LifecycleStage::Archived => ModelLifecycleStage::Archived,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VariantValueType {
    String,
    Number,
    Boolean,
    Json,
}

impl From<ModelVariantValueType> for VariantValueType {
    fn from(value: ModelVariantValueType) -> Self {
        match value {
            ModelVariantValueType::String => VariantValueType::String,
            ModelVariantValueType::Number => VariantValueType::Number,
            ModelVariantValueType::Boolean => VariantValueType::Boolean,
            ModelVariantValueType::Json => VariantValueType::Json,
        }
    }
}

impl From<VariantValueType> for ModelVariantValueType {
    fn from(value: VariantValueType) -> Self {
        match value {
            VariantValueType::String => ModelVariantValueType::String,
            VariantValueType::Number => ModelVariantValueType::Number,
            VariantValueType::Boolean => ModelVariantValueType::Boolean,
            VariantValueType::Json => ModelVariantValueType::Json,
        }
    }
}

impl From<DbVariantValueType> for VariantValueType {
    fn from(value: DbVariantValueType) -> Self {
        match value {
            DbVariantValueType::String => VariantValueType::String,
            DbVariantValueType::Number => VariantValueType::Number,
            DbVariantValueType::Boolean => VariantValueType::Boolean,
            DbVariantValueType::Json => VariantValueType::Json,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FeatureListQuery {
    pub name: Option<String>,
    pub feature_type: Option<FeatureType>,
    pub lifecycle_stage: Option<LifecycleStage>,
    pub stale: Option<bool>,
    pub include_archived: Option<bool>,
    pub owner: Option<String>,
    pub expired: Option<bool>,
    pub tag: Option<String>,
    pub dependency_status: Option<String>,
    pub approval_status: Option<String>,
    /// `release`, `experiment`, `ops`, `permission`, `config`, or `unclassified`.
    pub flag_kind: Option<String>,
    /// Jira issue key such as `PROJ-123`: only features linked to it.
    pub external_key: Option<String>,
    pub offset: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FeatureRolloutQuery {
    pub team_id: Option<String>,
    pub offset: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RolloutMetricsQuery {
    pub team_id: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FeatureRelationshipResponse {
    pub source_id: i32,
    pub target_id: i32,
}

impl crate::model::Relationship for FeatureRelationshipResponse {}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FeatureStageResponse {
    pub id: String,
    pub environment: EnvironmentResponse,
    pub order_index: i32,
    pub position: String,
    pub status: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FeatureVariantResponse {
    pub id: String,
    pub feature_id: String,
    pub control: String,
    pub value: serde_json::Value,
    pub value_type: VariantValueType,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FeatureResponse {
    pub id: String,
    pub key: String,
    pub description: Option<String>,
    pub feature_type: FeatureType,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub kill_switch_enabled: bool,
    pub kill_switch_activated_at: Option<DateTime<Utc>>,
    pub rollback_scheduled_at: Option<DateTime<Utc>>,
    pub emergency_override_reason: Option<String>,
    pub emergency_override_expires_at: Option<DateTime<Utc>>,
    pub emergency_override_actor_id: Option<String>,
    pub emergency_override_applied_at: Option<DateTime<Utc>>,
    pub lifecycle_stage: LifecycleStage,
    pub owner: Option<String>,
    pub purpose: Option<String>,
    pub reference_url: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub cleanup_reason: Option<String>,
    pub tags: Vec<String>,
    pub archived_at: Option<DateTime<Utc>>,
    pub deprecated_at: Option<DateTime<Utc>>,
    pub deprecation_notice: Option<String>,
    pub last_evaluated_at: Option<DateTime<Utc>>,
    pub evaluation_count_7d: i64,
    pub evaluation_count_30d: i64,
    pub evaluation_count_90d: i64,
    pub is_stale: bool,
    pub stale_reasons: Vec<String>,
    pub dependencies: Vec<String>,
    pub team_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_approval_request_id: Option<String>,
    pub flag_kind: Option<FlagKind>,
    pub flag_kind_source: Option<FlagKindSource>,
    /// Set only when the source is `ai`.
    pub flag_kind_confidence: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relationships: Option<Vec<FeatureRelationshipResponse>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stages: Option<Vec<FeatureStageResponse>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variants: Option<Vec<FeatureVariantResponse>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FeaturesResponse {
    pub items: Vec<FeatureResponse>,
    pub meta: PageMeta,
}

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FeatureVersionDiffEntryResponse {
    pub path: String,
    pub change_type: String,
    pub before: Option<serde_json::Value>,
    pub after: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FeatureVersionResponse {
    pub id: String,
    pub feature_id: String,
    pub version_number: i32,
    pub snapshot: serde_json::Value,
    pub change_summary: Vec<FeatureVersionDiffEntryResponse>,
    pub actor_id: Option<String>,
    pub actor_name: Option<String>,
    pub source: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FeatureVersionsResponse {
    pub items: Vec<FeatureVersionResponse>,
    pub meta: PageMeta,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FeatureVersionDiffResponse {
    pub version_id: String,
    pub version_number: i32,
    pub entries: Vec<FeatureVersionDiffEntryResponse>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RollbackFeatureVersionRequest {
    pub archive_confirmation: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CreateFeatureStageRequest {
    pub id: Option<String>,
    pub environment_id: String,
    pub order_index: i32,
    pub position: String,
    pub bucketing_key: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CreateFeatureVariantRequest {
    pub control: String,
    pub value: serde_json::Value,
    pub value_type: VariantValueType,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateFeatureRequest {
    pub key: String,
    pub description: Option<String>,
    pub feature_type: FeatureType,
    pub enabled: Option<bool>,
    pub lifecycle_stage: Option<LifecycleStage>,
    pub owner: Option<String>,
    pub purpose: Option<String>,
    pub reference_url: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub cleanup_reason: Option<String>,
    pub tags: Option<Vec<String>>,
    pub flag_kind: Option<FlagKind>,
    pub dependencies: Vec<String>,
    pub relationships: Vec<CreateRelationshipRequest>,
    pub stages: Vec<CreateFeatureStageRequest>,
    pub variants: Option<Vec<CreateFeatureVariantRequest>>,
}

/// Tells an absent field (`None`) from an explicit `null` (`Some(None)`).
fn deserialize_present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateFeatureRequest {
    pub key: String,
    pub description: Option<String>,
    pub feature_type: FeatureType,
    pub enabled: Option<bool>,
    pub lifecycle_stage: Option<LifecycleStage>,
    pub owner: Option<String>,
    pub purpose: Option<String>,
    pub reference_url: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub cleanup_reason: Option<String>,
    pub tags: Option<Vec<String>>,
    /// Absent or equal to the stored kind: unchanged. A different value, or
    /// `null` to clear it, counts as the user's choice and stops AI changes.
    #[serde(
        default,
        deserialize_with = "deserialize_present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schema(value_type = Option<FlagKind>, nullable = true)]
    pub flag_kind: Option<Option<FlagKind>>,
    pub archive_confirmation: Option<bool>,
    pub dependencies: Vec<String>,
    pub relationships: Vec<CreateRelationshipRequest>,
    pub stages: Vec<CreateFeatureStageRequest>,
    pub variants: Option<Vec<CreateFeatureVariantRequest>>,
    pub freeze_override_reason: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EmergencyDisableRequest {
    pub reason: String,
    pub rollback_in_minutes: Option<i32>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EmergencyEnableRequest {
    pub reason: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StageChangeRequest {
    DeploymentRequested,
    DeploymentRejected,
    Deployed,
    RollbackRequested,
    RollbackRejected,
    Rollbacked,
}

impl StageChangeRequest {
    pub fn as_str(&self) -> &'static str {
        match self {
            StageChangeRequest::DeploymentRequested => "DEPLOYMENT_REQUESTED",
            StageChangeRequest::DeploymentRejected => "DEPLOYMENT_REJECTED",
            StageChangeRequest::Deployed => "DEPLOYED",
            StageChangeRequest::RollbackRequested => "ROLLBACK_REQUESTED",
            StageChangeRequest::RollbackRejected => "ROLLBACK_REJECTED",
            StageChangeRequest::Rollbacked => "ROLLBACKED",
        }
    }
}

impl From<StageChangeRequest> for StageChangeRequestType {
    fn from(value: StageChangeRequest) -> Self {
        match value {
            StageChangeRequest::DeploymentRequested => StageChangeRequestType::DeploymentRequested,
            StageChangeRequest::DeploymentRejected => StageChangeRequestType::DeploymentRejected,
            StageChangeRequest::Deployed => StageChangeRequestType::Deployed,
            StageChangeRequest::RollbackRequested => StageChangeRequestType::RollbackRequested,
            StageChangeRequest::RollbackRejected => StageChangeRequestType::RollbackRejected,
            StageChangeRequest::Rollbacked => StageChangeRequestType::Rollbacked,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StageChangeRequestBody {
    pub request: StageChangeRequest,
    pub freeze_override_reason: Option<String>,
    /// Ticket or change id that asked for this change, for example a Jira
    /// issue key. Trimmed; 1-100 characters, no control characters.
    pub external_ref: Option<String>,
    /// Why the change is requested. Trimmed; 1-1000 characters, no control
    /// characters except newlines.
    pub reason: Option<String>,
}

const MAX_EXTERNAL_REF_CHARS: usize = 100;
const MAX_STAGE_CHANGE_REASON_CHARS: usize = 1000;

/// Trims both values, turns blank ones into `None` and checks their length
/// and characters. The error is the message for a 400 response.
pub(crate) fn validate_stage_change_meta(
    external_ref: Option<String>,
    reason: Option<String>,
) -> Result<StageChangeMeta, String> {
    fn trimmed(value: Option<String>) -> Option<String> {
        value
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }

    let external_ref = trimmed(external_ref);
    if let Some(external_ref) = &external_ref {
        if external_ref.chars().count() > MAX_EXTERNAL_REF_CHARS {
            return Err(format!(
                "externalRef must be at most {MAX_EXTERNAL_REF_CHARS} characters"
            ));
        }
        if external_ref.chars().any(char::is_control) {
            return Err("externalRef must not contain control characters".to_string());
        }
    }

    let reason = trimmed(reason);
    if let Some(reason) = &reason {
        if reason.chars().count() > MAX_STAGE_CHANGE_REASON_CHARS {
            return Err(format!(
                "reason must be at most {MAX_STAGE_CHANGE_REASON_CHARS} characters"
            ));
        }
        if reason.chars().any(|c| c.is_control() && c != '\n') {
            return Err(
                "reason must not contain control characters other than newlines".to_string(),
            );
        }
    }

    Ok(StageChangeMeta {
        external_ref,
        // A person typed it, so it is checked after the change (JI-53).
        check_reason: reason.is_some(),
        reason,
    })
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RolloutMetricsResponse {
    pub average_time_in_pipeline: f64,
    pub approval_rate: f64,
    pub features_deployed_this_week: i32,
    pub features_deployed_last_week: i32,
    pub deployment_change: f64,
    pub bottleneck_stage: String,
    pub bottleneck_duration: f64,
    pub total_pending_approvals: i32,
}

#[derive(Debug, Deserialize, Serialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BulkFeatureAction {
    UpdateOwner,
    UpdateTags,
    UpdateLifecycle,
    Archive,
    Export,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BulkFeatureActionRequest {
    pub feature_ids: Vec<String>,
    pub action: BulkFeatureAction,
    pub owner: Option<String>,
    pub tags: Option<Vec<String>>,
    pub lifecycle_stage: Option<LifecycleStage>,
    pub archive_confirmation: Option<bool>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BulkFeatureActionResult {
    pub feature_id: String,
    pub feature_key: Option<String>,
    pub status: String,
    pub message: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BulkFeatureExportRow {
    pub id: String,
    pub key: String,
    pub lifecycle_stage: String,
    pub owner: Option<String>,
    pub purpose: Option<String>,
    pub reference_url: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub tags: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BulkFeatureActionResponse {
    pub results: Vec<BulkFeatureActionResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub export_rows: Option<Vec<BulkFeatureExportRow>>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DependencyImpactQuery {
    pub action: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DependencyImpactNode {
    pub id: String,
    pub key: String,
    pub lifecycle_stage: String,
    pub enabled: bool,
    pub reason: String,
    pub severity: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DependencyImpactResponse {
    pub feature_id: String,
    pub action: String,
    pub severity: String,
    pub summary: String,
    pub direct_dependencies: Vec<DependencyImpactNode>,
    pub direct_dependents: Vec<DependencyImpactNode>,
    pub transitive_dependents: Vec<DependencyImpactNode>,
    pub missing_dependencies: Vec<String>,
    pub cycles: Vec<Vec<String>>,
    pub requires_confirmation: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditAnalyticsQuery {
    pub window_days: Option<i64>,
    pub actor_id: Option<String>,
    pub action: Option<String>,
    pub environment_id: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditAnalyticsBreakdownRow {
    pub key: String,
    pub count: i64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditAnalyticsTopFeature {
    pub feature_id: String,
    pub feature_key: String,
    pub change_count: i64,
    pub last_changed_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditAnalyticsEvent {
    pub id: String,
    pub activity_type: String,
    pub entity_type: String,
    pub entity_id: String,
    pub actor_name: Option<String>,
    pub description: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditAnalyticsResponse {
    pub total_events: i64,
    pub emergency_actions: i64,
    pub rollback_events: i64,
    pub rejection_rate: f64,
    pub approval_lead_time_hours: f64,
    pub top_changed_features: Vec<AuditAnalyticsTopFeature>,
    pub action_breakdown: Vec<AuditAnalyticsBreakdownRow>,
    pub actor_breakdown: Vec<AuditAnalyticsBreakdownRow>,
    pub recent_events: Vec<AuditAnalyticsEvent>,
    pub generated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update_body(extra: &str) -> UpdateFeatureRequest {
        let json = format!(
            r#"{{"key":"k","featureType":"SIMPLE","dependencies":[],"relationships":[],"stages":[]{extra}}}"#
        );
        serde_json::from_str(&json).expect("valid update body")
    }

    #[test]
    fn update_request_tells_absent_null_and_a_value_apart() {
        assert_eq!(update_body("").flag_kind, None);
        assert_eq!(update_body(r#","flagKind":null"#).flag_kind, Some(None));
        assert_eq!(
            update_body(r#","flagKind":"ops""#).flag_kind,
            Some(Some(FlagKind::Ops))
        );
    }

    #[test]
    fn update_request_rejects_an_unknown_kind() {
        let json = r#"{"key":"k","featureType":"SIMPLE","dependencies":[],"relationships":[],"stages":[],"flagKind":"bogus"}"#;
        assert!(serde_json::from_str::<UpdateFeatureRequest>(json).is_err());
    }

    #[test]
    fn create_request_takes_an_optional_kind() {
        let base =
            r#"{"key":"k","featureType":"SIMPLE","dependencies":[],"relationships":[],"stages":[]"#;
        let without: CreateFeatureRequest = serde_json::from_str(&format!("{base}}}")).unwrap();
        assert_eq!(without.flag_kind, None);
        let with: CreateFeatureRequest =
            serde_json::from_str(&format!(r#"{base},"flagKind":"release"}}"#)).unwrap();
        assert_eq!(with.flag_kind, Some(FlagKind::Release));
    }

    #[test]
    fn stage_change_body_reads_external_ref_and_reason() {
        let body: StageChangeRequestBody = serde_json::from_str(
            r#"{"request":"DEPLOYMENT_REQUESTED","externalRef":"PROJ-1","reason":"Ready"}"#,
        )
        .unwrap();
        assert_eq!(body.external_ref.as_deref(), Some("PROJ-1"));
        assert_eq!(body.reason.as_deref(), Some("Ready"));

        let without: StageChangeRequestBody =
            serde_json::from_str(r#"{"request":"DEPLOYMENT_REQUESTED"}"#).unwrap();
        assert_eq!(without.external_ref, None);
        assert_eq!(without.reason, None);
    }

    #[test]
    fn stage_change_meta_validation() {
        let some = |s: &str| Some(s.to_string());
        let meta = |external_ref: Option<&str>, reason: Option<&str>| StageChangeMeta {
            external_ref: external_ref.map(str::to_string),
            reason: reason.map(str::to_string),
            check_reason: reason.is_some(),
        };
        let long_ref = "A".repeat(101);
        let max_ref = "A".repeat(100);
        let long_reason = "r".repeat(1001);
        let max_reason = "é".repeat(1000);

        let cases: Vec<(
            Option<String>,
            Option<String>,
            Result<StageChangeMeta, &str>,
        )> = vec![
            (None, None, Ok(StageChangeMeta::default())),
            (some(""), some("   "), Ok(StageChangeMeta::default())),
            (
                some("  PROJ-1 "),
                some(" Ready\n"),
                Ok(meta(Some("PROJ-1"), Some("Ready"))),
            ),
            (
                some("CHG0012345"),
                some("line one\nline two"),
                Ok(meta(Some("CHG0012345"), Some("line one\nline two"))),
            ),
            (Some(max_ref.clone()), None, Ok(meta(Some(&max_ref), None))),
            (
                None,
                Some(max_reason.clone()),
                Ok(meta(None, Some(&max_reason))),
            ),
            (
                Some(long_ref),
                None,
                Err("externalRef must be at most 100 characters"),
            ),
            (
                None,
                Some(long_reason),
                Err("reason must be at most 1000 characters"),
            ),
            (
                some("PROJ-1\n2"),
                None,
                Err("externalRef must not contain control characters"),
            ),
            (
                None,
                some("tab\there"),
                Err("reason must not contain control characters other than newlines"),
            ),
            (
                None,
                some("bell\u{7}"),
                Err("reason must not contain control characters other than newlines"),
            ),
        ];

        for (external_ref, reason, expected) in cases {
            let input = format!("{external_ref:?} / {reason:?}");
            let result = validate_stage_change_meta(external_ref, reason);
            assert_eq!(result, expected.map_err(str::to_string), "input: {input}");
        }
    }
}
