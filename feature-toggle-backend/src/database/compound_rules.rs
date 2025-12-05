use crate::database::entity::{LogicOperator, RuleCondition, RuleGroup};
use crate::database::{Error, handle_error};
use log::{debug, info};
use mockall::automock;
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct CreateRuleGroupInput {
    pub criteria_id: Uuid,
    pub logic_operator: LogicOperator,
    pub conditions: Vec<CreateRuleConditionInput>,
}

#[derive(Debug, Clone)]
pub struct CreateRuleConditionInput {
    pub context_key: String,
    pub operator: String,
    pub value: JsonValue,
    pub order_index: i32,
}

#[derive(Debug, Clone)]
pub struct UpdateRuleGroupInput {
    pub logic_operator: Option<LogicOperator>,
    pub conditions: Option<Vec<CreateRuleConditionInput>>, // full replacement for simplicity
}

#[automock]
#[async_trait::async_trait]
pub trait CompoundRulesRepository: Send + Sync {
    /// Get all rule groups for a specific criterion
    async fn get_rule_groups_by_criteria(&self, criteria_id: Uuid)
    -> Result<Vec<RuleGroup>, Error>;

    /// Get a single rule group by ID with its conditions
    async fn get_rule_group_by_id(&self, group_id: Uuid) -> Result<RuleGroup, Error>;

    /// Get all conditions for a specific rule group
    async fn get_rule_conditions(&self, group_id: Uuid) -> Result<Vec<RuleCondition>, Error>;

    /// Create a new rule group with conditions
    async fn create_rule_group(&self, input: CreateRuleGroupInput) -> Result<RuleGroup, Error>;

    /// Update a rule group (logic operator and/or conditions)
    async fn update_rule_group(
        &self,
        group_id: Uuid,
        input: UpdateRuleGroupInput,
    ) -> Result<RuleGroup, Error>;

    /// Delete a rule group (cascade deletes conditions)
    async fn delete_rule_group(&self, group_id: Uuid) -> Result<(), Error>;

    /// Delete all rule groups for a criterion
    async fn delete_rule_groups_by_criteria(&self, criteria_id: Uuid) -> Result<(), Error>;

    fn clone_box(&self) -> Box<dyn CompoundRulesRepository>;
}

impl Clone for Box<dyn CompoundRulesRepository> {
    fn clone(&self) -> Box<dyn CompoundRulesRepository> {
        self.clone_box()
    }
}

pub fn compound_rules_repository(pool: PgPool) -> Box<dyn CompoundRulesRepository> {
    Box::new(CompoundRulesRepositoryImpl::new(pool))
}

#[derive(Clone)]
struct CompoundRulesRepositoryImpl {
    pool: PgPool,
}

impl CompoundRulesRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl CompoundRulesRepository for CompoundRulesRepositoryImpl {
    async fn get_rule_groups_by_criteria(
        &self,
        criteria_id: Uuid,
    ) -> Result<Vec<RuleGroup>, Error> {
        debug!("DB: get_rule_groups_by_criteria criteria_id={criteria_id}");

        let groups = sqlx::query!(
            r#"SELECT id, criteria_id, logic_operator as "logic_operator: LogicOperator",
                      created_at, updated_at
               FROM rule_groups
               WHERE criteria_id = $1
               ORDER BY created_at"#,
            criteria_id
        )
        .fetch_all(&self.pool)
        .await;

        let groups = handle_error(None, groups)?;

        Ok(groups
            .into_iter()
            .map(|row| RuleGroup {
                id: row.id,
                criteria_id: row.criteria_id,
                logic_operator: row.logic_operator,
                created_at: row.created_at.unwrap_or_else(chrono::Utc::now),
                updated_at: row.updated_at.unwrap_or_else(chrono::Utc::now),
            })
            .collect())
    }

    async fn get_rule_group_by_id(&self, group_id: Uuid) -> Result<RuleGroup, Error> {
        debug!("DB: get_rule_group_by_id group_id={group_id}");

        let row = sqlx::query!(
            r#"SELECT id, criteria_id, logic_operator as "logic_operator: LogicOperator",
                      created_at, updated_at
               FROM rule_groups
               WHERE id = $1"#,
            group_id
        )
        .fetch_one(&self.pool)
        .await;

        let row = handle_error(Some(group_id), row)?;

        Ok(RuleGroup {
            id: row.id,
            criteria_id: row.criteria_id,
            logic_operator: row.logic_operator,
            created_at: row.created_at.unwrap_or_else(chrono::Utc::now),
            updated_at: row.updated_at.unwrap_or_else(chrono::Utc::now),
        })
    }

    async fn get_rule_conditions(&self, group_id: Uuid) -> Result<Vec<RuleCondition>, Error> {
        debug!("DB: get_rule_conditions group_id={group_id}");

        let conditions = sqlx::query!(
            r#"SELECT id, group_id, context_key, operator, value, order_index,
                      created_at, updated_at
               FROM rule_conditions
               WHERE group_id = $1
               ORDER BY order_index, created_at"#,
            group_id
        )
        .fetch_all(&self.pool)
        .await;

        let conditions = handle_error(None, conditions)?;

        Ok(conditions
            .into_iter()
            .map(|row| RuleCondition {
                id: row.id,
                group_id: row.group_id,
                context_key: row.context_key,
                operator: row.operator,
                value: row.value,
                order_index: row.order_index,
                created_at: row.created_at.unwrap_or_else(chrono::Utc::now),
                updated_at: row.updated_at.unwrap_or_else(chrono::Utc::now),
            })
            .collect())
    }

    async fn create_rule_group(&self, input: CreateRuleGroupInput) -> Result<RuleGroup, Error> {
        info!(
            "DB: create_rule_group criteria_id={} logic_operator={:?}",
            input.criteria_id, input.logic_operator
        );

        let group_id = Uuid::new_v4();

        // Insert rule group
        let row = sqlx::query!(
            r#"INSERT INTO rule_groups (id, criteria_id, logic_operator)
               VALUES ($1, $2, $3)
               RETURNING id, criteria_id, logic_operator as "logic_operator: LogicOperator",
                         created_at, updated_at"#,
            group_id,
            input.criteria_id,
            input.logic_operator as LogicOperator
        )
        .fetch_one(&self.pool)
        .await;

        let _row = handle_error(None, row)?;

        // Insert conditions
        for condition in input.conditions {
            let condition_id = Uuid::new_v4();
            let _ = handle_error(
                None,
                sqlx::query!(
                    r#"INSERT INTO rule_conditions
                       (id, group_id, context_key, operator, value, order_index)
                       VALUES ($1, $2, $3, $4, $5, $6)"#,
                    condition_id,
                    group_id,
                    condition.context_key,
                    condition.operator,
                    condition.value,
                    condition.order_index
                )
                .execute(&self.pool)
                .await,
            )?;
        }

        self.get_rule_group_by_id(group_id).await
    }

    async fn update_rule_group(
        &self,
        group_id: Uuid,
        input: UpdateRuleGroupInput,
    ) -> Result<RuleGroup, Error> {
        info!("DB: update_rule_group group_id={group_id}");

        // Ensure group exists
        let _existing = self.get_rule_group_by_id(group_id).await?;

        // Update logic operator if provided
        if let Some(logic_operator) = input.logic_operator {
            let _ = handle_error(
                Some(group_id),
                sqlx::query!(
                    r#"UPDATE rule_groups SET logic_operator = $1, updated_at = CURRENT_TIMESTAMP
                       WHERE id = $2"#,
                    logic_operator as LogicOperator,
                    group_id
                )
                .execute(&self.pool)
                .await,
            )?;
        }

        // Replace conditions if provided
        if let Some(conditions) = input.conditions {
            // Delete existing conditions
            let _ = handle_error(
                Some(group_id),
                sqlx::query!(
                    r#"DELETE FROM rule_conditions WHERE group_id = $1"#,
                    group_id
                )
                .execute(&self.pool)
                .await,
            )?;

            // Insert new conditions
            for condition in conditions {
                let condition_id = Uuid::new_v4();
                let _ = handle_error(
                    None,
                    sqlx::query!(
                        r#"INSERT INTO rule_conditions
                           (id, group_id, context_key, operator, value, order_index)
                           VALUES ($1, $2, $3, $4, $5, $6)"#,
                        condition_id,
                        group_id,
                        condition.context_key,
                        condition.operator,
                        condition.value,
                        condition.order_index
                    )
                    .execute(&self.pool)
                    .await,
                )?;
            }
        }

        self.get_rule_group_by_id(group_id).await
    }

    async fn delete_rule_group(&self, group_id: Uuid) -> Result<(), Error> {
        info!("DB: delete_rule_group group_id={group_id}");

        // Ensure group exists
        let _ = self.get_rule_group_by_id(group_id).await?;

        // Delete conditions (should cascade, but being explicit)
        let _ = handle_error(
            Some(group_id),
            sqlx::query!(
                r#"DELETE FROM rule_conditions WHERE group_id = $1"#,
                group_id
            )
            .execute(&self.pool)
            .await,
        )?;

        // Delete group
        let _ = handle_error(
            Some(group_id),
            sqlx::query!(r#"DELETE FROM rule_groups WHERE id = $1"#, group_id)
                .execute(&self.pool)
                .await,
        )?;

        Ok(())
    }

    async fn delete_rule_groups_by_criteria(&self, criteria_id: Uuid) -> Result<(), Error> {
        info!("DB: delete_rule_groups_by_criteria criteria_id={criteria_id}");

        // Get all groups for this criterion
        let groups = self.get_rule_groups_by_criteria(criteria_id).await?;

        // Delete conditions for all groups
        for group in &groups {
            let _ = handle_error(
                Some(group.id),
                sqlx::query!(
                    r#"DELETE FROM rule_conditions WHERE group_id = $1"#,
                    group.id
                )
                .execute(&self.pool)
                .await,
            )?;
        }

        // Delete all groups
        let _ = handle_error(
            None,
            sqlx::query!(
                r#"DELETE FROM rule_groups WHERE criteria_id = $1"#,
                criteria_id
            )
            .execute(&self.pool)
            .await,
        )?;

        Ok(())
    }

    fn clone_box(&self) -> Box<dyn CompoundRulesRepository> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_rule_group() -> RuleGroup {
        RuleGroup {
            id: Uuid::new_v4(),
            criteria_id: Uuid::new_v4(),
            logic_operator: LogicOperator::And,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn sample_rule_condition() -> RuleCondition {
        RuleCondition {
            id: Uuid::new_v4(),
            group_id: Uuid::new_v4(),
            context_key: "country".to_string(),
            operator: "EQUALS".to_string(),
            value: json!("US"),
            order_index: 0,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn sample_create_rule_group_input() -> CreateRuleGroupInput {
        CreateRuleGroupInput {
            criteria_id: Uuid::new_v4(),
            logic_operator: LogicOperator::And,
            conditions: vec![
                CreateRuleConditionInput {
                    context_key: "country".to_string(),
                    operator: "EQUALS".to_string(),
                    value: json!("US"),
                    order_index: 0,
                },
                CreateRuleConditionInput {
                    context_key: "tier".to_string(),
                    operator: "IN".to_string(),
                    value: json!(["premium", "enterprise"]),
                    order_index: 1,
                },
            ],
        }
    }

    fn sample_update_rule_group_input() -> UpdateRuleGroupInput {
        UpdateRuleGroupInput {
            logic_operator: Some(LogicOperator::Or),
            conditions: Some(vec![CreateRuleConditionInput {
                context_key: "beta_user".to_string(),
                operator: "EQUALS".to_string(),
                value: json!(true),
                order_index: 0,
            }]),
        }
    }

    #[test]
    fn test_rule_group_struct_creation() {
        let group = sample_rule_group();
        assert_eq!(group.logic_operator, LogicOperator::And);
    }

    #[test]
    fn test_rule_condition_struct_creation() {
        let condition = sample_rule_condition();
        assert_eq!(condition.context_key, "country");
        assert_eq!(condition.operator, "EQUALS");
        assert_eq!(condition.value, json!("US"));
    }

    #[test]
    fn test_create_rule_group_input_struct() {
        let input = sample_create_rule_group_input();
        assert_eq!(input.logic_operator, LogicOperator::And);
        assert_eq!(input.conditions.len(), 2);
        assert_eq!(input.conditions[0].context_key, "country");
        assert_eq!(input.conditions[1].context_key, "tier");
    }

    #[test]
    fn test_update_rule_group_input_struct() {
        let input = sample_update_rule_group_input();
        assert_eq!(input.logic_operator, Some(LogicOperator::Or));
        assert!(input.conditions.is_some());
        let conditions = input.conditions.unwrap();
        assert_eq!(conditions.len(), 1);
        assert_eq!(conditions[0].context_key, "beta_user");
    }

    #[tokio::test]
    async fn test_mock_compound_rules_repository_get_rule_groups() {
        let mut mock_repo = MockCompoundRulesRepository::new();
        let criteria_id = Uuid::new_v4();
        let groups = vec![sample_rule_group()];

        mock_repo
            .expect_get_rule_groups_by_criteria()
            .with(mockall::predicate::eq(criteria_id))
            .times(1)
            .returning(move |_| Ok(groups.clone()));

        let result = mock_repo.get_rule_groups_by_criteria(criteria_id).await;
        assert!(result.is_ok());
        let retrieved_groups = result.unwrap();
        assert_eq!(retrieved_groups.len(), 1);
    }

    #[tokio::test]
    async fn test_mock_compound_rules_repository_get_rule_conditions() {
        let mut mock_repo = MockCompoundRulesRepository::new();
        let group_id = Uuid::new_v4();
        let conditions = vec![sample_rule_condition()];

        mock_repo
            .expect_get_rule_conditions()
            .with(mockall::predicate::eq(group_id))
            .times(1)
            .returning(move |_| Ok(conditions.clone()));

        let result = mock_repo.get_rule_conditions(group_id).await;
        assert!(result.is_ok());
        let retrieved_conditions = result.unwrap();
        assert_eq!(retrieved_conditions.len(), 1);
        assert_eq!(retrieved_conditions[0].context_key, "country");
    }

    #[tokio::test]
    async fn test_mock_compound_rules_repository_create_rule_group() {
        let mut mock_repo = MockCompoundRulesRepository::new();
        let input = sample_create_rule_group_input();
        let expected_group = sample_rule_group();

        mock_repo
            .expect_create_rule_group()
            .with(mockall::predicate::function(
                |input: &CreateRuleGroupInput| {
                    input.logic_operator == LogicOperator::And && input.conditions.len() == 2
                },
            ))
            .times(1)
            .returning(move |_| Ok(expected_group.clone()));

        let result = mock_repo.create_rule_group(input).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mock_compound_rules_repository_delete_rule_group() {
        let mut mock_repo = MockCompoundRulesRepository::new();
        let group_id = Uuid::new_v4();

        mock_repo
            .expect_delete_rule_group()
            .with(mockall::predicate::eq(group_id))
            .times(1)
            .returning(|_| Ok(()));

        let result = mock_repo.delete_rule_group(group_id).await;
        assert!(result.is_ok());
    }
}
