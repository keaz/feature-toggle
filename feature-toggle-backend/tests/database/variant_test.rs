use feature_toggle_backend::database::entity::VariantValueType;
use feature_toggle_backend::database::feature::{CreateFeature, CreateFeatureStage};
use feature_toggle_backend::database::{entity::FeatureType, feature, init_pg_pool};
use serde_json::json;
use uuid::Uuid;
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

    let feature_id = repository
        .create_feature(input)
        .await
        .expect("Feature creation should succeed");

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

    let boolean = db_variants
        .iter()
        .find(|v| v.control == "boolean_variant")
        .unwrap();
    assert_eq!(boolean.value_type, VariantValueType::Boolean);
    assert_eq!(boolean.value, json!(true));

    let json_var = db_variants
        .iter()
        .find(|v| v.control == "json_variant")
        .unwrap();
    assert_eq!(json_var.value_type, VariantValueType::Json);
    assert!(json_var.value.is_object());

    let number = db_variants
        .iter()
        .find(|v| v.control == "number_variant")
        .unwrap();
    assert_eq!(number.value_type, VariantValueType::Number);
    assert_eq!(number.value, json!(42));

    let string = db_variants
        .iter()
        .find(|v| v.control == "string_variant")
        .unwrap();
    assert_eq!(string.value_type, VariantValueType::String);
    assert_eq!(string.value, json!("Hello World"));
}
