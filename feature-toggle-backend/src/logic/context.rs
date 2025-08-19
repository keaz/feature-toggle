use crate::Error;
use crate::database::context::{
    ContextRepository, CreateContextInput as DbCreate, UpdateContextInput as DbUpdate,
};
use crate::database::entity;
use crate::graphql::schema::{
    Context as GqlContext, ContextEntry as GqlContextEntry, CreateContextInput, UpdateContextInput,
};
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

pub fn context_logic(repository: Box<dyn ContextRepository>) -> Box<dyn ContextLogic> {
    Box::new(ContextLogicImpl { repository })
}

#[derive(Clone)]
struct ContextLogicImpl {
    repository: Box<dyn ContextRepository>,
}

#[async_trait::async_trait]
impl ContextLogic for ContextLogicImpl {
    async fn get_context_by_id(&self, id: ID) -> Result<GqlContext, Error> {
        let id = Uuid::try_from(id).unwrap();
        let ctx = self.repository.get_context_by_id(id).await?;
        Ok(map_db_to_gql(ctx))
    }

    async fn get_contexts(
        &self,
        team_id: ID,
        key: Option<String>,
    ) -> Result<Vec<GqlContext>, Error> {
        let team_id = Uuid::try_from(team_id).unwrap();
        let list = self.repository.get_contexts(team_id, key).await?;
        Ok(list.into_iter().map(map_db_to_gql).collect())
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
        let team_id = Uuid::try_from(team_id).unwrap();
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
        if let Some(k) = &input.key {
            if k.trim().is_empty() {
                return Err(Error::InvalidInput(
                    "Context key cannot be empty".to_string(),
                ));
            }
        }
        if let Some(entries) = &input.entries {
            let mut set = std::collections::HashSet::new();
            for v in entries {
                if !set.insert(v) {
                    return Err(Error::InvalidInput("Duplicate context entry".to_string()));
                }
            }
        }
        let id = Uuid::try_from(id).unwrap();
        let updated = self
            .repository
            .update_context(
                id,
                DbUpdate {
                    key: input.key,
                    entries: input.entries,
                },
            )
            .await?;
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
        let logic = super::context_logic(Box::new(MockContextRepository::new()));
        let input = CreateContextInput {
            key: "  ".into(),
            entries: vec!["A".into()],
        };
        let res = logic.create_context(ID::from(Uuid::new_v4()), input).await;
        assert!(matches!(res, Err(Error::InvalidInput(msg)) if msg.contains("cannot be empty")));
    }

    #[tokio::test]
    async fn create_context_rejects_duplicate_entries() {
        let logic = super::context_logic(Box::new(MockContextRepository::new()));
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
        let logic = super::context_logic(Box::new(MockContextRepository::new()));
        let input = UpdateContextInput {
            key: Some("".into()),
            entries: None,
        };
        let res = logic.update_context(ID::from(Uuid::new_v4()), input).await;
        assert!(matches!(res, Err(Error::InvalidInput(msg)) if msg.contains("cannot be empty")));
    }

    #[tokio::test]
    async fn update_context_rejects_duplicate_entries() {
        let logic = super::context_logic(Box::new(MockContextRepository::new()));
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

        let logic = super::context_logic(Box::new(repo));
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
        let logic = super::context_logic(Box::new(repo));
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
        let logic = super::context_logic(Box::new(repo));
        logic.delete_context(ID::from(id)).await.unwrap();
    }
}
