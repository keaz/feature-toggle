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
        Err(e) => Err(Error::DatabaseError(e.into())),
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
        let result = sqlx::query("DELETE FROM roles WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await;

        let res = handle_error(Some(id), result)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(id));
        }

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
        if role_ids.is_empty() {
            return Ok(());
        }

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| Error::DatabaseError(e.into()))?;

        // Remove existing role assignments
        handle_error(
            Some(user_id),
            sqlx::query("DELETE FROM user_roles WHERE user_id = $1")
                .bind(user_id)
                .execute(&mut *tx)
                .await,
        )?;

        // Insert new role assignments
        for role_id in role_ids {
            handle_error(
                Some(user_id),
                sqlx::query(
                    r#"INSERT INTO user_roles (user_id, role_id, assigned_by) 
                       VALUES ($1, $2, $3)"#,
                )
                .bind(user_id)
                .bind(role_id)
                .bind(assigned_by)
                .execute(&mut *tx)
                .await,
            )?;
        }

        tx.commit()
            .await
            .map_err(|e| Error::DatabaseError(e.into()))?;
        Ok(())
    }

    async fn remove_user_role(&self, user_id: Uuid, role_id: Uuid) -> Result<(), Error> {
        handle_error(
            Some(user_id),
            sqlx::query("DELETE FROM user_roles WHERE user_id = $1 AND role_id = $2")
                .bind(user_id)
                .bind(role_id)
                .execute(&self.pool)
                .await,
        )?;

        Ok(())
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
            Err(e) => Err(Error::DatabaseError(e.into())),
        }
    }

    fn clone_box(&self) -> Box<dyn RoleRepository> {
        Box::new(self.clone())
    }
}

impl RoleRepositoryImpl {
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
        if role_ids.is_empty() {
            return Ok(());
        }

        // Remove existing role assignments
        handle_error(
            Some(user_id),
            sqlx::query("DELETE FROM user_roles WHERE user_id = $1")
                .bind(user_id)
                .execute(&mut *conn)
                .await,
        )?;

        // Insert new role assignments
        for role_id in role_ids {
            handle_error(
                Some(user_id),
                sqlx::query(
                    r#"INSERT INTO user_roles (user_id, role_id, assigned_by) 
                       VALUES ($1, $2, $3)"#,
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
        handle_error(
            Some(user_id),
            sqlx::query("DELETE FROM user_roles WHERE user_id = $1 AND role_id = $2")
                .bind(user_id)
                .bind(role_id)
                .execute(&mut *conn)
                .await,
        )?;

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
