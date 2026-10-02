use crate::Error;
use crate::database::context::{
    ContextRepository, CreateContextInput as DbCreate, UpdateContextInput as DbUpdate,
};
use crate::database::entity;
use crate::database::feature::FeatureRepository;
use crate::logic::stage_builder::id_to_uuid;
use crate::model::ID;
use crate::model::{
    Context as ModelContext, ContextEntry as ModelContextEntry, CreateContextInput,
    UpdateContextInput,
};
use mockall::automock;
use uuid::Uuid;

#[automock]
#[async_trait::async_trait]
pub trait ContextLogic: Send + Sync {
    async fn get_context_by_id(&self, id: ID) -> Result<ModelContext, Error>;
    async fn get_contexts(
        &self,
        team_id: ID,
        key: Option<String>,
    ) -> Result<Vec<ModelContext>, Error>;
    async fn get_contexts_paginated(
        &self,
        team_id: ID,
        key: Option<String>,
        page_number: i32,
        page_size: i32,
    ) -> Result<(Vec<ModelContext>, i64), Error>;
    async fn get_contexts_with_offset(
        &self,
        team_id: ID,
        key: Option<String>,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<ModelContext>, i64), Error>;
    async fn create_context(
        &self,
        team_id: ID,
        input: CreateContextInput,
    ) -> Result<ModelContext, Error>;
    async fn update_context(
        &self,
        id: ID,
        input: UpdateContextInput,
    ) -> Result<ModelContext, Error>;
    async fn delete_context(&self, id: ID) -> Result<(), Error>;
    fn clone_box(&self) -> Box<dyn ContextLogic>;
}

impl Clone for Box<dyn ContextLogic> {
    fn clone(&self) -> Box<dyn ContextLogic> {
        self.clone_box()
    }
}

pub fn context_logic(
    repository: Box<dyn ContextRepository>,
    feature_repo: Box<dyn FeatureRepository>,
    updates_tx: tokio::sync::broadcast::Sender<crate::grpc::pb::FeatureUpdate>,
) -> Box<dyn ContextLogic> {
    Box::new(ContextLogicImpl {
        repository,
        feature_repo,
        updates_tx,
    })
}

#[derive(Clone)]
struct ContextLogicImpl {
    repository: Box<dyn ContextRepository>,
    feature_repo: Box<dyn FeatureRepository>,
    updates_tx: tokio::sync::broadcast::Sender<crate::grpc::pb::FeatureUpdate>,
}

impl ContextLogicImpl {
    /// Sends an Upsert for each of the features, in the given order. Features
    /// and their child rows are loaded in batches; a batch that fails to load
    /// is skipped.
    async fn broadcast_feature_upserts(&self, feature_ids: Vec<Uuid>) {
        for chunk in feature_ids.chunks(crate::grpc::SNAPSHOT_MAPPING_BATCH_SIZE) {
            let mut by_id = match self.feature_repo.get_features_by_ids(chunk).await {
                Ok(features) => features
                    .into_iter()
                    .map(|feature| (feature.id, feature))
                    .collect::<std::collections::HashMap<_, _>>(),
                Err(e) => {
                    log::warn!("Failed to load features for context update broadcast: {e}");
                    continue;
                }
            };
            let features = chunk
                .iter()
                .filter_map(|id| by_id.remove(id))
                .collect::<Vec<_>>();
            let mapped =
                match crate::grpc::map_features_to_full(&*self.feature_repo, features).await {
                    Ok(mapped) => mapped,
                    Err(e) => {
                        log::warn!("Failed to map features for context update broadcast: {e}");
                        continue;
                    }
                };
            for full in mapped {
                let _ = self.updates_tx.send(crate::grpc::pb::FeatureUpdate {
                    message_id: uuid::Uuid::new_v4().to_string(),
                    action: crate::grpc::pb::feature_update::Action::Upsert as i32,
                    feature: Some(full),
                    feature_key: String::new(),
                    error: String::new(),
                });
            }
        }
    }
}

#[async_trait::async_trait]
impl ContextLogic for ContextLogicImpl {
    async fn get_context_by_id(&self, id: ID) -> Result<ModelContext, Error> {
        let id = id_to_uuid(id)?;
        let ctx = self.repository.get_context_by_id(id).await?;
        Ok(map_db_to_model(ctx))
    }

    async fn get_contexts(
        &self,
        team_id: ID,
        key: Option<String>,
    ) -> Result<Vec<ModelContext>, Error> {
        let team_id = id_to_uuid(team_id)?;
        let list = self.repository.get_contexts(team_id, key).await?;
        Ok(list.into_iter().map(map_db_to_model).collect())
    }

    async fn get_contexts_paginated(
        &self,
        team_id: ID,
        key: Option<String>,
        page_number: i32,
        page_size: i32,
    ) -> Result<(Vec<ModelContext>, i64), Error> {
        let team_id = id_to_uuid(team_id)?;
        let (list, total) = self
            .repository
            .get_contexts_paginated(team_id, key, page_number, page_size)
            .await?;
        let contexts = list.into_iter().map(map_db_to_model).collect();
        Ok((contexts, total))
    }

    async fn get_contexts_with_offset(
        &self,
        team_id: ID,
        key: Option<String>,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<ModelContext>, i64), Error> {
        let team_id = id_to_uuid(team_id)?;
        let (list, total) = self
            .repository
            .get_contexts_with_offset(team_id, key, offset, limit)
            .await?;
        let contexts = list.into_iter().map(map_db_to_model).collect();
        Ok((contexts, total))
    }

    async fn create_context(
        &self,
        team_id: ID,
        input: CreateContextInput,
    ) -> Result<ModelContext, Error> {
        // Basic validation
        if input.key.trim().is_empty() {
            return Err(Error::InvalidInput(
                "Context key cannot be empty".to_string(),
            ));
        }
        let mut set = std::collections::HashSet::new();
        for v in &input.entries {
            if !set.insert(v) {
                return Err(Error::InvalidInput("Duplicate context entry".to_string()));
            }
        }
        let team_id = id_to_uuid(team_id)?;
        let created = self
            .repository
            .create_context(
                team_id,
                DbCreate {
                    key: input.key,
                    entries: input.entries,
                },
            )
            .await?;
        Ok(map_db_to_model(created))
    }

    async fn update_context(
        &self,
        id: ID,
        input: UpdateContextInput,
    ) -> Result<ModelContext, Error> {
        if let Some(k) = &input.key
            && k.trim().is_empty()
        {
            return Err(Error::InvalidInput(
                "Context key cannot be empty".to_string(),
            ));
        }
        if let Some(entries) = &input.entries {
            let mut set = std::collections::HashSet::new();
            for v in entries {
                if !set.insert(v) {
                    return Err(Error::InvalidInput("Duplicate context entry".to_string()));
                }
            }
        }
        let id_uuid = Uuid::try_from(id).map_err(|e| Error::InvalidInput(e.to_string()))?;
        let updated = self
            .repository
            .update_context(
                id_uuid,
                DbUpdate {
                    key: input.key,
                    entries: input.entries,
                },
            )
            .await?;

        // After successful update, broadcast FeatureFull UPSERTs for all features referencing this context
        if self.updates_tx.receiver_count() > 0
            && let Ok(feature_ids) = self
                .feature_repo
                .get_feature_ids_by_context_id(id_uuid)
                .await
        {
            self.broadcast_feature_upserts(feature_ids).await;
        }

        Ok(map_db_to_model(updated))
    }

    async fn delete_context(&self, id: ID) -> Result<(), Error> {
        let id = Uuid::try_from(id).map_err(|e| Error::InvalidInput(e.to_string()))?;
        self.repository.delete_context(id).await
    }

    fn clone_box(&self) -> Box<dyn ContextLogic> {
        Box::new(self.clone())
    }
}

fn map_db_to_model(c: entity::Context) -> ModelContext {
    ModelContext {
        id: ID::from(c.id),
        team_id: ID::from(c.team_id),
        key: c.key,
        entries: c
            .entries
            .into_iter()
            .map(|e| ModelContextEntry {
                id: ID::from(e.id),
                value: e.value,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::context::MockContextRepository;
    use crate::database::entity::{Context as DbContext, ContextEntry as DbContextEntry};
    use crate::database::feature::MockFeatureRepository;
    use crate::grpc::pb;

    fn sample_db_context(team_id: Uuid) -> DbContext {
        DbContext {
            id: Uuid::new_v4(),
            team_id,
            key: "country".into(),
            entries: vec![
                DbContextEntry {
                    id: Uuid::new_v4(),
                    value: "US".into(),
                },
                DbContextEntry {
                    id: Uuid::new_v4(),
                    value: "UK".into(),
                },
            ],
        }
    }

    #[tokio::test]
    async fn create_context_rejects_empty_key() {
        let (tx, rx) = tokio::sync::broadcast::channel::<crate::grpc::pb::FeatureUpdate>(8);
        drop(rx);
        let logic = super::context_logic(
            Box::new(MockContextRepository::new()),
            Box::new(MockFeatureRepository::new()),
            tx,
        );
        let input = CreateContextInput {
            key: "  ".into(),
            entries: vec!["A".into()],
        };
        let res = logic.create_context(ID::from(Uuid::new_v4()), input).await;
        assert!(matches!(res, Err(Error::InvalidInput(msg)) if msg.contains("cannot be empty")));
    }

    #[tokio::test]
    async fn create_context_rejects_duplicate_entries() {
        let (tx, rx) = tokio::sync::broadcast::channel::<crate::grpc::pb::FeatureUpdate>(8);
        drop(rx);
        let logic = super::context_logic(
            Box::new(MockContextRepository::new()),
            Box::new(MockFeatureRepository::new()),
            tx,
        );
        let input = CreateContextInput {
            key: "k".into(),
            entries: vec!["A".into(), "A".into()],
        };
        let res = logic.create_context(ID::from(Uuid::new_v4()), input).await;
        assert!(
            matches!(res, Err(Error::InvalidInput(msg)) if msg.contains("Duplicate context entry"))
        );
    }

    #[tokio::test]
    async fn update_context_rejects_empty_key() {
        let (tx, rx) = tokio::sync::broadcast::channel::<crate::grpc::pb::FeatureUpdate>(8);
        drop(rx);
        let logic = super::context_logic(
            Box::new(MockContextRepository::new()),
            Box::new(MockFeatureRepository::new()),
            tx,
        );
        let input = UpdateContextInput {
            key: Some("".into()),
            entries: None,
        };
        let res = logic.update_context(ID::from(Uuid::new_v4()), input).await;
        assert!(matches!(res, Err(Error::InvalidInput(msg)) if msg.contains("cannot be empty")));
    }

    #[tokio::test]
    async fn update_context_rejects_duplicate_entries() {
        let (tx, rx) = tokio::sync::broadcast::channel::<crate::grpc::pb::FeatureUpdate>(8);
        drop(rx);
        let logic = super::context_logic(
            Box::new(MockContextRepository::new()),
            Box::new(MockFeatureRepository::new()),
            tx,
        );
        let input = UpdateContextInput {
            key: None,
            entries: Some(vec!["X".into(), "X".into()]),
        };
        let res = logic.update_context(ID::from(Uuid::new_v4()), input).await;
        assert!(
            matches!(res, Err(Error::InvalidInput(msg)) if msg.contains("Duplicate context entry"))
        );
    }

    #[tokio::test]
    async fn create_context_calls_repository_and_maps() {
        let mut repo = MockContextRepository::new();
        let team_id = Uuid::new_v4();
        let expected_key = "country".to_string();
        let expected_key_for_match = expected_key.clone();
        let team_id_s = team_id.to_string();
        repo.expect_create_context()
            .withf(move |tid, ci| {
                tid.to_string() == team_id_s
                    && ci.key == expected_key_for_match
                    && ci.entries.len() == 2
            })
            .times(1)
            .returning(|tid, _| Ok(sample_db_context(tid)));

        let (tx, rx) = tokio::sync::broadcast::channel::<crate::grpc::pb::FeatureUpdate>(8);
        drop(rx);
        let logic =
            super::context_logic(Box::new(repo), Box::new(MockFeatureRepository::new()), tx);
        let input = CreateContextInput {
            key: expected_key.clone(),
            entries: vec!["US".into(), "UK".into()],
        };
        let out = logic
            .create_context(ID::from(team_id), input)
            .await
            .unwrap();
        assert_eq!(out.key, expected_key);
        assert_eq!(out.entries.len(), 2);
    }

    #[tokio::test]
    async fn update_context_calls_repository_and_maps() {
        let mut repo = MockContextRepository::new();
        let id = Uuid::new_v4();
        let ctx = sample_db_context(Uuid::new_v4());
        let ctx_id = id;
        // For update, repository returns updated context
        repo.expect_update_context()
            .times(1)
            .returning(move |_id, _| {
                Ok(DbContext {
                    id: ctx_id,
                    ..ctx.clone()
                })
            });
        let (tx, rx) = tokio::sync::broadcast::channel::<crate::grpc::pb::FeatureUpdate>(8);
        drop(rx);
        let logic =
            super::context_logic(Box::new(repo), Box::new(MockFeatureRepository::new()), tx);
        let input = UpdateContextInput {
            key: Some("country".into()),
            entries: Some(vec!["US".into()]),
        };
        let out = logic.update_context(ID::from(id), input).await.unwrap();
        assert_eq!(out.key, "country");
    }

    #[tokio::test]
    async fn update_context_broadcasts_feature_updates() {
        let mut repo = MockContextRepository::new();
        let mut feature_repo = MockFeatureRepository::new();
        let ctx_id = Uuid::new_v4();
        let team_id = Uuid::new_v4();
        let feature_id = Uuid::new_v4();

        // Update returns the new context
        repo.expect_update_context().returning(move |_id, _| {
            Ok(DbContext {
                id: ctx_id,
                team_id,
                key: "user.tier".into(),
                entries: vec![],
            })
        });

        // Feature IDs referencing this context
        feature_repo
            .expect_get_feature_ids_by_context_id()
            .returning(move |_| Ok(vec![feature_id]));

        // Feature fetch for broadcast
        feature_repo
            .expect_get_features_by_ids()
            .returning(move |_| {
                Ok(vec![entity::Feature {
                    id: feature_id,
                    key: "example".into(),
                    description: None,
                    feature_type: entity::FeatureType::Contextual,
                    team_id,
                    active: true,
                    created_at: chrono::Utc::now(),
                    kill_switch_enabled: false,
                    kill_switch_activated_at: None,
                    rollback_scheduled_at: None,
                    emergency_override_reason: None,
                    emergency_override_expires_at: None,
                    emergency_override_actor_id: None,
                    emergency_override_applied_at: None,
                    lifecycle_stage: "active".to_string(),
                    owner: None,
                    purpose: None,
                    reference_url: None,
                    expires_at: None,
                    cleanup_reason: None,
                    tags: vec![],
                    archived_at: None,
                    deprecated_at: None,
                    deprecation_notice: None,
                    last_evaluated_at: None,
                    evaluation_count_7d: 0,
                    evaluation_count_30d: 0,
                    evaluation_count_90d: 0,
                    dependencies: vec![],
                    flag_kind: None,
                    flag_kind_confidence: None,
                    flag_kind_source: None,
                }])
            });

        feature_repo
            .expect_get_feature_stages_batch()
            .returning(|_| Ok(std::collections::HashMap::new()));
        feature_repo
            .expect_get_feature_variants_batch()
            .returning(|_| Ok(std::collections::HashMap::new()));

        let (tx, mut rx) = tokio::sync::broadcast::channel::<pb::FeatureUpdate>(4);
        let logic = super::context_logic(Box::new(repo), Box::new(feature_repo), tx.clone());

        let _ = logic
            .update_context(
                ID::from(ctx_id),
                UpdateContextInput {
                    key: Some("user.tier".into()),
                    entries: Some(vec!["gold".into()]),
                },
            )
            .await
            .expect("update should succeed");

        let msg = rx.recv().await.expect("expected broadcast");
        assert_eq!(msg.action, pb::feature_update::Action::Upsert as i32);
        assert!(msg.feature.is_some(), "feature payload missing");
    }

    #[tokio::test]
    async fn delete_context_calls_repository() {
        let mut repo = MockContextRepository::new();
        let id = Uuid::new_v4();
        let id_s = id.to_string();
        repo.expect_delete_context()
            .withf(move |i| i.to_string() == id_s)
            .times(1)
            .returning(|_| Ok(()));
        let (tx, rx) = tokio::sync::broadcast::channel::<crate::grpc::pb::FeatureUpdate>(8);
        drop(rx);
        let logic =
            super::context_logic(Box::new(repo), Box::new(MockFeatureRepository::new()), tx);
        logic.delete_context(ID::from(id)).await.unwrap();
    }

    #[tokio::test]
    async fn test_get_contexts_paginated_success() {
        let mut repo = MockContextRepository::new();
        let team_id = Uuid::new_v4();
        let context1_id = Uuid::new_v4();
        let context2_id = Uuid::new_v4();

        let expected_contexts = vec![
            DbContext {
                id: context1_id,
                team_id,
                key: "country".into(),
                entries: vec![DbContextEntry {
                    id: Uuid::new_v4(),
                    value: "US".into(),
                }],
            },
            DbContext {
                id: context2_id,
                team_id,
                key: "language".into(),
                entries: vec![
                    DbContextEntry {
                        id: Uuid::new_v4(),
                        value: "en".into(),
                    },
                    DbContextEntry {
                        id: Uuid::new_v4(),
                        value: "es".into(),
                    },
                ],
            },
        ];

        repo.expect_get_contexts_paginated()
            .with(
                mockall::predicate::eq(team_id),
                mockall::predicate::eq(None::<String>),
                mockall::predicate::eq(1),
                mockall::predicate::eq(10),
            )
            .times(1)
            .returning(move |_, _, _, _| Ok((expected_contexts.clone(), 25)));

        let (tx, rx) = tokio::sync::broadcast::channel::<crate::grpc::pb::FeatureUpdate>(8);
        drop(rx);
        let logic =
            super::context_logic(Box::new(repo), Box::new(MockFeatureRepository::new()), tx);

        let (contexts, total) = logic
            .get_contexts_paginated(ID::from(team_id), None, 1, 10)
            .await
            .unwrap();

        assert_eq!(contexts.len(), 2);
        assert_eq!(total, 25);
        assert_eq!(contexts[0].key, "country");
        assert_eq!(contexts[0].entries.len(), 1);
        assert_eq!(contexts[1].key, "language");
        assert_eq!(contexts[1].entries.len(), 2);
    }

    #[tokio::test]
    async fn test_get_contexts_paginated_with_key_filter() {
        let mut repo = MockContextRepository::new();
        let team_id = Uuid::new_v4();

        repo.expect_get_contexts_paginated()
            .with(
                mockall::predicate::eq(team_id),
                mockall::predicate::eq(Some("country".to_string())),
                mockall::predicate::eq(2),
                mockall::predicate::eq(5),
            )
            .times(1)
            .returning(|_, _, _, _| Ok((vec![], 0)));

        let (tx, rx) = tokio::sync::broadcast::channel::<crate::grpc::pb::FeatureUpdate>(8);
        drop(rx);
        let logic =
            super::context_logic(Box::new(repo), Box::new(MockFeatureRepository::new()), tx);

        let (contexts, total) = logic
            .get_contexts_paginated(ID::from(team_id), Some("country".to_string()), 2, 5)
            .await
            .unwrap();

        assert_eq!(contexts.len(), 0);
        assert_eq!(total, 0);
    }

    #[tokio::test]
    async fn test_get_contexts_paginated_invalid_team_id() {
        let repo = MockContextRepository::new();
        let (tx, rx) = tokio::sync::broadcast::channel::<crate::grpc::pb::FeatureUpdate>(8);
        drop(rx);
        let logic =
            super::context_logic(Box::new(repo), Box::new(MockFeatureRepository::new()), tx);

        let result = logic
            .get_contexts_paginated(ID::from("invalid-uuid"), None, 1, 10)
            .await;

        assert!(matches!(result, Err(Error::InvalidInput(_))));
    }
}
