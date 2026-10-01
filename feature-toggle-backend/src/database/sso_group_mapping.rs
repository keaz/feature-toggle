//! Persistence for IdP group to role/team/admin mappings.

use crate::Error;
use crate::database::entity::SsoGroupMapping;
use mockall::automock;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSsoGroupMapping {
    pub group_value: String,
    /// `role`, `team` or `admin`.
    pub target_type: String,
    /// `None` exactly when `target_type` is `admin`.
    pub target_id: Option<Uuid>,
}

#[automock]
#[async_trait::async_trait]
pub trait SsoGroupMappingRepository: Send + Sync {
    async fn list_mappings(&self, provider_id: Uuid) -> Result<Vec<SsoGroupMapping>, Error>;
    fn clone_box(&self) -> Box<dyn SsoGroupMappingRepository>;
}

impl Clone for Box<dyn SsoGroupMappingRepository> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

#[async_trait::async_trait]
pub trait SsoGroupMappingRepositoryTx: SsoGroupMappingRepository {
    async fn list_mappings_tx(
        &self,
        conn: &mut PgConnection,
        provider_id: Uuid,
    ) -> Result<Vec<SsoGroupMapping>, Error>;
    /// Replaces every mapping of the provider with `mappings`. Duplicate entries
    /// collapse to one row.
    async fn replace_mappings_tx(
        &self,
        conn: &mut PgConnection,
        provider_id: Uuid,
        mappings: Vec<NewSsoGroupMapping>,
    ) -> Result<Vec<SsoGroupMapping>, Error>;
}

pub fn sso_group_mapping_repository(pool: PgPool) -> Box<dyn SsoGroupMappingRepository> {
    Box::new(SsoGroupMappingRepositoryImpl { pool })
}

pub fn sso_group_mapping_repository_tx(pool: PgPool) -> SsoGroupMappingRepositoryImpl {
    SsoGroupMappingRepositoryImpl { pool }
}

#[derive(Clone)]
pub struct SsoGroupMappingRepositoryImpl {
    pool: PgPool,
}

impl SsoGroupMappingRepositoryImpl {
    async fn list(
        conn: &mut PgConnection,
        provider_id: Uuid,
    ) -> Result<Vec<SsoGroupMapping>, Error> {
        sqlx::query_as!(
            SsoGroupMapping,
            r#"
            SELECT id, provider_id, group_value, target_type, target_id
            FROM sso_group_mappings
            WHERE provider_id = $1
            ORDER BY group_value, target_type, target_id
            "#,
            provider_id
        )
        .fetch_all(&mut *conn)
        .await
        .map_err(Error::DatabaseError)
    }
}

#[async_trait::async_trait]
impl SsoGroupMappingRepository for SsoGroupMappingRepositoryImpl {
    async fn list_mappings(&self, provider_id: Uuid) -> Result<Vec<SsoGroupMapping>, Error> {
        let mut conn = self.pool.acquire().await.map_err(Error::DatabaseError)?;
        Self::list(&mut conn, provider_id).await
    }

    fn clone_box(&self) -> Box<dyn SsoGroupMappingRepository> {
        Box::new(self.clone())
    }
}

#[async_trait::async_trait]
impl SsoGroupMappingRepositoryTx for SsoGroupMappingRepositoryImpl {
    async fn list_mappings_tx(
        &self,
        conn: &mut PgConnection,
        provider_id: Uuid,
    ) -> Result<Vec<SsoGroupMapping>, Error> {
        Self::list(conn, provider_id).await
    }

    async fn replace_mappings_tx(
        &self,
        conn: &mut PgConnection,
        provider_id: Uuid,
        mappings: Vec<NewSsoGroupMapping>,
    ) -> Result<Vec<SsoGroupMapping>, Error> {
        sqlx::query!(
            "DELETE FROM sso_group_mappings WHERE provider_id = $1",
            provider_id
        )
        .execute(&mut *conn)
        .await
        .map_err(Error::DatabaseError)?;

        for mapping in mappings {
            // ON CONFLICT (no target) covers both the table UNIQUE constraint and the
            // partial unique index that guards NULL target ids.
            sqlx::query!(
                r#"
                INSERT INTO sso_group_mappings (id, provider_id, group_value, target_type, target_id)
                VALUES ($1, $2, $3, $4, $5)
                ON CONFLICT DO NOTHING
                "#,
                Uuid::new_v4(),
                provider_id,
                mapping.group_value,
                mapping.target_type,
                mapping.target_id
            )
            .execute(&mut *conn)
            .await
            .map_err(|e| match &e {
                sqlx::Error::Database(db) if db.code().as_deref() == Some("23514") => {
                    Error::InvalidInput(
                        "Mapping target is inconsistent: admin mappings take no target id, role and team mappings require one"
                            .to_string(),
                    )
                }
                _ => Error::DatabaseError(e),
            })?;
        }

        Self::list(conn, provider_id).await
    }
}
