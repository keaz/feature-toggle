//! Transaction management utilities for database operations.
//!
//! This module provides types and utilities for managing database transactions
//! across multiple repository operations, ensuring atomicity of GraphQL mutations.

use sqlx::{PgPool, Postgres, Transaction};

/// A type alias for a database transaction.
pub type DbTransaction<'a> = Transaction<'a, Postgres>;

/// Transaction manager for creating and managing database transactions.
///
/// This is injected into the GraphQL context and used by mutations
/// to ensure all repository operations within a single mutation
/// share the same transaction context.
#[derive(Clone)]
pub struct TransactionManager {
    pool: PgPool,
}

impl TransactionManager {
    /// Create a new transaction manager with the given connection pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Begin a new database transaction.
    ///
    /// The returned transaction should be passed to repository methods
    /// and committed after all operations succeed, or rolled back on error.
    ///
    /// # Example
    /// ```ignore
    /// let mut tx = transaction_manager.begin().await?;
    /// repository.create_team_tx(&mut tx, input).await?;
    /// activity_log.create_activity_tx(&mut tx, activity).await?;
    /// tx.commit().await?;
    /// ```
    pub async fn begin(&self) -> Result<DbTransaction<'_>, sqlx::Error> {
        self.pool.begin().await
    }

    /// Get a reference to the underlying connection pool.
    ///
    /// This can be used for read-only operations that don't need
    /// transaction semantics.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}
