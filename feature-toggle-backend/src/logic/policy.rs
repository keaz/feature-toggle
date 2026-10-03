use actix_web::http::Method;
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use crate::database::activity_log::{CreateActivityLog, activity_log_repository};

const POLICY_ALLOW_ACTIVITY_TYPE: &str = "policy_allow";
const POLICY_DENY_ACTIVITY_TYPE: &str = "policy_deny";
const TEAM_ADMIN_ROLE: &str = "Team Admin";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorKind {
    Anonymous,
    User,
    SystemClient,
}

#[derive(Debug, Clone)]
pub struct PolicyActor {
    pub kind: ActorKind,
    pub id: Option<Uuid>,
    pub username: Option<String>,
    pub is_admin: bool,
    pub roles: Vec<String>,
}

impl PolicyActor {
    pub fn anonymous() -> Self {
        Self {
            kind: ActorKind::Anonymous,
            id: None,
            username: None,
            is_admin: false,
            roles: Vec::new(),
        }
    }

    pub fn user(id: Uuid, username: String, is_admin: bool, roles: Vec<String>) -> Self {
        Self {
            kind: ActorKind::User,
            id: Some(id),
            username: Some(username),
            is_admin,
            roles,
        }
    }

    pub fn system_client(id: Uuid, username: String, roles: Vec<String>) -> Self {
        Self {
            kind: ActorKind::SystemClient,
            id: Some(id),
            username: Some(username),
            // System clients must not inherit human-admin privileges from token claims.
            is_admin: false,
            roles,
        }
    }

    fn has_role_ignore_case(&self, role_name: &str) -> bool {
        self.roles
            .iter()
            .any(|role| role.eq_ignore_ascii_case(role_name))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyAction {
    CreateAdmin,
    ManageUsers,
    AssignRoles,
    ManageRoles,
    UpdateTeamResource,
    ManageSystemClients,
    ManageJiraIntegrations,
    ManageSso,
}

impl PolicyAction {
    fn as_str(self) -> &'static str {
        match self {
            PolicyAction::CreateAdmin => "create_admin",
            PolicyAction::ManageUsers => "manage_users",
            PolicyAction::AssignRoles => "assign_roles",
            PolicyAction::ManageRoles => "manage_roles",
            PolicyAction::UpdateTeamResource => "update_team_resource",
            PolicyAction::ManageSystemClients => "manage_system_clients",
            PolicyAction::ManageJiraIntegrations => "manage_jira_integrations",
            PolicyAction::ManageSso => "manage_sso",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyResource {
    Admin,
    User,
    Role,
    Client,
    Context,
    Environment,
    Pipeline,
    Feature,
    SystemClient,
    SystemClientToken,
    JiraIntegration,
    Sso,
}

impl PolicyResource {
    fn as_str(self) -> &'static str {
        match self {
            PolicyResource::Admin => "admin",
            PolicyResource::User => "user",
            PolicyResource::Role => "role",
            PolicyResource::Client => "client",
            PolicyResource::Context => "context",
            PolicyResource::Environment => "environment",
            PolicyResource::Pipeline => "pipeline",
            PolicyResource::Feature => "feature",
            PolicyResource::SystemClient => "system_client",
            PolicyResource::SystemClientToken => "system_client_token",
            PolicyResource::JiraIntegration => "jira_integration",
            PolicyResource::Sso => "sso",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    #[error("authentication required")]
    Unauthorized,
    #[error("{0}")]
    Forbidden(String),
    #[error("policy service unavailable")]
    Internal(#[source] crate::Error),
}

#[derive(Debug, Clone)]
struct RoutePolicy {
    action: PolicyAction,
    resource: PolicyResource,
    resource_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
struct PolicyRequest {
    action: PolicyAction,
    resource: PolicyResource,
    resource_id: Option<Uuid>,
    team_id: Option<Uuid>,
    actor: Option<PolicyActor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PolicyDecision {
    allowed: bool,
    reason: &'static str,
    unauthorized: bool,
}

impl PolicyDecision {
    fn allow(reason: &'static str) -> Self {
        Self {
            allowed: true,
            reason,
            unauthorized: false,
        }
    }

    fn forbidden(reason: &'static str) -> Self {
        Self {
            allowed: false,
            reason,
            unauthorized: false,
        }
    }

    fn unauthorized(reason: &'static str) -> Self {
        Self {
            allowed: false,
            reason,
            unauthorized: true,
        }
    }
}

pub async fn enforce_for_route(
    pool: &sqlx::PgPool,
    method: &Method,
    path: &str,
    actor: Option<PolicyActor>,
) -> Result<(), PolicyError> {
    let Some(route_policy) = route_policy_for_request(method, path) else {
        return Ok(());
    };

    let team_id = match route_policy.action {
        PolicyAction::UpdateTeamResource => match route_policy.resource_id {
            Some(resource_id) => {
                resolve_team_id_for_resource(pool, route_policy.resource, resource_id).await?
            }
            None => None,
        },
        PolicyAction::ManageSystemClients | PolicyAction::ManageJiraIntegrations => {
            resolve_team_id_for_path(pool, path).await?
        }
        _ => None,
    };

    let policy_request = PolicyRequest {
        action: route_policy.action,
        resource: route_policy.resource,
        resource_id: route_policy.resource_id,
        team_id,
        actor,
    };

    let decision = evaluate(pool, &policy_request).await?;
    record_policy_decision(pool, &policy_request, decision).await;
    decision_to_result(decision)
}

fn decision_to_result(decision: PolicyDecision) -> Result<(), PolicyError> {
    if decision.allowed {
        Ok(())
    } else if decision.unauthorized {
        Err(PolicyError::Unauthorized)
    } else {
        Err(PolicyError::Forbidden(decision.reason.to_string()))
    }
}

/// The decision `PATCH /features/{id}` and `POST /features/{id}/emergency-*` get from
/// `enforce_for_route`, for callers that act on a feature outside that route (the
/// scheduled-change create handler and executor): a user actor that is a system admin,
/// or a `Team Admin` who belongs to `team_id`. System clients are always denied.
pub(crate) async fn authorize_feature_update(
    pool: &sqlx::PgPool,
    feature_id: Uuid,
    team_id: Uuid,
    actor: PolicyActor,
) -> Result<(), PolicyError> {
    let policy_request = PolicyRequest {
        action: PolicyAction::UpdateTeamResource,
        resource: PolicyResource::Feature,
        resource_id: Some(feature_id),
        team_id: Some(team_id),
        actor: Some(actor),
    };
    let decision = evaluate(pool, &policy_request).await?;
    record_policy_decision(pool, &policy_request, decision).await;
    decision_to_result(decision)
}

/// Whether `path` is a route only a team admin or system admin may use (system client
/// or Jira integration management, any method). System clients skip the token scope
/// check on these routes so the policy denies them (403 `policy_denied`, audited).
pub(crate) fn is_team_admin_management_route(path: &str) -> bool {
    matches!(
        route_policy_for_request(&Method::GET, path),
        Some(RoutePolicy {
            action: PolicyAction::ManageSystemClients | PolicyAction::ManageJiraIntegrations,
            ..
        })
    )
}

fn route_policy_for_request(method: &Method, path: &str) -> Option<RoutePolicy> {
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    if parts.len() < 3 || parts[0] != "api" || parts[1] != "v1" {
        return None;
    }

    let parse_uuid_at = |index: usize| -> Option<Uuid> {
        parts.get(index).and_then(|raw| Uuid::parse_str(raw).ok())
    };

    // System client management: every method and every sub-route, so a route added
    // later under these prefixes is guarded by default.
    match parts[2] {
        "system-clients" => {
            return Some(RoutePolicy {
                action: PolicyAction::ManageSystemClients,
                resource: PolicyResource::SystemClient,
                resource_id: parse_uuid_at(3),
            });
        }
        "system-client-tokens" => {
            return Some(RoutePolicy {
                action: PolicyAction::ManageSystemClients,
                resource: PolicyResource::SystemClientToken,
                resource_id: parse_uuid_at(3),
            });
        }
        // SSO administration: every method and sub-route, system admin only.
        "sso" => {
            return Some(RoutePolicy {
                action: PolicyAction::ManageSso,
                resource: PolicyResource::Sso,
                resource_id: parse_uuid_at(4),
            });
        }
        "teams" if parts.get(4) == Some(&"system-clients") => {
            return Some(RoutePolicy {
                action: PolicyAction::ManageSystemClients,
                resource: PolicyResource::SystemClient,
                resource_id: None,
            });
        }
        // Jira integration management: every method and sub-route, team admin or admin.
        "jira-integrations" => {
            return Some(RoutePolicy {
                action: PolicyAction::ManageJiraIntegrations,
                resource: PolicyResource::JiraIntegration,
                resource_id: parse_uuid_at(3),
            });
        }
        "teams" if parts.get(4) == Some(&"jira-integrations") => {
            return Some(RoutePolicy {
                action: PolicyAction::ManageJiraIntegrations,
                resource: PolicyResource::JiraIntegration,
                resource_id: None,
            });
        }
        _ => {}
    }

    match (method, parts[2]) {
        (&Method::POST, "admins") if parts.len() == 3 => Some(RoutePolicy {
            action: PolicyAction::CreateAdmin,
            resource: PolicyResource::Admin,
            resource_id: None,
        }),
        (&Method::POST, "users") if parts.len() == 3 => Some(RoutePolicy {
            action: PolicyAction::ManageUsers,
            resource: PolicyResource::User,
            resource_id: None,
        }),
        (&Method::PATCH, "users") if parts.len() == 4 => Some(RoutePolicy {
            action: PolicyAction::ManageUsers,
            resource: PolicyResource::User,
            resource_id: parse_uuid_at(3),
        }),
        (&Method::POST, "users") if parts.len() == 5 && parts[4] == "teams" => Some(RoutePolicy {
            action: PolicyAction::ManageUsers,
            resource: PolicyResource::User,
            resource_id: parse_uuid_at(3),
        }),
        (&Method::POST, "users") if parts.len() == 5 && parts[4] == "roles" => Some(RoutePolicy {
            action: PolicyAction::AssignRoles,
            resource: PolicyResource::User,
            resource_id: parse_uuid_at(3),
        }),
        (&Method::POST, "roles") if parts.len() == 3 => Some(RoutePolicy {
            action: PolicyAction::ManageRoles,
            resource: PolicyResource::Role,
            resource_id: None,
        }),
        (&Method::DELETE, "roles") if parts.len() == 4 => Some(RoutePolicy {
            action: PolicyAction::ManageRoles,
            resource: PolicyResource::Role,
            resource_id: parse_uuid_at(3),
        }),
        (&Method::POST, "auth")
            if parts.len() == 6 && parts[3] == "users" && parts[5] == "temporary-password" =>
        {
            Some(RoutePolicy {
                action: PolicyAction::ManageUsers,
                resource: PolicyResource::User,
                resource_id: parse_uuid_at(4),
            })
        }
        (&Method::PATCH, "clients") if parts.len() == 4 => Some(RoutePolicy {
            action: PolicyAction::UpdateTeamResource,
            resource: PolicyResource::Client,
            resource_id: parse_uuid_at(3),
        }),
        (&Method::PATCH, "contexts") if parts.len() == 4 => Some(RoutePolicy {
            action: PolicyAction::UpdateTeamResource,
            resource: PolicyResource::Context,
            resource_id: parse_uuid_at(3),
        }),
        (&Method::PATCH, "environments") if parts.len() == 4 => Some(RoutePolicy {
            action: PolicyAction::UpdateTeamResource,
            resource: PolicyResource::Environment,
            resource_id: parse_uuid_at(3),
        }),
        (&Method::PATCH, "pipelines") if parts.len() == 4 => Some(RoutePolicy {
            action: PolicyAction::UpdateTeamResource,
            resource: PolicyResource::Pipeline,
            resource_id: parse_uuid_at(3),
        }),
        (&Method::PATCH, "features") if parts.len() == 4 => Some(RoutePolicy {
            action: PolicyAction::UpdateTeamResource,
            resource: PolicyResource::Feature,
            resource_id: parse_uuid_at(3),
        }),
        (&Method::POST, "features")
            if parts.len() == 5
                && (parts[4] == "emergency-disable" || parts[4] == "emergency-enable") =>
        {
            Some(RoutePolicy {
                action: PolicyAction::UpdateTeamResource,
                resource: PolicyResource::Feature,
                resource_id: parse_uuid_at(3),
            })
        }
        _ => None,
    }
}

async fn evaluate(
    pool: &sqlx::PgPool,
    policy_request: &PolicyRequest,
) -> Result<PolicyDecision, PolicyError> {
    match policy_request.action {
        PolicyAction::CreateAdmin => evaluate_create_admin(pool, policy_request).await,
        PolicyAction::ManageUsers
        | PolicyAction::AssignRoles
        | PolicyAction::ManageRoles
        | PolicyAction::ManageSso => Ok(require_user_admin(policy_request.actor.as_ref())),
        PolicyAction::UpdateTeamResource => {
            evaluate_team_admin_or_admin(
                pool,
                policy_request,
                "system_client_updates_not_permitted",
            )
            .await
        }
        PolicyAction::ManageSystemClients => {
            evaluate_team_admin_or_admin(
                pool,
                policy_request,
                "system_client_management_not_permitted",
            )
            .await
        }
        PolicyAction::ManageJiraIntegrations => {
            evaluate_team_admin_or_admin(
                pool,
                policy_request,
                "jira_integration_management_not_permitted",
            )
            .await
        }
    }
}

async fn evaluate_create_admin(
    pool: &sqlx::PgPool,
    policy_request: &PolicyRequest,
) -> Result<PolicyDecision, PolicyError> {
    if !admin_exists(pool).await? {
        return Ok(PolicyDecision::allow("bootstrap_admin_creation_allowed"));
    }

    Ok(require_user_admin(policy_request.actor.as_ref()))
}

fn require_user_admin(actor: Option<&PolicyActor>) -> PolicyDecision {
    let Some(actor) = actor else {
        return PolicyDecision::unauthorized("authentication_required");
    };

    if actor.kind != ActorKind::User {
        return PolicyDecision::forbidden("user_session_required");
    }

    if actor.is_admin {
        PolicyDecision::allow("admin_access_granted")
    } else {
        PolicyDecision::forbidden("admin_access_required")
    }
}

/// Allows a user actor who is a system admin, or who holds the `Team Admin` role and
/// is a member of the owning team. System clients are denied with `system_client_reason`.
async fn evaluate_team_admin_or_admin(
    pool: &sqlx::PgPool,
    policy_request: &PolicyRequest,
    system_client_reason: &'static str,
) -> Result<PolicyDecision, PolicyError> {
    let Some(actor) = policy_request.actor.as_ref() else {
        return Ok(PolicyDecision::unauthorized("authentication_required"));
    };

    if actor.kind != ActorKind::User {
        return Ok(PolicyDecision::forbidden(system_client_reason));
    }

    if actor.is_admin {
        return Ok(PolicyDecision::allow("admin_access_granted"));
    }

    if !actor.has_role_ignore_case(TEAM_ADMIN_ROLE) {
        return Ok(PolicyDecision::forbidden("team_admin_role_required"));
    }

    let Some(team_id) = policy_request.team_id else {
        return Ok(PolicyDecision::forbidden("team_scope_not_resolved"));
    };
    let Some(actor_id) = actor.id else {
        return Ok(PolicyDecision::unauthorized("actor_id_missing"));
    };

    if user_in_team(pool, actor_id, team_id).await? {
        Ok(PolicyDecision::allow("team_membership_verified"))
    } else {
        Ok(PolicyDecision::forbidden("team_membership_required"))
    }
}

/// Whether at least one enabled admin account exists. Disabled admins cannot log
/// in, so they must not count towards the "admin already configured" decision.
/// System-client shadow users are never human admins and are ignored.
pub(crate) async fn admin_exists<'e, E>(executor: E) -> Result<bool, PolicyError>
where
    E: sqlx::PgExecutor<'e>,
{
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM users \
         WHERE is_admin = TRUE AND enabled = TRUE \
           AND id NOT IN (SELECT id FROM system_clients))",
    )
    .fetch_one(executor)
    .await
    .map_err(|e| PolicyError::Internal(crate::Error::DatabaseError(e)))
}

async fn user_in_team(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    team_id: Uuid,
) -> Result<bool, PolicyError> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM user_teams WHERE user_id = $1 AND team_id = $2)",
    )
    .bind(user_id)
    .bind(team_id)
    .fetch_one(pool)
    .await
    .map_err(|e| PolicyError::Internal(crate::Error::DatabaseError(e)))
}

/// Owning team of a system-client or Jira integration management route, via the same resolver the JWT
/// guard uses for system-client token scoping. A malformed id in the path resolves
/// to no team (non-admins are then denied; admins reach the handler, which 400s/404s).
async fn resolve_team_id_for_path(
    pool: &sqlx::PgPool,
    path: &str,
) -> Result<Option<Uuid>, PolicyError> {
    match super::authorization::request_scope_resolver(pool.clone())
        .resolve_team_id_for_request(path)
        .await
    {
        Ok(team_id) => Ok(team_id),
        Err(crate::Error::InvalidInput(_)) => Ok(None),
        Err(err) => Err(PolicyError::Internal(err)),
    }
}

async fn resolve_team_id_for_resource(
    pool: &sqlx::PgPool,
    resource: PolicyResource,
    resource_id: Uuid,
) -> Result<Option<Uuid>, PolicyError> {
    let query = match resource {
        PolicyResource::Client => "SELECT team_id FROM clients WHERE id = $1",
        PolicyResource::Context => "SELECT team_id FROM contexts WHERE id = $1",
        PolicyResource::Environment => "SELECT team_id FROM environments WHERE id = $1",
        PolicyResource::Pipeline => "SELECT team_id FROM pipelines WHERE id = $1",
        PolicyResource::Feature => "SELECT team_id FROM features WHERE id = $1",
        _ => return Ok(None),
    };

    sqlx::query(query)
        .bind(resource_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| PolicyError::Internal(crate::Error::DatabaseError(e)))
        .map(|row_opt| row_opt.map(|row| row.get::<Uuid, _>("team_id")))
}

async fn record_policy_decision(
    pool: &sqlx::PgPool,
    policy_request: &PolicyRequest,
    decision: PolicyDecision,
) {
    let repo = activity_log_repository(pool.clone());
    let actor_id = policy_request.actor.as_ref().and_then(|a| a.id);
    let actor_name = policy_request
        .actor
        .as_ref()
        .and_then(|a| a.username.clone());
    let activity_type = if decision.allowed {
        POLICY_ALLOW_ACTIVITY_TYPE
    } else {
        POLICY_DENY_ACTIVITY_TYPE
    };
    let entity_id = policy_request
        .resource_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| "n/a".to_string());
    let action = policy_request.action.as_str();
    let resource = policy_request.resource.as_str();
    let team_id = policy_request.team_id.map(|id| id.to_string());
    let actor_kind = policy_request
        .actor
        .as_ref()
        .map(|a| match a.kind {
            ActorKind::Anonymous => "anonymous",
            ActorKind::User => "user",
            ActorKind::SystemClient => "system_client",
        })
        .unwrap_or("anonymous");
    let decision_label = if decision.allowed { "allow" } else { "deny" };

    let activity = CreateActivityLog {
        activity_type: activity_type.to_string(),
        entity_type: resource.to_string(),
        entity_id: entity_id.clone(),
        actor_id,
        actor_name,
        description: format!(
            "Policy {decision_label} for action '{action}' on resource '{resource}' ({entity_id}): {}",
            decision.reason
        ),
        metadata: Some(json!({
            "actor_kind": actor_kind,
            "action": action,
            "resource": resource,
            "resource_id": entity_id,
            "team_id": team_id,
            "decision": decision_label,
            "reason": decision.reason,
        })),
    };

    if let Err(err) = repo.create_activity(activity).await {
        log::warn!("Failed to record policy decision audit log: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::PgPoolOptions;

    async fn test_pool() -> sqlx::PgPool {
        let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL not set");
        PgPoolOptions::new()
            .max_connections(5)
            .connect(&db_url)
            .await
            .expect("Failed to connect to database")
    }

    async fn insert_team(pool: &sqlx::PgPool) -> Uuid {
        let team_id = Uuid::new_v4();
        let name = format!("policy-team-{team_id}");
        sqlx::query!(
            r#"INSERT INTO teams (id, name, description) VALUES ($1, $2, $3)"#,
            team_id,
            name,
            "policy test team"
        )
        .execute(pool)
        .await
        .expect("Failed to insert team");
        team_id
    }

    async fn insert_environment(pool: &sqlx::PgPool, team_id: Uuid) -> Uuid {
        let environment_id = Uuid::new_v4();
        sqlx::query!(
            r#"INSERT INTO environments (id, name, active, team_id, environment_type)
               VALUES ($1, $2, $3, $4, $5)"#,
            environment_id,
            format!("policy-env-{environment_id}"),
            true,
            team_id,
            "Development",
        )
        .execute(pool)
        .await
        .expect("Failed to insert environment");
        environment_id
    }

    async fn insert_client(pool: &sqlx::PgPool, team_id: Uuid, environment_id: Uuid) -> Uuid {
        let client_id = Uuid::new_v4();
        sqlx::query!(
            r#"INSERT INTO clients (id, team_id, environment_id, name, description, enabled, client_type, api_key)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"#,
            client_id,
            team_id,
            environment_id,
            format!("policy-client-{client_id}"),
            Some("policy test client".to_string()),
            true,
            "Web",
            format!("policy-key-{client_id}"),
        )
        .execute(pool)
        .await
        .expect("Failed to insert client");
        client_id
    }

    async fn insert_feature(pool: &sqlx::PgPool, team_id: Uuid) -> Uuid {
        let feature_id = Uuid::new_v4();
        sqlx::query!(
            r#"INSERT INTO features (id, key, description, feature_type, team_id)
               VALUES ($1, $2, $3, $4, $5)"#,
            feature_id,
            format!("policy_feature_{feature_id}"),
            Some("policy feature".to_string()),
            "Simple",
            team_id,
        )
        .execute(pool)
        .await
        .expect("Failed to insert feature");
        feature_id
    }

    async fn insert_user(pool: &sqlx::PgPool, is_admin: bool, tag: &str) -> Uuid {
        let user_id = Uuid::new_v4();
        sqlx::query!(
            r#"INSERT INTO users (id, username, password_hash, first_name, last_name, email, is_admin)
               VALUES ($1, $2, $3, $4, $5, $6, $7)"#,
            user_id,
            format!("policy_user_{tag}_{user_id}"),
            "not-used-hash",
            "Policy",
            "Tester",
            format!("policy_{tag}_{user_id}@example.com"),
            is_admin,
        )
        .execute(pool)
        .await
        .expect("Failed to insert user");
        user_id
    }

    async fn assign_user_to_team(pool: &sqlx::PgPool, user_id: Uuid, team_id: Uuid) {
        sqlx::query!(
            r#"INSERT INTO user_teams (user_id, team_id) VALUES ($1, $2)
               ON CONFLICT (user_id, team_id) DO NOTHING"#,
            user_id,
            team_id
        )
        .execute(pool)
        .await
        .expect("Failed to assign user to team");
    }

    async fn team_admin_role_id(pool: &sqlx::PgPool) -> Uuid {
        sqlx::query_scalar!(
            r#"SELECT id FROM roles WHERE LOWER(name) = LOWER($1) LIMIT 1"#,
            TEAM_ADMIN_ROLE
        )
        .fetch_one(pool)
        .await
        .expect("Team Admin role must exist")
    }

    async fn assign_role(pool: &sqlx::PgPool, user_id: Uuid, role_id: Uuid) {
        sqlx::query!(
            r#"INSERT INTO user_roles (user_id, role_id, assigned_by) VALUES ($1, $2, $3)
               ON CONFLICT (user_id, role_id) DO NOTHING"#,
            user_id,
            role_id,
            user_id,
        )
        .execute(pool)
        .await
        .expect("Failed to assign role");
    }

    #[tokio::test]
    async fn allows_team_admin_update_for_client_in_same_team() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let environment_id = insert_environment(&pool, team_id).await;
        let client_id = insert_client(&pool, team_id, environment_id).await;
        let user_id = insert_user(&pool, false, "client_allow").await;
        let role_id = team_admin_role_id(&pool).await;
        assign_role(&pool, user_id, role_id).await;
        assign_user_to_team(&pool, user_id, team_id).await;

        let actor = PolicyActor::user(
            user_id,
            "team-admin".to_string(),
            false,
            vec![TEAM_ADMIN_ROLE.to_string()],
        );

        let result = enforce_for_route(
            &pool,
            &Method::PATCH,
            &format!("/api/v1/clients/{client_id}"),
            Some(actor),
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn denies_team_admin_update_for_feature_in_different_team() {
        let pool = test_pool().await;
        let caller_team_id = insert_team(&pool).await;
        let feature_team_id = insert_team(&pool).await;
        let feature_id = insert_feature(&pool, feature_team_id).await;
        let user_id = insert_user(&pool, false, "feature_deny").await;
        let role_id = team_admin_role_id(&pool).await;
        assign_role(&pool, user_id, role_id).await;
        assign_user_to_team(&pool, user_id, caller_team_id).await;

        let actor = PolicyActor::user(
            user_id,
            "cross-team-admin".to_string(),
            false,
            vec![TEAM_ADMIN_ROLE.to_string()],
        );

        let result = enforce_for_route(
            &pool,
            &Method::PATCH,
            &format!("/api/v1/features/{feature_id}"),
            Some(actor),
        )
        .await;

        assert!(matches!(result, Err(PolicyError::Forbidden(_))));
    }

    #[tokio::test]
    async fn create_admin_requires_admin_after_bootstrap() {
        let pool = test_pool().await;
        let _existing_admin = insert_user(&pool, true, "existing_admin").await;
        let non_admin_id = insert_user(&pool, false, "non_admin").await;
        let actor_non_admin =
            PolicyActor::user(non_admin_id, "non-admin".to_string(), false, Vec::new());
        let admin_id = insert_user(&pool, true, "admin_actor").await;
        let actor_admin = PolicyActor::user(admin_id, "admin".to_string(), true, Vec::new());

        let no_actor_result = enforce_for_route(&pool, &Method::POST, "/api/v1/admins", None).await;
        assert!(matches!(no_actor_result, Err(PolicyError::Unauthorized)));

        let non_admin_result = enforce_for_route(
            &pool,
            &Method::POST,
            "/api/v1/admins",
            Some(actor_non_admin),
        )
        .await;
        assert!(matches!(non_admin_result, Err(PolicyError::Forbidden(_))));

        let admin_result =
            enforce_for_route(&pool, &Method::POST, "/api/v1/admins", Some(actor_admin)).await;
        assert!(admin_result.is_ok());
    }

    #[tokio::test]
    async fn admin_exists_ignores_disabled_admins() {
        let pool = test_pool().await;
        let mut tx = pool.begin().await.expect("begin tx");

        // Disable every admin inside a transaction that is rolled back afterwards.
        sqlx::query("UPDATE users SET enabled = FALSE WHERE is_admin = TRUE")
            .execute(&mut *tx)
            .await
            .expect("disable admins");
        assert!(!admin_exists(&mut *tx).await.expect("query admins"));

        // A single enabled admin flips the answer back.
        let enabled_admin = Uuid::new_v4();
        sqlx::query(
            r#"INSERT INTO users (id, username, password_hash, first_name, last_name, email, is_admin, enabled)
               VALUES ($1, $2, 'x', 'A', 'B', $3, TRUE, TRUE)"#,
        )
        .bind(enabled_admin)
        .bind(format!("policy_admin_exists_{enabled_admin}"))
        .bind(format!("policy_admin_exists_{enabled_admin}@example.com"))
        .execute(&mut *tx)
        .await
        .expect("insert admin");
        assert!(admin_exists(&mut *tx).await.expect("query admins"));

        tx.rollback().await.expect("rollback");
    }

    // ---- system client management routes ----

    async fn insert_system_client(pool: &sqlx::PgPool, team_id: Uuid) -> (Uuid, Uuid) {
        let system_client_id = Uuid::new_v4();
        sqlx::query(
            r#"INSERT INTO system_clients (id, team_id, name, enabled, expires_at)
               VALUES ($1, $2, $3, TRUE, NOW() + INTERVAL '1 day')"#,
        )
        .bind(system_client_id)
        .bind(team_id)
        .bind(format!("policy-sc-{system_client_id}"))
        .execute(pool)
        .await
        .expect("Failed to insert system client");
        let token_id = Uuid::new_v4();
        sqlx::query(
            r#"INSERT INTO system_client_tokens (id, system_client_id, token_hash, expires_at)
               VALUES ($1, $2, $3, NOW() + INTERVAL '1 day')"#,
        )
        .bind(token_id)
        .bind(system_client_id)
        .bind(format!("policy-token-{token_id}"))
        .execute(pool)
        .await
        .expect("Failed to insert system client token");
        (system_client_id, token_id)
    }

    /// Every system client management route (method, path).
    fn management_routes(
        team_id: Uuid,
        system_client_id: Uuid,
        token_id: Uuid,
    ) -> Vec<(Method, String)> {
        vec![
            (
                Method::GET,
                format!("/api/v1/teams/{team_id}/system-clients"),
            ),
            (
                Method::POST,
                format!("/api/v1/teams/{team_id}/system-clients"),
            ),
            (
                Method::GET,
                format!("/api/v1/system-clients/{system_client_id}"),
            ),
            (
                Method::PATCH,
                format!("/api/v1/system-clients/{system_client_id}"),
            ),
            (
                Method::POST,
                format!("/api/v1/system-clients/{system_client_id}/regenerate-token"),
            ),
            (
                Method::GET,
                format!("/api/v1/system-clients/{system_client_id}/tokens"),
            ),
            (
                Method::POST,
                format!("/api/v1/system-clients/{system_client_id}/tokens"),
            ),
            (
                Method::POST,
                format!("/api/v1/system-client-tokens/{token_id}/revoke"),
            ),
        ]
    }

    fn team_admin_actor(user_id: Uuid) -> PolicyActor {
        PolicyActor::user(
            user_id,
            "team-admin".to_string(),
            false,
            vec![TEAM_ADMIN_ROLE.to_string()],
        )
    }

    #[test]
    fn every_system_client_route_and_method_has_a_management_policy() {
        let id = Uuid::new_v4();
        for method in [
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::PUT,
            Method::DELETE,
        ] {
            for path in [
                format!("/api/v1/teams/{id}/system-clients"),
                format!("/api/v1/teams/{id}/system-clients/"),
                "/api/v1/system-clients".to_string(),
                format!("/api/v1/system-clients/{id}"),
                format!("/api/v1/system-clients/{id}/tokens"),
                format!("/api/v1/system-clients/{id}/regenerate-token"),
                format!("/api/v1/system-client-tokens/{id}/revoke"),
            ] {
                let policy = route_policy_for_request(&method, &path)
                    .unwrap_or_else(|| panic!("{method} {path} has no policy"));
                assert_eq!(policy.action, PolicyAction::ManageSystemClients);
                assert!(is_team_admin_management_route(&path));
            }
        }
        // Neighbouring team routes stay out of scope.
        assert!(!is_team_admin_management_route(&format!(
            "/api/v1/teams/{id}/clients"
        )));
    }

    #[tokio::test]
    async fn denies_non_admin_without_team_admin_role_on_all_system_client_routes() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let (system_client_id, token_id) = insert_system_client(&pool, team_id).await;
        let user_id = insert_user(&pool, false, "sc_plain").await;
        assign_user_to_team(&pool, user_id, team_id).await;

        for (method, path) in management_routes(team_id, system_client_id, token_id) {
            let actor = PolicyActor::user(user_id, "plain".to_string(), false, Vec::new());
            let result = enforce_for_route(&pool, &method, &path, Some(actor)).await;
            assert!(
                matches!(result, Err(PolicyError::Forbidden(_))),
                "{method} {path}: {result:?}"
            );
        }
    }

    #[tokio::test]
    async fn denies_team_admin_of_another_team_on_all_system_client_routes() {
        let pool = test_pool().await;
        let owning_team = insert_team(&pool).await;
        let other_team = insert_team(&pool).await;
        let (system_client_id, token_id) = insert_system_client(&pool, owning_team).await;
        let user_id = insert_user(&pool, false, "sc_other_team").await;
        let role_id = team_admin_role_id(&pool).await;
        assign_role(&pool, user_id, role_id).await;
        assign_user_to_team(&pool, user_id, other_team).await;

        for (method, path) in management_routes(owning_team, system_client_id, token_id) {
            let result =
                enforce_for_route(&pool, &method, &path, Some(team_admin_actor(user_id))).await;
            assert!(
                matches!(result, Err(PolicyError::Forbidden(_))),
                "{method} {path}: {result:?}"
            );
        }
    }

    #[tokio::test]
    async fn denies_team_admin_role_without_membership_in_owning_team() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let user_id = insert_user(&pool, false, "sc_no_member").await;
        let role_id = team_admin_role_id(&pool).await;
        assign_role(&pool, user_id, role_id).await;

        let result = enforce_for_route(
            &pool,
            &Method::GET,
            &format!("/api/v1/teams/{team_id}/system-clients"),
            Some(team_admin_actor(user_id)),
        )
        .await;
        assert!(matches!(result, Err(PolicyError::Forbidden(_))));
    }

    #[tokio::test]
    async fn allows_team_admin_of_owning_team_on_all_system_client_routes() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let (system_client_id, token_id) = insert_system_client(&pool, team_id).await;
        let user_id = insert_user(&pool, false, "sc_team_admin").await;
        let role_id = team_admin_role_id(&pool).await;
        assign_role(&pool, user_id, role_id).await;
        assign_user_to_team(&pool, user_id, team_id).await;

        for (method, path) in management_routes(team_id, system_client_id, token_id) {
            let result =
                enforce_for_route(&pool, &method, &path, Some(team_admin_actor(user_id))).await;
            assert!(result.is_ok(), "{method} {path}: {result:?}");
        }
    }

    #[tokio::test]
    async fn allows_system_admin_on_system_client_routes_even_for_unknown_resources() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let (system_client_id, token_id) = insert_system_client(&pool, team_id).await;
        let admin_id = insert_user(&pool, true, "sc_admin").await;

        for (method, path) in management_routes(team_id, system_client_id, token_id)
            .into_iter()
            .chain([
                (
                    Method::GET,
                    format!("/api/v1/system-clients/{}", Uuid::new_v4()),
                ),
                (Method::GET, "/api/v1/system-clients/not-a-uuid".to_string()),
            ])
        {
            let actor = PolicyActor::user(admin_id, "admin".to_string(), true, Vec::new());
            let result = enforce_for_route(&pool, &method, &path, Some(actor)).await;
            assert!(result.is_ok(), "{method} {path}: {result:?}");
        }
    }

    #[tokio::test]
    async fn denies_unknown_system_client_resource_for_team_admin() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let user_id = insert_user(&pool, false, "sc_unknown").await;
        let role_id = team_admin_role_id(&pool).await;
        assign_role(&pool, user_id, role_id).await;
        assign_user_to_team(&pool, user_id, team_id).await;

        for path in [
            format!("/api/v1/system-clients/{}", Uuid::new_v4()),
            "/api/v1/system-clients/not-a-uuid".to_string(),
            "/api/v1/system-clients".to_string(),
        ] {
            let result =
                enforce_for_route(&pool, &Method::GET, &path, Some(team_admin_actor(user_id)))
                    .await;
            assert!(
                matches!(result, Err(PolicyError::Forbidden(_))),
                "{path}: {result:?}"
            );
        }
    }

    #[tokio::test]
    async fn denies_system_client_actor_and_anonymous_on_system_client_routes() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let (system_client_id, token_id) = insert_system_client(&pool, team_id).await;
        // Even a token whose roles claim Team Admin must be denied.
        let roles = vec![TEAM_ADMIN_ROLE.to_string()];

        for (method, path) in management_routes(team_id, system_client_id, token_id) {
            let actor =
                PolicyActor::system_client(system_client_id, "bot".to_string(), roles.clone());
            let result = enforce_for_route(&pool, &method, &path, Some(actor)).await;
            assert!(
                matches!(result, Err(PolicyError::Forbidden(_))),
                "{method} {path}: {result:?}"
            );

            let result = enforce_for_route(&pool, &method, &path, None).await;
            assert!(
                matches!(result, Err(PolicyError::Unauthorized)),
                "{method} {path}: {result:?}"
            );
        }
    }

    #[tokio::test]
    async fn system_client_management_decisions_are_audited() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let user_id = insert_user(&pool, false, "sc_audit").await;

        let result = enforce_for_route(
            &pool,
            &Method::POST,
            &format!("/api/v1/teams/{team_id}/system-clients"),
            Some(PolicyActor::user(
                user_id,
                "audited".to_string(),
                false,
                Vec::new(),
            )),
        )
        .await;
        assert!(matches!(result, Err(PolicyError::Forbidden(_))));

        let (count, reason): (i64, Option<String>) = sqlx::query_as(
            r#"SELECT COUNT(*), MAX(metadata->>'reason') FROM activity_log
               WHERE activity_type = 'policy_deny'
                 AND actor_id = $1
                 AND metadata->>'action' = 'manage_system_clients'
                 AND metadata->>'team_id' = $2"#,
        )
        .bind(user_id)
        .bind(team_id.to_string())
        .fetch_one(&pool)
        .await
        .expect("query activity log");
        assert_eq!(count, 1);
        assert_eq!(reason.as_deref(), Some("team_admin_role_required"));
    }

    #[tokio::test]
    async fn admin_exists_ignores_system_client_shadow_users() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let mut tx = pool.begin().await.expect("begin tx");

        sqlx::query("UPDATE users SET enabled = FALSE WHERE is_admin = TRUE")
            .execute(&mut *tx)
            .await
            .expect("disable admins");

        // A legacy shadow user: enabled, flagged admin, but backed by a system client.
        let shadow_id = Uuid::new_v4();
        sqlx::query(
            r#"INSERT INTO system_clients (id, team_id, name, enabled, expires_at)
               VALUES ($1, $2, $3, TRUE, NOW() + INTERVAL '1 day')"#,
        )
        .bind(shadow_id)
        .bind(team_id)
        .bind(format!("policy-shadow-{shadow_id}"))
        .execute(&mut *tx)
        .await
        .expect("insert system client");
        sqlx::query(
            r#"INSERT INTO users (id, username, password_hash, first_name, last_name, email, is_admin, enabled)
               VALUES ($1, $2, 'x', 'A', 'B', $3, TRUE, TRUE)"#,
        )
        .bind(shadow_id)
        .bind(format!("policy_shadow_{shadow_id}"))
        .bind(format!("policy_shadow_{shadow_id}@example.com"))
        .execute(&mut *tx)
        .await
        .expect("insert shadow user");

        assert!(!admin_exists(&mut *tx).await.expect("query admins"));

        tx.rollback().await.expect("rollback");
    }

    // ---- Jira integration management routes ----

    async fn insert_jira_integration(pool: &sqlx::PgPool, team_id: Uuid) -> Uuid {
        let actor_id = insert_user(pool, false, "jira_shadow").await;
        let integration_id = Uuid::new_v4();
        sqlx::query(
            r#"INSERT INTO jira_integrations (id, team_id, name, secret_hash, environment_field, actor_user_id)
               VALUES ($1, $2, $3, 'hash', 'labels', $4)"#,
        )
        .bind(integration_id)
        .bind(team_id)
        .bind(format!("policy-jira-{integration_id}"))
        .bind(actor_id)
        .execute(pool)
        .await
        .expect("Failed to insert Jira integration");
        integration_id
    }

    /// Every Jira integration management route (method, path).
    fn jira_integration_routes(team_id: Uuid, integration_id: Uuid) -> Vec<(Method, String)> {
        vec![
            (
                Method::GET,
                format!("/api/v1/teams/{team_id}/jira-integrations"),
            ),
            (
                Method::POST,
                format!("/api/v1/teams/{team_id}/jira-integrations"),
            ),
            (
                Method::GET,
                format!("/api/v1/jira-integrations/{integration_id}"),
            ),
            (
                Method::PATCH,
                format!("/api/v1/jira-integrations/{integration_id}"),
            ),
            (
                Method::DELETE,
                format!("/api/v1/jira-integrations/{integration_id}"),
            ),
            (
                Method::POST,
                format!("/api/v1/jira-integrations/{integration_id}/rotate-secret"),
            ),
            (
                Method::POST,
                format!("/api/v1/jira-integrations/{integration_id}/native-webhook-secret"),
            ),
            (
                Method::DELETE,
                format!("/api/v1/jira-integrations/{integration_id}/native-webhook-secret"),
            ),
            (
                Method::GET,
                format!("/api/v1/jira-integrations/{integration_id}/rules"),
            ),
            (
                Method::PUT,
                format!("/api/v1/jira-integrations/{integration_id}/rules"),
            ),
            (
                Method::GET,
                format!("/api/v1/jira-integrations/{integration_id}/events"),
            ),
            (
                Method::PUT,
                format!("/api/v1/jira-integrations/{integration_id}/writeback"),
            ),
            (
                Method::POST,
                format!("/api/v1/jira-integrations/{integration_id}/writeback/test"),
            ),
            (
                Method::POST,
                format!("/api/v1/jira-integrations/{integration_id}/writeback/resume"),
            ),
            (
                Method::GET,
                format!("/api/v1/jira-integrations/{integration_id}/outbound-jobs"),
            ),
            (
                Method::POST,
                format!(
                    "/api/v1/jira-integrations/{integration_id}/outbound-jobs/{}/retry",
                    Uuid::new_v4()
                ),
            ),
        ]
    }

    #[test]
    fn every_jira_integration_route_and_method_has_a_management_policy() {
        let id = Uuid::new_v4();
        for method in [
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::PUT,
            Method::DELETE,
        ] {
            for path in [
                format!("/api/v1/teams/{id}/jira-integrations"),
                format!("/api/v1/teams/{id}/jira-integrations/"),
                "/api/v1/jira-integrations".to_string(),
                format!("/api/v1/jira-integrations/{id}"),
                format!("/api/v1/jira-integrations/{id}/rules"),
                format!("/api/v1/jira-integrations/{id}/rotate-secret"),
                format!("/api/v1/jira-integrations/{id}/native-webhook-secret"),
                format!("/api/v1/jira-integrations/{id}/events"),
                format!("/api/v1/jira-integrations/{id}/writeback"),
                format!("/api/v1/jira-integrations/{id}/writeback/test"),
                format!("/api/v1/jira-integrations/{id}/writeback/resume"),
                format!("/api/v1/jira-integrations/{id}/outbound-jobs"),
                format!("/api/v1/jira-integrations/{id}/outbound-jobs/{id}/retry"),
                // A route added later under the prefix is guarded by default.
                format!("/api/v1/jira-integrations/{id}/events"),
            ] {
                let policy = route_policy_for_request(&method, &path)
                    .unwrap_or_else(|| panic!("{method} {path} has no policy"));
                assert_eq!(policy.action, PolicyAction::ManageJiraIntegrations);
                assert!(is_team_admin_management_route(&path), "{path}");
            }
        }
        assert!(is_team_admin_management_route(&format!(
            "/api/v1/system-clients/{id}"
        )));
        assert!(!is_team_admin_management_route(&format!(
            "/api/v1/teams/{id}/clients"
        )));
        assert!(!is_team_admin_management_route(&format!(
            "/api/v1/features/{id}/external-links"
        )));
    }

    #[tokio::test]
    async fn allows_team_admin_of_owning_team_on_all_jira_integration_routes() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let integration_id = insert_jira_integration(&pool, team_id).await;
        let user_id = insert_user(&pool, false, "jira_team_admin").await;
        let role_id = team_admin_role_id(&pool).await;
        assign_role(&pool, user_id, role_id).await;
        assign_user_to_team(&pool, user_id, team_id).await;

        for (method, path) in jira_integration_routes(team_id, integration_id) {
            let result =
                enforce_for_route(&pool, &method, &path, Some(team_admin_actor(user_id))).await;
            assert!(result.is_ok(), "{method} {path}: {result:?}");
        }
    }

    #[tokio::test]
    async fn allows_system_admin_on_jira_integration_routes() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let integration_id = insert_jira_integration(&pool, team_id).await;
        let admin_id = insert_user(&pool, true, "jira_admin").await;

        for (method, path) in jira_integration_routes(team_id, integration_id)
            .into_iter()
            .chain([(
                Method::GET,
                format!("/api/v1/jira-integrations/{}", Uuid::new_v4()),
            )])
        {
            let actor = PolicyActor::user(admin_id, "admin".to_string(), true, Vec::new());
            let result = enforce_for_route(&pool, &method, &path, Some(actor)).await;
            assert!(result.is_ok(), "{method} {path}: {result:?}");
        }
    }

    #[tokio::test]
    async fn denies_plain_users_other_team_admins_and_system_clients_on_jira_integration_routes() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let other_team = insert_team(&pool).await;
        let integration_id = insert_jira_integration(&pool, team_id).await;
        let role_id = team_admin_role_id(&pool).await;

        let plain = insert_user(&pool, false, "jira_plain").await;
        assign_user_to_team(&pool, plain, team_id).await;
        let other_admin = insert_user(&pool, false, "jira_other_admin").await;
        assign_role(&pool, other_admin, role_id).await;
        assign_user_to_team(&pool, other_admin, other_team).await;
        let (system_client_id, _) = insert_system_client(&pool, team_id).await;

        for (method, path) in jira_integration_routes(team_id, integration_id) {
            let actors = [
                PolicyActor::user(plain, "plain".to_string(), false, Vec::new()),
                team_admin_actor(other_admin),
                // Even a token whose roles claim Team Admin must be denied.
                PolicyActor::system_client(
                    system_client_id,
                    "bot".to_string(),
                    vec![TEAM_ADMIN_ROLE.to_string()],
                ),
            ];
            for actor in actors {
                let result = enforce_for_route(&pool, &method, &path, Some(actor)).await;
                assert!(
                    matches!(result, Err(PolicyError::Forbidden(_))),
                    "{method} {path}: {result:?}"
                );
            }
            let result = enforce_for_route(&pool, &method, &path, None).await;
            assert!(
                matches!(result, Err(PolicyError::Unauthorized)),
                "{method} {path}: {result:?}"
            );
        }

        // A Team Admin cannot reach an integration that does not exist.
        let owner_admin = insert_user(&pool, false, "jira_owner_admin").await;
        assign_role(&pool, owner_admin, role_id).await;
        assign_user_to_team(&pool, owner_admin, team_id).await;
        let result = enforce_for_route(
            &pool,
            &Method::GET,
            &format!("/api/v1/jira-integrations/{}", Uuid::new_v4()),
            Some(team_admin_actor(owner_admin)),
        )
        .await;
        assert!(
            matches!(result, Err(PolicyError::Forbidden(_))),
            "{result:?}"
        );
    }

    fn sso_admin_routes() -> Vec<(Method, String)> {
        let id = Uuid::new_v4();
        vec![
            (Method::GET, "/api/v1/sso/providers".to_string()),
            (Method::POST, "/api/v1/sso/providers".to_string()),
            (Method::GET, format!("/api/v1/sso/providers/{id}")),
            (Method::PATCH, format!("/api/v1/sso/providers/{id}")),
            (Method::DELETE, format!("/api/v1/sso/providers/{id}")),
            (Method::POST, format!("/api/v1/sso/providers/{id}/test")),
            (Method::GET, format!("/api/v1/sso/providers/{id}/mappings")),
            (Method::PUT, format!("/api/v1/sso/providers/{id}/mappings")),
            (Method::GET, "/api/v1/sso/settings".to_string()),
            (Method::PUT, "/api/v1/sso/settings".to_string()),
            // A route added later under the prefix is guarded by default.
            (Method::POST, "/api/v1/sso/future-route".to_string()),
        ]
    }

    #[test]
    fn every_sso_admin_route_has_a_manage_sso_policy() {
        for (method, path) in sso_admin_routes() {
            let policy = route_policy_for_request(&method, &path)
                .unwrap_or_else(|| panic!("{method} {path} has no policy"));
            assert_eq!(policy.action, PolicyAction::ManageSso, "{method} {path}");
        }
        // The public provider list is not an admin route.
        assert!(route_policy_for_request(&Method::GET, "/api/v1/auth/sso/providers").is_none());
    }

    #[tokio::test]
    async fn sso_admin_routes_allow_only_system_admin_users() {
        let pool = test_pool().await;
        let actor_id = Uuid::new_v4();
        for (method, path) in sso_admin_routes() {
            let admin = PolicyActor::user(actor_id, "admin".to_string(), true, Vec::new());
            assert!(
                enforce_for_route(&pool, &method, &path, Some(admin))
                    .await
                    .is_ok(),
                "admin {method} {path}"
            );

            let plain = PolicyActor::user(actor_id, "plain".to_string(), false, Vec::new());
            let team_admin = PolicyActor::user(
                actor_id,
                "team-admin".to_string(),
                false,
                vec![TEAM_ADMIN_ROLE.to_string()],
            );
            let system_client =
                PolicyActor::system_client(actor_id, "client".to_string(), Vec::new());
            for actor in [plain, team_admin, system_client] {
                let result = enforce_for_route(&pool, &method, &path, Some(actor)).await;
                assert!(
                    matches!(result, Err(PolicyError::Forbidden(_))),
                    "{method} {path}: {result:?}"
                );
            }

            let anonymous = enforce_for_route(&pool, &method, &path, None).await;
            assert!(
                matches!(anonymous, Err(PolicyError::Unauthorized)),
                "{method} {path}: {anonymous:?}"
            );
        }
    }
}
