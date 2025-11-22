use feature_toggle_backend::database::compound_rules::{
    CreateRuleConditionInput, CreateRuleGroupInput, UpdateRuleGroupInput, compound_rules_repository,
};
use feature_toggle_backend::database::entity::LogicOperator;
use feature_toggle_backend::database::feature::{CreateStageCriterion, feature_repository};
use feature_toggle_backend::database::init_pg_pool;
use serde_json::json;
use uuid::Uuid;

/// Test creating a rule group with multiple conditions
#[tokio::test]
async fn test_create_rule_group_with_conditions() {
    let pool = init_pg_pool().await;
    let repo = compound_rules_repository(pool.clone());
    let feature_repo = feature_repository(pool.clone());

    // Use a dedicated stage for this test to avoid interference
    let stage_id = Uuid::parse_str("aaaaaaaa-1111-4111-8111-111111111111").unwrap();

    // Create a criterion to attach rule groups to
    let criteria = vec![CreateStageCriterion { priority: 0 }];

    let created_criteria = feature_repo
        .set_stage_criteria(stage_id, criteria)
        .await
        .expect("Failed to create criteria");
    let criteria_id = created_criteria[0].id;

    // Create a rule group with AND logic
    let input = CreateRuleGroupInput {
        criteria_id,
        logic_operator: LogicOperator::And,
        conditions: vec![
            CreateRuleConditionInput {
                context_key: "user.country".to_string(),
                operator: "EQUALS".to_string(),
                value: json!("US"),
                order_index: 0,
            },
            CreateRuleConditionInput {
                context_key: "user.tier".to_string(),
                operator: "IN".to_string(),
                value: json!(["premium", "enterprise"]),
                order_index: 1,
            },
        ],
    };

    let result = repo.create_rule_group(input).await;
    assert!(result.is_ok(), "Failed to create rule group: {:?}", result);

    let rule_group = result.unwrap();
    assert_eq!(rule_group.logic_operator, LogicOperator::And);

    // Fetch conditions separately
    let conditions = repo.get_rule_conditions(rule_group.id).await.unwrap();
    assert_eq!(conditions.len(), 2);

    // Verify conditions
    let cond1 = &conditions[0];
    assert_eq!(cond1.context_key, "user.country");
    assert_eq!(cond1.operator, "EQUALS");
    assert_eq!(cond1.value, json!("US"));
    assert_eq!(cond1.order_index, 0);

    let cond2 = &conditions[1];
    assert_eq!(cond2.context_key, "user.tier");
    assert_eq!(cond2.operator, "IN");
    assert_eq!(cond2.value, json!(["premium", "enterprise"]));
    assert_eq!(cond2.order_index, 1);
}

/// Test retrieving rule groups by criteria ID
#[tokio::test]
async fn test_get_rule_groups_by_criteria() {
    let pool = init_pg_pool().await;
    let repo = compound_rules_repository(pool.clone());
    let feature_repo = feature_repository(pool.clone());

    // Use a dedicated stage for this test to avoid interference
    let stage_id = Uuid::parse_str("bbbbbbbb-2222-4222-8222-222222222222").unwrap();

    let criteria = vec![CreateStageCriterion { priority: 0 }];

    let created_criteria = feature_repo
        .set_stage_criteria(stage_id, criteria)
        .await
        .expect("Failed to create criteria");
    let criteria_id = created_criteria[0].id;

    // Create two rule groups (OR'd together)
    let group1_input = CreateRuleGroupInput {
        criteria_id,
        logic_operator: LogicOperator::And,
        conditions: vec![CreateRuleConditionInput {
            context_key: "user.beta".to_string(),
            operator: "EQUALS".to_string(),
            value: json!(true),
            order_index: 0,
        }],
    };

    let group2_input = CreateRuleGroupInput {
        criteria_id,
        logic_operator: LogicOperator::Or,
        conditions: vec![
            CreateRuleConditionInput {
                context_key: "user.role".to_string(),
                operator: "EQUALS".to_string(),
                value: json!("admin"),
                order_index: 0,
            },
            CreateRuleConditionInput {
                context_key: "user.permissions".to_string(),
                operator: "CONTAINS".to_string(),
                value: json!("beta_access"),
                order_index: 1,
            },
        ],
    };

    let g1 = repo
        .create_rule_group(group1_input)
        .await
        .expect("Failed to create group 1");
    let g2 = repo
        .create_rule_group(group2_input)
        .await
        .expect("Failed to create group 2");

    // Retrieve all groups for this criteria
    let result = repo.get_rule_groups_by_criteria(criteria_id).await;
    assert!(result.is_ok());

    let groups = result.unwrap();
    assert_eq!(groups.len(), 2, "Should have 2 rule groups");

    // Verify groups exist
    assert!(groups.iter().any(|g| g.id == g1.id));
    assert!(groups.iter().any(|g| g.id == g2.id));

    // Verify first group has 1 condition
    let group1_conditions = repo.get_rule_conditions(g1.id).await.unwrap();
    assert_eq!(group1_conditions.len(), 1);

    // Verify second group has 2 conditions
    let group2_conditions = repo.get_rule_conditions(g2.id).await.unwrap();
    assert_eq!(group2_conditions.len(), 2);
}

/// Test updating a rule group
#[tokio::test]
async fn test_update_rule_group() {
    let pool = init_pg_pool().await;
    let repo = compound_rules_repository(pool.clone());
    let feature_repo = feature_repository(pool.clone());

    // Use a dedicated stage for this test to avoid interference
    let stage_id = Uuid::parse_str("cccccccc-3333-4333-8333-333333333333").unwrap();

    let criteria = vec![CreateStageCriterion { priority: 0 }];

    let created_criteria = feature_repo
        .set_stage_criteria(stage_id, criteria)
        .await
        .unwrap();
    let criteria_id = created_criteria[0].id;

    // Create initial rule group
    let input = CreateRuleGroupInput {
        criteria_id,
        logic_operator: LogicOperator::And,
        conditions: vec![CreateRuleConditionInput {
            context_key: "user.status".to_string(),
            operator: "EQUALS".to_string(),
            value: json!("active"),
            order_index: 0,
        }],
    };

    let created = repo.create_rule_group(input).await.unwrap();
    let group_id = created.id;

    // Update the group: change logic operator and add a condition
    let update_input = UpdateRuleGroupInput {
        logic_operator: Some(LogicOperator::Or),
        conditions: Some(vec![
            CreateRuleConditionInput {
                context_key: "user.status".to_string(),
                operator: "EQUALS".to_string(),
                value: json!("active"),
                order_index: 0,
            },
            CreateRuleConditionInput {
                context_key: "user.trial".to_string(),
                operator: "EQUALS".to_string(),
                value: json!(true),
                order_index: 1,
            },
        ]),
    };

    let result = repo.update_rule_group(group_id, update_input).await;
    assert!(result.is_ok());

    let updated = result.unwrap();
    assert_eq!(updated.logic_operator, LogicOperator::Or);

    // Fetch updated conditions
    let updated_conditions = repo.get_rule_conditions(group_id).await.unwrap();
    assert_eq!(updated_conditions.len(), 2);
}

/// Test deleting a rule group
#[tokio::test]
async fn test_delete_rule_group() {
    let pool = init_pg_pool().await;
    let repo = compound_rules_repository(pool.clone());
    let feature_repo = feature_repository(pool.clone());

    // Use a dedicated stage for this test to avoid interference
    let stage_id = Uuid::parse_str("dddddddd-4444-4444-8444-444444444444").unwrap();

    let criteria = vec![CreateStageCriterion { priority: 0 }];

    let created_criteria = feature_repo
        .set_stage_criteria(stage_id, criteria)
        .await
        .unwrap();
    let criteria_id = created_criteria[0].id;

    // Create rule group
    let input = CreateRuleGroupInput {
        criteria_id,
        logic_operator: LogicOperator::And,
        conditions: vec![CreateRuleConditionInput {
            context_key: "temp.key".to_string(),
            operator: "EQUALS".to_string(),
            value: json!("value"),
            order_index: 0,
        }],
    };

    let created = repo.create_rule_group(input).await.unwrap();
    let group_id = created.id;

    // Verify it exists
    let before_delete = repo.get_rule_groups_by_criteria(criteria_id).await.unwrap();
    assert_eq!(before_delete.len(), 1);

    // Delete the group
    let delete_result = repo.delete_rule_group(group_id).await;
    assert!(delete_result.is_ok());

    // Verify it's gone
    let after_delete = repo.get_rule_groups_by_criteria(criteria_id).await.unwrap();
    assert_eq!(after_delete.len(), 0);
}

/// Test all supported operators
#[tokio::test]
async fn test_all_operators() {
    let pool = init_pg_pool().await;
    let repo = compound_rules_repository(pool.clone());
    let feature_repo = feature_repository(pool.clone());

    // Use a dedicated stage for this test to avoid interference
    let stage_id = Uuid::parse_str("eeeeeeee-5555-4555-8555-555555555555").unwrap();

    let criteria = vec![CreateStageCriterion { priority: 0 }];

    let created_criteria = feature_repo
        .set_stage_criteria(stage_id, criteria)
        .await
        .unwrap();
    let criteria_id = created_criteria[0].id;

    let operators = vec![
        "EQUALS",
        "NOT_EQUALS",
        "GREATER_THAN",
        "LESS_THAN",
        "GREATER_THAN_OR_EQUAL",
        "LESS_THAN_OR_EQUAL",
        "CONTAINS",
        "STARTS_WITH",
        "ENDS_WITH",
        "REGEX",
        "IN",
        "NOT_IN",
        "SEMVER_GREATER_THAN",
        "SEMVER_LESS_THAN",
    ];

    for (idx, op) in operators.iter().enumerate() {
        let input = CreateRuleGroupInput {
            criteria_id,
            logic_operator: LogicOperator::And,
            conditions: vec![CreateRuleConditionInput {
                context_key: format!("test.{}", op.to_lowercase()),
                operator: op.to_string(),
                value: json!("test_value"),
                order_index: idx as i32,
            }],
        };

        let result = repo.create_rule_group(input).await;
        assert!(
            result.is_ok(),
            "Failed to create rule group with operator {}: {:?}",
            op,
            result
        );

        let group = result.unwrap();
        let conditions = repo.get_rule_conditions(group.id).await.unwrap();
        assert_eq!(conditions[0].operator, *op);
    }
}

/// Test complex nested rules scenario
#[tokio::test]
async fn test_complex_compound_rules() {
    let pool = init_pg_pool().await;
    let repo = compound_rules_repository(pool.clone());
    let feature_repo = feature_repository(pool.clone());

    // Use a dedicated stage for this test to avoid interference
    let stage_id = Uuid::parse_str("ffffffff-6666-4666-8666-666666666666").unwrap();

    let criteria = vec![CreateStageCriterion { priority: 0 }];

    let created_criteria = feature_repo
        .set_stage_criteria(stage_id, criteria)
        .await
        .unwrap();
    let criteria_id = created_criteria[0].id;

    // Create complex rule: (country=US AND tier IN [premium,enterprise]) OR (beta_user=true)
    // Group 1: country=US AND tier IN [premium,enterprise]
    let group1 = CreateRuleGroupInput {
        criteria_id,
        logic_operator: LogicOperator::And,
        conditions: vec![
            CreateRuleConditionInput {
                context_key: "user.country".to_string(),
                operator: "EQUALS".to_string(),
                value: json!("US"),
                order_index: 0,
            },
            CreateRuleConditionInput {
                context_key: "user.tier".to_string(),
                operator: "IN".to_string(),
                value: json!(["premium", "enterprise"]),
                order_index: 1,
            },
        ],
    };

    // Group 2: beta_user=true
    let group2 = CreateRuleGroupInput {
        criteria_id,
        logic_operator: LogicOperator::And,
        conditions: vec![CreateRuleConditionInput {
            context_key: "user.beta".to_string(),
            operator: "EQUALS".to_string(),
            value: json!(true),
            order_index: 0,
        }],
    };

    let g1 = repo.create_rule_group(group1).await.unwrap();
    let g2 = repo.create_rule_group(group2).await.unwrap();

    // Retrieve all groups and verify structure
    let groups = repo.get_rule_groups_by_criteria(criteria_id).await.unwrap();
    assert_eq!(groups.len(), 2);

    // Verify group 1
    let retrieved_g1 = groups.iter().find(|g| g.id == g1.id).unwrap();
    let g1_conditions = repo.get_rule_conditions(retrieved_g1.id).await.unwrap();
    assert_eq!(g1_conditions.len(), 2);
    assert_eq!(retrieved_g1.logic_operator, LogicOperator::And);

    // Verify group 2
    let retrieved_g2 = groups.iter().find(|g| g.id == g2.id).unwrap();
    let g2_conditions = repo.get_rule_conditions(retrieved_g2.id).await.unwrap();
    assert_eq!(g2_conditions.len(), 1);
    assert_eq!(retrieved_g2.logic_operator, LogicOperator::And);
}

/// Test that deleting criteria also deletes associated rule groups (cascade)
#[tokio::test]
async fn test_cascade_delete_rule_groups_with_criteria() {
    let pool = init_pg_pool().await;
    let repo = compound_rules_repository(pool.clone());
    let feature_repo = feature_repository(pool.clone());

    // Use a dedicated stage for this test to avoid interference
    let stage_id = Uuid::parse_str("11111111-7777-4777-8777-777777777777").unwrap();

    // Create criterion with rule groups
    let criteria = vec![CreateStageCriterion { priority: 0 }];

    let created_criteria = feature_repo
        .set_stage_criteria(stage_id, criteria)
        .await
        .unwrap();
    let criteria_id = created_criteria[0].id;

    // Create rule groups
    let input = CreateRuleGroupInput {
        criteria_id,
        logic_operator: LogicOperator::And,
        conditions: vec![CreateRuleConditionInput {
            context_key: "test.key".to_string(),
            operator: "EQUALS".to_string(),
            value: json!("value"),
            order_index: 0,
        }],
    };

    repo.create_rule_group(input).await.unwrap();

    // Verify rule groups exist
    let before = repo.get_rule_groups_by_criteria(criteria_id).await.unwrap();
    assert_eq!(before.len(), 1);

    // Delete all criteria for this stage (should cascade delete rule groups)
    feature_repo
        .set_stage_criteria(stage_id, vec![])
        .await
        .unwrap();

    // Verify rule groups are gone
    let after = repo.get_rule_groups_by_criteria(criteria_id).await.unwrap();
    assert_eq!(after.len(), 0);
}
