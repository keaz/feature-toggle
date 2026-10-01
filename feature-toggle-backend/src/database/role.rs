use crate::Error;
use crate::database::entity::Role;
use mockall::automock;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

#[derive(Clone)]
pub struct RoleRepositoryImpl {
    pool: PgPool,
}

pub fn role_repository(pool: PgPool) -> Box<dyn RoleRepository> {
    Box::new(RoleRepositoryImpl { pool })
}

#[derive(Clone, Debug)]
pub struct AssignRoleInput {
    pub user_id: Uuid,
    pub role_id: Uuid,
    pub assigned_by: Option<Uuid>,
}

#[automock]
#[async_trait::async_trait]
pub trait RoleRepository: Send + Sync {
    async fn get_all_roles(&self) -> Result<Vec<Role>, Error>;
    async fn get_role_by_id(&self, id: Uuid) -> Result<Role, Error>;
    async fn get_role_by_name(&self, name: &str) -> Result<Role, Error>;
    async fn create_role(&self, name: &str, description: &str) -> Result<Role, Error>;
    async fn delete_role(&self, id: Uuid) -> Result<(), Error>;
    async fn get_user_roles(&self, user_id: Uuid) -> Result<Vec<Role>, Error>;
    async fn assign_user_roles(
        &self,
        user_id: Uuid,
        role_ids: Vec<Uuid>,
        assigned_by: Option<Uuid>,
    ) -> Result<(), Error>;
    async fn remove_user_role(&self, user_id: Uuid, role_id: Uuid) -> Result<(), Error>;
    async fn user_has_role(&self, user_id: Uuid, role_name: &str) -> Result<bool, Error>;
    /// Ids of the roles the user holds through SSO group sync (`source = 'sso'`).
    async fn list_sso_role_ids(&self, user_id: Uuid) -> Result<Vec<Uuid>, Error>;
    /// Batch variant of `list_sso_role_ids`: `(user_id, role_id)` pairs for list endpoints.
    async fn list_sso_role_ids_for_users(
        &self,
        user_ids: Vec<Uuid>,
    ) -> Result<Vec<(Uuid, Uuid)>, Error>;
    fn clone_box(&self) -> Box<dyn RoleRepository>;
}

impl Clone for Box<dyn RoleRepository> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

/// Extension trait for transaction-aware repository operations.
/// These methods accept a mutable connection reference for use within transactions.
#[async_trait::async_trait]
pub trait RoleRepositoryTx: RoleRepository {
    async fn create_role_tx(
        &self,
        conn: &mut PgConnection,
        name: &str,
        description: &str,
    ) -> Result<Role, Error>;
    async fn delete_role_tx(&self, conn: &mut PgConnection, id: Uuid) -> Result<(), Error>;
    async fn assign_user_roles_tx(
        &self,
        conn: &mut PgConnection,
        user_id: Uuid,
        role_ids: Vec<Uuid>,
        assigned_by: Option<Uuid>,
    ) -> Result<(), Error>;
    async fn get_user_roles_tx(
        &self,
        conn: &mut PgConnection,
        user_id: Uuid,
    ) -> Result<Vec<Role>, Error>;
    async fn remove_user_role_tx(
        &self,
        conn: &mut PgConnection,
        user_id: Uuid,
        role_id: Uuid,
    ) -> Result<(), Error>;
    async fn list_sso_role_ids_tx(
        &self,
        conn: &mut PgConnection,
        user_id: Uuid,
    ) -> Result<Vec<Uuid>, Error>;
    /// Adds roles with `source = 'sso'`. A role the user already holds, manually or
    /// through SSO, is left as is (manual wins). Returns the ids actually inserted.
    async fn add_sso_user_roles_tx(
        &self,
        conn: &mut PgConnection,
        user_id: Uuid,
        role_ids: Vec<Uuid>,
    ) -> Result<Vec<Uuid>, Error>;
    /// Removes only `source = 'sso'` rows; manual assignments are never touched.
    async fn remove_sso_user_roles_tx(
        &self,
        conn: &mut PgConnection,
        user_id: Uuid,
        role_ids: Vec<Uuid>,
    ) -> Result<(), Error>;
    /// The subset of `role_ids` that still exist.
    async fn existing_role_ids_tx(
        &self,
        conn: &mut PgConnection,
        role_ids: Vec<Uuid>,
    ) -> Result<Vec<Uuid>, Error>;
}

/// Returns a repository that also implements RoleRepositoryTx for transaction support.
pub fn role_repository_tx(pool: PgPool) -> RoleRepositoryImpl {
    RoleRepositoryImpl { pool }
}

fn handle_error<T>(id: Option<Uuid>, result: Result<T, sqlx::Error>) -> Result<T, Error> {
    match result {
        Ok(val) => Ok(val),
        Err(sqlx::Error::RowNotFound) => {
            if let Some(id) = id {
                Err(Error::NotFound(id))
            } else {
                Err(Error::InvalidInput("Role not found".to_string()))
            }
        }
        Err(sqlx::Error::Database(db_err)) => {
            if let Some(code) = db_err.code()
                && code == "23505"
            {
                let field = match db_err.constraint() {
                    Some(c) if c.contains("roles_name_key") => "role name",
                    Some(c) if c.contains("user_roles_user_id_role_id_key") => "user role",
                    _ => "record",
                };
                return Err(Error::RecordAlreadyExists(field.to_string()));
            }
            Err(Error::DatabaseError(sqlx::Error::Database(db_err)))
        }
        Err(e) => Err(Error::DatabaseError(e)),
    }
}

#[async_trait::async_trait]
impl RoleRepository for RoleRepositoryImpl {
    async fn get_all_roles(&self) -> Result<Vec<Role>, Error> {
        let result = sqlx::query_as!(
            Role,
            "SELECT id, name, description, created_at, updated_at FROM roles ORDER BY name"
        )
        .fetch_all(&self.pool)
        .await;

        handle_error(None, result)
    }

    async fn get_role_by_id(&self, id: Uuid) -> Result<Role, Error> {
        let result = sqlx::query_as!(
            Role,
            "SELECT id, name, description, created_at, updated_at FROM roles WHERE id = $1",
            id
        )
        .fetch_one(&self.pool)
        .await;

        handle_error(Some(id), result)
    }

    async fn get_role_by_name(&self, name: &str) -> Result<Role, Error> {
        let result = sqlx::query_as!(
            Role,
            "SELECT id, name, description, created_at, updated_at FROM roles WHERE name = $1",
            name
        )
        .fetch_one(&self.pool)
        .await;

        handle_error(None, result)
    }

    async fn create_role(&self, name: &str, description: &str) -> Result<Role, Error> {
        let result = sqlx::query_as!(
            Role,
            r#"INSERT INTO roles (name, description) 
               VALUES ($1, $2) 
               RETURNING id, name, description, created_at, updated_at"#,
            name,
            description
        )
        .fetch_one(&self.pool)
        .await;

        handle_error(None, result)
    }

    async fn delete_role(&self, id: Uuid) -> Result<(), Error> {
        // One transaction, so the role and its SSO group mappings go together.
        let mut tx = self.pool.begin().await.map_err(Error::DatabaseError)?;
        Self::delete_role_internal(&mut tx, id).await?;
        tx.commit().await.map_err(Error::DatabaseError)?;
        Ok(())
    }

    async fn get_user_roles(&self, user_id: Uuid) -> Result<Vec<Role>, Error> {
        let result = sqlx::query_as!(
            Role,
            r#"SELECT r.id, r.name, r.description, r.created_at, r.updated_at 
               FROM roles r 
               JOIN user_roles ur ON r.id = ur.role_id 
               WHERE ur.user_id = $1 
               ORDER BY r.name"#,
            user_id
        )
        .fetch_all(&self.pool)
        .await;

        handle_error(Some(user_id), result)
    }

    async fn assign_user_roles(
        &self,
        user_id: Uuid,
        role_ids: Vec<Uuid>,
        assigned_by: Option<Uuid>,
    ) -> Result<(), Error> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| Error::DatabaseError(e))?;

        // Remove existing manual role assignments; SSO-sourced rows are left alone
        handle_error(
            Some(user_id),
            sqlx::query("DELETE FROM user_roles WHERE user_id = $1 AND source = 'manual'")
                .bind(user_id)
                .execute(&mut *tx)
                .await,
        )?;

        // Insert new role assignments
        for role_id in role_ids {
            handle_error(
                Some(user_id),
                sqlx::query(
                    r#"INSERT INTO user_roles (user_id, role_id, assigned_by, source)
                       VALUES ($1, $2, $3, 'manual')
                       ON CONFLICT (user_id, role_id)
                       DO UPDATE SET source = 'manual', assigned_by = EXCLUDED.assigned_by"#,
                )
                .bind(user_id)
                .bind(role_id)
                .bind(assigned_by)
                .execute(&mut *tx)
                .await,
            )?;
        }

        tx.commit().await.map_err(|e| Error::DatabaseError(e))?;
        Ok(())
    }

    async fn remove_user_role(&self, user_id: Uuid, role_id: Uuid) -> Result<(), Error> {
        let mut conn = self.pool.acquire().await.map_err(Error::DatabaseError)?;
        Self::remove_user_role_internal(&mut conn, user_id, role_id).await
    }

    async fn user_has_role(&self, user_id: Uuid, role_name: &str) -> Result<bool, Error> {
        let result = sqlx::query!(
            r#"SELECT COUNT(*) as count 
               FROM user_roles ur 
               JOIN roles r ON ur.role_id = r.id 
               WHERE ur.user_id = $1 AND r.name = $2"#,
            user_id,
            role_name
        )
        .fetch_one(&self.pool)
        .await;

        match result {
            Ok(row) => Ok(row.count.unwrap_or(0) > 0),
            Err(e) => Err(Error::DatabaseError(e)),
        }
    }

    async fn list_sso_role_ids(&self, user_id: Uuid) -> Result<Vec<Uuid>, Error> {
        let mut conn = self.pool.acquire().await.map_err(Error::DatabaseError)?;
        Self::list_sso_role_ids_internal(&mut conn, user_id).await
    }

    async fn list_sso_role_ids_for_users(
        &self,
        user_ids: Vec<Uuid>,
    ) -> Result<Vec<(Uuid, Uuid)>, Error> {
        let rows = sqlx::query!(
            "SELECT user_id, role_id FROM user_roles \
             WHERE source = 'sso' AND user_id = ANY($1) ORDER BY user_id, role_id",
            &user_ids
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::DatabaseError)?;
        Ok(rows.into_iter().map(|r| (r.user_id, r.role_id)).collect())
    }

    fn clone_box(&self) -> Box<dyn RoleRepository> {
        Box::new(self.clone())
    }
}

impl RoleRepositoryImpl {
    async fn list_sso_role_ids_internal(
        conn: &mut PgConnection,
        user_id: Uuid,
    ) -> Result<Vec<Uuid>, Error> {
        let result = sqlx::query_scalar!(
            "SELECT role_id FROM user_roles WHERE user_id = $1 AND source = 'sso' ORDER BY role_id",
            user_id
        )
        .fetch_all(&mut *conn)
        .await;

        handle_error(Some(user_id), result)
    }

    async fn add_sso_user_roles_internal(
        conn: &mut PgConnection,
        user_id: Uuid,
        role_ids: Vec<Uuid>,
    ) -> Result<Vec<Uuid>, Error> {
        let mut inserted = Vec::new();
        for role_id in role_ids {
            let row = handle_error(
                Some(user_id),
                sqlx::query_scalar!(
                    r#"INSERT INTO user_roles (user_id, role_id, assigned_by, source)
                       VALUES ($1, $2, NULL, 'sso')
                       ON CONFLICT (user_id, role_id) DO NOTHING
                       RETURNING role_id"#,
                    user_id,
                    role_id
                )
                .fetch_optional(&mut *conn)
                .await,
            )?;
            inserted.extend(row);
        }
        Ok(inserted)
    }

    async fn remove_sso_user_roles_internal(
        conn: &mut PgConnection,
        user_id: Uuid,
        role_ids: Vec<Uuid>,
    ) -> Result<(), Error> {
        handle_error(
            Some(user_id),
            sqlx::query!(
                "DELETE FROM user_roles WHERE user_id = $1 AND source = 'sso' AND role_id = ANY($2)",
                user_id,
                &role_ids
            )
            .execute(&mut *conn)
            .await,
        )?;
        Ok(())
    }

    async fn create_role_internal(
        conn: &mut PgConnection,
        name: &str,
        description: &str,
    ) -> Result<Role, Error> {
        let result = sqlx::query_as!(
            Role,
            r#"INSERT INTO roles (name, description) 
               VALUES ($1, $2) 
               RETURNING id, name, description, created_at, updated_at"#,
            name,
            description
        )
        .fetch_one(&mut *conn)
        .await;

        handle_error(None, result)
    }

    async fn delete_role_internal(conn: &mut PgConnection, id: Uuid) -> Result<(), Error> {
        // sso_group_mappings.target_id is polymorphic (no FK): remove the role's
        // mappings explicitly so none is left pointing at a deleted role.
        sqlx::query!(
            "DELETE FROM sso_group_mappings WHERE target_type = 'role' AND target_id = $1",
            id
        )
        .execute(&mut *conn)
        .await
        .map_err(Error::DatabaseError)?;
        let result = sqlx::query("DELETE FROM roles WHERE id = $1")
            .bind(id)
            .execute(&mut *conn)
            .await;

        let res = handle_error(Some(id), result)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(id));
        }

        Ok(())
    }

    async fn assign_user_roles_internal(
        conn: &mut PgConnection,
        user_id: Uuid,
        role_ids: Vec<Uuid>,
        assigned_by: Option<Uuid>,
    ) -> Result<(), Error> {
        // Remove existing manual role assignments; SSO-sourced rows are left alone.
        // Always runs, so an empty list removes the last manual role.
        handle_error(
            Some(user_id),
            sqlx::query("DELETE FROM user_roles WHERE user_id = $1 AND source = 'manual'")
                .bind(user_id)
                .execute(&mut *conn)
                .await,
        )?;

        // Insert new role assignments
        for role_id in role_ids {
            handle_error(
                Some(user_id),
                sqlx::query(
                    r#"INSERT INTO user_roles (user_id, role_id, assigned_by, source)
                       VALUES ($1, $2, $3, 'manual')
                       ON CONFLICT (user_id, role_id)
                       DO UPDATE SET source = 'manual', assigned_by = EXCLUDED.assigned_by"#,
                )
                .bind(user_id)
                .bind(role_id)
                .bind(assigned_by)
                .execute(&mut *conn)
                .await,
            )?;
        }

        Ok(())
    }

    async fn get_user_roles_internal(
        conn: &mut PgConnection,
        user_id: Uuid,
    ) -> Result<Vec<Role>, Error> {
        let result = sqlx::query_as!(
            Role,
            r#"SELECT r.id, r.name, r.description, r.created_at, r.updated_at 
               FROM roles r 
               JOIN user_roles ur ON r.id = ur.role_id 
               WHERE ur.user_id = $1 
               ORDER BY r.name"#,
            user_id
        )
        .fetch_all(&mut *conn)
        .await;

        handle_error(Some(user_id), result)
    }

    async fn remove_user_role_internal(
        conn: &mut PgConnection,
        user_id: Uuid,
        role_id: Uuid,
    ) -> Result<(), Error> {
        let result = handle_error(
            Some(user_id),
            sqlx::query(
                "DELETE FROM user_roles WHERE user_id = $1 AND role_id = $2 AND source = 'manual'",
            )
            .bind(user_id)
            .bind(role_id)
            .execute(&mut *conn)
            .await,
        )?;

        if result.rows_affected() == 0 {
            // An SSO-sourced assignment is owned by group sync; refuse instead of
            // reporting a silent no-op.
            let sso_held = handle_error(
                Some(user_id),
                sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS(SELECT 1 FROM user_roles \
                     WHERE user_id = $1 AND role_id = $2 AND source = 'sso')",
                )
                .bind(user_id)
                .bind(role_id)
                .fetch_one(&mut *conn)
                .await,
            )?;
            if sso_held {
                return Err(Error::SsoManaged);
            }
        }

        Ok(())
    }
}

#[async_trait::async_trait]
impl RoleRepositoryTx for RoleRepositoryImpl {
    async fn create_role_tx(
        &self,
        conn: &mut PgConnection,
        name: &str,
        description: &str,
    ) -> Result<Role, Error> {
        Self::create_role_internal(conn, name, description).await
    }

    async fn delete_role_tx(&self, conn: &mut PgConnection, id: Uuid) -> Result<(), Error> {
        Self::delete_role_internal(conn, id).await
    }

    async fn assign_user_roles_tx(
        &self,
        conn: &mut PgConnection,
        user_id: Uuid,
        role_ids: Vec<Uuid>,
        assigned_by: Option<Uuid>,
    ) -> Result<(), Error> {
        Self::assign_user_roles_internal(conn, user_id, role_ids, assigned_by).await
    }

    async fn get_user_roles_tx(
        &self,
        conn: &mut PgConnection,
        user_id: Uuid,
    ) -> Result<Vec<Role>, Error> {
        Self::get_user_roles_internal(conn, user_id).await
    }

    async fn remove_user_role_tx(
        &self,
        conn: &mut PgConnection,
        user_id: Uuid,
        role_id: Uuid,
    ) -> Result<(), Error> {
        Self::remove_user_role_internal(conn, user_id, role_id).await
    }

    async fn list_sso_role_ids_tx(
        &self,
        conn: &mut PgConnection,
        user_id: Uuid,
    ) -> Result<Vec<Uuid>, Error> {
        Self::list_sso_role_ids_internal(conn, user_id).await
    }

    async fn existing_role_ids_tx(
        &self,
        conn: &mut PgConnection,
        role_ids: Vec<Uuid>,
    ) -> Result<Vec<Uuid>, Error> {
        handle_error(
            None,
            sqlx::query_scalar!("SELECT id FROM roles WHERE id = ANY($1)", &role_ids)
                .fetch_all(&mut *conn)
                .await,
        )
    }

    async fn add_sso_user_roles_tx(
        &self,
        conn: &mut PgConnection,
        user_id: Uuid,
        role_ids: Vec<Uuid>,
    ) -> Result<Vec<Uuid>, Error> {
        Self::add_sso_user_roles_internal(conn, user_id, role_ids).await
    }

    async fn remove_sso_user_roles_tx(
        &self,
        conn: &mut PgConnection,
        user_id: Uuid,
        role_ids: Vec<Uuid>,
    ) -> Result<(), Error> {
        Self::remove_sso_user_roles_internal(conn, user_id, role_ids).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::entity::Role;
    use mockall::predicate::*;

    #[tokio::test]
    async fn test_get_all_roles() {
        let mut mock = MockRoleRepository::new();
        let expected_roles = vec![
            Role {
                id: Uuid::new_v4(),
                name: "Approver".to_string(),
                description: "Can approve deployment requests".to_string(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            },
            Role {
                id: Uuid::new_v4(),
                name: "Requester".to_string(),
                description: "Can request deployments".to_string(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            },
        ];

        mock.expect_get_all_roles()
            .times(1)
            .return_once(move || Ok(expected_roles.clone()));

        let roles = mock.get_all_roles().await.unwrap();
        assert_eq!(roles.len(), 2);
        assert_eq!(roles[0].name, "Approver");
        assert_eq!(roles[1].name, "Requester");
    }

    #[tokio::test]
    async fn test_assign_user_roles() {
        let mut mock = MockRoleRepository::new();
        let user_id = Uuid::new_v4();
        let role_ids = vec![Uuid::new_v4(), Uuid::new_v4()];
        let assigned_by = Some(Uuid::new_v4());

        mock.expect_assign_user_roles()
            .with(eq(user_id), eq(role_ids.clone()), eq(assigned_by))
            .times(1)
            .return_once(|_, _, _| Ok(()));

        let result = mock.assign_user_roles(user_id, role_ids, assigned_by).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_user_has_role() {
        let mut mock = MockRoleRepository::new();
        let user_id = Uuid::new_v4();
        let role_name = "Approver";

        mock.expect_user_has_role()
            .with(eq(user_id), eq(role_name))
            .times(1)
            .return_once(|_, _| Ok(true));

        let result = mock.user_has_role(user_id, role_name).await.unwrap();
        assert!(result);
    }

    #[tokio::test]
    async fn test_create_role() {
        let mut mock = MockRoleRepository::new();
        let expected_role = Role {
            id: Uuid::new_v4(),
            name: "Custom".to_string(),
            description: "Custom role".to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        mock.expect_create_role()
            .with(eq("Custom"), eq("Custom role"))
            .times(1)
            .return_once(move |_, _| Ok(expected_role.clone()));

        let created = mock.create_role("Custom", "Custom role").await.unwrap();
        assert_eq!(created.name, "Custom");
        assert_eq!(created.description, "Custom role");
    }

    #[tokio::test]
    async fn test_delete_role() {
        let mut mock = MockRoleRepository::new();
        let role_id = Uuid::new_v4();

        mock.expect_delete_role()
            .with(eq(role_id))
            .times(1)
            .return_once(|_| Ok(()));

        let result = mock.delete_role(role_id).await;
        assert!(result.is_ok());
    }
}
