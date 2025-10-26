use crate::Error;
use crate::database::entity::Role;
use crate::database::role::RoleRepository;
use async_graphql::ID;
use mockall::automock;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct GqlRole {
    pub id: ID,
    pub name: String,
    pub description: String,
    pub created_at: String,
    pub updated_at: String,
}

impl From<Role> for GqlRole {
    fn from(role: Role) -> Self {
        GqlRole {
            id: ID::from(role.id),
            name: role.name,
            description: role.description,
            created_at: role.created_at.to_rfc3339(),
            updated_at: role.updated_at.to_rfc3339(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AssignRolesInput {
    pub user_id: ID,
    pub role_ids: Vec<ID>,
}

#[automock]
#[async_trait::async_trait]
pub trait RoleLogic: Send + Sync {
    async fn get_all_roles(&self) -> Result<Vec<GqlRole>, Error>;
    async fn get_role_by_id(&self, id: ID) -> Result<GqlRole, Error>;
    async fn get_user_roles(&self, user_id: ID) -> Result<Vec<GqlRole>, Error>;
    async fn assign_user_roles(
        &self,
        user_id: ID,
        role_ids: Vec<ID>,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<Vec<GqlRole>, Error>;
    async fn user_has_role(&self, user_id: ID, role_name: &str) -> Result<bool, Error>;
    fn clone_box(&self) -> Box<dyn RoleLogic>;
}

impl Clone for Box<dyn RoleLogic> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

pub fn role_logic(
    repository: Box<dyn RoleRepository>,
    activity_log_repository: Box<dyn crate::database::activity_log::ActivityLogRepository>,
) -> Box<dyn RoleLogic> {
    Box::new(RoleLogicImpl {
        repository,
        activity_log_repository,
    })
}

struct RoleLogicImpl {
    repository: Box<dyn RoleRepository>,
    activity_log_repository: Box<dyn crate::database::activity_log::ActivityLogRepository>,
}

impl Clone for RoleLogicImpl {
    fn clone(&self) -> Self {
        Self {
            repository: self.repository.clone_box(),
            activity_log_repository: self.activity_log_repository.clone_box(),
        }
    }
}

#[async_trait::async_trait]
impl RoleLogic for RoleLogicImpl {
    async fn get_all_roles(&self) -> Result<Vec<GqlRole>, Error> {
        let roles = self.repository.get_all_roles().await?;
        Ok(roles.into_iter().map(GqlRole::from).collect())
    }

    async fn get_role_by_id(&self, id: ID) -> Result<GqlRole, Error> {
        let uuid_id = Uuid::try_from(id).map_err(|e| Error::InvalidInput(e.to_string()))?;
        let role = self.repository.get_role_by_id(uuid_id).await?;
        Ok(GqlRole::from(role))
    }

    async fn get_user_roles(&self, user_id: ID) -> Result<Vec<GqlRole>, Error> {
        let uuid_id = Uuid::try_from(user_id).map_err(|e| Error::InvalidInput(e.to_string()))?;
        let roles = self.repository.get_user_roles(uuid_id).await?;
        Ok(roles.into_iter().map(GqlRole::from).collect())
    }

    async fn assign_user_roles(
        &self,
        user_id: ID,
        role_ids: Vec<ID>,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<Vec<GqlRole>, Error> {
        let user_uuid =
            Uuid::try_from(user_id.clone()).map_err(|e| Error::InvalidInput(e.to_string()))?;
        let role_uuids: Result<Vec<Uuid>, _> = role_ids
            .iter()
            .map(|id| Uuid::try_from(id.clone()))
            .collect();
        let role_uuids = role_uuids.map_err(|e| Error::InvalidInput(e.to_string()))?;

        // Extract actor information for repository call (needs Uuid) and activity logging
        let (actor_id, actor_name) = actor
            .as_ref()
            .map(|a| a.as_option())
            .unwrap_or((None, None));

        self.repository
            .assign_user_roles(user_uuid, role_uuids.clone(), actor_id)
            .await?;

        // Log activity for each role assignment (ignore errors to not fail the operation)
        for role_uuid in role_uuids {
            // Get role name for better logging
            if let Ok(role) = self.repository.get_role_by_id(role_uuid).await {
                let _ = crate::utils::activity_logger::log_role_activity(
                    &self.activity_log_repository,
                    crate::utils::activity_logger::activity_types::ROLE_ASSIGNED,
                    &user_uuid.to_string(),
                    actor_id,
                    actor_name.clone(),
                    format!("Assigned role '{}' to user", role.name),
                    Some(serde_json::json!({
                        "user_id": user_uuid.to_string(),
                        "role_id": role_uuid.to_string(),
                        "role_name": role.name,
                    })),
                )
                .await;
            }
        }

        // Return the updated roles for the user
        self.get_user_roles(user_id).await
    }

    async fn user_has_role(&self, user_id: ID, role_name: &str) -> Result<bool, Error> {
        let uuid_id = Uuid::try_from(user_id).map_err(|e| Error::InvalidInput(e.to_string()))?;
        self.repository.user_has_role(uuid_id, role_name).await
    }

    fn clone_box(&self) -> Box<dyn RoleLogic> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::activity_log::{ActivityLogRepository, MockActivityLogRepository};
    use crate::database::role::MockRoleRepository;
    use uuid::Uuid;

    fn create_mock_activity_log() -> Box<dyn ActivityLogRepository> {
        let mut mock = MockActivityLogRepository::new();
        mock.expect_create_activity().returning(|_| {
            Ok(crate::database::activity_log::ActivityLogRow {
                id: uuid::Uuid::new_v4(),
                activity_type: "TEST".to_string(),
                entity_type: "test".to_string(),
                entity_id: "test".to_string(),
                actor_id: None,
                actor_name: None,
                description: "test".to_string(),
                metadata: None,
                created_at: chrono::Utc::now(),
            })
        });
        mock.expect_clone_box()
            .returning(|| create_mock_activity_log());
        Box::new(mock)
    }

    #[tokio::test]
    async fn test_get_all_roles() {
        let mut mock_repo = MockRoleRepository::new();
        let expected_roles = vec![Role {
            id: Uuid::new_v4(),
            name: "Approver".to_string(),
            description: "Can approve deployment requests".to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }];

        mock_repo
            .expect_get_all_roles()
            .times(1)
            .return_once(move || Ok(expected_roles));

        let logic = role_logic(Box::new(mock_repo), create_mock_activity_log());
        let result = logic.get_all_roles().await.unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "Approver");
    }

    #[tokio::test]
    async fn test_assign_user_roles() {
        let mut mock_repo = MockRoleRepository::new();
        let user_id = ID::from(Uuid::new_v4());
        let role_id = ID::from(Uuid::new_v4());
        let role_ids = vec![role_id.clone()];
        let role_uuid = Uuid::try_from(role_id.clone()).unwrap();

        // Mock the assign operation
        mock_repo
            .expect_assign_user_roles()
            .times(1)
            .return_once(|_, _, _| Ok(()));

        // Mock get_role_by_id (called for activity logging)
        mock_repo
            .expect_get_role_by_id()
            .times(1)
            .return_once(move |_| {
                Ok(Role {
                    id: role_uuid,
                    name: "Approver".to_string(),
                    description: "Can approve deployment requests".to_string(),
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                })
            });

        // Mock the get_user_roles call that happens after assignment
        let expected_role = Role {
            id: Uuid::try_from(role_id.clone()).unwrap(),
            name: "Approver".to_string(),
            description: "Can approve deployment requests".to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        mock_repo
            .expect_get_user_roles()
            .times(1)
            .return_once(move |_| Ok(vec![expected_role]));

        let logic = role_logic(Box::new(mock_repo), create_mock_activity_log());
        let result = logic
            .assign_user_roles(user_id, role_ids, None)
            .await
            .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "Approver");
    }
}
