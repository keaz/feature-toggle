//! Persistence for OIDC identity provider configuration.

use crate::Error;
use crate::database::entity::SsoProvider;
use mockall::automock;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct CreateSsoProvider {
    pub slug: String,
    pub display_name: String,
    pub issuer_url: String,
    pub client_id: String,
    /// Already encrypted (see `logic::secret_box`).
    pub client_secret_enc: Option<String>,
    pub scopes: Vec<String>,
    pub groups_claim: String,
    pub allowed_email_domains: Vec<String>,
    pub jit_provisioning: bool,
    pub allow_email_linking: bool,
    pub role_sync_mode: String,
    pub enabled: bool,
}

/// Partial update; `None` leaves a column unchanged.
#[derive(Debug, Clone, Default)]
pub struct UpdateSsoProvider {
    pub slug: Option<String>,
    pub display_name: Option<String>,
    pub issuer_url: Option<String>,
    pub client_id: Option<String>,
    /// `None` = unchanged, `Some(None)` = clear, `Some(Some(v))` = set to the encrypted value `v`.
    pub client_secret_enc: Option<Option<String>>,
    pub scopes: Option<Vec<String>>,
    pub groups_claim: Option<String>,
    pub allowed_email_domains: Option<Vec<String>>,
    pub jit_provisioning: Option<bool>,
    pub allow_email_linking: Option<bool>,
    pub role_sync_mode: Option<String>,
    pub enabled: Option<bool>,
}

#[automock]
#[async_trait::async_trait]
pub trait SsoProviderRepository: Send + Sync {
    async fn list_providers(&self) -> Result<Vec<SsoProvider>, Error>;
    async fn list_enabled_providers(&self) -> Result<Vec<SsoProvider>, Error>;
    async fn get_provider_by_id(&self, id: Uuid) -> Result<SsoProvider, Error>;
    async fn find_provider_by_slug(&self, slug: &str) -> Result<Option<SsoProvider>, Error>;
    fn clone_box(&self) -> Box<dyn SsoProviderRepository>;
}

impl Clone for Box<dyn SsoProviderRepository> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

/// Transaction-aware operations for writes.
#[async_trait::async_trait]
pub trait SsoProviderRepositoryTx: SsoProviderRepository {
    async fn get_provider_by_id_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
    ) -> Result<SsoProvider, Error>;
    async fn create_provider_tx(
        &self,
        conn: &mut PgConnection,
        input: CreateSsoProvider,
    ) -> Result<SsoProvider, Error>;
    async fn update_provider_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        input: UpdateSsoProvider,
    ) -> Result<SsoProvider, Error>;
    /// Deletes the provider; identities, mappings, login states and codes cascade.
    async fn delete_provider_tx(&self, conn: &mut PgConnection, id: Uuid) -> Result<(), Error>;
}

pub fn sso_provider_repository(pool: PgPool) -> Box<dyn SsoProviderRepository> {
    Box::new(SsoProviderRepositoryImpl { pool })
}

pub fn sso_provider_repository_tx(pool: PgPool) -> SsoProviderRepositoryImpl {
    SsoProviderRepositoryImpl { pool }
}

#[derive(Clone)]
pub struct SsoProviderRepositoryImpl {
    pool: PgPool,
}

fn map_error(id: Option<Uuid>, err: sqlx::Error) -> Error {
    match &err {
        sqlx::Error::RowNotFound => match id {
            Some(id) => Error::NotFound(id),
            None => Error::DatabaseError(err),
        },
        sqlx::Error::Database(db_err)
            if db_err.code().as_deref() == Some("23505")
                && db_err
                    .constraint()
                    .is_some_and(|c| c.contains("sso_providers_slug_key")) =>
        {
            Error::RecordAlreadyExists("slug".to_string())
        }
        _ => Error::DatabaseError(err),
    }
}

impl SsoProviderRepositoryImpl {
    async fn get_by_id(conn: &mut PgConnection, id: Uuid) -> Result<SsoProvider, Error> {
        sqlx::query_as!(
            SsoProvider,
            r#"
            SELECT id, slug, display_name, issuer_url, client_id, client_secret_enc, scopes,
                   groups_claim, allowed_email_domains, jit_provisioning, allow_email_linking,
                   role_sync_mode, enabled, created_at, updated_at
            FROM sso_providers WHERE id = $1
            "#,
            id
        )
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| map_error(Some(id), e))
    }

    async fn insert(
        conn: &mut PgConnection,
        input: CreateSsoProvider,
    ) -> Result<SsoProvider, Error> {
        sqlx::query_as!(
            SsoProvider,
            r#"
            INSERT INTO sso_providers (
                id, slug, display_name, issuer_url, client_id, client_secret_enc, scopes,
                groups_claim, allowed_email_domains, jit_provisioning, allow_email_linking,
                role_sync_mode, enabled
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            RETURNING id, slug, display_name, issuer_url, client_id, client_secret_enc, scopes,
                      groups_claim, allowed_email_domains, jit_provisioning, allow_email_linking,
                      role_sync_mode, enabled, created_at, updated_at
            "#,
            Uuid::new_v4(),
            input.slug,
            input.display_name,
            input.issuer_url,
            input.client_id,
            input.client_secret_enc,
            &input.scopes,
            input.groups_claim,
            &input.allowed_email_domains,
            input.jit_provisioning,
            input.allow_email_linking,
            input.role_sync_mode,
            input.enabled
        )
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| map_error(None, e))
    }

    async fn update(
        conn: &mut PgConnection,
        id: Uuid,
        input: UpdateSsoProvider,
    ) -> Result<SsoProvider, Error> {
        let set_secret = input.client_secret_enc.is_some();
        let secret = input.client_secret_enc.flatten();
        sqlx::query_as!(
            SsoProvider,
            r#"
            UPDATE sso_providers SET
                slug = COALESCE($2, slug),
                display_name = COALESCE($3, display_name),
                issuer_url = COALESCE($4, issuer_url),
                client_id = COALESCE($5, client_id),
                client_secret_enc = CASE WHEN $6 THEN $7 ELSE client_secret_enc END,
                scopes = COALESCE($8, scopes),
                groups_claim = COALESCE($9, groups_claim),
                allowed_email_domains = COALESCE($10, allowed_email_domains),
                jit_provisioning = COALESCE($11, jit_provisioning),
                allow_email_linking = COALESCE($12, allow_email_linking),
                role_sync_mode = COALESCE($13, role_sync_mode),
                enabled = COALESCE($14, enabled),
                updated_at = now()
            WHERE id = $1
            RETURNING id, slug, display_name, issuer_url, client_id, client_secret_enc, scopes,
                      groups_claim, allowed_email_domains, jit_provisioning, allow_email_linking,
                      role_sync_mode, enabled, created_at, updated_at
            "#,
            id,
            input.slug,
            input.display_name,
            input.issuer_url,
            input.client_id,
            set_secret,
            secret,
            input.scopes.as_deref(),
            input.groups_claim,
            input.allowed_email_domains.as_deref(),
            input.jit_provisioning,
            input.allow_email_linking,
            input.role_sync_mode,
            input.enabled
        )
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| map_error(Some(id), e))
    }
}

#[async_trait::async_trait]
impl SsoProviderRepository for SsoProviderRepositoryImpl {
    async fn list_providers(&self) -> Result<Vec<SsoProvider>, Error> {
        sqlx::query_as!(
            SsoProvider,
            r#"
            SELECT id, slug, display_name, issuer_url, client_id, client_secret_enc, scopes,
                   groups_claim, allowed_email_domains, jit_provisioning, allow_email_linking,
                   role_sync_mode, enabled, created_at, updated_at
            FROM sso_providers ORDER BY slug
            "#
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| map_error(None, e))
    }

    async fn list_enabled_providers(&self) -> Result<Vec<SsoProvider>, Error> {
        sqlx::query_as!(
            SsoProvider,
            r#"
            SELECT id, slug, display_name, issuer_url, client_id, client_secret_enc, scopes,
                   groups_claim, allowed_email_domains, jit_provisioning, allow_email_linking,
                   role_sync_mode, enabled, created_at, updated_at
            FROM sso_providers WHERE enabled ORDER BY display_name, slug
            "#
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| map_error(None, e))
    }

    async fn get_provider_by_id(&self, id: Uuid) -> Result<SsoProvider, Error> {
        let mut conn = self.pool.acquire().await.map_err(Error::DatabaseError)?;
        Self::get_by_id(&mut conn, id).await
    }

    async fn find_provider_by_slug(&self, slug: &str) -> Result<Option<SsoProvider>, Error> {
        sqlx::query_as!(
            SsoProvider,
            r#"
            SELECT id, slug, display_name, issuer_url, client_id, client_secret_enc, scopes,
                   groups_claim, allowed_email_domains, jit_provisioning, allow_email_linking,
                   role_sync_mode, enabled, created_at, updated_at
            FROM sso_providers WHERE slug = $1
            "#,
            slug
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| map_error(None, e))
    }

    fn clone_box(&self) -> Box<dyn SsoProviderRepository> {
        Box::new(self.clone())
    }
}

#[async_trait::async_trait]
impl SsoProviderRepositoryTx for SsoProviderRepositoryImpl {
    async fn get_provider_by_id_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
    ) -> Result<SsoProvider, Error> {
        Self::get_by_id(conn, id).await
    }

    async fn create_provider_tx(
        &self,
        conn: &mut PgConnection,
        input: CreateSsoProvider,
    ) -> Result<SsoProvider, Error> {
        Self::insert(conn, input).await
    }

    async fn update_provider_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        input: UpdateSsoProvider,
    ) -> Result<SsoProvider, Error> {
        Self::update(conn, id, input).await
    }

    async fn delete_provider_tx(&self, conn: &mut PgConnection, id: Uuid) -> Result<(), Error> {
        let result = sqlx::query!("DELETE FROM sso_providers WHERE id = $1", id)
            .execute(&mut *conn)
            .await
            .map_err(|e| map_error(Some(id), e))?;
        if result.rows_affected() == 0 {
            return Err(Error::NotFound(id));
        }
        Ok(())
    }
}
