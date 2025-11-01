use crate::Error;
use crate::database::context::{
    ContextRepository, CreateContextInput as DbCreate, UpdateContextInput as DbUpdate,
};
use crate::database::entity;
use crate::database::feature::FeatureRepository;
use crate::graphql::schema::{
    Context as GqlContext, ContextEntry as GqlContextEntry, CreateContextInput, UpdateContextInput,
};
use crate::logic::stage_builder::id_to_uuid;
use async_graphql::ID;
use mockall::automock;
use uuid::Uuid;

#[automock]
#[async_trait::async_trait]
pub trait ContextLogic: Send + Sync {
    async fn get_context_by_id(&self, id: ID) -> Result<GqlContext, Error>;
    async fn get_contexts(
        &self,
        team_id: ID,
        key: Option<String>,
    ) -> Result<Vec<GqlContext>, Error>;
    async fn get_contexts_paginated(
        &self,
        team_id: ID,
        key: Option<String>,
        page_number: i32,
        page_size: i32,
    ) -> Result<(Vec<GqlContext>, i64), Error>;
    async fn create_context(
        &self,
        team_id: ID,
        input: CreateContextInput,
    ) -> Result<GqlContext, Error>;
    async fn update_context(&self, id: ID, input: UpdateContextInput) -> Result<GqlContext, Error>;
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

// Helper to map DB Feature to gRPC FeatureFull using repository to load criteria
async fn map_db_feature_to_full_for_broadcast(
    repo: &dyn FeatureRepository,
    f: crate::database::entity::Feature,
) -> Result<crate::grpc::pb::FeatureFull, crate::Error> {
    use crate::grpc::pb;
    // stages with criterias
    let stages = repo.get_feature_stages(f.id).await?;
    let mut stage_msgs: Vec<pb::FeatureStageFull> = Vec::with_capacity(stages.len());
    for s in stages.iter() {
        let crits = repo.get_stage_criteria(s.id).await?;
        let criterias = crits
            .into_iter()
            .map(|c| pb::StageCriterionFull {
                id: c.id.to_string(),
                context_key: c.context_key,
                context: Some(pb::CriterionContext {
                    key: c.context.key,
                    entries: c.context.entries.into_iter().map(|e| e.value).collect(),
                }),
                rollout_percentage: c.rollout_percentage,
            })
            .collect::<Vec<_>>();

        stage_msgs.push(pb::FeatureStageFull {
            id: s.id.to_string(),
            environment_id: s.environment_id.to_string(),
            order_index: s.order_index,
            position: s.position.clone(),
            enabled: s.enabled,
            bucketing_key: s.bucketing_key.clone().unwrap_or_default(),
            criterias,
        });
    }

    let deps = f
        .dependencies
        .iter()
        .map(|d| pb::FeatureDependencyFull {
            feature_id: d.feature_id.to_string(),
            depends_on_id: d.depends_on_id.to_string(),
        })
        .collect::<Vec<_>>();

    let feature = pb::FeatureFull {
        id: f.id.to_string(),
        key: f.key,
        description: f.description.unwrap_or_default(),
        feature_type: format!("{:?}", f.feature_type),
        team_id: f.team_id.to_string(),
        active: f.active,
        created_at: f.created_at.to_rfc3339(),
        kill_switch_enabled: f.kill_switch_enabled,
        kill_switch_activated_at: f
            .kill_switch_activated_at
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default(),
        rollback_scheduled_at: f
            .rollback_scheduled_at
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default(),
        stages: stage_msgs,
        dependencies: deps,
    };
    Ok(feature)
}

#[async_trait::async_trait]
impl ContextLogic for ContextLogicImpl {
    async fn get_context_by_id(&self, id: ID) -> Result<GqlContext, Error> {
        let id = id_to_uuid(id)?;
        let ctx = self.repository.get_context_by_id(id).await?;
        Ok(map_db_to_gql(ctx))
    }

    async fn get_contexts(
        &self,
        team_id: ID,
        key: Option<String>,
    ) -> Result<Vec<GqlContext>, Error> {
        let team_id = id_to_uuid(team_id)?;
        let list = self.repository.get_contexts(team_id, key).await?;
        Ok(list.into_iter().map(map_db_to_gql).collect())
    }

    async fn get_contexts_paginated(
        &self,
        team_id: ID,
        key: Option<String>,
        page_number: i32,
        page_size: i32,
    ) -> Result<(Vec<GqlContext>, i64), Error> {
        let team_id = id_to_uuid(team_id)?;
        let (list, total) = self
            .repository
            .get_contexts_paginated(team_id, key, page_number, page_size)
            .await?;
        let contexts = list.into_iter().map(map_db_to_gql).collect();
        Ok((contexts, total))
    }

    async fn create_context(
        &self,
        team_id: ID,
        input: CreateContextInput,
    ) -> Result<GqlContext, Error> {
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
        Ok(map_db_to_gql(created))
    }

    async fn update_context(&self, id: ID, input: UpdateContextInput) -> Result<GqlContext, Error> {
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
        let id_uuid = Uuid::try_from(id.clone()).unwrap();
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
            for fid in feature_ids {
                if let Ok(db_feature) = self.feature_repo.get_feature_by_id(fid).await {
                    if let Ok(full) =
                        map_db_feature_to_full_for_broadcast(&*self.feature_repo, db_feature).await
                    {
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

        Ok(map_db_to_gql(updated))
    }

    async fn delete_context(&self, id: ID) -> Result<(), Error> {
        let id = Uuid::try_from(id).unwrap();
        self.repository.delete_context(id).await
    }

    fn clone_box(&self) -> Box<dyn ContextLogic> {
        Box::new(self.clone())
    }
}

fn map_db_to_gql(c: entity::Context) -> GqlContext {
    GqlContext {
        id: ID::from(c.id),
        team_id: ID::from(c.team_id),
        key: c.key,
        entries: c
            .entries
            .into_iter()
            .map(|e| GqlContextEntry {
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
