use crate::database::entity::Role;
use crate::database::role::RoleRepository;
use crate::Error;
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
    async fn assign_user_roles(&self, user_id: ID, role_ids: Vec<ID>, assigned_by: Option<ID>) -> Result<Vec<GqlRole>, Error>;
    async fn user_has_role(&self, user_id: ID, role_name: &str) -> Result<bool, Error>;
    fn clone_box(&self) -> Box<dyn RoleLogic>;
}

impl Clone for Box<dyn RoleLogic> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

pub fn role_logic(repository: Box<dyn RoleRepository>) -> Box<dyn RoleLogic> {
    Box::new(RoleLogicImpl { repository })
}

#[derive(Clone)]
struct RoleLogicImpl {
    repository: Box<dyn RoleRepository>,
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

    async fn assign_user_roles(&self, user_id: ID, role_ids: Vec<ID>, assigned_by: Option<ID>) -> Result<Vec<GqlRole>, Error> {
        let user_uuid = Uuid::try_from(user_id.clone()).map_err(|e| Error::InvalidInput(e.to_string()))?;
        let role_uuids: Result<Vec<Uuid>, _> = role_ids.iter().map(|id| Uuid::try_from(id.clone())).collect();
        let role_uuids = role_uuids.map_err(|e| Error::InvalidInput(e.to_string()))?;
        
        let assigned_by_uuid = if let Some(id) = assigned_by {
            Some(Uuid::try_from(id).map_err(|e| Error::InvalidInput(e.to_string()))?)
        } else {
            None
        };

        self.repository.assign_user_roles(user_uuid, role_uuids, assigned_by_uuid).await?;
        
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
    use crate::database::role::MockRoleRepository;
    use uuid::Uuid;

    #[tokio::test]
    async fn test_get_all_roles() {
        let mut mock_repo = MockRoleRepository::new();
        let expected_roles = vec![
            Role {
                id: Uuid::new_v4(),
                name: "Approver".to_string(),
                description: "Can approve deployment requests".to_string(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            },
        ];

        mock_repo.expect_get_all_roles()
            .times(1)
            .return_once(move || Ok(expected_roles));

        let logic = role_logic(Box::new(mock_repo));
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
        
        // Mock the assign operation
        mock_repo.expect_assign_user_roles()
            .times(1)
            .return_once(|_, _, _| Ok(()));
            
        // Mock the get_user_roles call that happens after assignment
        let expected_role = Role {
            id: Uuid::try_from(role_id.clone()).unwrap(),
            name: "Approver".to_string(),
            description: "Can approve deployment requests".to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        
        mock_repo.expect_get_user_roles()
            .times(1)
            .return_once(move |_| Ok(vec![expected_role]));

        let logic = role_logic(Box::new(mock_repo));
        let result = logic.assign_user_roles(user_id, role_ids, None).await.unwrap();
        
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "Approver");
    }
}
