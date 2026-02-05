use crate::model::{CreateRelationshipInput, ID};
use uuid::Uuid;

/// Trait for stage objects that can be used in relationship building
pub trait StageWithRelationship: Clone {
    fn order_index(&self) -> i32;
    fn set_parent_stage(&mut self, parent: Box<Self>);
}

/// Builds relationships between stages based on relationship definitions
/// This is shared logic used by both feature and pipeline stage creation
pub fn build_stage_relationships<T: StageWithRelationship>(
    mut stages: Vec<T>,
    relationships: Vec<CreateRelationshipInput>,
) -> Vec<T> {
    if relationships.is_empty() {
        return stages;
    }

    let cloned_stages = stages.clone();
    let relationships_map = relationships
        .iter()
        .filter_map(|relationship| {
            let stage = cloned_stages
                .iter()
                .find(|stage| stage.order_index() == relationship.source_id);
            stage.map(|s| (relationship.source_id, relationship.target_id, s))
        })
        .collect::<Vec<(i32, i32, &T)>>();

    for (_, target_id, parent_stage) in relationships_map {
        if let Some(target_stage) = stages.iter_mut().find(|s| s.order_index() == target_id) {
            target_stage.set_parent_stage(Box::new(parent_stage.clone()));
        }
    }

    stages
}

/// Helper function to safely convert GraphQL ID to Uuid
pub fn id_to_uuid(id: ID) -> Result<Uuid, crate::Error> {
    Uuid::try_from(id).map_err(|_| crate::Error::InvalidInput("Invalid UUID format".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::CreateRelationshipInput;

    #[derive(Debug, Clone, PartialEq)]
    struct MockStage {
        order_index: i32,
        parent_stage: Option<Box<MockStage>>,
    }

    impl StageWithRelationship for MockStage {
        fn order_index(&self) -> i32 {
            self.order_index
        }

        fn set_parent_stage(&mut self, parent: Box<Self>) {
            self.parent_stage = Some(parent);
        }
    }

    #[test]
    fn test_build_stage_relationships_no_relationships() {
        let stages = vec![
            MockStage {
                order_index: 0,
                parent_stage: None,
            },
            MockStage {
                order_index: 1,
                parent_stage: None,
            },
        ];

        let result = build_stage_relationships(stages, vec![]);
        assert_eq!(result.len(), 2);
        assert!(result[0].parent_stage.is_none());
        assert!(result[1].parent_stage.is_none());
    }

    #[test]
    fn test_build_stage_relationships_with_relationship() {
        let stages = vec![
            MockStage {
                order_index: 0,
                parent_stage: None,
            },
            MockStage {
                order_index: 1,
                parent_stage: None,
            },
        ];

        let relationships = vec![CreateRelationshipInput {
            source_id: 0,
            target_id: 1,
        }];

        let result = build_stage_relationships(stages, relationships);
        assert_eq!(result.len(), 2);
        assert!(result[0].parent_stage.is_none());
        assert!(result[1].parent_stage.is_some());
        assert_eq!(result[1].parent_stage.as_ref().unwrap().order_index, 0);
    }

    #[test]
    fn test_build_stage_relationships_invalid_relationship() {
        let stages = vec![MockStage {
            order_index: 0,
            parent_stage: None,
        }];

        let relationships = vec![CreateRelationshipInput {
            source_id: 99, // Non-existent source
            target_id: 0,
        }];

        let result = build_stage_relationships(stages, relationships);
        assert_eq!(result.len(), 1);
        assert!(result[0].parent_stage.is_none());
    }

    #[test]
    fn test_id_to_uuid_valid() {
        let uuid = Uuid::new_v4();
        let id = ID::from(uuid);
        let result = id_to_uuid(id).unwrap();
        assert_eq!(result, uuid);
    }

    #[test]
    fn test_id_to_uuid_invalid() {
        let id = ID::from("invalid-uuid");
        let result = id_to_uuid(id);
        assert!(result.is_err());
    }
}
