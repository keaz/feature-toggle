use crate::database::entity::VariantAllocation;
use crate::database::{Error, handle_error};
use log::{debug, info};
use mockall::automock;
use sqlx::PgPool;
use uuid::Uuid;

/// Input for creating a variant allocation
#[derive(Debug, Clone)]
pub struct CreateVariantAllocationInput {
    pub criteria_id: Uuid,
    pub variant_control: String,
    pub weight: i32,
}

/// Input for updating a variant allocation
#[derive(Debug, Clone)]
pub struct UpdateVariantAllocationInput {
    pub weight: i32,
}

/// Repository for managing variant allocations (weighted traffic splits)
#[automock]
#[async_trait::async_trait]
pub trait VariantAllocationsRepository: Send + Sync {
    /// Get all variant allocations for a specific criterion
    async fn get_allocations_by_criteria(
        &self,
        criteria_id: Uuid,
    ) -> Result<Vec<VariantAllocation>, Error>;

    /// Get a single variant allocation by ID
    async fn get_allocation_by_id(&self, allocation_id: Uuid) -> Result<VariantAllocation, Error>;

    /// Create a new variant allocation
    async fn create_allocation(
        &self,
        input: CreateVariantAllocationInput,
    ) -> Result<VariantAllocation, Error>;

    /// Update an existing variant allocation
    async fn update_allocation(
        &self,
        allocation_id: Uuid,
        input: UpdateVariantAllocationInput,
    ) -> Result<VariantAllocation, Error>;

    /// Delete a variant allocation
    async fn delete_allocation(&self, allocation_id: Uuid) -> Result<(), Error>;

    /// Delete all variant allocations for a criterion
    async fn delete_allocations_by_criteria(&self, criteria_id: Uuid) -> Result<(), Error>;

    /// Set all variant allocations for a criterion (replaces existing)
    /// This ensures atomic update of all allocations
    async fn set_allocations(
        &self,
        criteria_id: Uuid,
        allocations: Vec<CreateVariantAllocationInput>,
    ) -> Result<Vec<VariantAllocation>, Error>;

    fn clone_box(&self) -> Box<dyn VariantAllocationsRepository>;
}

impl Clone for Box<dyn VariantAllocationsRepository> {
    fn clone(&self) -> Box<dyn VariantAllocationsRepository> {
        self.clone_box()
    }
}

/// Factory function to create a variant allocations repository
pub fn variant_allocations_repository(pool: PgPool) -> Box<dyn VariantAllocationsRepository> {
    Box::new(VariantAllocationsRepositoryImpl::new(pool))
}

#[derive(Clone)]
pub struct VariantAllocationsRepositoryImpl {
    pool: PgPool,
}

impl VariantAllocationsRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl VariantAllocationsRepository for VariantAllocationsRepositoryImpl {
    async fn get_allocations_by_criteria(
        &self,
        criteria_id: Uuid,
    ) -> Result<Vec<VariantAllocation>, Error> {
        debug!(
            "DB: get_allocations_by_criteria criteria_id={}",
            criteria_id
        );

        let allocations = sqlx::query_as!(
            VariantAllocation,
            r#"SELECT id, criteria_id, variant_control, weight,
                      created_at as "created_at!", updated_at as "updated_at!"
               FROM variant_allocations
               WHERE criteria_id = $1
               ORDER BY variant_control"#,
            criteria_id
        )
        .fetch_all(&self.pool)
        .await;

        handle_error(None, allocations)
    }

    async fn get_allocation_by_id(&self, allocation_id: Uuid) -> Result<VariantAllocation, Error> {
        debug!("DB: get_allocation_by_id allocation_id={}", allocation_id);

        let allocation = sqlx::query_as!(
            VariantAllocation,
            r#"SELECT id, criteria_id, variant_control, weight,
                      created_at as "created_at!", updated_at as "updated_at!"
               FROM variant_allocations
               WHERE id = $1"#,
            allocation_id
        )
        .fetch_one(&self.pool)
        .await;

        handle_error(Some(allocation_id), allocation)
    }

    async fn create_allocation(
        &self,
        input: CreateVariantAllocationInput,
    ) -> Result<VariantAllocation, Error> {
        info!(
            "DB: create_allocation criteria_id={} variant={} weight={}",
            input.criteria_id, input.variant_control, input.weight
        );

        let allocation_id = Uuid::new_v4();

        let result = sqlx::query_as!(
            VariantAllocation,
            r#"INSERT INTO variant_allocations (id, criteria_id, variant_control, weight)
               VALUES ($1, $2, $3, $4)
               RETURNING id, criteria_id, variant_control, weight,
                         created_at as "created_at!", updated_at as "updated_at!""#,
            allocation_id,
            input.criteria_id,
            input.variant_control,
            input.weight
        )
        .fetch_one(&self.pool)
        .await;

        handle_error(None, result)
    }

    async fn update_allocation(
        &self,
        allocation_id: Uuid,
        input: UpdateVariantAllocationInput,
    ) -> Result<VariantAllocation, Error> {
        info!(
            "DB: update_allocation allocation_id={} weight={}",
            allocation_id, input.weight
        );

        let result = sqlx::query_as!(
            VariantAllocation,
            r#"UPDATE variant_allocations
               SET weight = $1,
               updated_at = CURRENT_TIMESTAMP
               WHERE id = $2
               RETURNING id, criteria_id, variant_control, weight,
                         created_at as "created_at!", updated_at as "updated_at!""#,
            input.weight,
            allocation_id
        )
        .fetch_one(&self.pool)
        .await;

        handle_error(Some(allocation_id), result)
    }

    async fn delete_allocation(&self, allocation_id: Uuid) -> Result<(), Error> {
        info!("DB: delete_allocation allocation_id={}", allocation_id);

        let result = sqlx::query!(
            r#"DELETE FROM variant_allocations WHERE id = $1"#,
            allocation_id
        )
        .execute(&self.pool)
        .await;

        handle_error(Some(allocation_id), result)?;
        Ok(())
    }

    async fn delete_allocations_by_criteria(&self, criteria_id: Uuid) -> Result<(), Error> {
        info!(
            "DB: delete_allocations_by_criteria criteria_id={}",
            criteria_id
        );

        let result = sqlx::query!(
            r#"DELETE FROM variant_allocations WHERE criteria_id = $1"#,
            criteria_id
        )
        .execute(&self.pool)
        .await;

        handle_error(None, result)?;
        Ok(())
    }

    async fn set_allocations(
        &self,
        criteria_id: Uuid,
        allocations: Vec<CreateVariantAllocationInput>,
    ) -> Result<Vec<VariantAllocation>, Error> {
        info!(
            "DB: set_allocations criteria_id={} count={}",
            criteria_id,
            allocations.len()
        );

        // Use a transaction to ensure atomicity
        let mut tx = self.pool.begin().await.map_err(Error::DatabaseError)?;

        // Delete all existing allocations for this criterion
        let _ = sqlx::query!(
            r#"DELETE FROM variant_allocations WHERE criteria_id = $1"#,
            criteria_id
        )
        .execute(&mut *tx)
        .await
        .map_err(Error::DatabaseError)?;

        // Insert new allocations
        let mut result_allocations = Vec::new();
        for alloc in allocations {
            let allocation_id = Uuid::new_v4();
            let allocation = sqlx::query_as!(
                VariantAllocation,
                r#"INSERT INTO variant_allocations (id, criteria_id, variant_control, weight)
                   VALUES ($1, $2, $3, $4)
                   RETURNING id, criteria_id, variant_control, weight,
                             created_at as "created_at!", updated_at as "updated_at!""#,
                allocation_id,
                criteria_id,
                alloc.variant_control,
                alloc.weight
            )
            .fetch_one(&mut *tx)
            .await
            .map_err(Error::DatabaseError)?;

            result_allocations.push(allocation);
        }

        // Commit transaction
        tx.commit().await.map_err(Error::DatabaseError)?;

        Ok(result_allocations)
    }

    fn clone_box(&self) -> Box<dyn VariantAllocationsRepository> {
        Box::new(self.clone())
    }
}

pub fn variant_allocations_repository_tx(pool: PgPool) -> VariantAllocationsRepositoryImpl {
    VariantAllocationsRepositoryImpl::new(pool)
}

#[async_trait::async_trait]
pub trait VariantAllocationsRepositoryTx: VariantAllocationsRepository {
    async fn set_allocations_tx(
        &self,
        conn: &mut sqlx::PgConnection,
        criteria_id: Uuid,
        allocations: Vec<CreateVariantAllocationInput>,
    ) -> Result<Vec<VariantAllocation>, Error>;
}

#[async_trait::async_trait]
impl VariantAllocationsRepositoryTx for VariantAllocationsRepositoryImpl {
    async fn set_allocations_tx(
        &self,
        conn: &mut sqlx::PgConnection,
        criteria_id: Uuid,
        allocations: Vec<CreateVariantAllocationInput>,
    ) -> Result<Vec<VariantAllocation>, Error> {
        // Delete all existing allocations for this criterion
        let _ = sqlx::query!(
            r#"DELETE FROM variant_allocations WHERE criteria_id = $1"#,
            criteria_id
        )
        .execute(&mut *conn)
        .await
        .map_err(Error::DatabaseError)?;

        // Insert new allocations
        let mut result_allocations = Vec::new();
        for alloc in allocations {
            let allocation_id = Uuid::new_v4();
            let allocation = sqlx::query_as!(
                VariantAllocation,
                r#"INSERT INTO variant_allocations (id, criteria_id, variant_control, weight)
                   VALUES ($1, $2, $3, $4)
                   RETURNING id, criteria_id, variant_control, weight,
                             created_at as "created_at!", updated_at as "updated_at!""#,
                allocation_id,
                criteria_id,
                alloc.variant_control,
                alloc.weight
            )
            .fetch_one(&mut *conn)
            .await
            .map_err(Error::DatabaseError)?;

            result_allocations.push(allocation);
        }

        Ok(result_allocations)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_variant_allocation_input() {
        let input = CreateVariantAllocationInput {
            criteria_id: Uuid::new_v4(),
            variant_control: "variant_a".to_string(),
            weight: 50,
        };

        assert_eq!(input.weight, 50);
        assert_eq!(input.variant_control, "variant_a");
    }

    #[test]
    fn test_update_variant_allocation_input() {
        let input = UpdateVariantAllocationInput { weight: 75 };

        assert_eq!(input.weight, 75);
    }
}
