use feature_toggle_backend::Error;
use feature_toggle_backend::database::entity::{FeatureType, VariantSelectionMode};
use feature_toggle_backend::database::feature::{
    CreateFeature, CreateFeatureStage, CreateStageCriterion,
};
use feature_toggle_backend::database::{feature, init_pg_pool};
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn test_get_stage_criteria_returns_seeded_values() {
    let pool = init_pg_pool().await;
    let repo = feature::feature_repository(pool);

    // Known seeded stage id (from init.sql)
    let stage_id = Uuid::parse_str("3eef17bc-9e06-411d-b5f4-7a786e68bb96").unwrap();

    let result = repo.get_stage_criteria(stage_id).await;
    assert!(result.is_ok());
    let criteria = result.unwrap();

    assert!(!criteria.is_empty());
    assert!(criteria.iter().all(|c| c.stage_id == stage_id));

    let mut priorities: Vec<i32> = criteria.iter().map(|c| c.priority).collect();
    priorities.sort_unstable();
    assert_eq!(priorities, vec![0, 1, 2, 3]);
}

#[tokio::test]
async fn test_get_stage_criteria_empty() {
    let pool = init_pg_pool().await;
    let repo = feature::feature_repository(pool);

    // This stage id exists and is seeded but choose one without criteria
    let stage_without_criteria = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();

    let result = repo.get_stage_criteria(stage_without_criteria).await;
    assert!(result.is_ok());
    let criteria = result.unwrap();
    assert!(criteria.is_empty());
}

#[tokio::test]
async fn test_set_stage_criteria_replaces_existing() {
    let pool = init_pg_pool().await;
    let repo = feature::feature_repository(pool);

    // Create an isolated feature + stage for this test to avoid cross-test interference
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let env_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let stage_id = Uuid::new_v4();
    let feature_id = repo
        .create_feature(CreateFeature {
            team_id,
            key: format!("criteria-replace-{}", Uuid::new_v4()),
            description: None,
            feature_type: FeatureType::Simple,
            lifecycle_stage: "active".to_string(),
            owner: None,
            purpose: None,
            reference_url: None,
            expires_at: None,
            cleanup_reason: None,
            tags: vec![],
            stages: vec![CreateFeatureStage {
                id: stage_id,
                environment_id: env_id,
                order_index: 0,
                parent_stage: None,
                position: "{ x: 0, y: 0 }".to_string(),
                enabled: true,
            }],
            dependencies: vec![],
            variants: None,
        })
        .await
        .expect("feature to be created for criteria test");

    // First set some initial criteria
    let initial_crit = vec![
        CreateStageCriterion {
            priority: 0,
            variant_selection_mode: VariantSelectionMode::WeightedSplit,
            selected_variant_control: None,
        },
        CreateStageCriterion {
            priority: 1,
            variant_selection_mode: VariantSelectionMode::WeightedSplit,
            selected_variant_control: None,
        },
    ];

    let _ = repo.set_stage_criteria(stage_id, initial_crit).await;

    // Now replace them with a single criterion
    let crit = vec![CreateStageCriterion {
        priority: 0,
        variant_selection_mode: VariantSelectionMode::WeightedSplit,
        selected_variant_control: None,
    }];

    let set_result = repo.set_stage_criteria(stage_id, crit).await;
    assert!(set_result.is_ok());
    let updated = set_result.unwrap();

    // Should now be exactly 1 criterion
    assert_eq!(updated.len(), 1);
    let c = &updated[0];
    assert_eq!(c.stage_id, stage_id);

    // Cleanup
    let _ = repo.delete_feature(feature_id).await;
}

#[tokio::test]
async fn test_set_stage_criteria_stage_not_found() {
    let pool = init_pg_pool().await;
    let repo = feature::feature_repository(pool);

    let non_existing_stage = Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").unwrap();

    let crit = vec![CreateStageCriterion {
        priority: 0,
        variant_selection_mode: VariantSelectionMode::WeightedSplit,
        selected_variant_control: None,
    }];

    let result = repo.set_stage_criteria(non_existing_stage, crit).await;
    assert!(result.is_err());
    let err = result.err().unwrap();
    // Should be NotFound for the stage id
    match err {
        feature_toggle_backend::Error::NotFound(id) => assert_eq!(id, non_existing_stage),
        _ => panic!("expected NotFound error"),
    }
}

#[tokio::test]
async fn test_set_stage_criteria_rejects_variant_from_other_feature() {
    let pool = init_pg_pool().await;
    let repo = feature::feature_repository(pool);

    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let env_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let stage_id = Uuid::new_v4();

    let primary_feature = repo
        .create_feature(CreateFeature {
            team_id,
            key: format!("criteria-variant-primary-{}", Uuid::new_v4()),
            description: Some("feature with a valid local variant".to_string()),
            feature_type: FeatureType::Contextual,
            lifecycle_stage: "active".to_string(),
            owner: None,
            purpose: None,
            reference_url: None,
            expires_at: None,
            cleanup_reason: None,
            tags: vec![],
            stages: vec![CreateFeatureStage {
                id: stage_id,
                environment_id: env_id,
                order_index: 0,
                parent_stage: None,
                position: "{ x: 10, y: 10 }".to_string(),
                enabled: true,
            }],
            dependencies: vec![],
            variants: Some(vec![(
                "control".to_string(),
                json!({"enabled": true}),
                feature_toggle_backend::database::entity::VariantValueType::Json,
                Some("primary control variant".to_string()),
            )]),
        })
        .await
        .expect("primary feature should be created");

    let foreign_feature = repo
        .create_feature(CreateFeature {
            team_id,
            key: format!("criteria-variant-foreign-{}", Uuid::new_v4()),
            description: Some("feature with a foreign variant".to_string()),
            feature_type: FeatureType::Contextual,
            lifecycle_stage: "active".to_string(),
            owner: None,
            purpose: None,
            reference_url: None,
            expires_at: None,
            cleanup_reason: None,
            tags: vec![],
            stages: vec![],
            dependencies: vec![],
            variants: Some(vec![(
                "foreign-control".to_string(),
                json!({"enabled": false}),
                feature_toggle_backend::database::entity::VariantValueType::Json,
                Some("foreign control variant".to_string()),
            )]),
        })
        .await
        .expect("foreign feature should be created");

    let valid = repo
        .set_stage_criteria(
            stage_id,
            vec![CreateStageCriterion {
                priority: 0,
                variant_selection_mode: VariantSelectionMode::SpecificVariant,
                selected_variant_control: Some("control".to_string()),
            }],
        )
        .await
        .expect("local variant should be accepted");
    assert_eq!(valid.len(), 1);
    assert_eq!(
        valid[0].selected_variant_control.as_deref(),
        Some("control")
    );

    let invalid = repo
        .set_stage_criteria(
            stage_id,
            vec![CreateStageCriterion {
                priority: 0,
                variant_selection_mode: VariantSelectionMode::SpecificVariant,
                selected_variant_control: Some("foreign-control".to_string()),
            }],
        )
        .await;
    assert!(matches!(
        invalid,
        Err(Error::InvalidInput(message)) if message.contains("Selected variant")
    ));

    let after = repo
        .get_stage_criteria(stage_id)
        .await
        .expect("criteria should still be readable");
    assert_eq!(after.len(), 1);
    assert_eq!(
        after[0].selected_variant_control.as_deref(),
        Some("control")
    );

    let _ = repo.delete_feature(foreign_feature).await;
    let _ = repo.delete_feature(primary_feature).await;
}

/// Serializes rows for comparison. Rule-group order comes from a HashMap in
/// both the single and batch paths, so it is sorted by id; the order of the
/// rows themselves is compared by id as well where SQL does not define it.
fn comparable<T: serde::Serialize>(rows: &[T]) -> Vec<serde_json::Value> {
    let mut values: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            let mut value = serde_json::to_value(row).expect("serializable row");
            if let Some(groups) = value
                .get_mut("rule_groups")
                .and_then(serde_json::Value::as_array_mut)
            {
                groups.sort_by_key(|group| group["id"].to_string());
            }
            value
        })
        .collect();
    values.sort_by_key(|value| value["id"].to_string());
    values
}

#[tokio::test]
async fn test_batch_loaders_match_per_id_loaders_for_seeded_team() {
    let pool = init_pg_pool().await;
    let repo = feature::feature_repository(pool);
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();

    let features = repo
        .get_features(team_id, None, None)
        .await
        .expect("seeded features");
    let feature_ids: Vec<Uuid> = features.iter().map(|f| f.id).collect();

    // Stages
    let stages_by_feature = repo
        .get_feature_stages_batch(&feature_ids)
        .await
        .expect("stages batch");
    let mut stage_ids = Vec::new();
    for feature_id in &feature_ids {
        let single = repo
            .get_feature_stages(*feature_id)
            .await
            .expect("stages of one feature");
        let batch = stages_by_feature
            .get(feature_id)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            comparable(&batch),
            comparable(&single),
            "stages of {feature_id}"
        );
        stage_ids.extend(single.iter().map(|stage| stage.id));
    }
    assert!(!stage_ids.is_empty(), "seed data should have stages");

    // Criteria, priority order within each stage included
    let criteria_by_stage = repo
        .get_stage_criteria_batch(team_id, &stage_ids)
        .await
        .expect("criteria batch");
    let mut criteria_seen = 0;
    for stage_id in &stage_ids {
        let single = repo
            .get_stage_criteria(*stage_id)
            .await
            .expect("criteria of one stage");
        let batch = criteria_by_stage.get(stage_id).cloned().unwrap_or_default();
        let order = |rows: &[feature_toggle_backend::database::entity::StageCriterion]| {
            rows.iter().map(|c| (c.priority, c.id)).collect::<Vec<_>>()
        };
        assert_eq!(
            order(&batch),
            order(&single),
            "criteria order of {stage_id}"
        );
        assert_eq!(
            comparable(&batch),
            comparable(&single),
            "criteria of {stage_id}"
        );
        criteria_seen += single.len();
    }
    assert!(criteria_seen > 0, "seed data should have stage criteria");

    // Variants, created_at order within each feature included
    let variants_by_feature = repo
        .get_feature_variants_batch(&feature_ids)
        .await
        .expect("variants batch");
    for feature_id in &feature_ids {
        let single = repo
            .get_feature_variants(*feature_id)
            .await
            .expect("variants of one feature");
        let batch = variants_by_feature
            .get(feature_id)
            .cloned()
            .unwrap_or_default();
        let created = |rows: &[feature_toggle_backend::database::entity::FeatureVariant]| {
            rows.iter().map(|v| v.created_at).collect::<Vec<_>>()
        };
        assert_eq!(
            created(&batch),
            created(&single),
            "variant order of {feature_id}"
        );
        assert_eq!(
            comparable(&batch),
            comparable(&single),
            "variants of {feature_id}"
        );
    }

    // Empty input runs no query and returns nothing.
    assert!(repo.get_feature_stages_batch(&[]).await.unwrap().is_empty());
    assert!(
        repo.get_stage_criteria_batch(team_id, &[])
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        repo.get_feature_variants_batch(&[])
            .await
            .unwrap()
            .is_empty()
    );
}
