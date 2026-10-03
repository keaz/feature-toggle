//! Persistence for Jira integrations (`jira_integrations`) and their status rules
//! (`jira_status_rules`). Each integration owns a shadow user that acts for it.

use std::collections::BTreeMap;

use mockall::automock;
use sqlx::types::Json;
use sqlx::{PgConnection, PgPool, Postgres, QueryBuilder};
use uuid::Uuid;

use crate::Error;
use crate::database::entity::{JiraIntegrationRow, JiraStatusRuleRow};
use crate::database::handle_error;

const INTEGRATION_COLUMNS: &str = "id, team_id, name, jira_base_url, secret_hash, environment_field, \
     environment_aliases, jira_approved_environment_ids, feature_key_field, actor_user_id, enabled, \
     created_at, updated_at";
const RULE_COLUMNS: &str =
    "id, integration_id, jira_status, action, environment_ids, enabled, position";
const NAME_UNIQUE_CONSTRAINT: &str = "jira_integrations_team_name_unique";
const TEAM_FOREIGN_KEY: &str = "jira_integrations_team_id_fkey";
const ROLE_ID_REQUESTER: Uuid = Uuid::from_u128(0x00000000_0000_0000_0000_000000000002);
/// Stored as the shadow user's password hash; it is not a hash, so no password matches.
const SHADOW_USER_NO_LOGIN: &str = "JIRA_INTEGRATION_NO_LOGIN";

#[derive(Debug, Clone)]
pub struct CreateJiraIntegration {
    pub team_id: Uuid,
    pub name: String,
    pub jira_base_url: Option<String>,
    pub secret_hash: String,
    pub environment_field: String,
    pub environment_aliases: BTreeMap<String, Uuid>,
    pub jira_approved_environment_ids: Vec<Uuid>,
    pub feature_key_field: Option<String>,
    pub enabled: bool,
}

/// `None` keeps the stored value; for nullable columns `Some(None)` clears it.
#[derive(Debug, Clone, Default)]
pub struct UpdateJiraIntegration {
    pub name: Option<String>,
    pub jira_base_url: Option<Option<String>>,
    pub environment_field: Option<String>,
    pub environment_aliases: Option<BTreeMap<String, Uuid>>,
    pub jira_approved_environment_ids: Option<Vec<Uuid>>,
    pub feature_key_field: Option<Option<String>>,
    pub enabled: Option<bool>,
}

/// A validated status rule; its `position` is its index in the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewJiraStatusRule {
    pub jira_status: String,
    pub action: String,
    pub environment_ids: Option<Vec<Uuid>>,
    pub enabled: bool,
}

#[automock]
#[async_trait::async_trait]
pub trait JiraIntegrationRepository: Send + Sync {
    async fn get(&self, id: Uuid) -> Result<Option<JiraIntegrationRow>, Error>;
    /// Integrations of the team, by name.
    async fn list_for_team(&self, team_id: Uuid) -> Result<Vec<JiraIntegrationRow>, Error>;
    /// Rules of the integration in `position` order.
    async fn list_rules(&self, integration_id: Uuid) -> Result<Vec<JiraStatusRuleRow>, Error>;
    /// Creates the integration and its shadow user (`Requester` role only) in one
    /// transaction. `Error::RecordAlreadyExists` when the team already has the name,
    /// `Error::NotFound(team_id)` when the team does not exist.
    async fn create(&self, input: CreateJiraIntegration) -> Result<JiraIntegrationRow, Error>;
    /// `None` when the integration does not exist.
    async fn update(
        &self,
        id: Uuid,
        input: UpdateJiraIntegration,
    ) -> Result<Option<JiraIntegrationRow>, Error>;
    /// Deletes the integration (rules cascade) and disables its shadow user, which is
    /// kept for the audit trail. The deleted row, `None` when it does not exist.
    async fn delete(&self, id: Uuid) -> Result<Option<JiraIntegrationRow>, Error>;
    /// Replaces every rule of the integration in one transaction; `position` is the
    /// index in `rules`.
    async fn replace_rules(
        &self,
        integration_id: Uuid,
        rules: Vec<NewJiraStatusRule>,
    ) -> Result<Vec<JiraStatusRuleRow>, Error>;
    /// `None` when the integration does not exist.
    async fn set_secret_hash(
        &self,
        id: Uuid,
        secret_hash: String,
    ) -> Result<Option<JiraIntegrationRow>, Error>;
    /// Ids of every environment of the team (active or not); `None` when the team
    /// does not exist.
    async fn team_environment_ids(&self, team_id: Uuid) -> Result<Option<Vec<Uuid>>, Error>;
    fn clone_box(&self) -> Box<dyn JiraIntegrationRepository>;
}

impl Clone for Box<dyn JiraIntegrationRepository> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

#[async_trait::async_trait]
pub trait JiraIntegrationRepositoryTx: JiraIntegrationRepository {
    async fn get_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
    ) -> Result<Option<JiraIntegrationRow>, Error>;
    async fn create_tx(
        &self,
        conn: &mut PgConnection,
        input: CreateJiraIntegration,
    ) -> Result<JiraIntegrationRow, Error>;
    async fn update_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        input: UpdateJiraIntegration,
    ) -> Result<Option<JiraIntegrationRow>, Error>;
    async fn delete_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
    ) -> Result<Option<JiraIntegrationRow>, Error>;
    async fn replace_rules_tx(
        &self,
        conn: &mut PgConnection,
        integration_id: Uuid,
        rules: Vec<NewJiraStatusRule>,
    ) -> Result<Vec<JiraStatusRuleRow>, Error>;
    async fn set_secret_hash_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        secret_hash: String,
    ) -> Result<Option<JiraIntegrationRow>, Error>;
    async fn team_environment_ids_tx(
        &self,
        conn: &mut PgConnection,
        team_id: Uuid,
    ) -> Result<Option<Vec<Uuid>>, Error>;
}

pub fn jira_integration_repository(pool: PgPool) -> Box<dyn JiraIntegrationRepository> {
    Box::new(JiraIntegrationRepositoryImpl { pool })
}

pub fn jira_integration_repository_tx(pool: PgPool) -> JiraIntegrationRepositoryImpl {
    JiraIntegrationRepositoryImpl { pool }
}

#[derive(Clone)]
pub struct JiraIntegrationRepositoryImpl {
    pool: PgPool,
}

impl JiraIntegrationRepositoryImpl {
    async fn get_conn(
        conn: &mut PgConnection,
        id: Uuid,
    ) -> Result<Option<JiraIntegrationRow>, Error> {
        let result = sqlx::query_as::<_, JiraIntegrationRow>(&format!(
            "SELECT {INTEGRATION_COLUMNS} FROM jira_integrations WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&mut *conn)
        .await;
        handle_error(None, result)
    }

    async fn list_rules_conn(
        conn: &mut PgConnection,
        integration_id: Uuid,
    ) -> Result<Vec<JiraStatusRuleRow>, Error> {
        let result = sqlx::query_as::<_, JiraStatusRuleRow>(&format!(
            "SELECT {RULE_COLUMNS} FROM jira_status_rules \
             WHERE integration_id = $1 ORDER BY position, id"
        ))
        .bind(integration_id)
        .fetch_all(&mut *conn)
        .await;
        handle_error(None, result)
    }

    async fn create_conn(
        conn: &mut PgConnection,
        input: CreateJiraIntegration,
    ) -> Result<JiraIntegrationRow, Error> {
        let integration_id = Uuid::new_v4();
        let actor_user_id = Uuid::new_v4();

        // The shadow user acts for the integration: no password login, not an
        // admin, `Requester` only, so it is never an eligible approver.
        let result = sqlx::query(
            "INSERT INTO users (id, username, password_hash, first_name, last_name, email, \
             is_admin, enabled, is_temporary_password, auth_source) \
             VALUES ($1, $2, $3, $4, 'Jira', $5, FALSE, TRUE, FALSE, 'system')",
        )
        .bind(actor_user_id)
        .bind(format!("jira-integration-{integration_id}"))
        .bind(SHADOW_USER_NO_LOGIN)
        .bind(&input.name)
        .bind(format!(
            "jira-integration-{}@automation.local",
            integration_id.simple()
        ))
        .execute(&mut *conn)
        .await;
        handle_error(None, result)?;

        let result = sqlx::query(
            "INSERT INTO user_roles (user_id, role_id, assigned_by) VALUES ($1, $2, NULL)",
        )
        .bind(actor_user_id)
        .bind(ROLE_ID_REQUESTER)
        .execute(&mut *conn)
        .await;
        handle_error(None, result)?;

        let result = sqlx::query_as::<_, JiraIntegrationRow>(&format!(
            "INSERT INTO jira_integrations (id, team_id, name, jira_base_url, secret_hash, \
             environment_field, environment_aliases, jira_approved_environment_ids, \
             feature_key_field, actor_user_id, enabled) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) \
             RETURNING {INTEGRATION_COLUMNS}"
        ))
        .bind(integration_id)
        .bind(input.team_id)
        .bind(&input.name)
        .bind(&input.jira_base_url)
        .bind(&input.secret_hash)
        .bind(&input.environment_field)
        .bind(Json(&input.environment_aliases))
        .bind(&input.jira_approved_environment_ids)
        .bind(&input.feature_key_field)
        .bind(actor_user_id)
        .bind(input.enabled)
        .fetch_one(&mut *conn)
        .await;
        if let Err(sqlx::Error::Database(db_err)) = &result {
            match db_err.constraint() {
                Some(NAME_UNIQUE_CONSTRAINT) => {
                    return Err(Error::RecordAlreadyExists(format!(
                        "Jira integration '{}'",
                        input.name
                    )));
                }
                Some(TEAM_FOREIGN_KEY) => return Err(Error::NotFound(input.team_id)),
                _ => {}
            }
        }
        handle_error(None, result)
    }

    async fn update_conn(
        conn: &mut PgConnection,
        id: Uuid,
        input: UpdateJiraIntegration,
    ) -> Result<Option<JiraIntegrationRow>, Error> {
        let name = input.name.clone();
        let mut query =
            QueryBuilder::<Postgres>::new("UPDATE jira_integrations SET updated_at = now()");
        if let Some(name) = input.name {
            query.push(", name = ").push_bind(name);
        }
        if let Some(jira_base_url) = input.jira_base_url {
            query.push(", jira_base_url = ").push_bind(jira_base_url);
        }
        if let Some(environment_field) = input.environment_field {
            query
                .push(", environment_field = ")
                .push_bind(environment_field);
        }
        if let Some(aliases) = input.environment_aliases {
            query
                .push(", environment_aliases = ")
                .push_bind(Json(aliases));
        }
        if let Some(ids) = input.jira_approved_environment_ids {
            query
                .push(", jira_approved_environment_ids = ")
                .push_bind(ids);
        }
        if let Some(feature_key_field) = input.feature_key_field {
            query
                .push(", feature_key_field = ")
                .push_bind(feature_key_field);
        }
        if let Some(enabled) = input.enabled {
            query.push(", enabled = ").push_bind(enabled);
        }
        query.push(" WHERE id = ").push_bind(id);
        query.push(format!(" RETURNING {INTEGRATION_COLUMNS}"));

        let result = query
            .build_query_as::<JiraIntegrationRow>()
            .fetch_optional(&mut *conn)
            .await;
        if let Err(sqlx::Error::Database(db_err)) = &result
            && db_err.constraint() == Some(NAME_UNIQUE_CONSTRAINT)
        {
            return Err(Error::RecordAlreadyExists(format!(
                "Jira integration '{}'",
                name.unwrap_or_default()
            )));
        }
        handle_error(None, result)
    }

    async fn delete_conn(
        conn: &mut PgConnection,
        id: Uuid,
    ) -> Result<Option<JiraIntegrationRow>, Error> {
        let result = sqlx::query_as::<_, JiraIntegrationRow>(&format!(
            "DELETE FROM jira_integrations WHERE id = $1 RETURNING {INTEGRATION_COLUMNS}"
        ))
        .bind(id)
        .fetch_optional(&mut *conn)
        .await;
        let Some(deleted) = handle_error(None, result)? else {
            return Ok(None);
        };
        let result =
            sqlx::query("UPDATE users SET enabled = FALSE, updated_at = now() WHERE id = $1")
                .bind(deleted.actor_user_id)
                .execute(&mut *conn)
                .await;
        handle_error(None, result)?;
        Ok(Some(deleted))
    }

    async fn replace_rules_conn(
        conn: &mut PgConnection,
        integration_id: Uuid,
        rules: Vec<NewJiraStatusRule>,
    ) -> Result<Vec<JiraStatusRuleRow>, Error> {
        let result = sqlx::query("DELETE FROM jira_status_rules WHERE integration_id = $1")
            .bind(integration_id)
            .execute(&mut *conn)
            .await;
        handle_error(None, result)?;
        for (position, rule) in rules.into_iter().enumerate() {
            let result = sqlx::query(
                "INSERT INTO jira_status_rules \
                 (id, integration_id, jira_status, action, environment_ids, enabled, position) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
            )
            .bind(Uuid::new_v4())
            .bind(integration_id)
            .bind(rule.jira_status)
            .bind(rule.action)
            .bind(rule.environment_ids)
            .bind(rule.enabled)
            .bind(i32::try_from(position).unwrap_or(i32::MAX))
            .execute(&mut *conn)
            .await;
            handle_error(None, result)?;
        }
        Self::list_rules_conn(conn, integration_id).await
    }

    async fn set_secret_hash_conn(
        conn: &mut PgConnection,
        id: Uuid,
        secret_hash: String,
    ) -> Result<Option<JiraIntegrationRow>, Error> {
        let result = sqlx::query_as::<_, JiraIntegrationRow>(&format!(
            "UPDATE jira_integrations SET secret_hash = $2, updated_at = now() \
             WHERE id = $1 RETURNING {INTEGRATION_COLUMNS}"
        ))
        .bind(id)
        .bind(secret_hash)
        .fetch_optional(&mut *conn)
        .await;
        handle_error(None, result)
    }

    async fn team_environment_ids_conn(
        conn: &mut PgConnection,
        team_id: Uuid,
    ) -> Result<Option<Vec<Uuid>>, Error> {
        let result = sqlx::query_scalar::<_, Option<Uuid>>(
            "SELECT e.id FROM teams t LEFT JOIN environments e ON e.team_id = t.id WHERE t.id = $1",
        )
        .bind(team_id)
        .fetch_all(&mut *conn)
        .await;
        let rows = handle_error(None, result)?;
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(rows.into_iter().flatten().collect()))
    }
}

#[async_trait::async_trait]
impl JiraIntegrationRepository for JiraIntegrationRepositoryImpl {
    async fn get(&self, id: Uuid) -> Result<Option<JiraIntegrationRow>, Error> {
        let mut conn = self.pool.acquire().await.map_err(Error::DatabaseError)?;
        Self::get_conn(&mut conn, id).await
    }

    async fn list_for_team(&self, team_id: Uuid) -> Result<Vec<JiraIntegrationRow>, Error> {
        let result = sqlx::query_as::<_, JiraIntegrationRow>(&format!(
            "SELECT {INTEGRATION_COLUMNS} FROM jira_integrations WHERE team_id = $1 ORDER BY name, id"
        ))
        .bind(team_id)
        .fetch_all(&self.pool)
        .await;
        handle_error(None, result)
    }

    async fn list_rules(&self, integration_id: Uuid) -> Result<Vec<JiraStatusRuleRow>, Error> {
        let mut conn = self.pool.acquire().await.map_err(Error::DatabaseError)?;
        Self::list_rules_conn(&mut conn, integration_id).await
    }

    async fn create(&self, input: CreateJiraIntegration) -> Result<JiraIntegrationRow, Error> {
        let mut tx = self.pool.begin().await.map_err(Error::DatabaseError)?;
        let created = Self::create_conn(&mut tx, input).await?;
        tx.commit().await.map_err(Error::DatabaseError)?;
        Ok(created)
    }

    async fn update(
        &self,
        id: Uuid,
        input: UpdateJiraIntegration,
    ) -> Result<Option<JiraIntegrationRow>, Error> {
        let mut conn = self.pool.acquire().await.map_err(Error::DatabaseError)?;
        Self::update_conn(&mut conn, id, input).await
    }

    async fn delete(&self, id: Uuid) -> Result<Option<JiraIntegrationRow>, Error> {
        let mut tx = self.pool.begin().await.map_err(Error::DatabaseError)?;
        let deleted = Self::delete_conn(&mut tx, id).await?;
        tx.commit().await.map_err(Error::DatabaseError)?;
        Ok(deleted)
    }

    async fn replace_rules(
        &self,
        integration_id: Uuid,
        rules: Vec<NewJiraStatusRule>,
    ) -> Result<Vec<JiraStatusRuleRow>, Error> {
        let mut tx = self.pool.begin().await.map_err(Error::DatabaseError)?;
        let replaced = Self::replace_rules_conn(&mut tx, integration_id, rules).await?;
        tx.commit().await.map_err(Error::DatabaseError)?;
        Ok(replaced)
    }

    async fn set_secret_hash(
        &self,
        id: Uuid,
        secret_hash: String,
    ) -> Result<Option<JiraIntegrationRow>, Error> {
        let mut conn = self.pool.acquire().await.map_err(Error::DatabaseError)?;
        Self::set_secret_hash_conn(&mut conn, id, secret_hash).await
    }

    async fn team_environment_ids(&self, team_id: Uuid) -> Result<Option<Vec<Uuid>>, Error> {
        let mut conn = self.pool.acquire().await.map_err(Error::DatabaseError)?;
        Self::team_environment_ids_conn(&mut conn, team_id).await
    }

    fn clone_box(&self) -> Box<dyn JiraIntegrationRepository> {
        Box::new(self.clone())
    }
}

#[async_trait::async_trait]
impl JiraIntegrationRepositoryTx for JiraIntegrationRepositoryImpl {
    async fn get_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
    ) -> Result<Option<JiraIntegrationRow>, Error> {
        Self::get_conn(conn, id).await
    }

    async fn create_tx(
        &self,
        conn: &mut PgConnection,
        input: CreateJiraIntegration,
    ) -> Result<JiraIntegrationRow, Error> {
        Self::create_conn(conn, input).await
    }

    async fn update_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        input: UpdateJiraIntegration,
    ) -> Result<Option<JiraIntegrationRow>, Error> {
        Self::update_conn(conn, id, input).await
    }

    async fn delete_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
    ) -> Result<Option<JiraIntegrationRow>, Error> {
        Self::delete_conn(conn, id).await
    }

    async fn replace_rules_tx(
        &self,
        conn: &mut PgConnection,
        integration_id: Uuid,
        rules: Vec<NewJiraStatusRule>,
    ) -> Result<Vec<JiraStatusRuleRow>, Error> {
        Self::replace_rules_conn(conn, integration_id, rules).await
    }

    async fn set_secret_hash_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        secret_hash: String,
    ) -> Result<Option<JiraIntegrationRow>, Error> {
        Self::set_secret_hash_conn(conn, id, secret_hash).await
    }

    async fn team_environment_ids_tx(
        &self,
        conn: &mut PgConnection,
        team_id: Uuid,
    ) -> Result<Option<Vec<Uuid>>, Error> {
        Self::team_environment_ids_conn(conn, team_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::approval::approver_qualifies_sql;
    use sqlx::postgres::PgPoolOptions;

    async fn test_pool() -> PgPool {
        let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL not set");
        PgPoolOptions::new()
            .max_connections(2)
            .connect(&db_url)
            .await
            .expect("Failed to connect to database")
    }

    async fn qualifies_as_approver(pool: &PgPool, team_id: Uuid, user_id: Uuid) -> bool {
        // Every role may approve and no user list applies: the widest policy.
        let sql = format!(
            "SELECT EXISTS (SELECT 1 FROM users u WHERE u.id = $2 AND {})",
            approver_qualifies_sql(
                "$1",
                "ARRAY(SELECT id FROM roles)",
                "ARRAY[]::uuid[]",
                "TRUE",
                "NULL::uuid",
            )
        );
        sqlx::query_scalar(&sql)
            .bind(team_id)
            .bind(user_id)
            .fetch_one(pool)
            .await
            .expect("approver query")
    }

    #[tokio::test]
    async fn shadow_user_is_never_an_eligible_approver() {
        let pool = test_pool().await;
        let team_id = Uuid::new_v4();
        sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, 'jira')")
            .bind(team_id)
            .bind(format!("jira-approver-{team_id}"))
            .execute(&pool)
            .await
            .expect("insert team");

        // Control: a human team member with the Approver role qualifies.
        let human = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users (id, username, password_hash, first_name, last_name, email) \
             VALUES ($1, $2, 'x', 'Human', 'Approver', $3)",
        )
        .bind(human)
        .bind(format!("jira_approver_{human}"))
        .bind(format!("jira_approver_{human}@example.com"))
        .execute(&pool)
        .await
        .expect("insert human");
        sqlx::query(
            "INSERT INTO user_roles (user_id, role_id) \
             SELECT $1, id FROM roles WHERE name = 'Approver'",
        )
        .bind(human)
        .execute(&pool)
        .await
        .expect("approver role");

        let integration = jira_integration_repository(pool.clone())
            .create(CreateJiraIntegration {
                team_id,
                name: "Jira".to_string(),
                jira_base_url: None,
                secret_hash: "hash".to_string(),
                environment_field: "labels".to_string(),
                environment_aliases: BTreeMap::new(),
                jira_approved_environment_ids: Vec::new(),
                feature_key_field: None,
                enabled: true,
            })
            .await
            .expect("create integration");
        let shadow = integration.actor_user_id;

        // Even as a team member, the shadow user has no Approver role.
        for user_id in [human, shadow] {
            sqlx::query("INSERT INTO user_teams (user_id, team_id) VALUES ($1, $2)")
                .bind(user_id)
                .bind(team_id)
                .execute(&pool)
                .await
                .expect("team membership");
        }
        assert!(qualifies_as_approver(&pool, team_id, human).await);
        assert!(!qualifies_as_approver(&pool, team_id, shadow).await);

        sqlx::query("DELETE FROM teams WHERE id = $1")
            .bind(team_id)
            .execute(&pool)
            .await
            .expect("delete team");
        sqlx::query("DELETE FROM users WHERE id = ANY($1)")
            .bind(vec![human, shadow])
            .execute(&pool)
            .await
            .expect("delete users");
    }
}
