//! Persistence for links between features and external issues (`feature_external_links`).

use mockall::automock;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::Error;
use crate::database::entity::ExternalLinkRow;
use crate::database::handle_error;

const LINK_COLUMNS: &str = "id, feature_id, system, external_key, url, created_by, created_at";
const UNIQUE_CONSTRAINT: &str = "feature_external_links_unique";

#[derive(Debug, Clone)]
pub struct CreateExternalLink {
    pub feature_id: Uuid,
    pub system: String,
    pub external_key: String,
    pub url: Option<String>,
    pub created_by: Option<Uuid>,
}

/// The feature a link belongs to: its team and key.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct FeatureScope {
    pub team_id: Uuid,
    pub key: String,
}

#[automock]
#[async_trait::async_trait]
pub trait ExternalLinkRepository: Send + Sync {
    /// Team and key of the feature, `None` when the feature does not exist.
    async fn feature_scope(&self, feature_id: Uuid) -> Result<Option<FeatureScope>, Error>;
    /// Links of the feature, oldest first.
    async fn list_for_feature(&self, feature_id: Uuid) -> Result<Vec<ExternalLinkRow>, Error>;
    /// `Error::RecordAlreadyExists` when the feature already links the key.
    async fn create(&self, input: CreateExternalLink) -> Result<ExternalLinkRow, Error>;
    /// False when the link does not exist or belongs to another feature.
    async fn delete(&self, feature_id: Uuid, link_id: Uuid) -> Result<bool, Error>;
    /// Features of `team_id` that link `system`/`external_key`.
    async fn feature_ids_for_key(
        &self,
        team_id: Uuid,
        system: &str,
        external_key: &str,
    ) -> Result<Vec<Uuid>, Error>;
    fn clone_box(&self) -> Box<dyn ExternalLinkRepository>;
}

impl Clone for Box<dyn ExternalLinkRepository> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

#[async_trait::async_trait]
pub trait ExternalLinkRepositoryTx: ExternalLinkRepository {
    async fn feature_scope_tx(
        &self,
        conn: &mut PgConnection,
        feature_id: Uuid,
    ) -> Result<Option<FeatureScope>, Error>;
    async fn create_tx(
        &self,
        conn: &mut PgConnection,
        input: CreateExternalLink,
    ) -> Result<ExternalLinkRow, Error>;
    /// The deleted row, `None` when the link does not exist or belongs to another feature.
    async fn delete_tx(
        &self,
        conn: &mut PgConnection,
        feature_id: Uuid,
        link_id: Uuid,
    ) -> Result<Option<ExternalLinkRow>, Error>;
}

pub fn external_link_repository(pool: PgPool) -> Box<dyn ExternalLinkRepository> {
    Box::new(ExternalLinkRepositoryImpl { pool })
}

pub fn external_link_repository_tx(pool: PgPool) -> ExternalLinkRepositoryImpl {
    ExternalLinkRepositoryImpl { pool }
}

#[derive(Clone)]
pub struct ExternalLinkRepositoryImpl {
    pool: PgPool,
}

impl ExternalLinkRepositoryImpl {
    async fn feature_scope_conn(
        conn: &mut PgConnection,
        feature_id: Uuid,
    ) -> Result<Option<FeatureScope>, Error> {
        let result = sqlx::query_as::<_, FeatureScope>(
            "SELECT team_id, key::TEXT AS key FROM features WHERE id = $1",
        )
        .bind(feature_id)
        .fetch_optional(&mut *conn)
        .await;
        handle_error(None, result)
    }

    async fn create_conn(
        conn: &mut PgConnection,
        input: CreateExternalLink,
    ) -> Result<ExternalLinkRow, Error> {
        let result = sqlx::query_as::<_, ExternalLinkRow>(&format!(
            "INSERT INTO feature_external_links (id, feature_id, system, external_key, url, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             RETURNING {LINK_COLUMNS}"
        ))
        .bind(Uuid::new_v4())
        .bind(input.feature_id)
        .bind(&input.system)
        .bind(&input.external_key)
        .bind(&input.url)
        .bind(input.created_by)
        .fetch_one(&mut *conn)
        .await;
        if let Err(sqlx::Error::Database(db_err)) = &result
            && db_err.constraint() == Some(UNIQUE_CONSTRAINT)
        {
            return Err(Error::RecordAlreadyExists(format!(
                "external link {} {}",
                input.system, input.external_key
            )));
        }
        handle_error(None, result)
    }

    async fn delete_conn(
        conn: &mut PgConnection,
        feature_id: Uuid,
        link_id: Uuid,
    ) -> Result<Option<ExternalLinkRow>, Error> {
        let result = sqlx::query_as::<_, ExternalLinkRow>(&format!(
            "DELETE FROM feature_external_links WHERE id = $1 AND feature_id = $2 \
             RETURNING {LINK_COLUMNS}"
        ))
        .bind(link_id)
        .bind(feature_id)
        .fetch_optional(&mut *conn)
        .await;
        handle_error(None, result)
    }
}

#[async_trait::async_trait]
impl ExternalLinkRepository for ExternalLinkRepositoryImpl {
    async fn feature_scope(&self, feature_id: Uuid) -> Result<Option<FeatureScope>, Error> {
        let mut conn = self.pool.acquire().await.map_err(Error::DatabaseError)?;
        Self::feature_scope_conn(&mut conn, feature_id).await
    }

    async fn list_for_feature(&self, feature_id: Uuid) -> Result<Vec<ExternalLinkRow>, Error> {
        let result = sqlx::query_as::<_, ExternalLinkRow>(&format!(
            "SELECT {LINK_COLUMNS} FROM feature_external_links \
             WHERE feature_id = $1 ORDER BY created_at, id"
        ))
        .bind(feature_id)
        .fetch_all(&self.pool)
        .await;
        handle_error(None, result)
    }

    async fn create(&self, input: CreateExternalLink) -> Result<ExternalLinkRow, Error> {
        let mut conn = self.pool.acquire().await.map_err(Error::DatabaseError)?;
        Self::create_conn(&mut conn, input).await
    }

    async fn delete(&self, feature_id: Uuid, link_id: Uuid) -> Result<bool, Error> {
        let mut conn = self.pool.acquire().await.map_err(Error::DatabaseError)?;
        Ok(Self::delete_conn(&mut conn, feature_id, link_id)
            .await?
            .is_some())
    }

    async fn feature_ids_for_key(
        &self,
        team_id: Uuid,
        system: &str,
        external_key: &str,
    ) -> Result<Vec<Uuid>, Error> {
        let result = sqlx::query_scalar::<_, Uuid>(
            "SELECT DISTINCT l.feature_id FROM feature_external_links l \
             JOIN features f ON f.id = l.feature_id \
             WHERE f.team_id = $1 AND l.system = $2 AND l.external_key = $3",
        )
        .bind(team_id)
        .bind(system)
        .bind(external_key)
        .fetch_all(&self.pool)
        .await;
        handle_error(None, result)
    }

    fn clone_box(&self) -> Box<dyn ExternalLinkRepository> {
        Box::new(self.clone())
    }
}

#[async_trait::async_trait]
impl ExternalLinkRepositoryTx for ExternalLinkRepositoryImpl {
    async fn feature_scope_tx(
        &self,
        conn: &mut PgConnection,
        feature_id: Uuid,
    ) -> Result<Option<FeatureScope>, Error> {
        Self::feature_scope_conn(conn, feature_id).await
    }

    async fn create_tx(
        &self,
        conn: &mut PgConnection,
        input: CreateExternalLink,
    ) -> Result<ExternalLinkRow, Error> {
        Self::create_conn(conn, input).await
    }

    async fn delete_tx(
        &self,
        conn: &mut PgConnection,
        feature_id: Uuid,
        link_id: Uuid,
    ) -> Result<Option<ExternalLinkRow>, Error> {
        Self::delete_conn(conn, feature_id, link_id).await
    }
}
