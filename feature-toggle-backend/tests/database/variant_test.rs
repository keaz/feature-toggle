use feature_toggle_backend::database::entity::{FeatureType, VariantValueType};
use feature_toggle_backend::database::feature::{CreateFeature, CreateFeatureStage, CreateStageCriterion};
use feature_toggle_backend::database::{feature, init_pg_pool};
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn test_create_feature_with_variants() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool.clone());

    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let env_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let random_key = format!("Feature With Variants {}", Uuid::new_v4());

    let stage = CreateFeatureStage {
        id: Uuid::new_v4(),
        environment_id: env_id,
        order_index: 0,
        parent_stage: None,
        position: json!({"x": 100, "y": 100}).to_string(),
        enabled: true,
        bucketing_key: Some("userId".to_string()),
    };

    let variants = vec![
        (
            "control".to_string(),
            json!(false),
            VariantValueType::Boolean,
            Some("Control variant".to_string()),
        ),
        (
            "treatment".to_string(),
            json!("Enhanced UI"),
            VariantValueType::String,
            Some("Treatment variant".to_string()),
        ),
        (
            "premium".to_string(),
            json!({"theme": "dark", "features": ["chat"]}),
            VariantValueType::Json,
            Some("Premium configuration".to_string()),
        ),
    ];

    let input = CreateFeature {
        team_id,
        key: random_key.clone(),
        description: Some("Test feature with variants".to_string()),
        feature_type: FeatureType::Contextual,
        stages: vec![stage],
        dependencies: vec![],
        variants: Some(variants),
    };

    let result = repository.create_feature(input).await;
    assert!(result.is_ok(), "Feature creation should succeed");

    let feature_id = result.unwrap();

    // Verify variants were created
    let feature = repository
        .get_feature_by_id(feature_id)
        .await
        .expect("Feature should exist");
    assert_eq!(feature.key, random_key);

    // Query variants directly from database
    let mut tx = pool.begin().await.expect("Transaction should start");
    let db_variants = sqlx::query!(
        r#"SELECT id, control, value, value_type as "value_type: VariantValueType", description
           FROM feature_variants
           WHERE feature_id = $1
           ORDER BY created_at"#,
        feature_id
    )
    .fetch_all(&mut *tx)
    .await
    .expect("Should fetch variants");

    tx.commit().await.expect("Transaction should commit");

    assert_eq!(db_variants.len(), 3, "Should have 3 variants");
    assert_eq!(db_variants[0].control, "control");
    assert_eq!(db_variants[0].value_type, VariantValueType::Boolean);
    assert_eq!(db_variants[1].control, "treatment");
    assert_eq!(db_variants[1].value_type, VariantValueType::String);
    assert_eq!(db_variants[2].control, "premium");
    assert_eq!(db_variants[2].value_type, VariantValueType::Json);
}

#[tokio::test]
async fn test_get_feature_variants() {
    let pool = init_pg_pool().await;
    let _repository = feature::feature_repository(pool.clone());

    // Use test data from init.sql
    let feature_id = Uuid::parse_str("5eef17bc-9e06-411d-b5f4-7a786e68bb99").unwrap();

    let mut tx = pool.begin().await.expect("Transaction should start");
    let variants = sqlx::query!(
        r#"SELECT id, control, value, value_type as "value_type: VariantValueType", description
           FROM feature_variants
           WHERE feature_id = $1
           ORDER BY created_at"#,
        feature_id
    )
    .fetch_all(&mut *tx)
    .await
    .expect("Should fetch variants");

    tx.commit().await.expect("Transaction should commit");

    assert!(variants.len() >= 3, "Should have at least 3 variants from test data");

    // Verify the test data variants
    let control_variant = variants.iter().find(|v| v.control == "control");
    assert!(control_variant.is_some());
    assert_eq!(control_variant.unwrap().value_type, VariantValueType::Boolean);

    let treatment_a = variants.iter().find(|v| v.control == "treatment-a");
    assert!(treatment_a.is_some());
    assert_eq!(treatment_a.unwrap().value_type, VariantValueType::String);

    let treatment_b = variants.iter().find(|v| v.control == "treatment-b");
    assert!(treatment_b.is_some());
    assert_eq!(treatment_b.unwrap().value_type, VariantValueType::Json);
}

#[tokio::test]
async fn test_set_stage_criteria_with_serve() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool.clone());

    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let env_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let context_id = Uuid::parse_str("cb461425-373b-49d9-9634-9a248612d7b7").unwrap();

    // Create feature with variants
    let random_key = format!("Feature For Serve {}", Uuid::new_v4());
    let stage = CreateFeatureStage {
        id: Uuid::new_v4(),
        environment_id: env_id,
        order_index: 0,
        parent_stage: None,
        position: json!({"x": 100, "y": 100}).to_string(),
        enabled: true,
        bucketing_key: Some("userId".to_string()),
    };

    let variants = vec![
        (
            "blue".to_string(),
            json!("#0000FF"),
            VariantValueType::String,
            Some("Blue theme".to_string()),
        ),
        (
            "green".to_string(),
            json!("#00FF00"),
            VariantValueType::String,
            Some("Green theme".to_string()),
        ),
    ];

    let input = CreateFeature {
        team_id,
        key: random_key.clone(),
        description: Some("Test feature for serve field".to_string()),
        feature_type: FeatureType::Contextual,
        stages: vec![stage],
        dependencies: vec![],
        variants: Some(variants),
    };

    let feature_id = repository.create_feature(input).await.expect("Feature creation should succeed");

    let stages = repository.get_feature_stages(feature_id).await.expect("Should get stages");
    let stage_id = stages[0].id;

    // Set stage criteria with serve field
    let criteria = vec![
        CreateStageCriterion {
            context_id,
            context_key: "filter".to_string(),
            rollout_percentage: 50,
            serve: Some("blue".to_string()),
            priority: 0,
            operator: Some("IN".to_string()),
        },
        CreateStageCriterion {
            context_id,
            context_key: "filter2".to_string(),
            rollout_percentage: 30,
            serve: Some("green".to_string()),
            priority: 1,
            operator: Some("IN".to_string()),
        },
    ];

    let result = repository.set_stage_criteria(stage_id, criteria).await;
    assert!(result.is_ok(), "Setting stage criteria should succeed");

    let db_criteria = result.unwrap();
    assert_eq!(db_criteria.len(), 2);

    assert_eq!(db_criteria[0].serve, Some("blue".to_string()));
    assert_eq!(db_criteria[0].rollout_percentage, 50);

    assert_eq!(db_criteria[1].serve, Some("green".to_string()));
    assert_eq!(db_criteria[1].rollout_percentage, 30);
}

#[tokio::test]
async fn test_a_stage_criteria_serve_references_variant() {
    let pool = init_pg_pool().await;

    // Query the test data to verify serve field references correct variants
    let stage_id = Uuid::parse_str("3eef17bc-9e06-411d-b5f4-7a786e68bb96").unwrap();

    let mut tx = pool.begin().await.expect("Transaction should start");
    let criteria = sqlx::query!(
        r#"SELECT id, context_key, rollout_percentage, serve
           FROM feature_stage_criteria
           WHERE stage_id = $1"#,
        stage_id
    )
    .fetch_all(&mut *tx)
    .await
    .expect("Should fetch criteria");

    tx.commit().await.expect("Transaction should commit");

    assert!(criteria.len() >= 2, "Should have at least 2 criteria from test data");

    // Verify serve fields from init.sql
    let treatment_a_criterion = criteria.iter().find(|c| c.serve == Some("treatment-a".to_string()));
    assert!(treatment_a_criterion.is_some());
    assert_eq!(treatment_a_criterion.unwrap().rollout_percentage, 50);

    let treatment_b_criterion = criteria.iter().find(|c| c.serve == Some("treatment-b".to_string()));
    assert!(treatment_b_criterion.is_some());
    assert_eq!(treatment_b_criterion.unwrap().rollout_percentage, 30);
}

#[tokio::test]
async fn test_variant_value_types() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool.clone());

    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let env_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();

    let random_key = format!("Type Test {}", Uuid::new_v4());
    let stage = CreateFeatureStage {
        id: Uuid::new_v4(),
        environment_id: env_id,
        order_index: 0,
        parent_stage: None,
        position: json!({"x": 100, "y": 100}).to_string(),
        enabled: true,
        bucketing_key: None,
    };

    // Test all variant value types
    let variants = vec![
        (
            "string_variant".to_string(),
            json!("Hello World"),
            VariantValueType::String,
            None,
        ),
        (
            "number_variant".to_string(),
            json!(42),
            VariantValueType::Number,
            None,
        ),
        (
            "boolean_variant".to_string(),
            json!(true),
            VariantValueType::Boolean,
            None,
        ),
        (
            "json_variant".to_string(),
            json!({"nested": {"key": "value"}, "array": [1, 2, 3]}),
            VariantValueType::Json,
            None,
        ),
    ];

    let input = CreateFeature {
        team_id,
        key: random_key,
        description: Some("Test all variant types".to_string()),
        feature_type: FeatureType::Simple,
        stages: vec![stage],
        dependencies: vec![],
        variants: Some(variants),
    };

    let feature_id = repository.create_feature(input).await.expect("Feature creation should succeed");

    // Verify all types were stored correctly
    let mut tx = pool.begin().await.expect("Transaction should start");
    let db_variants = sqlx::query!(
        r#"SELECT control, value, value_type as "value_type: VariantValueType"
           FROM feature_variants
           WHERE feature_id = $1
           ORDER BY control"#,
        feature_id
    )
    .fetch_all(&mut *tx)
    .await
    .expect("Should fetch variants");

    tx.commit().await.expect("Transaction should commit");

    assert_eq!(db_variants.len(), 4);

    let boolean = db_variants.iter().find(|v| v.control == "boolean_variant").unwrap();
    assert_eq!(boolean.value_type, VariantValueType::Boolean);
    assert_eq!(boolean.value, json!(true));

    let json_var = db_variants.iter().find(|v| v.control == "json_variant").unwrap();
    assert_eq!(json_var.value_type, VariantValueType::Json);
    assert!(json_var.value.is_object());

    let number = db_variants.iter().find(|v| v.control == "number_variant").unwrap();
    assert_eq!(number.value_type, VariantValueType::Number);
    assert_eq!(number.value, json!(42));

    let string = db_variants.iter().find(|v| v.control == "string_variant").unwrap();
    assert_eq!(string.value_type, VariantValueType::String);
    assert_eq!(string.value, json!("Hello World"));
}
