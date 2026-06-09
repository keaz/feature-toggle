use crate::database::entity::{ApprovalPolicy, ApprovalRequest, ApprovalStatus, ApprovalVoteValue};
use crate::database::{Error, handle_error};
use chrono::{DateTime, Utc};
use mockall::automock;
use sqlx::query_builder::QueryBuilder;
use sqlx::{PgConnection, PgPool, Row};
use uuid::Uuid;

pub const DEFAULT_APPROVAL_PAGE_SIZE: i32 = 20;

pub struct CreateApprovalPolicyInput {
    pub team_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub applies_to: String,
    pub environment_ids: Option<Vec<Uuid>>,
    pub required_approvers: i32,
    pub approver_role_ids: Vec<Uuid>,
    pub approver_user_ids: Vec<Uuid>,
    pub allow_admin_override: bool,
    pub fallback_to_roles: bool,
    pub auto_approve_after_hours: Option<i32>,
    pub enabled: bool,
}

pub struct UpdateApprovalPolicyInput {
    pub name: Option<String>,
    pub description: Option<String>,
    pub applies_to: Option<String>,
    pub environment_ids: Option<Vec<Uuid>>,
    pub required_approvers: Option<i32>,
    pub approver_role_ids: Option<Vec<Uuid>>,
    pub approver_user_ids: Option<Vec<Uuid>>,
    pub allow_admin_override: Option<bool>,
    pub fallback_to_roles: Option<bool>,
    pub auto_approve_after_hours: Option<i32>,
    pub enabled: Option<bool>,
}

pub struct CreateApprovalRequestInput {
    pub policy_id: Uuid,
    pub feature_id: Uuid,
    pub environment_id: Option<Uuid>,
    pub change_type: String,
    pub change_payload: serde_json::Value,
    pub change_description: Option<String>,
    pub requested_by: Uuid,
    pub eligible_approver_ids: Vec<Uuid>,
    pub routing_reason: Option<String>,
    pub admin_override_enabled: bool,
}

pub struct CreateApprovalVoteInput {
    pub request_id: Uuid,
    pub approver_id: Uuid,
    pub vote: ApprovalVoteValue,
    pub comment: Option<String>,
}

#[automock]
#[async_trait::async_trait]
pub trait ApprovalRepository: Send + Sync {
    async fn create_policy(
        &self,
        input: CreateApprovalPolicyInput,
    ) -> Result<ApprovalPolicy, Error>;
    async fn update_policy(
        &self,
        id: Uuid,
        input: UpdateApprovalPolicyInput,
    ) -> Result<ApprovalPolicy, Error>;
    async fn delete_policy(&self, id: Uuid) -> Result<bool, Error>;
    async fn list_policies_for_team(&self, team_id: Uuid) -> Result<Vec<ApprovalPolicy>, Error>;
    async fn get_policy_by_id(&self, id: Uuid) -> Result<Option<ApprovalPolicy>, Error>;

    async fn create_request(
        &self,
        input: CreateApprovalRequestInput,
    ) -> Result<ApprovalRequest, Error>;
    async fn get_request_by_id(&self, id: Uuid) -> Result<Option<ApprovalRequest>, Error>;
    async fn add_vote(
        &self,
        input: CreateApprovalVoteInput,
        required_approvers: i32,
    ) -> Result<ApprovalRequest, Error>;
    async fn list_votes_for_request(
        &self,
        request_id: Uuid,
    ) -> Result<Vec<crate::database::entity::ApprovalVote>, Error>;
    async fn update_request_status(
        &self,
        request_id: Uuid,
        status: ApprovalStatus,
        executed_at: Option<DateTime<Utc>>,
    ) -> Result<ApprovalRequest, Error>;
    async fn cancel_request(
        &self,
        request_id: Uuid,
        cancelled_by: Uuid,
    ) -> Result<ApprovalRequest, Error>;
    async fn list_requests_for_team(
        &self,
        team_id: Option<Uuid>,
        statuses: Option<Vec<ApprovalStatus>>,
        page_number: Option<i32>,
        page_size: Option<i32>,
    ) -> Result<(Vec<ApprovalRequest>, i64), Error>;
    async fn list_requests_for_team_with_offset(
        &self,
        team_id: Option<Uuid>,
        statuses: Option<Vec<ApprovalStatus>>,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<ApprovalRequest>, i64), Error>;
    async fn list_requests_due_for_auto_approval(&self) -> Result<Vec<ApprovalRequest>, Error>;

    fn clone_box(&self) -> Box<dyn ApprovalRepository>;
}

impl Clone for Box<dyn ApprovalRepository> {
    fn clone(&self) -> Box<dyn ApprovalRepository> {
        self.clone_box()
    }
}

/// Extension trait for transaction-aware repository operations.
/// These methods accept a mutable connection reference for use within transactions.
#[async_trait::async_trait]
pub trait ApprovalRepositoryTx: ApprovalRepository {
    async fn create_policy_tx(
        &self,
        conn: &mut PgConnection,
        input: CreateApprovalPolicyInput,
    ) -> Result<ApprovalPolicy, Error>;
    async fn update_policy_tx(
        &self,
        conn: &mut PgConnection,
        policy_id: Uuid,
        input: UpdateApprovalPolicyInput,
    ) -> Result<ApprovalPolicy, Error>;
    async fn delete_policy_tx(
        &self,
        conn: &mut PgConnection,
        policy_id: Uuid,
    ) -> Result<bool, Error>;
    async fn create_request_tx(
        &self,
        conn: &mut PgConnection,
        input: CreateApprovalRequestInput,
    ) -> Result<ApprovalRequest, Error>;
    async fn update_request_status_tx(
        &self,
        conn: &mut PgConnection,
        request_id: Uuid,
        status: ApprovalStatus,
        executed_at: Option<DateTime<Utc>>,
    ) -> Result<ApprovalRequest, Error>;
    async fn add_vote_tx(
        &self,
        conn: &mut PgConnection,
        input: CreateApprovalVoteInput,
        required_approvers: i32,
    ) -> Result<ApprovalRequest, Error>;
}

pub fn approval_repository(pool: PgPool) -> Box<dyn ApprovalRepository> {
    Box::new(ApprovalRepositoryImpl::new(pool))
}

/// Returns a repository that also implements ApprovalRepositoryTx for transaction support.
pub fn approval_repository_tx(pool: PgPool) -> ApprovalRepositoryImpl {
    ApprovalRepositoryImpl::new(pool)
}

#[derive(Clone)]
pub struct ApprovalRepositoryImpl {
    pool: PgPool,
}

impl ApprovalRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn map_request_row(row: sqlx::postgres::PgRow) -> ApprovalRequest {
        let status: String = row.get("status");
        ApprovalRequest {
            id: row.get("id"),
            policy_id: row.get("policy_id"),
            feature_id: row.get("feature_id"),
            environment_id: row.get("environment_id"),
            change_type: row.get("change_type"),
            change_payload: row.get("change_payload"),
            change_description: row.get("change_description"),
            requested_by: row.get("requested_by"),
            eligible_approver_ids: row.get("eligible_approver_ids"),
            routing_reason: row.get("routing_reason"),
            admin_override_enabled: row.get("admin_override_enabled"),
            status: ApprovalStatus::from_str(&status),
            approved_count: row.get::<i32, _>("approved_count"),
            rejected_count: row.get::<i32, _>("rejected_count"),
            executed_at: row.get("executed_at"),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
        }
    }
}

#[async_trait::async_trait]
impl ApprovalRepository for ApprovalRepositoryImpl {
    async fn create_policy(
        &self,
        input: CreateApprovalPolicyInput,
    ) -> Result<ApprovalPolicy, Error> {
        let result = sqlx::query_as::<_, ApprovalPolicy>(
            r#"
            INSERT INTO approval_policies (
                team_id,
                name,
                description,
                applies_to,
                environment_ids,
                required_approvers,
                approver_role_ids,
                approver_user_ids,
                allow_admin_override,
                fallback_to_roles,
                auto_approve_after_hours,
                enabled
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
            RETURNING id, team_id, name, description, applies_to, environment_ids, required_approvers,
                      approver_role_ids, approver_user_ids, allow_admin_override, fallback_to_roles,
                      auto_approve_after_hours, enabled, created_at
            "#,
        )
        .bind(input.team_id)
        .bind(input.name)
        .bind(input.description)
        .bind(input.applies_to)
        .bind(input.environment_ids.as_deref())
        .bind(input.required_approvers)
        .bind(&input.approver_role_ids)
        .bind(&input.approver_user_ids)
        .bind(input.allow_admin_override)
        .bind(input.fallback_to_roles)
        .bind(input.auto_approve_after_hours)
        .bind(input.enabled)
        .fetch_one(&self.pool)
        .await;

        handle_error(None, result)
    }

    async fn list_policies_for_team(&self, team_id: Uuid) -> Result<Vec<ApprovalPolicy>, Error> {
        let result = sqlx::query_as::<_, ApprovalPolicy>(
            r#"
            SELECT id, team_id, name, description, applies_to, environment_ids, required_approvers,
                   approver_role_ids, approver_user_ids, allow_admin_override, fallback_to_roles,
                   auto_approve_after_hours, enabled, created_at
            FROM approval_policies
            WHERE team_id = $1
            ORDER BY created_at DESC
            "#,
        )
        .bind(team_id)
        .fetch_all(&self.pool)
        .await;

        handle_error(None, result)
    }

    async fn get_policy_by_id(&self, id: Uuid) -> Result<Option<ApprovalPolicy>, Error> {
        let result = sqlx::query_as::<_, ApprovalPolicy>(
            r#"
            SELECT id, team_id, name, description, applies_to, environment_ids, required_approvers,
                   approver_role_ids, approver_user_ids, allow_admin_override, fallback_to_roles,
                   auto_approve_after_hours, enabled, created_at
            FROM approval_policies
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await;

        handle_error(Some(id), result)
    }

    async fn update_policy(
        &self,
        id: Uuid,
        input: UpdateApprovalPolicyInput,
    ) -> Result<ApprovalPolicy, Error> {
        // First fetch the existing policy
        let existing = self
            .get_policy_by_id(id)
            .await?
            .ok_or(Error::NotFound(id))?;

        // Build update with optional fields
        let name = input.name.unwrap_or(existing.name);
        let description = input.description.or(existing.description);
        let applies_to = input.applies_to.unwrap_or(existing.applies_to);
        let environment_ids = input.environment_ids.or(existing.environment_ids);
        let required_approvers = input
            .required_approvers
            .unwrap_or(existing.required_approvers);
        let approver_role_ids = input
            .approver_role_ids
            .unwrap_or(existing.approver_role_ids);
        let approver_user_ids = input
            .approver_user_ids
            .unwrap_or(existing.approver_user_ids);
        let allow_admin_override = input
            .allow_admin_override
            .unwrap_or(existing.allow_admin_override);
        let fallback_to_roles = input
            .fallback_to_roles
            .unwrap_or(existing.fallback_to_roles);
        let auto_approve_after_hours = input
            .auto_approve_after_hours
            .or(existing.auto_approve_after_hours);
        let enabled = input.enabled.unwrap_or(existing.enabled);

        let result = sqlx::query_as::<_, ApprovalPolicy>(
            r#"
            UPDATE approval_policies
            SET name = $2,
                description = $3,
                applies_to = $4,
                environment_ids = $5,
                required_approvers = $6,
                approver_role_ids = $7,
                approver_user_ids = $8,
                allow_admin_override = $9,
                fallback_to_roles = $10,
                auto_approve_after_hours = $11,
                enabled = $12
            WHERE id = $1
            RETURNING id, team_id, name, description, applies_to, environment_ids, required_approvers,
                      approver_role_ids, approver_user_ids, allow_admin_override, fallback_to_roles,
                      auto_approve_after_hours, enabled, created_at
            "#,
        )
        .bind(id)
        .bind(name)
        .bind(description)
        .bind(applies_to)
        .bind(environment_ids.as_deref())
        .bind(required_approvers)
        .bind(&approver_role_ids)
        .bind(&approver_user_ids)
        .bind(allow_admin_override)
        .bind(fallback_to_roles)
        .bind(auto_approve_after_hours)
        .bind(enabled)
        .fetch_one(&self.pool)
        .await;

        handle_error(Some(id), result)
    }

    async fn delete_policy(&self, id: Uuid) -> Result<bool, Error> {
        let result = sqlx::query(
            r#"
            DELETE FROM approval_policies WHERE id = $1
            "#,
        )
        .bind(id)
        .execute(&self.pool)
        .await;

        match result {
            Ok(result) => Ok(result.rows_affected() > 0),
            Err(e) => Err(Error::DatabaseError(e)),
        }
    }

    async fn create_request(
        &self,
        input: CreateApprovalRequestInput,
    ) -> Result<ApprovalRequest, Error> {
        let result = sqlx::query(
            r#"
            INSERT INTO approval_requests (
                policy_id, feature_id, environment_id, change_type, change_payload,
                change_description, requested_by, eligible_approver_ids, routing_reason,
                admin_override_enabled
            )
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
            RETURNING id, policy_id, feature_id, environment_id, change_type, change_payload,
                      change_description, requested_by, eligible_approver_ids, routing_reason,
                      admin_override_enabled, status, approved_count, rejected_count, executed_at,
                      created_at, updated_at
            "#,
        )
        .bind(input.policy_id)
        .bind(input.feature_id)
        .bind(input.environment_id)
        .bind(input.change_type)
        .bind(input.change_payload)
        .bind(input.change_description)
        .bind(input.requested_by)
        .bind(&input.eligible_approver_ids)
        .bind(input.routing_reason)
        .bind(input.admin_override_enabled)
        .map(Self::map_request_row)
        .fetch_one(&self.pool)
        .await;

        handle_error(None, result)
    }

    async fn get_request_by_id(&self, id: Uuid) -> Result<Option<ApprovalRequest>, Error> {
        let result = sqlx::query(
            r#"
            SELECT id, policy_id, feature_id, environment_id, change_type, change_payload,
                   change_description, requested_by, eligible_approver_ids, routing_reason,
                   admin_override_enabled, status, approved_count, rejected_count, executed_at,
                   created_at, updated_at
            FROM approval_requests WHERE id = $1
            "#,
        )
        .bind(id)
        .map(Self::map_request_row)
        .fetch_optional(&self.pool)
        .await;

        handle_error(Some(id), result)
    }

    async fn add_vote(
        &self,
        input: CreateApprovalVoteInput,
        required_approvers: i32,
    ) -> Result<ApprovalRequest, Error> {
        let mut tx = self.pool.begin().await.map_err(Error::DatabaseError)?;

        // Record vote (unique constraint enforces one per approver)
        if let Err(e) = sqlx::query!(
            r#"
            INSERT INTO approval_votes (request_id, approver_id, vote, comment)
            VALUES ($1, $2, $3, $4)
            "#,
            input.request_id,
            input.approver_id,
            input.vote.as_str(),
            input.comment
        )
        .execute(&mut *tx)
        .await
        {
            // Map duplicate vote to a friendlier error
            if let sqlx::Error::Database(db_err) = &e
                && db_err.code().map(|c| c == "23505").unwrap_or(false)
            {
                tx.rollback().await.ok();
                return Err(Error::RecordAlreadyExists("vote".into()));
            }
            tx.rollback().await.ok();
            return Err(Error::DatabaseError(e));
        }

        // Update counts and status based on vote
        let result = sqlx::query(
            r#"
            UPDATE approval_requests
            SET approved_count = approved_count + CASE WHEN $2 = 'approve' THEN 1 ELSE 0 END,
                rejected_count = rejected_count + CASE WHEN $2 = 'reject' THEN 1 ELSE 0 END,
                status = CASE
                    WHEN status = 'cancelled' THEN status
                    WHEN $2 = 'reject' THEN 'rejected'
                    WHEN approved_count + CASE WHEN $2 = 'approve' THEN 1 ELSE 0 END >= $3 THEN 'approved'
                    ELSE status
                END,
                updated_at = NOW()
            WHERE id = $1
            RETURNING id, policy_id, feature_id, environment_id, change_type, change_payload,
                      change_description, requested_by, eligible_approver_ids, routing_reason,
                      admin_override_enabled, status, approved_count, rejected_count, executed_at,
                      created_at, updated_at
            "#,
        )
        .bind(input.request_id)
        .bind(input.vote.as_str())
        .bind(required_approvers)
        .map(Self::map_request_row)
        .fetch_one(&mut *tx)
        .await;

        let updated = handle_error(Some(input.request_id), result)?;
        tx.commit().await.map_err(Error::DatabaseError)?;
        Ok(updated)
    }

    async fn update_request_status(
        &self,
        request_id: Uuid,
        status: ApprovalStatus,
        executed_at: Option<DateTime<Utc>>,
    ) -> Result<ApprovalRequest, Error> {
        let result = sqlx::query(
            r#"
            UPDATE approval_requests
            SET status = $2,
                executed_at = COALESCE($3, executed_at),
                updated_at = NOW()
            WHERE id = $1
            RETURNING id, policy_id, feature_id, environment_id, change_type, change_payload,
                      change_description, requested_by, eligible_approver_ids, routing_reason,
                      admin_override_enabled, status, approved_count, rejected_count, executed_at,
                      created_at, updated_at
            "#,
        )
        .bind(request_id)
        .bind(status.as_str())
        .bind(executed_at)
        .map(Self::map_request_row)
        .fetch_one(&self.pool)
        .await;

        handle_error(Some(request_id), result)
    }

    async fn list_votes_for_request(
        &self,
        request_id: Uuid,
    ) -> Result<Vec<crate::database::entity::ApprovalVote>, Error> {
        let result = sqlx::query_as::<_, crate::database::entity::ApprovalVote>(
            r#"
            SELECT id, request_id, approver_id, vote, comment, created_at
            FROM approval_votes
            WHERE request_id = $1
            ORDER BY created_at ASC
            "#,
        )
        .bind(request_id)
        .fetch_all(&self.pool)
        .await;

        handle_error(Some(request_id), result)
    }

    async fn list_requests_for_team(
        &self,
        team_id: Option<Uuid>,
        statuses: Option<Vec<ApprovalStatus>>,
        page_number: Option<i32>,
        page_size: Option<i32>,
    ) -> Result<(Vec<ApprovalRequest>, i64), Error> {
        let page_num = page_number.unwrap_or(1).max(1);
        let page_sz = page_size.unwrap_or(DEFAULT_APPROVAL_PAGE_SIZE).max(1);
        let offset = (page_num - 1) as i64 * page_sz as i64;

        let status_values = statuses.and_then(|list| {
            let converted: Vec<String> = list
                .into_iter()
                .map(|status| status.as_str().to_string())
                .collect();
            if converted.is_empty() {
                None
            } else {
                Some(converted)
            }
        });

        let mut select_builder = QueryBuilder::new(
            "SELECT r.* FROM approval_requests r
             JOIN approval_policies p ON p.id = r.policy_id",
        );
        let mut count_builder = QueryBuilder::new(
            "SELECT COUNT(*) FROM approval_requests r
             JOIN approval_policies p ON p.id = r.policy_id",
        );

        let mut select_has_where = false;
        let mut count_has_where = false;

        if let Some(team_id) = team_id {
            if !select_has_where {
                select_builder.push(" WHERE ");
                select_has_where = true;
            } else {
                select_builder.push(" AND ");
            }
            select_builder.push("p.team_id = ").push_bind(team_id);

            if !count_has_where {
                count_builder.push(" WHERE ");
                count_has_where = true;
            } else {
                count_builder.push(" AND ");
            }
            count_builder.push("p.team_id = ").push_bind(team_id);
        }

        if let Some(values) = status_values.clone() {
            if !select_has_where {
                select_builder.push(" WHERE ");
            } else {
                select_builder.push(" AND ");
            }
            select_builder
                .push("r.status = ANY(")
                .push_bind(values.clone())
                .push(")");

            if !count_has_where {
                count_builder.push(" WHERE ");
            } else {
                count_builder.push(" AND ");
            }
            count_builder
                .push("r.status = ANY(")
                .push_bind(values)
                .push(")");
        }

        select_builder.push(" ORDER BY r.created_at DESC");
        select_builder.push(" LIMIT ").push_bind(page_sz);
        select_builder.push(" OFFSET ").push_bind(offset);

        let items = handle_error(
            None,
            select_builder
                .build()
                .map(Self::map_request_row)
                .fetch_all(&self.pool)
                .await,
        )?;

        let count_row = handle_error(None, count_builder.build().fetch_one(&self.pool).await)?;
        let total: i64 = count_row.get::<i64, _>(0);

        Ok((items, total))
    }

    async fn list_requests_for_team_with_offset(
        &self,
        team_id: Option<Uuid>,
        statuses: Option<Vec<ApprovalStatus>>,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<ApprovalRequest>, i64), Error> {
        let offset = offset.max(0);
        let limit = limit.max(1);

        let status_values = statuses.and_then(|list| {
            let converted: Vec<String> = list
                .into_iter()
                .map(|status| status.as_str().to_string())
                .collect();
            if converted.is_empty() {
                None
            } else {
                Some(converted)
            }
        });

        let mut select_builder = QueryBuilder::new(
            "SELECT r.* FROM approval_requests r
             JOIN approval_policies p ON p.id = r.policy_id",
        );
        let mut count_builder = QueryBuilder::new(
            "SELECT COUNT(*) FROM approval_requests r
             JOIN approval_policies p ON p.id = r.policy_id",
        );

        let mut select_has_where = false;
        let mut count_has_where = false;

        if let Some(team_id) = team_id {
            if !select_has_where {
                select_builder.push(" WHERE ");
                select_has_where = true;
            } else {
                select_builder.push(" AND ");
            }
            select_builder.push("p.team_id = ").push_bind(team_id);

            if !count_has_where {
                count_builder.push(" WHERE ");
                count_has_where = true;
            } else {
                count_builder.push(" AND ");
            }
            count_builder.push("p.team_id = ").push_bind(team_id);
        }

        if let Some(values) = status_values.clone() {
            if !select_has_where {
                select_builder.push(" WHERE ");
            } else {
                select_builder.push(" AND ");
            }
            select_builder
                .push("r.status = ANY(")
                .push_bind(values.clone())
                .push(")");

            if !count_has_where {
                count_builder.push(" WHERE ");
            } else {
                count_builder.push(" AND ");
            }
            count_builder
                .push("r.status = ANY(")
                .push_bind(values)
                .push(")");
        }

        select_builder.push(" ORDER BY r.created_at DESC");
        select_builder.push(" LIMIT ").push_bind(limit);
        select_builder.push(" OFFSET ").push_bind(offset);

        let items = handle_error(
            None,
            select_builder
                .build()
                .map(Self::map_request_row)
                .fetch_all(&self.pool)
                .await,
        )?;

        let count_row = handle_error(None, count_builder.build().fetch_one(&self.pool).await)?;
        let total: i64 = count_row.get::<i64, _>(0);

        Ok((items, total))
    }

    async fn list_requests_due_for_auto_approval(&self) -> Result<Vec<ApprovalRequest>, Error> {
        handle_error(
            None,
            sqlx::query(
                r#"
                SELECT r.*
                FROM approval_requests r
                JOIN approval_policies p ON p.id = r.policy_id
                WHERE r.status = 'pending'
                  AND p.auto_approve_after_hours IS NOT NULL
                  AND r.created_at + make_interval(hours => p.auto_approve_after_hours) <= NOW()
                ORDER BY r.created_at ASC
                "#,
            )
            .map(Self::map_request_row)
            .fetch_all(&self.pool)
            .await,
        )
    }

    async fn cancel_request(
        &self,
        request_id: Uuid,
        _cancelled_by: Uuid,
    ) -> Result<ApprovalRequest, Error> {
        // We track cancelled_by in audits later; for now just flip status.
        self.update_request_status(request_id, ApprovalStatus::Cancelled, None)
            .await
    }

    fn clone_box(&self) -> Box<dyn ApprovalRepository> {
        Box::new(self.clone())
    }
}

impl ApprovalRepositoryImpl {
    async fn create_policy_internal(
        conn: &mut PgConnection,
        input: CreateApprovalPolicyInput,
    ) -> Result<ApprovalPolicy, Error> {
        let result = sqlx::query_as::<_, ApprovalPolicy>(
            r#"
            INSERT INTO approval_policies (
                team_id,
                name,
                description,
                applies_to,
                environment_ids,
                required_approvers,
                approver_role_ids,
                approver_user_ids,
                allow_admin_override,
                fallback_to_roles,
                auto_approve_after_hours,
                enabled
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
            RETURNING id, team_id, name, description, applies_to, environment_ids, required_approvers,
                      approver_role_ids, approver_user_ids, allow_admin_override, fallback_to_roles,
                      auto_approve_after_hours, enabled, created_at
            "#,
        )
        .bind(input.team_id)
        .bind(input.name)
        .bind(input.description)
        .bind(input.applies_to)
        .bind(input.environment_ids.as_deref())
        .bind(input.required_approvers)
        .bind(&input.approver_role_ids)
        .bind(&input.approver_user_ids)
        .bind(input.allow_admin_override)
        .bind(input.fallback_to_roles)
        .bind(input.auto_approve_after_hours)
        .bind(input.enabled)
        .fetch_one(&mut *conn)
        .await;

        handle_error(None, result)
    }

    async fn create_request_internal(
        conn: &mut PgConnection,
        input: CreateApprovalRequestInput,
    ) -> Result<ApprovalRequest, Error> {
        let result = sqlx::query(
            r#"
            INSERT INTO approval_requests (
                policy_id, feature_id, environment_id, change_type, change_payload,
                change_description, requested_by, eligible_approver_ids, routing_reason,
                admin_override_enabled
            )
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
            RETURNING id, policy_id, feature_id, environment_id, change_type, change_payload,
                      change_description, requested_by, eligible_approver_ids, routing_reason,
                      admin_override_enabled, status, approved_count, rejected_count, executed_at,
                      created_at, updated_at
            "#,
        )
        .bind(input.policy_id)
        .bind(input.feature_id)
        .bind(input.environment_id)
        .bind(input.change_type)
        .bind(input.change_payload)
        .bind(input.change_description)
        .bind(input.requested_by)
        .bind(&input.eligible_approver_ids)
        .bind(input.routing_reason)
        .bind(input.admin_override_enabled)
        .map(Self::map_request_row)
        .fetch_one(&mut *conn)
        .await;

        handle_error(None, result)
    }

    async fn update_request_status_internal(
        conn: &mut PgConnection,
        request_id: Uuid,
        status: ApprovalStatus,
        executed_at: Option<DateTime<Utc>>,
    ) -> Result<ApprovalRequest, Error> {
        let result = sqlx::query(
            r#"
            UPDATE approval_requests
            SET status = $2,
                executed_at = COALESCE($3, executed_at),
                updated_at = NOW()
            WHERE id = $1
            RETURNING id, policy_id, feature_id, environment_id, change_type, change_payload,
                      change_description, requested_by, eligible_approver_ids, routing_reason,
                      admin_override_enabled, status, approved_count, rejected_count, executed_at,
                      created_at, updated_at
            "#,
        )
        .bind(request_id)
        .bind(status.as_str())
        .bind(executed_at)
        .map(Self::map_request_row)
        .fetch_one(&mut *conn)
        .await;

        handle_error(Some(request_id), result)
    }
}

#[async_trait::async_trait]
impl ApprovalRepositoryTx for ApprovalRepositoryImpl {
    async fn create_policy_tx(
        &self,
        conn: &mut PgConnection,
        input: CreateApprovalPolicyInput,
    ) -> Result<ApprovalPolicy, Error> {
        Self::create_policy_internal(conn, input).await
    }

    async fn update_policy_tx(
        &self,
        conn: &mut PgConnection,
        policy_id: Uuid,
        input: UpdateApprovalPolicyInput,
    ) -> Result<ApprovalPolicy, Error> {
        // First fetch the existing policy within the transaction
        let existing = sqlx::query_as::<_, ApprovalPolicy>(
            r#"
            SELECT id, team_id, name, description, applies_to, environment_ids, required_approvers,
                   approver_role_ids, approver_user_ids, allow_admin_override, fallback_to_roles,
                   auto_approve_after_hours, enabled, created_at
            FROM approval_policies
            WHERE id = $1
            "#,
        )
        .bind(policy_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(Error::DatabaseError)?
        .ok_or(Error::NotFound(policy_id))?;

        // Build update with optional fields
        let name = input.name.unwrap_or(existing.name);
        let description = input.description.or(existing.description);
        let applies_to = input.applies_to.unwrap_or(existing.applies_to);
        let environment_ids = input.environment_ids.or(existing.environment_ids);
        let required_approvers = input
            .required_approvers
            .unwrap_or(existing.required_approvers);
        let approver_role_ids = input
            .approver_role_ids
            .unwrap_or(existing.approver_role_ids);
        let approver_user_ids = input
            .approver_user_ids
            .unwrap_or(existing.approver_user_ids);
        let allow_admin_override = input
            .allow_admin_override
            .unwrap_or(existing.allow_admin_override);
        let fallback_to_roles = input
            .fallback_to_roles
            .unwrap_or(existing.fallback_to_roles);
        let auto_approve_after_hours = input
            .auto_approve_after_hours
            .or(existing.auto_approve_after_hours);
        let enabled = input.enabled.unwrap_or(existing.enabled);

        let result = sqlx::query_as::<_, ApprovalPolicy>(
            r#"
            UPDATE approval_policies
            SET name = $2,
                description = $3,
                applies_to = $4,
                environment_ids = $5,
                required_approvers = $6,
                approver_role_ids = $7,
                approver_user_ids = $8,
                allow_admin_override = $9,
                fallback_to_roles = $10,
                auto_approve_after_hours = $11,
                enabled = $12
            WHERE id = $1
            RETURNING id, team_id, name, description, applies_to, environment_ids, required_approvers,
                      approver_role_ids, approver_user_ids, allow_admin_override, fallback_to_roles,
                      auto_approve_after_hours, enabled, created_at
            "#,
        )
        .bind(policy_id)
        .bind(name)
        .bind(description)
        .bind(applies_to)
        .bind(environment_ids.as_deref())
        .bind(required_approvers)
        .bind(&approver_role_ids)
        .bind(&approver_user_ids)
        .bind(allow_admin_override)
        .bind(fallback_to_roles)
        .bind(auto_approve_after_hours)
        .bind(enabled)
        .fetch_one(&mut *conn)
        .await;

        handle_error(Some(policy_id), result)
    }

    async fn delete_policy_tx(
        &self,
        conn: &mut PgConnection,
        policy_id: Uuid,
    ) -> Result<bool, Error> {
        let result = sqlx::query(
            r#"
            DELETE FROM approval_policies WHERE id = $1
            "#,
        )
        .bind(policy_id)
        .execute(&mut *conn)
        .await;

        match result {
            Ok(r) => Ok(r.rows_affected() > 0),
            Err(e) => Err(Error::DatabaseError(e)),
        }
    }

    async fn create_request_tx(
        &self,
        conn: &mut PgConnection,
        input: CreateApprovalRequestInput,
    ) -> Result<ApprovalRequest, Error> {
        Self::create_request_internal(conn, input).await
    }

    async fn update_request_status_tx(
        &self,
        conn: &mut PgConnection,
        request_id: Uuid,
        status: ApprovalStatus,
        executed_at: Option<DateTime<Utc>>,
    ) -> Result<ApprovalRequest, Error> {
        Self::update_request_status_internal(conn, request_id, status, executed_at).await
    }

    async fn add_vote_tx(
        &self,
        conn: &mut PgConnection,
        input: CreateApprovalVoteInput,
        required_approvers: i32,
    ) -> Result<ApprovalRequest, Error> {
        if let Err(e) = sqlx::query!(
            r#"
            INSERT INTO approval_votes (request_id, approver_id, vote, comment)
            VALUES ($1, $2, $3, $4)
            "#,
            input.request_id,
            input.approver_id,
            input.vote.as_str(),
            input.comment
        )
        .execute(&mut *conn)
        .await
        {
            if let sqlx::Error::Database(db_err) = &e
                && db_err.code().map(|c| c == "23505").unwrap_or(false)
            {
                return Err(Error::RecordAlreadyExists("vote".into()));
            }
            return Err(Error::DatabaseError(e));
        }

        let result = sqlx::query(
            r#"
            UPDATE approval_requests
            SET approved_count = approved_count + CASE WHEN $2 = 'approve' THEN 1 ELSE 0 END,
                rejected_count = rejected_count + CASE WHEN $2 = 'reject' THEN 1 ELSE 0 END,
                status = CASE
                    WHEN status = 'cancelled' THEN status
                    WHEN $2 = 'reject' THEN 'rejected'
                    WHEN approved_count + CASE WHEN $2 = 'approve' THEN 1 ELSE 0 END >= $3 THEN 'approved'
                    ELSE status
                END,
                updated_at = NOW()
            WHERE id = $1
            RETURNING id, policy_id, feature_id, environment_id, change_type, change_payload,
                      change_description, requested_by, eligible_approver_ids, routing_reason,
                      admin_override_enabled, status, approved_count, rejected_count, executed_at,
                      created_at, updated_at
            "#,
        )
        .bind(input.request_id)
        .bind(input.vote.as_str())
        .bind(required_approvers)
        .map(Self::map_request_row)
        .fetch_one(&mut *conn)
        .await;

        handle_error(Some(input.request_id), result)
    }
}
