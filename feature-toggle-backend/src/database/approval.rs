use crate::database::entity::{ApprovalPolicy, ApprovalRequest, ApprovalStatus, ApprovalVoteValue};
use crate::database::{Error, handle_error};
use chrono::{DateTime, Utc};
use mockall::automock;
use sqlx::query_builder::QueryBuilder;
use sqlx::{PgPool, Row};
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
    pub auto_approve_after_hours: Option<i32>,
    pub enabled: bool,
}

pub struct CreateApprovalRequestInput {
    pub policy_id: Uuid,
    pub feature_id: Uuid,
    pub environment_id: Option<Uuid>,
    pub change_type: String,
    pub change_payload: serde_json::Value,
    pub change_description: Option<String>,
    pub requested_by: Uuid,
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
    async fn list_requests_due_for_auto_approval(&self) -> Result<Vec<ApprovalRequest>, Error>;

    fn clone_box(&self) -> Box<dyn ApprovalRepository>;
}

impl Clone for Box<dyn ApprovalRepository> {
    fn clone(&self) -> Box<dyn ApprovalRepository> {
        self.clone_box()
    }
}

pub fn approval_repository(pool: PgPool) -> Box<dyn ApprovalRepository> {
    Box::new(ApprovalRepositoryImpl::new(pool))
}

#[derive(Clone)]
struct ApprovalRepositoryImpl {
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
                auto_approve_after_hours,
                enabled
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
            RETURNING id, team_id, name, description, applies_to, environment_ids, required_approvers, approver_role_ids, auto_approve_after_hours, enabled, created_at
            "#,
        )
        .bind(input.team_id)
        .bind(input.name)
        .bind(input.description)
        .bind(input.applies_to)
        .bind(input.environment_ids.as_deref())
        .bind(input.required_approvers)
        .bind(&input.approver_role_ids)
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
                   approver_role_ids, auto_approve_after_hours, enabled, created_at
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
                   approver_role_ids, auto_approve_after_hours, enabled, created_at
            FROM approval_policies
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await;

        handle_error(Some(id), result)
    }

    async fn create_request(
        &self,
        input: CreateApprovalRequestInput,
    ) -> Result<ApprovalRequest, Error> {
        let result = sqlx::query(
            r#"
            INSERT INTO approval_requests (
                policy_id, feature_id, environment_id, change_type, change_payload,
                change_description, requested_by
            )
            VALUES ($1,$2,$3,$4,$5,$6,$7)
            RETURNING id, policy_id, feature_id, environment_id, change_type, change_payload,
                      change_description, requested_by, status, approved_count, rejected_count,
                      executed_at, created_at, updated_at
            "#,
        )
        .bind(input.policy_id)
        .bind(input.feature_id)
        .bind(input.environment_id)
        .bind(input.change_type)
        .bind(input.change_payload)
        .bind(input.change_description)
        .bind(input.requested_by)
        .map(Self::map_request_row)
        .fetch_one(&self.pool)
        .await;

        handle_error(None, result)
    }

    async fn get_request_by_id(&self, id: Uuid) -> Result<Option<ApprovalRequest>, Error> {
        let result = sqlx::query(
            r#"
            SELECT id, policy_id, feature_id, environment_id, change_type, change_payload,
                   change_description, requested_by, status, approved_count, rejected_count,
                   executed_at, created_at, updated_at
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
                      change_description, requested_by, status, approved_count, rejected_count,
                      executed_at, created_at, updated_at
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
                      change_description, requested_by, status, approved_count, rejected_count,
                      executed_at, created_at, updated_at
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
                select_has_where = true;
            } else {
                select_builder.push(" AND ");
            }
            select_builder
                .push("r.status = ANY(")
                .push_bind(values.clone())
                .push(")");

            if !count_has_where {
                count_builder.push(" WHERE ");
                count_has_where = true;
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
