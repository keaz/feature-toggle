use async_trait::async_trait;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScopedResourceKind {
    Team,
    Feature,
    Stage,
    ApprovalRequest,
    Pipeline,
    Environment,
    Context,
    Client,
    SystemClient,
}

impl ScopedResourceKind {
    fn from_path_segment(segment: &str) -> Option<Self> {
        match segment {
            "teams" => Some(Self::Team),
            "features" => Some(Self::Feature),
            "stages" => Some(Self::Stage),
            "approval-requests" => Some(Self::ApprovalRequest),
            "pipelines" => Some(Self::Pipeline),
            "environments" => Some(Self::Environment),
            "contexts" => Some(Self::Context),
            "clients" => Some(Self::Client),
            "system-clients" => Some(Self::SystemClient),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ScopedResourceRef {
    kind: ScopedResourceKind,
    id: Uuid,
}

fn parse_scoped_resource(path: &str) -> Result<Option<ScopedResourceRef>, Error> {
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    if parts.len() < 3 || parts[0] != "api" || parts[1] != "v1" {
        return Ok(None);
    }

    let Some(kind) = ScopedResourceKind::from_path_segment(parts[2]) else {
        return Ok(None);
    };

    if parts.len() < 4 {
        return Ok(None);
    }

    let id = Uuid::parse_str(parts[3]).map_err(|err| {
        let label = if kind == ScopedResourceKind::Team {
            "team id"
        } else {
            "resource id"
        };
        Error::InvalidInput(format!("invalid {label} in request path: {err}"))
    })?;

    Ok(Some(ScopedResourceRef { kind, id }))
}

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait RequestScopeResolver: Send + Sync {
    async fn resolve_team_id_for_request(&self, path: &str) -> Result<Option<Uuid>, Error>;

    fn clone_box(&self) -> Box<dyn RequestScopeResolver>;
}

impl Clone for Box<dyn RequestScopeResolver> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

#[derive(Clone)]
struct DatabaseRequestScopeResolver {
    pool: PgPool,
}

pub fn request_scope_resolver(pool: PgPool) -> Box<dyn RequestScopeResolver> {
    Box::new(DatabaseRequestScopeResolver { pool })
}

impl DatabaseRequestScopeResolver {
    async fn fetch_team_id(&self, query: &str, resource_id: Uuid) -> Result<Option<Uuid>, Error> {
        sqlx::query(query)
            .bind(resource_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(Error::DatabaseError)
            .map(|row| row.map(|value| value.get("team_id")))
    }
}

#[async_trait]
impl RequestScopeResolver for DatabaseRequestScopeResolver {
    async fn resolve_team_id_for_request(&self, path: &str) -> Result<Option<Uuid>, Error> {
        let Some(resource) = parse_scoped_resource(path)? else {
            return Ok(None);
        };

        match resource.kind {
            ScopedResourceKind::Team => Ok(Some(resource.id)),
            ScopedResourceKind::Feature => {
                self.fetch_team_id("SELECT team_id FROM features WHERE id = $1", resource.id)
                    .await
            }
            ScopedResourceKind::Stage => {
                self.fetch_team_id(
                    r#"
                    SELECT f.team_id
                    FROM features_pipeline_stages fs
                    JOIN features f ON f.id = fs.feature_id
                    WHERE fs.id = $1
                    "#,
                    resource.id,
                )
                .await
            }
            ScopedResourceKind::ApprovalRequest => {
                self.fetch_team_id(
                    r#"
                    SELECT f.team_id
                    FROM approval_requests ar
                    JOIN features f ON f.id = ar.feature_id
                    WHERE ar.id = $1
                    "#,
                    resource.id,
                )
                .await
            }
            ScopedResourceKind::Pipeline => {
                self.fetch_team_id("SELECT team_id FROM pipelines WHERE id = $1", resource.id)
                    .await
            }
            ScopedResourceKind::Environment => {
                self.fetch_team_id(
                    "SELECT team_id FROM environments WHERE id = $1",
                    resource.id,
                )
                .await
            }
            ScopedResourceKind::Context => {
                self.fetch_team_id("SELECT team_id FROM contexts WHERE id = $1", resource.id)
                    .await
            }
            ScopedResourceKind::Client => {
                self.fetch_team_id("SELECT team_id FROM clients WHERE id = $1", resource.id)
                    .await
            }
            ScopedResourceKind::SystemClient => {
                self.fetch_team_id(
                    "SELECT team_id FROM system_clients WHERE id = $1",
                    resource.id,
                )
                .await
            }
        }
    }

    fn clone_box(&self) -> Box<dyn RequestScopeResolver> {
        Box::new(self.clone())
    }
}

/// Authorization helper functions for role-based access control
pub struct RoleAuthorizer;

impl RoleAuthorizer {
    /// Check if user has the Requester role
    pub fn has_requester_role(user_roles: &[String]) -> bool {
        user_roles.iter().any(|role| role == "Requester")
    }

    /// Check if user has the Approver role
    pub fn has_approver_role(user_roles: &[String]) -> bool {
        user_roles.iter().any(|role| role == "Approver")
    }

    /// Authorize stage change request based on user roles and request type
    pub fn authorize_stage_change_request(
        user_roles: &[String],
        request_type: &str,
    ) -> Result<(), Error> {
        match request_type {
            // Requester role operations
            "DEPLOYMENT_REQUESTED" | "ROLLBACK_REQUESTED" => {
                if Self::has_requester_role(user_roles) {
                    Ok(())
                } else {
                    Err(Error::InvalidInput(
                        "Only users with 'Requester' role can request deployments or rollbacks"
                            .to_string(),
                    ))
                }
            }
            // Approver role operations
            "DEPLOYMENT_REJECTED" | "ROLLBACK_REJECTED" => {
                if Self::has_approver_role(user_roles) {
                    Ok(())
                } else {
                    Err(Error::InvalidInput(
                        "Only users with 'Approver' role can approve or reject requests"
                            .to_string(),
                    ))
                }
            }
            // Finalize deployment/rollback after approvals: either Requester or Approver can execute
            "DEPLOYED" | "ROLLBACKED" => {
                if Self::has_requester_role(user_roles) || Self::has_approver_role(user_roles) {
                    Ok(())
                } else {
                    Err(Error::InvalidInput(
                        "Only users with 'Requester' or 'Approver' role can execute deployments or rollbacks"
                            .to_string(),
                    ))
                }
            }
            _ => Err(Error::InvalidInput(format!(
                "Unknown stage change request type: {}",
                request_type
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::PgPoolOptions;

    fn test_pool() -> sqlx::PgPool {
        PgPoolOptions::new()
            .connect_lazy("postgres://user:pass@localhost/test_db")
            .expect("Failed to create test pool")
    }

    async fn test_db_pool_from_env() -> Option<sqlx::PgPool> {
        let Ok(db_url) = std::env::var("DATABASE_URL") else {
            return None;
        };

        Some(
            PgPoolOptions::new()
                .max_connections(1)
                .connect(&db_url)
                .await
                .expect("Failed to connect to DATABASE_URL for scope resolution tests"),
        )
    }

    #[test]
    fn test_has_requester_role() {
        let roles_with_requester = vec!["Requester".to_string(), "Team Admin".to_string()];
        let roles_without_requester = vec!["Approver".to_string(), "Team Admin".to_string()];
        let empty_roles = vec![];

        assert!(RoleAuthorizer::has_requester_role(&roles_with_requester));
        assert!(!RoleAuthorizer::has_requester_role(
            &roles_without_requester
        ));
        assert!(!RoleAuthorizer::has_requester_role(&empty_roles));
    }

    #[test]
    fn test_has_approver_role() {
        let roles_with_approver = vec!["Approver".to_string(), "Team Admin".to_string()];
        let roles_without_approver = vec!["Requester".to_string(), "Team Admin".to_string()];
        let empty_roles = vec![];

        assert!(RoleAuthorizer::has_approver_role(&roles_with_approver));
        assert!(!RoleAuthorizer::has_approver_role(&roles_without_approver));
        assert!(!RoleAuthorizer::has_approver_role(&empty_roles));
    }

    #[test]
    fn test_authorize_stage_change_request_requester_operations() {
        let requester_roles = vec!["Requester".to_string()];
        let approver_roles = vec!["Approver".to_string()];
        let no_roles = vec![];

        // Test deployment request - requires Requester role
        assert!(
            RoleAuthorizer::authorize_stage_change_request(
                &requester_roles,
                "DEPLOYMENT_REQUESTED"
            )
            .is_ok()
        );
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&approver_roles, "DEPLOYMENT_REQUESTED")
                .is_err()
        );
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&no_roles, "DEPLOYMENT_REQUESTED")
                .is_err()
        );

        // Test rollback request - requires Requester role
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&requester_roles, "ROLLBACK_REQUESTED")
                .is_ok()
        );
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&approver_roles, "ROLLBACK_REQUESTED")
                .is_err()
        );
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&no_roles, "ROLLBACK_REQUESTED")
                .is_err()
        );
    }

    #[test]
    fn test_authorize_stage_change_request_approver_operations() {
        let requester_roles = vec!["Requester".to_string()];
        let approver_roles = vec!["Approver".to_string()];
        let no_roles = vec![];

        // Test deployment execution - requires Requester or Approver role
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&approver_roles, "DEPLOYED").is_ok()
        );
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&requester_roles, "DEPLOYED").is_ok()
        );
        assert!(RoleAuthorizer::authorize_stage_change_request(&no_roles, "DEPLOYED").is_err());

        // Test deployment rejection - requires Approver role
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&approver_roles, "DEPLOYMENT_REJECTED")
                .is_ok()
        );
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&requester_roles, "DEPLOYMENT_REJECTED")
                .is_err()
        );

        // Test rollback execution - requires Requester or Approver role
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&approver_roles, "ROLLBACKED").is_ok()
        );
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&requester_roles, "ROLLBACKED").is_ok()
        );

        // Test rollback rejection - requires Approver role
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&approver_roles, "ROLLBACK_REJECTED")
                .is_ok()
        );
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&requester_roles, "ROLLBACK_REJECTED")
                .is_err()
        );
    }

    #[test]
    fn test_authorize_stage_change_request_with_multiple_roles() {
        let both_roles = vec!["Requester".to_string(), "Approver".to_string()];

        // User with both roles should be able to perform all operations
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&both_roles, "DEPLOYMENT_REQUESTED")
                .is_ok()
        );
        assert!(RoleAuthorizer::authorize_stage_change_request(&both_roles, "DEPLOYED").is_ok());
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&both_roles, "ROLLBACK_REQUESTED")
                .is_ok()
        );
        assert!(RoleAuthorizer::authorize_stage_change_request(&both_roles, "ROLLBACKED").is_ok());
    }

    #[test]
    fn test_authorize_stage_change_request_unknown_operation() {
        let requester_roles = vec!["Requester".to_string()];

        assert!(
            RoleAuthorizer::authorize_stage_change_request(&requester_roles, "UNKNOWN_OPERATION")
                .is_err()
        );
    }

    #[test]
    fn parse_scoped_resource_ignores_unscoped_paths() {
        assert_eq!(parse_scoped_resource("/health").unwrap(), None);
        assert_eq!(
            parse_scoped_resource("/api/v1/metrics/track").unwrap(),
            None
        );
    }

    #[test]
    fn parse_scoped_resource_rejects_invalid_team_ids() {
        let err = parse_scoped_resource("/api/v1/teams/not-a-uuid")
            .expect_err("invalid team IDs should fail");

        match err {
            Error::InvalidInput(message) => {
                assert!(message.starts_with("invalid team id in request path:"));
            }
            other => panic!("expected invalid input error, got {other:?}"),
        }
    }

    #[test]
    fn parse_scoped_resource_extracts_stage_resource() {
        let stage_id = Uuid::new_v4();
        let parsed = parse_scoped_resource(&format!("/api/v1/stages/{stage_id}"))
            .expect("stage path should parse");

        assert_eq!(
            parsed,
            Some(ScopedResourceRef {
                kind: ScopedResourceKind::Stage,
                id: stage_id,
            })
        );
    }

    #[tokio::test]
    async fn scope_resolver_returns_none_for_paths_without_team_scope() {
        let resolver = request_scope_resolver(test_pool());
        let resolved = resolver
            .resolve_team_id_for_request("/api/v1/metrics/track")
            .await
            .expect("unscoped routes should not error");

        assert_eq!(resolved, None);
    }

    #[tokio::test]
    async fn scope_resolver_resolves_stage_scope() {
        let Some(pool) = test_db_pool_from_env().await else {
            eprintln!("Skipping stage scope resolution test: DATABASE_URL is not set");
            return;
        };

        let stage_and_team = sqlx::query_as::<_, (Uuid, Uuid)>(
            r#"
            SELECT fps.id, f.team_id
            FROM features_pipeline_stages fps
            JOIN features f ON f.id = fps.feature_id
            LIMIT 1
            "#,
        )
        .fetch_optional(&pool)
        .await
        .expect("Failed to load stage/team data for scope resolution test");

        let (stage_id, expected_team_id) = match stage_and_team {
            Some(values) => values,
            None => {
                eprintln!(
                    "Skipping stage scope resolution test: no records in features_pipeline_stages"
                );
                return;
            }
        };

        let resolver = request_scope_resolver(pool);
        let resolved = resolver
            .resolve_team_id_for_request(&format!("/api/v1/stages/{stage_id}"))
            .await
            .expect("Stage scope resolution should succeed");

        assert_eq!(resolved, Some(expected_team_id));
    }

    #[tokio::test]
    async fn scope_resolver_returns_none_for_missing_stage() {
        let Some(pool) = test_db_pool_from_env().await else {
            eprintln!("Skipping missing stage test: DATABASE_URL is not set");
            return;
        };

        let missing_stage_id = loop {
            let candidate = Uuid::new_v4();
            let existing = sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM features_pipeline_stages WHERE id = $1",
            )
            .bind(candidate)
            .fetch_optional(&pool)
            .await
            .expect("Failed to verify missing stage id");

            if existing.is_none() {
                break candidate;
            }
        };

        let resolver = request_scope_resolver(pool);
        let resolved = resolver
            .resolve_team_id_for_request(&format!("/api/v1/stages/{missing_stage_id}"))
            .await
            .expect("Missing stage lookup should not error");

        assert_eq!(resolved, None);
    }
}
