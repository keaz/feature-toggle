use crate::database::Error;
use crate::database::entity::Team;
use crate::database::handle_error;
use chrono::{DateTime, Utc};
use mockall::automock;
use sqlx::{PgConnection, PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct User {
    pub id: Uuid,
    pub username: String,
    /// `None` for SSO-only accounts, which cannot log in with a password.
    pub password_hash: Option<String>,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub mobile_number: Option<String>,
    pub is_admin: bool,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_login: Option<DateTime<Utc>>,
    pub is_temporary_password: bool,
    /// How the account is managed: `local`, `sso` or `system`.
    pub auth_source: String,
}

/// System-client shadow users are machine identities managed through the system
/// client API. Updating them through the users API (notably `is_admin = true`)
/// would hand a machine token human privileges, so it is rejected.
async fn ensure_not_system_client<'e, E>(executor: E, user_id: Uuid) -> Result<(), Error>
where
    E: sqlx::PgExecutor<'e>,
{
    let is_system_client =
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM system_clients WHERE id = $1)")
            .bind(user_id)
            .fetch_one(executor)
            .await
            .map_err(Error::DatabaseError)?;
    if is_system_client {
        return Err(Error::InvalidInput(
            "system_client_user_immutable: system client identities cannot be updated through the users API; use the system client API"
                .to_string(),
        ));
    }
    Ok(())
}

pub struct CreateUser {
    pub username: String,
    pub password_hash: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub mobile_number: Option<String>,
    pub is_admin: bool,
    pub is_temporary_password: bool,
}

/// A user created on first SSO login.
#[derive(Debug, Clone)]
pub struct CreateSsoUser {
    pub username: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub last_login: Option<DateTime<Utc>>,
}

pub struct UpdateUser {
    pub id: Uuid,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub email: Option<String>,
    pub mobile_number: Option<String>,
    pub is_admin: Option<bool>,
    pub enabled: Option<bool>,
}

#[automock]
#[async_trait::async_trait]
pub trait UserRepository: Send + Sync {
    async fn get_user_by_id(&self, id: Uuid) -> Result<User, Error>;
    async fn get_user_by_username(&self, username: &str) -> Result<User, Error>;
    async fn get_user_by_email(&self, email: &str) -> Result<User, Error>;
    async fn user_exists_by_username(&self, username: &str) -> Result<bool, Error>;
    async fn user_exists_by_email(
        &self,
        email: &str,
        exclude_id: Option<Uuid>,
    ) -> Result<bool, Error>;
    async fn create_user(&self, input: CreateUser) -> Result<User, Error>;
    async fn update_user(&self, input: UpdateUser) -> Result<User, Error>;
    async fn update_last_login(&self, id: Uuid, when: DateTime<Utc>) -> Result<(), Error>;
    async fn update_password(
        &self,
        id: Uuid,
        password_hash: String,
        is_temporary: bool,
    ) -> Result<(), Error>;
    async fn set_user_teams(&self, id: Uuid, team_ids: Vec<Uuid>) -> Result<(), Error>;
    async fn search_users(
        &self,
        team_id: Option<Uuid>,
        name: Option<String>,
        page_number: i32,
        page_size: i32,
    ) -> Result<(Vec<User>, i64), Error>;
    async fn get_user_teams(&self, id: Uuid) -> Result<Vec<Team>, Error>;
    /// Ids of the teams the user belongs to through SSO group sync (`source = 'sso'`).
    async fn list_sso_team_ids(&self, id: Uuid) -> Result<Vec<Uuid>, Error>;
    /// Batch variant of `list_sso_team_ids`: `(user_id, team_id)` pairs for list endpoints.
    async fn list_sso_team_ids_for_users(
        &self,
        user_ids: Vec<Uuid>,
    ) -> Result<Vec<(Uuid, Uuid)>, Error>;
    /// `(user_id, admin_source)` for admins with a recorded source, for list endpoints.
    async fn list_admin_sources_for_users(
        &self,
        user_ids: Vec<Uuid>,
    ) -> Result<Vec<(Uuid, String)>, Error>;
    async fn admin_exists(&self) -> Result<bool, Error>;
    fn clone_box(&self) -> Box<dyn UserRepository>;
}

impl Clone for Box<dyn UserRepository> {
    fn clone(&self) -> Box<dyn UserRepository> {
        self.clone_box()
    }
}

/// Extension trait for transaction-aware repository operations.
/// These methods accept a mutable connection reference for use within transactions.
#[async_trait::async_trait]
pub trait UserRepositoryTx: UserRepository {
    async fn get_user_by_id_tx(&self, conn: &mut PgConnection, id: Uuid) -> Result<User, Error>;
    async fn create_user_tx(
        &self,
        conn: &mut PgConnection,
        input: CreateUser,
    ) -> Result<User, Error>;
    async fn update_user_tx(
        &self,
        conn: &mut PgConnection,
        input: UpdateUser,
    ) -> Result<User, Error>;
    async fn set_user_teams_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        team_ids: Vec<Uuid>,
    ) -> Result<(), Error>;
    async fn update_password_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        password_hash: String,
        is_temporary: bool,
    ) -> Result<(), Error>;
    async fn list_sso_team_ids_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
    ) -> Result<Vec<Uuid>, Error>;
    /// Adds team memberships with `source = 'sso'`. A team the user already belongs
    /// to, manually or through SSO, is left as is (manual wins). Returns the ids
    /// actually inserted.
    async fn add_sso_user_teams_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        team_ids: Vec<Uuid>,
    ) -> Result<Vec<Uuid>, Error>;
    /// Removes only `source = 'sso'` memberships; manual ones are never touched.
    async fn remove_sso_user_teams_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        team_ids: Vec<Uuid>,
    ) -> Result<(), Error>;
    /// `(is_admin, admin_source)` of the user.
    async fn get_admin_state_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
    ) -> Result<(bool, Option<String>), Error>;
    /// Sets `is_admin` with its source (`Some("sso")` when granting, `None` when
    /// revoking). Only used by SSO group sync.
    async fn set_admin_with_source_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        is_admin: bool,
        source: Option<&str>,
    ) -> Result<(), Error>;
    /// Grants admin with source `sso` only if the user is not an admin right now.
    /// Returns whether a row changed.
    async fn grant_sso_admin_tx(&self, conn: &mut PgConnection, id: Uuid) -> Result<bool, Error>;
    /// Revokes admin only if it is still an SSO grant right now, so a concurrent
    /// manual grant is never revoked. Returns whether a row changed.
    async fn revoke_sso_admin_tx(&self, conn: &mut PgConnection, id: Uuid) -> Result<bool, Error>;
    /// The subset of `team_ids` that still exist.
    async fn existing_team_ids_tx(
        &self,
        conn: &mut PgConnection,
        team_ids: Vec<Uuid>,
    ) -> Result<Vec<Uuid>, Error>;
    /// The user whose email matches case-insensitively (oldest first if the
    /// case-sensitive unique constraint lets several differ only in case).
    async fn find_user_by_email_ci_tx(
        &self,
        conn: &mut PgConnection,
        email: &str,
    ) -> Result<Option<User>, Error>;
    /// Whether a username is taken, compared case-insensitively.
    async fn username_exists_tx(
        &self,
        conn: &mut PgConnection,
        username: &str,
    ) -> Result<bool, Error>;
    /// Creates a just-in-time provisioned SSO user: no password,
    /// `auth_source = 'sso'`, not an admin, no temporary password.
    async fn create_sso_user_tx(
        &self,
        conn: &mut PgConnection,
        input: CreateSsoUser,
    ) -> Result<User, Error>;
    /// Whether the user is the shadow user of a system client.
    async fn is_system_client_tx(&self, conn: &mut PgConnection, id: Uuid) -> Result<bool, Error>;
}

pub fn user_repository(pool: PgPool) -> Box<dyn UserRepository> {
    Box::new(UserRepositoryImpl::new(pool))
}

/// Returns a repository that also implements UserRepositoryTx for transaction support.
pub fn user_repository_tx(pool: PgPool) -> UserRepositoryImpl {
    UserRepositoryImpl::new(pool)
}

#[derive(Clone)]
pub struct UserRepositoryImpl {
    pool: PgPool,
}

impl UserRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl UserRepository for UserRepositoryImpl {
    async fn get_user_by_id(&self, id: Uuid) -> Result<User, Error> {
        let result = sqlx::query!(
            r#"SELECT id, username, password_hash, first_name, last_name, email, mobile_number, is_admin, enabled,
                       created_at, updated_at, last_login, is_temporary_password, auth_source
                FROM users WHERE id = $1"#,
            id
        )
        .fetch_one(&self.pool)
        .await;

        let row = handle_error(Some(id), result)?;
        Ok(User {
            id: row.id,
            username: row.username,
            password_hash: row.password_hash,
            first_name: row.first_name,
            last_name: row.last_name,
            email: row.email,
            mobile_number: row.mobile_number,
            is_admin: row.is_admin,
            enabled: row.enabled,
            created_at: row.created_at,
            updated_at: row.updated_at,
            last_login: row.last_login,
            is_temporary_password: row.is_temporary_password,
            auth_source: row.auth_source,
        })
    }

    async fn get_user_by_username(&self, username: &str) -> Result<User, Error> {
        let row = sqlx::query!(
            r#"SELECT id, username, password_hash, first_name, last_name, email, mobile_number, is_admin, enabled,
                       created_at, updated_at, last_login, is_temporary_password, auth_source
                FROM users WHERE username = $1"#,
            username
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::DatabaseError)?
        .ok_or_else(|| Error::InvalidInput("User not found".to_string()))?;
        Ok(User {
            id: row.id,
            username: row.username,
            password_hash: row.password_hash,
            first_name: row.first_name,
            last_name: row.last_name,
            email: row.email,
            mobile_number: row.mobile_number,
            is_admin: row.is_admin,
            enabled: row.enabled,
            created_at: row.created_at,
            updated_at: row.updated_at,
            last_login: row.last_login,
            is_temporary_password: row.is_temporary_password,
            auth_source: row.auth_source,
        })
    }

    async fn get_user_by_email(&self, email: &str) -> Result<User, Error> {
        let row = sqlx::query!(
            r#"SELECT id, username, password_hash, first_name, last_name, email, mobile_number, is_admin, enabled,
                       created_at, updated_at, last_login, is_temporary_password, auth_source
                FROM users WHERE email = $1"#,
            email
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::DatabaseError)?
        .ok_or_else(|| Error::InvalidInput("User not found".to_string()))?;
        Ok(User {
            id: row.id,
            username: row.username,
            password_hash: row.password_hash,
            first_name: row.first_name,
            last_name: row.last_name,
            email: row.email,
            mobile_number: row.mobile_number,
            is_admin: row.is_admin,
            enabled: row.enabled,
            created_at: row.created_at,
            updated_at: row.updated_at,
            last_login: row.last_login,
            is_temporary_password: row.is_temporary_password,
            auth_source: row.auth_source,
        })
    }

    async fn create_user(&self, input: CreateUser) -> Result<User, Error> {
        let id = Uuid::new_v4();
        let result = sqlx::query!(
            r#"INSERT INTO users (id, username, password_hash, first_name, last_name, email, mobile_number, is_admin, is_temporary_password, admin_source)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
               RETURNING id, username, password_hash, first_name, last_name, email, mobile_number, is_admin, enabled, created_at, updated_at, last_login, is_temporary_password, auth_source"#,
            id,
            input.username,
            input.password_hash,
            input.first_name,
            input.last_name,
            input.email,
            input.mobile_number,
            input.is_admin,
            input.is_temporary_password,
            input.is_admin.then_some("manual")
        )
        .fetch_one(&self.pool)
        .await;

        let row = handle_error(None, result)?;
        Ok(User {
            id: row.id,
            username: row.username,
            password_hash: row.password_hash,
            first_name: row.first_name,
            last_name: row.last_name,
            email: row.email,
            mobile_number: row.mobile_number,
            is_admin: row.is_admin,
            enabled: row.enabled,
            created_at: row.created_at,
            updated_at: row.updated_at,
            last_login: row.last_login,
            is_temporary_password: row.is_temporary_password,
            auth_source: row.auth_source,
        })
    }

    async fn update_user(&self, input: UpdateUser) -> Result<User, Error> {
        let existing = self.get_user_by_id(input.id).await?;
        ensure_not_system_client(&self.pool, input.id).await?;
        let result = sqlx::query!(
            r#"UPDATE users
               SET first_name = $1, last_name = $2, email = $3, mobile_number = $4, is_admin = $5, enabled = $6, updated_at = now(),
                   admin_source = CASE WHEN NOT $5 THEN NULL WHEN is_admin THEN admin_source ELSE 'manual' END
               WHERE id = $7
               RETURNING id, username, password_hash, first_name, last_name, email, mobile_number, is_admin, enabled, created_at, updated_at, last_login, is_temporary_password, auth_source"#,
            input.first_name.unwrap_or(existing.first_name),
            input.last_name.unwrap_or(existing.last_name),
            input.email.unwrap_or(existing.email),
            input.mobile_number.or(existing.mobile_number),
            input.is_admin.unwrap_or(existing.is_admin),
            input.enabled.unwrap_or(existing.enabled),
            input.id
        )
        .fetch_one(&self.pool)
        .await;

        let row = handle_error(Some(input.id), result)?;
        Ok(User {
            id: row.id,
            username: row.username,
            password_hash: row.password_hash,
            first_name: row.first_name,
            last_name: row.last_name,
            email: row.email,
            mobile_number: row.mobile_number,
            is_admin: row.is_admin,
            enabled: row.enabled,
            created_at: row.created_at,
            updated_at: row.updated_at,
            last_login: row.last_login,
            is_temporary_password: row.is_temporary_password,
            auth_source: row.auth_source,
        })
    }

    async fn update_last_login(&self, id: Uuid, when: DateTime<Utc>) -> Result<(), Error> {
        let _ = handle_error(
            Some(id),
            sqlx::query!(
                r#"UPDATE users SET last_login = $1, updated_at = now() WHERE id = $2"#,
                when,
                id
            )
            .execute(&self.pool)
            .await,
        )?;
        Ok(())
    }

    async fn update_password(
        &self,
        id: Uuid,
        password_hash: String,
        is_temporary: bool,
    ) -> Result<(), Error> {
        let _ = handle_error(
            Some(id),
            sqlx::query!(
                r#"UPDATE users SET password_hash = $1, is_temporary_password = $2, updated_at = now() WHERE id = $3"#,
                password_hash,
                is_temporary,
                id
            )
            .execute(&self.pool)
            .await,
        )?;
        Ok(())
    }

    async fn user_exists_by_username(&self, username: &str) -> Result<bool, Error> {
        let row = handle_error(
            None,
            sqlx::query("SELECT EXISTS(SELECT 1 FROM users WHERE username = $1) AS exists")
                .bind(username)
                .fetch_one(&self.pool)
                .await,
        )?;
        let exists: bool = row.get::<bool, _>("exists");
        Ok(exists)
    }

    async fn user_exists_by_email(
        &self,
        email: &str,
        exclude_id: Option<Uuid>,
    ) -> Result<bool, Error> {
        let query = match exclude_id {
            Some(id) => sqlx::query(
                "SELECT EXISTS(SELECT 1 FROM users WHERE email = $1 AND id <> $2) AS exists",
            )
            .bind(email)
            .bind(id),
            None => sqlx::query("SELECT EXISTS(SELECT 1 FROM users WHERE email = $1) AS exists")
                .bind(email),
        };
        let row = handle_error(None, query.fetch_one(&self.pool).await)?;
        let exists: bool = row.get::<bool, _>("exists");
        Ok(exists)
    }

    async fn set_user_teams(&self, id: Uuid, team_ids: Vec<Uuid>) -> Result<(), Error> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| Error::DatabaseError(e))?;
        // delete existing manual assignments; SSO-sourced rows are left alone
        handle_error(
            Some(id),
            sqlx::query("DELETE FROM user_teams WHERE user_id = $1 AND source = 'manual'")
                .bind(id)
                .execute(&mut *tx)
                .await,
        )?;
        // insert new ones, if any
        if !team_ids.is_empty() {
            // Build a single multi-values insert for efficiency
            // Use UNNEST for cleaner binding
            let user_ids: Vec<Uuid> = team_ids.iter().map(|_| id).collect();
            handle_error(
                Some(id),
                sqlx::query(
                    r#"INSERT INTO user_teams (user_id, team_id, source)
                       SELECT DISTINCT u, t, 'manual' FROM UNNEST($1::uuid[], $2::uuid[]) AS x(u, t)
                       ON CONFLICT (user_id, team_id) DO UPDATE SET source = 'manual'"#,
                )
                .bind(&user_ids)
                .bind(&team_ids)
                .execute(&mut *tx)
                .await,
            )?;
        }
        tx.commit().await.map_err(|e| Error::DatabaseError(e))?;
        Ok(())
    }

    async fn search_users(
        &self,
        team_id: Option<Uuid>,
        name: Option<String>,
        page_number: i32,
        page_size: i32,
    ) -> Result<(Vec<User>, i64), Error> {
        let mut base = QueryBuilder::<Postgres>::new(
            "SELECT u.id, u.username, u.password_hash, u.first_name, u.last_name, u.email, u.mobile_number, u.is_admin, u.enabled, u.created_at, u.updated_at, u.last_login, u.is_temporary_password, u.auth_source FROM users u",
        );
        if team_id.is_some() {
            base.push(" JOIN user_teams ut ON ut.user_id = u.id");
        }
        base.push(" WHERE 1=1");
        if let Some(tid) = team_id {
            base.push(" AND ut.team_id = ").push_bind(tid);
        }
        if let Some(n) = name.clone() {
            let pattern = format!("%{}%", n);
            base.push(" AND (u.first_name ILIKE ")
                .push_bind(pattern.clone())
                .push(" OR u.last_name ILIKE ")
                .push_bind(pattern.clone())
                .push(" OR u.username ILIKE ")
                .push_bind(pattern)
                .push(")");
        }
        // Pagination
        let page = if page_number < 1 { 1 } else { page_number } as i64;
        let size = if page_size < 1 { 10 } else { page_size } as i64;
        let offset = (page - 1) * size;
        base.push(" ORDER BY u.username ASC LIMIT ")
            .push_bind(size)
            .push(" OFFSET ")
            .push_bind(offset);

        let rows = handle_error(None, base.build().fetch_all(&self.pool).await)?;
        let mut users: Vec<User> = Vec::with_capacity(rows.len());
        for row in rows {
            users.push(User {
                id: row.get::<Uuid, _>(0),
                username: row.get::<String, _>(1),
                password_hash: row.try_get::<String, _>(2).ok(),
                first_name: row.get::<String, _>(3),
                last_name: row.get::<String, _>(4),
                email: row.get::<String, _>(5),
                mobile_number: row.try_get::<String, _>(6).ok(),
                is_admin: row.get::<bool, _>(7),
                enabled: row.get::<bool, _>(8),
                created_at: row.get::<DateTime<Utc>, _>(9),
                updated_at: row.get::<DateTime<Utc>, _>(10),
                last_login: row.try_get::<DateTime<Utc>, _>(11).ok(),
                is_temporary_password: row.get::<bool, _>(12),
                auth_source: row.get::<String, _>(13),
            });
        }

        // Total count
        let mut cnt =
            QueryBuilder::<Postgres>::new("SELECT COUNT(DISTINCT u.id) AS c FROM users u");
        if team_id.is_some() {
            cnt.push(" JOIN user_teams ut ON ut.user_id = u.id");
        }
        cnt.push(" WHERE 1=1");
        if let Some(tid) = team_id {
            cnt.push(" AND ut.team_id = ").push_bind(tid);
        }
        if let Some(n) = name {
            let pattern = format!("%{}%", n);
            cnt.push(" AND (u.first_name ILIKE ")
                .push_bind(pattern.clone())
                .push(" OR u.last_name ILIKE ")
                .push_bind(pattern.clone())
                .push(" OR u.username ILIKE ")
                .push_bind(pattern)
                .push(")");
        }
        let row = handle_error(None, cnt.build().fetch_one(&self.pool).await)?;
        let total: i64 = row.get::<i64, _>(0);

        Ok((users, total))
    }

    async fn get_user_teams(&self, id: Uuid) -> Result<Vec<Team>, Error> {
        let result = sqlx::query_as::<_, Team>(
            r#"SELECT t.id, t.name, t.description
               FROM teams t
               INNER JOIN user_teams ut ON ut.team_id = t.id
               WHERE ut.user_id = $1
               ORDER BY t.name"#,
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await;
        let teams = handle_error(Some(id), result)?;
        Ok(teams)
    }

    async fn list_sso_team_ids(&self, id: Uuid) -> Result<Vec<Uuid>, Error> {
        let mut conn = self.pool.acquire().await.map_err(Error::DatabaseError)?;
        Self::list_sso_team_ids_internal(&mut conn, id).await
    }

    async fn list_sso_team_ids_for_users(
        &self,
        user_ids: Vec<Uuid>,
    ) -> Result<Vec<(Uuid, Uuid)>, Error> {
        let rows = sqlx::query!(
            "SELECT user_id, team_id FROM user_teams \
             WHERE source = 'sso' AND user_id = ANY($1) ORDER BY user_id, team_id",
            &user_ids
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::DatabaseError)?;
        Ok(rows.into_iter().map(|r| (r.user_id, r.team_id)).collect())
    }

    async fn list_admin_sources_for_users(
        &self,
        user_ids: Vec<Uuid>,
    ) -> Result<Vec<(Uuid, String)>, Error> {
        let rows = sqlx::query(
            "SELECT id, admin_source FROM users \
             WHERE is_admin = TRUE AND admin_source IS NOT NULL AND id = ANY($1)",
        )
        .bind(&user_ids)
        .fetch_all(&self.pool)
        .await
        .map_err(Error::DatabaseError)?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<Uuid, _>("id"), r.get::<String, _>("admin_source")))
            .collect())
    }

    async fn admin_exists(&self) -> Result<bool, Error> {
        let row = handle_error(
            None,
            sqlx::query("SELECT EXISTS(SELECT 1 FROM users WHERE is_admin = true AND enabled = true AND id NOT IN (SELECT id FROM system_clients)) AS exists")
                .fetch_one(&self.pool)
                .await,
        )?;
        let exists: bool = row.get::<bool, _>("exists");
        Ok(exists)
    }

    fn clone_box(&self) -> Box<dyn UserRepository> {
        Box::new(self.clone())
    }
}

impl UserRepositoryImpl {
    async fn list_sso_team_ids_internal(
        conn: &mut PgConnection,
        id: Uuid,
    ) -> Result<Vec<Uuid>, Error> {
        let result = sqlx::query_scalar!(
            "SELECT team_id FROM user_teams WHERE user_id = $1 AND source = 'sso' ORDER BY team_id",
            id
        )
        .fetch_all(&mut *conn)
        .await;
        handle_error(Some(id), result)
    }

    async fn add_sso_user_teams_internal(
        conn: &mut PgConnection,
        id: Uuid,
        team_ids: Vec<Uuid>,
    ) -> Result<Vec<Uuid>, Error> {
        let mut inserted = Vec::new();
        for team_id in team_ids {
            let row = handle_error(
                Some(id),
                sqlx::query_scalar!(
                    r#"INSERT INTO user_teams (user_id, team_id, source)
                       VALUES ($1, $2, 'sso')
                       ON CONFLICT (user_id, team_id) DO NOTHING
                       RETURNING team_id"#,
                    id,
                    team_id
                )
                .fetch_optional(&mut *conn)
                .await,
            )?;
            inserted.extend(row);
        }
        Ok(inserted)
    }

    async fn remove_sso_user_teams_internal(
        conn: &mut PgConnection,
        id: Uuid,
        team_ids: Vec<Uuid>,
    ) -> Result<(), Error> {
        handle_error(
            Some(id),
            sqlx::query!(
                "DELETE FROM user_teams WHERE user_id = $1 AND source = 'sso' AND team_id = ANY($2)",
                id,
                &team_ids
            )
            .execute(&mut *conn)
            .await,
        )?;
        Ok(())
    }

    async fn get_user_by_id_internal(conn: &mut PgConnection, id: Uuid) -> Result<User, Error> {
        let result = sqlx::query!(
            r#"SELECT id, username, password_hash, first_name, last_name, email, mobile_number, is_admin, enabled,
                       created_at, updated_at, last_login, is_temporary_password, auth_source
                FROM users WHERE id = $1"#,
            id
        )
        .fetch_one(&mut *conn)
        .await;

        let row = handle_error(Some(id), result)?;
        Ok(User {
            id: row.id,
            username: row.username,
            password_hash: row.password_hash,
            first_name: row.first_name,
            last_name: row.last_name,
            email: row.email,
            mobile_number: row.mobile_number,
            is_admin: row.is_admin,
            enabled: row.enabled,
            created_at: row.created_at,
            updated_at: row.updated_at,
            last_login: row.last_login,
            is_temporary_password: row.is_temporary_password,
            auth_source: row.auth_source,
        })
    }

    async fn create_user_internal(
        conn: &mut PgConnection,
        input: CreateUser,
    ) -> Result<User, Error> {
        let id = Uuid::new_v4();
        let result = sqlx::query!(
            r#"INSERT INTO users (id, username, password_hash, first_name, last_name, email, mobile_number, is_admin, is_temporary_password, admin_source)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
               RETURNING id, username, password_hash, first_name, last_name, email, mobile_number, is_admin, enabled, created_at, updated_at, last_login, is_temporary_password, auth_source"#,
            id,
            input.username,
            input.password_hash,
            input.first_name,
            input.last_name,
            input.email,
            input.mobile_number,
            input.is_admin,
            input.is_temporary_password,
            input.is_admin.then_some("manual")
        )
        .fetch_one(&mut *conn)
        .await;

        let row = handle_error(None, result)?;
        Ok(User {
            id: row.id,
            username: row.username,
            password_hash: row.password_hash,
            first_name: row.first_name,
            last_name: row.last_name,
            email: row.email,
            mobile_number: row.mobile_number,
            is_admin: row.is_admin,
            enabled: row.enabled,
            created_at: row.created_at,
            updated_at: row.updated_at,
            last_login: row.last_login,
            is_temporary_password: row.is_temporary_password,
            auth_source: row.auth_source,
        })
    }

    async fn update_user_internal(
        conn: &mut PgConnection,
        input: UpdateUser,
        existing: User,
    ) -> Result<User, Error> {
        let result = sqlx::query!(
            r#"UPDATE users
               SET first_name = $1, last_name = $2, email = $3, mobile_number = $4, is_admin = $5, enabled = $6, updated_at = now(),
                   admin_source = CASE WHEN NOT $5 THEN NULL WHEN is_admin THEN admin_source ELSE 'manual' END
               WHERE id = $7
               RETURNING id, username, password_hash, first_name, last_name, email, mobile_number, is_admin, enabled, created_at, updated_at, last_login, is_temporary_password, auth_source"#,
            input.first_name.unwrap_or(existing.first_name),
            input.last_name.unwrap_or(existing.last_name),
            input.email.unwrap_or(existing.email),
            input.mobile_number.or(existing.mobile_number),
            input.is_admin.unwrap_or(existing.is_admin),
            input.enabled.unwrap_or(existing.enabled),
            input.id
        )
        .fetch_one(&mut *conn)
        .await;

        let row = handle_error(Some(input.id), result)?;
        Ok(User {
            id: row.id,
            username: row.username,
            password_hash: row.password_hash,
            first_name: row.first_name,
            last_name: row.last_name,
            email: row.email,
            mobile_number: row.mobile_number,
            is_admin: row.is_admin,
            enabled: row.enabled,
            created_at: row.created_at,
            updated_at: row.updated_at,
            last_login: row.last_login,
            is_temporary_password: row.is_temporary_password,
            auth_source: row.auth_source,
        })
    }

    async fn set_user_teams_internal(
        conn: &mut PgConnection,
        id: Uuid,
        team_ids: Vec<Uuid>,
    ) -> Result<(), Error> {
        // delete existing manual assignments; SSO-sourced rows are left alone
        handle_error(
            Some(id),
            sqlx::query("DELETE FROM user_teams WHERE user_id = $1 AND source = 'manual'")
                .bind(id)
                .execute(&mut *conn)
                .await,
        )?;
        // insert new ones, if any
        if !team_ids.is_empty() {
            let user_ids: Vec<Uuid> = team_ids.iter().map(|_| id).collect();
            handle_error(
                Some(id),
                sqlx::query(
                    r#"INSERT INTO user_teams (user_id, team_id, source)
                       SELECT DISTINCT u, t, 'manual' FROM UNNEST($1::uuid[], $2::uuid[]) AS x(u, t)
                       ON CONFLICT (user_id, team_id) DO UPDATE SET source = 'manual'"#,
                )
                .bind(&user_ids)
                .bind(&team_ids)
                .execute(&mut *conn)
                .await,
            )?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl UserRepositoryTx for UserRepositoryImpl {
    async fn get_user_by_id_tx(&self, conn: &mut PgConnection, id: Uuid) -> Result<User, Error> {
        Self::get_user_by_id_internal(conn, id).await
    }

    async fn create_user_tx(
        &self,
        conn: &mut PgConnection,
        input: CreateUser,
    ) -> Result<User, Error> {
        Self::create_user_internal(conn, input).await
    }

    async fn update_user_tx(
        &self,
        conn: &mut PgConnection,
        input: UpdateUser,
    ) -> Result<User, Error> {
        let existing = Self::get_user_by_id_internal(conn, input.id).await?;
        ensure_not_system_client(&mut *conn, input.id).await?;
        Self::update_user_internal(conn, input, existing).await
    }

    async fn set_user_teams_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        team_ids: Vec<Uuid>,
    ) -> Result<(), Error> {
        Self::set_user_teams_internal(conn, id, team_ids).await
    }

    async fn update_password_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        password_hash: String,
        is_temporary: bool,
    ) -> Result<(), Error> {
        let result = sqlx::query!(
            r#"
            UPDATE users
            SET password_hash = $2,
                is_temporary_password = $3,
                updated_at = NOW()
            WHERE id = $1
            "#,
            id,
            password_hash,
            is_temporary
        )
        .execute(&mut *conn)
        .await;

        handle_error(Some(id), result)?;
        Ok(())
    }

    async fn list_sso_team_ids_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
    ) -> Result<Vec<Uuid>, Error> {
        Self::list_sso_team_ids_internal(conn, id).await
    }

    async fn add_sso_user_teams_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        team_ids: Vec<Uuid>,
    ) -> Result<Vec<Uuid>, Error> {
        Self::add_sso_user_teams_internal(conn, id, team_ids).await
    }

    async fn remove_sso_user_teams_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        team_ids: Vec<Uuid>,
    ) -> Result<(), Error> {
        Self::remove_sso_user_teams_internal(conn, id, team_ids).await
    }

    async fn get_admin_state_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
    ) -> Result<(bool, Option<String>), Error> {
        let row = handle_error(
            Some(id),
            sqlx::query!("SELECT is_admin, admin_source FROM users WHERE id = $1", id)
                .fetch_one(&mut *conn)
                .await,
        )?;
        Ok((row.is_admin, row.admin_source))
    }

    async fn set_admin_with_source_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        is_admin: bool,
        source: Option<&str>,
    ) -> Result<(), Error> {
        handle_error(
            Some(id),
            sqlx::query!(
                "UPDATE users SET is_admin = $2, admin_source = $3, updated_at = now() WHERE id = $1",
                id,
                is_admin,
                source
            )
            .execute(&mut *conn)
            .await,
        )?;
        Ok(())
    }

    async fn grant_sso_admin_tx(&self, conn: &mut PgConnection, id: Uuid) -> Result<bool, Error> {
        let result = handle_error(
            Some(id),
            sqlx::query!(
                "UPDATE users SET is_admin = TRUE, admin_source = 'sso', updated_at = now() \
                 WHERE id = $1 AND is_admin = FALSE",
                id
            )
            .execute(&mut *conn)
            .await,
        )?;
        Ok(result.rows_affected() > 0)
    }

    async fn revoke_sso_admin_tx(&self, conn: &mut PgConnection, id: Uuid) -> Result<bool, Error> {
        let result = handle_error(
            Some(id),
            sqlx::query!(
                "UPDATE users SET is_admin = FALSE, admin_source = NULL, updated_at = now() \
                 WHERE id = $1 AND is_admin = TRUE AND admin_source = 'sso'",
                id
            )
            .execute(&mut *conn)
            .await,
        )?;
        Ok(result.rows_affected() > 0)
    }

    async fn existing_team_ids_tx(
        &self,
        conn: &mut PgConnection,
        team_ids: Vec<Uuid>,
    ) -> Result<Vec<Uuid>, Error> {
        handle_error(
            None,
            sqlx::query_scalar!("SELECT id FROM teams WHERE id = ANY($1)", &team_ids)
                .fetch_all(&mut *conn)
                .await,
        )
    }

    async fn find_user_by_email_ci_tx(
        &self,
        conn: &mut PgConnection,
        email: &str,
    ) -> Result<Option<User>, Error> {
        let row = sqlx::query!(
            r#"SELECT id, username, password_hash, first_name, last_name, email, mobile_number, is_admin, enabled,
                       created_at, updated_at, last_login, is_temporary_password, auth_source
                FROM users WHERE lower(email) = lower($1)
                ORDER BY created_at
                LIMIT 1"#,
            email
        )
        .fetch_optional(&mut *conn)
        .await
        .map_err(Error::DatabaseError)?;
        Ok(row.map(|row| User {
            id: row.id,
            username: row.username,
            password_hash: row.password_hash,
            first_name: row.first_name,
            last_name: row.last_name,
            email: row.email,
            mobile_number: row.mobile_number,
            is_admin: row.is_admin,
            enabled: row.enabled,
            created_at: row.created_at,
            updated_at: row.updated_at,
            last_login: row.last_login,
            is_temporary_password: row.is_temporary_password,
            auth_source: row.auth_source,
        }))
    }

    async fn username_exists_tx(
        &self,
        conn: &mut PgConnection,
        username: &str,
    ) -> Result<bool, Error> {
        let exists = sqlx::query_scalar!(
            r#"SELECT EXISTS(SELECT 1 FROM users WHERE lower(username) = lower($1)) AS "exists!""#,
            username
        )
        .fetch_one(&mut *conn)
        .await
        .map_err(Error::DatabaseError)?;
        Ok(exists)
    }

    async fn create_sso_user_tx(
        &self,
        conn: &mut PgConnection,
        input: CreateSsoUser,
    ) -> Result<User, Error> {
        let result = sqlx::query!(
            r#"INSERT INTO users (id, username, password_hash, first_name, last_name, email, is_admin,
                                  is_temporary_password, auth_source, last_login)
               VALUES ($1, $2, NULL, $3, $4, $5, FALSE, FALSE, 'sso', $6)
               RETURNING id, username, password_hash, first_name, last_name, email, mobile_number, is_admin, enabled,
                         created_at, updated_at, last_login, is_temporary_password, auth_source"#,
            Uuid::new_v4(),
            input.username,
            input.first_name,
            input.last_name,
            input.email,
            input.last_login
        )
        .fetch_one(&mut *conn)
        .await;
        let row = handle_error(None, result)?;
        Ok(User {
            id: row.id,
            username: row.username,
            password_hash: row.password_hash,
            first_name: row.first_name,
            last_name: row.last_name,
            email: row.email,
            mobile_number: row.mobile_number,
            is_admin: row.is_admin,
            enabled: row.enabled,
            created_at: row.created_at,
            updated_at: row.updated_at,
            last_login: row.last_login,
            is_temporary_password: row.is_temporary_password,
            auth_source: row.auth_source,
        })
    }

    async fn is_system_client_tx(&self, conn: &mut PgConnection, id: Uuid) -> Result<bool, Error> {
        let exists = sqlx::query_scalar!(
            r#"SELECT EXISTS(SELECT 1 FROM system_clients WHERE id = $1) AS "exists!""#,
            id
        )
        .fetch_one(&mut *conn)
        .await
        .map_err(Error::DatabaseError)?;
        Ok(exists)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    fn sample_user() -> User {
        User {
            id: Uuid::new_v4(),
            username: "jdoe".to_string(),
            password_hash: Some("hashed_password".to_string()),
            first_name: "John".to_string(),
            last_name: "Doe".to_string(),
            email: "john@example.com".to_string(),
            mobile_number: Some("+15550001111".to_string()),
            is_admin: false,
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_login: None,
            is_temporary_password: false,
            auth_source: "local".to_string(),
        }
    }

    fn sample_create_user() -> CreateUser {
        CreateUser {
            username: "newuser".to_string(),
            password_hash: "hashed_new_password".to_string(),
            first_name: "New".to_string(),
            last_name: "User".to_string(),
            email: "new@example.com".to_string(),
            mobile_number: Some("+15550002222".to_string()),
            is_admin: false,
            is_temporary_password: false,
        }
    }

    fn sample_update_user(id: Uuid) -> UpdateUser {
        UpdateUser {
            id,
            first_name: Some("Updated".to_string()),
            last_name: Some("Name".to_string()),
            email: Some("updated@example.com".to_string()),
            mobile_number: Some("+15550003333".to_string()),
            is_admin: Some(true),
            enabled: Some(false),
        }
    }

    #[test]
    fn test_user_struct_creation() {
        let user = sample_user();
        assert_eq!(user.username, "jdoe");
        assert_eq!(user.email, "john@example.com");
        assert!(!user.is_admin);
        assert!(user.enabled);
        assert!(!user.is_temporary_password);
    }

    #[test]
    fn test_create_user_struct() {
        let create_user = sample_create_user();
        assert_eq!(create_user.username, "newuser");
        assert_eq!(create_user.first_name, "New");
        assert_eq!(create_user.last_name, "User");
        assert_eq!(create_user.email, "new@example.com");
        assert!(!create_user.is_admin);
        assert!(!create_user.is_temporary_password);
    }

    #[test]
    fn test_update_user_struct() {
        let user_id = Uuid::new_v4();
        let update_user = sample_update_user(user_id);

        assert_eq!(update_user.id, user_id);
        assert_eq!(update_user.first_name, Some("Updated".to_string()));
        assert_eq!(update_user.last_name, Some("Name".to_string()));
        assert_eq!(update_user.email, Some("updated@example.com".to_string()));
        assert_eq!(update_user.mobile_number, Some("+15550003333".to_string()));
        assert_eq!(update_user.is_admin, Some(true));
        assert_eq!(update_user.enabled, Some(false));
    }

    #[test]
    fn test_user_repository_factory() {
        // This test only verifies the factory function signature
        // In a real test environment, this would use a test database connection
        use sqlx::PgPool;

        // We can't actually create a pool in unit tests without a database
        // Just verify this compiles and is the correct function signature
        fn _verify_signature(_pool: PgPool) -> Box<dyn UserRepository> {
            user_repository(_pool)
        }

        // Test passes if it compiles
        assert!(true);
    }

    #[test]
    fn test_user_repository_impl_creation() {
        // This test only verifies the constructor signature
        // In a real test environment, this would use a test database connection
        use sqlx::PgPool;

        // Just verify this compiles and is the correct function signature
        fn _verify_signature(_pool: PgPool) -> UserRepositoryImpl {
            UserRepositoryImpl::new(_pool)
        }

        // Test passes if it compiles
        assert!(true);
    }

    #[tokio::test]
    async fn test_mock_user_repository_get_user_by_id() {
        let mut mock_repo = MockUserRepository::new();
        let user = sample_user();
        let user_id = user.id;

        mock_repo
            .expect_get_user_by_id()
            .with(mockall::predicate::eq(user_id))
            .times(1)
            .returning(move |_| Ok(user.clone()));

        let result = mock_repo.get_user_by_id(user_id).await;
        assert!(result.is_ok());
        let retrieved_user = result.unwrap();
        assert_eq!(retrieved_user.username, "jdoe");
    }

    #[tokio::test]
    async fn test_mock_user_repository_get_user_by_username() {
        let mut mock_repo = MockUserRepository::new();
        let user = sample_user();

        mock_repo
            .expect_get_user_by_username()
            .with(mockall::predicate::eq("jdoe"))
            .times(1)
            .returning(move |_| Ok(user.clone()));

        let result = mock_repo.get_user_by_username("jdoe").await;
        assert!(result.is_ok());
        let retrieved_user = result.unwrap();
        assert_eq!(retrieved_user.username, "jdoe");
    }

    #[tokio::test]
    async fn test_mock_user_repository_get_user_by_email() {
        let mut mock_repo = MockUserRepository::new();
        let user = sample_user();

        mock_repo
            .expect_get_user_by_email()
            .with(mockall::predicate::eq("john@example.com"))
            .times(1)
            .returning(move |_| Ok(user.clone()));

        let result = mock_repo.get_user_by_email("john@example.com").await;
        assert!(result.is_ok());
        let retrieved_user = result.unwrap();
        assert_eq!(retrieved_user.email, "john@example.com");
    }

    #[tokio::test]
    async fn test_mock_user_repository_user_exists_by_username() {
        let mut mock_repo = MockUserRepository::new();

        mock_repo
            .expect_user_exists_by_username()
            .with(mockall::predicate::eq("existing_user"))
            .times(1)
            .returning(|_| Ok(true));

        mock_repo
            .expect_user_exists_by_username()
            .with(mockall::predicate::eq("nonexistent_user"))
            .times(1)
            .returning(|_| Ok(false));

        let exists = mock_repo
            .user_exists_by_username("existing_user")
            .await
            .unwrap();
        assert!(exists);

        let not_exists = mock_repo
            .user_exists_by_username("nonexistent_user")
            .await
            .unwrap();
        assert!(!not_exists);
    }

    #[tokio::test]
    async fn test_mock_user_repository_user_exists_by_email() {
        let mut mock_repo = MockUserRepository::new();

        mock_repo
            .expect_user_exists_by_email()
            .with(
                mockall::predicate::eq("existing@example.com"),
                mockall::predicate::eq(None),
            )
            .times(1)
            .returning(|_, _| Ok(true));

        let exists = mock_repo
            .user_exists_by_email("existing@example.com", None)
            .await
            .unwrap();
        assert!(exists);
    }

    #[tokio::test]
    async fn test_mock_user_repository_create_user() {
        let mut mock_repo = MockUserRepository::new();
        let create_input = sample_create_user();
        let expected_user = sample_user();

        mock_repo
            .expect_create_user()
            .withf(|input| input.username == "newuser")
            .times(1)
            .returning(move |_| Ok(expected_user.clone()));

        let result = mock_repo.create_user(create_input).await;
        assert!(result.is_ok());
        let created_user = result.unwrap();
        assert_eq!(created_user.username, "jdoe");
    }

    #[tokio::test]
    async fn test_mock_user_repository_update_user() {
        let mut mock_repo = MockUserRepository::new();
        let user_id = Uuid::new_v4();
        let update_input = sample_update_user(user_id);
        let expected_user = User {
            id: user_id,
            username: "jdoe".to_string(),
            first_name: "Updated".to_string(),
            last_name: "Name".to_string(),
            email: "updated@example.com".to_string(),
            is_admin: true,
            enabled: false,
            ..sample_user()
        };

        mock_repo
            .expect_update_user()
            .withf(move |input| input.id == user_id)
            .times(1)
            .returning(move |_| Ok(expected_user.clone()));

        let result = mock_repo.update_user(update_input).await;
        assert!(result.is_ok());
        let updated_user = result.unwrap();
        assert_eq!(updated_user.first_name, "Updated");
        assert_eq!(updated_user.last_name, "Name");
        assert!(updated_user.is_admin);
        assert!(!updated_user.enabled);
    }

    #[tokio::test]
    async fn test_mock_user_repository_update_last_login() {
        let mut mock_repo = MockUserRepository::new();
        let user_id = Uuid::new_v4();
        let login_time = Utc::now();

        mock_repo
            .expect_update_last_login()
            .with(
                mockall::predicate::eq(user_id),
                mockall::predicate::always(),
            )
            .times(1)
            .returning(|_, _| Ok(()));

        let result = mock_repo.update_last_login(user_id, login_time).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mock_user_repository_update_password() {
        let mut mock_repo = MockUserRepository::new();
        let user_id = Uuid::new_v4();
        let new_password_hash = "new_hashed_password".to_string();

        mock_repo
            .expect_update_password()
            .with(
                mockall::predicate::eq(user_id),
                mockall::predicate::eq(new_password_hash.clone()),
                mockall::predicate::eq(false),
            )
            .times(1)
            .returning(|_, _, _| Ok(()));

        let result = mock_repo
            .update_password(user_id, new_password_hash, false)
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mock_user_repository_set_user_teams() {
        let mut mock_repo = MockUserRepository::new();
        let user_id = Uuid::new_v4();
        let team_ids = vec![Uuid::new_v4(), Uuid::new_v4()];

        mock_repo
            .expect_set_user_teams()
            .with(
                mockall::predicate::eq(user_id),
                mockall::predicate::eq(team_ids.clone()),
            )
            .times(1)
            .returning(|_, _| Ok(()));

        let result = mock_repo.set_user_teams(user_id, team_ids).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mock_user_repository_search_users() {
        let mut mock_repo = MockUserRepository::new();
        let team_id = Some(Uuid::new_v4());
        let name_filter = Some("John".to_string());
        let users = vec![sample_user()];
        let total_count = 1i64;

        mock_repo
            .expect_search_users()
            .with(
                mockall::predicate::eq(team_id),
                mockall::predicate::eq(name_filter.clone()),
                mockall::predicate::eq(1),  // page_number
                mockall::predicate::eq(10), // page_size
            )
            .times(1)
            .returning(move |_, _, _, _| Ok((users.clone(), total_count)));

        let result = mock_repo.search_users(team_id, name_filter, 1, 10).await;
        assert!(result.is_ok());
        let (returned_users, count) = result.unwrap();
        assert_eq!(returned_users.len(), 1);
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_mock_user_repository_get_user_teams() {
        let mut mock_repo = MockUserRepository::new();
        let user_id = Uuid::new_v4();
        let teams = vec![Team {
            id: Uuid::new_v4(),
            name: "Test Team".to_string(),
            description: "Test team description".to_string(),
        }];

        mock_repo
            .expect_get_user_teams()
            .with(mockall::predicate::eq(user_id))
            .times(1)
            .returning(move |_| Ok(teams.clone()));

        let result = mock_repo.get_user_teams(user_id).await;
        assert!(result.is_ok());
        let user_teams = result.unwrap();
        assert_eq!(user_teams.len(), 1);
        assert_eq!(user_teams[0].name, "Test Team");
    }

    #[tokio::test]
    async fn test_mock_user_repository_admin_exists() {
        let mut mock_repo = MockUserRepository::new();

        mock_repo
            .expect_admin_exists()
            .times(1)
            .returning(|| Ok(true));

        mock_repo
            .expect_admin_exists()
            .times(1)
            .returning(|| Ok(false));

        let admin_exists = mock_repo.admin_exists().await.unwrap();
        assert!(admin_exists);

        let no_admin_exists = mock_repo.admin_exists().await.unwrap();
        assert!(!no_admin_exists);
    }

    #[tokio::test]
    async fn test_mock_user_repository_error_scenarios() {
        let mut mock_repo = MockUserRepository::new();
        let user_id = Uuid::new_v4();

        // Test not found error
        mock_repo
            .expect_get_user_by_id()
            .with(mockall::predicate::eq(user_id))
            .times(1)
            .returning(move |id| Err(Error::NotFound(id)));

        let result = mock_repo.get_user_by_id(user_id).await;
        assert!(result.is_err());
        match result.err().unwrap() {
            Error::NotFound(id) => assert_eq!(id, user_id),
            _ => panic!("Expected NotFound error"),
        }
    }
}
