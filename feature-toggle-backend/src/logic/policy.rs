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
}

impl PolicyAction {
    fn as_str(self) -> &'static str {
        match self {
            PolicyAction::CreateAdmin => "create_admin",
            PolicyAction::ManageUsers => "manage_users",
            PolicyAction::AssignRoles => "assign_roles",
            PolicyAction::ManageRoles => "manage_roles",
            PolicyAction::UpdateTeamResource => "update_team_resource",
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

    let team_id = if route_policy.action == PolicyAction::UpdateTeamResource {
        match route_policy.resource_id {
            Some(resource_id) => {
                resolve_team_id_for_resource(pool, route_policy.resource, resource_id).await?
            }
            None => None,
        }
    } else {
        None
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

    if decision.allowed {
        Ok(())
    } else if decision.unauthorized {
        Err(PolicyError::Unauthorized)
    } else {
        Err(PolicyError::Forbidden(decision.reason.to_string()))
    }
}

fn route_policy_for_request(method: &Method, path: &str) -> Option<RoutePolicy> {
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    if parts.len() < 3 || parts[0] != "api" || parts[1] != "v1" {
        return None;
    }

    let parse_uuid_at = |index: usize| -> Option<Uuid> {
        parts.get(index).and_then(|raw| Uuid::parse_str(raw).ok())
    };

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
        PolicyAction::ManageUsers | PolicyAction::AssignRoles | PolicyAction::ManageRoles => {
            Ok(require_user_admin(policy_request.actor.as_ref()))
        }
        PolicyAction::UpdateTeamResource => {
            evaluate_team_resource_update(pool, policy_request).await
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

async fn evaluate_team_resource_update(
    pool: &sqlx::PgPool,
    policy_request: &PolicyRequest,
) -> Result<PolicyDecision, PolicyError> {
    let Some(actor) = policy_request.actor.as_ref() else {
        return Ok(PolicyDecision::unauthorized("authentication_required"));
    };

    if actor.kind != ActorKind::User {
        return Ok(PolicyDecision::forbidden(
            "system_client_updates_not_permitted",
        ));
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

async fn admin_exists(pool: &sqlx::PgPool) -> Result<bool, PolicyError> {
    sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM users WHERE is_admin = TRUE)")
        .fetch_one(pool)
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
}
