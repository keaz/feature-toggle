use crate::Error;
use crate::database::client::ClientRepository;
use crate::database::user_flag_assignment::{UserFlagAssignmentRepository, UserFlagAssignmentRow};
use mockall::automock;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum UserFlagLogicError {
    #[error("Invalid input: {0}")]
    InvalidInput(String),
    #[error("Not found: {0}")]
    NotFound(Uuid),
    #[error("Unauthenticated: {0}")]
    Unauthenticated(String),
    #[error("Permission denied: {0}")]
    PermissionDenied(String),
    #[error("Database error")]
    DatabaseError(#[from] crate::Error),
}

#[automock]
#[async_trait::async_trait]
pub trait UserFlagLogic: Send + Sync {
    // Validates client_id/client_secret and returns the team_id if OK
    async fn authenticate_client(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> Result<Uuid, UserFlagLogicError>;

    // Upsert a single assignment after successful authentication
    async fn upsert_after_auth(
        &self,
        user_id: &str,
        feature_id: &str,
        environment_id: &str,
        assigned: bool,
        variant: Option<String>,
    ) -> Result<(), UserFlagLogicError>;

    // List assignments scoped by client's team; feature/environment ids are optional strings
    async fn list_user_assignments(
        &self,
        team_id: Uuid,
        feature_id: Option<String>,
        environment_id: Option<String>,
    ) -> Result<Vec<UserFlagAssignmentRow>, UserFlagLogicError>;

    fn clone_box(&self) -> Box<dyn UserFlagLogic>;
}

impl Clone for Box<dyn UserFlagLogic> {
    fn clone(&self) -> Box<dyn UserFlagLogic> {
        self.clone_box()
    }
}

pub fn user_flag_logic(
    client_repo: Box<dyn ClientRepository>,
    user_flag_repo: Box<dyn UserFlagAssignmentRepository>,
) -> Box<dyn UserFlagLogic> {
    Box::new(UserFlagLogicImpl::new(client_repo, user_flag_repo))
}

pub struct UserFlagLogicImpl {
    client_repo: Box<dyn ClientRepository>,
    user_flag_repo: Box<dyn UserFlagAssignmentRepository>,
}

impl UserFlagLogicImpl {
    pub fn new(
        client_repo: Box<dyn ClientRepository>,
        user_flag_repo: Box<dyn UserFlagAssignmentRepository>,
    ) -> Self {
        Self {
            client_repo,
            user_flag_repo,
        }
    }

    fn parse_uuid(label: &str, value: &str) -> Result<Uuid, UserFlagLogicError> {
        Uuid::parse_str(value)
            .map_err(|_| UserFlagLogicError::InvalidInput(format!("{label} must be a UUID")))
    }
}

#[async_trait::async_trait]
impl UserFlagLogic for UserFlagLogicImpl {
    async fn authenticate_client(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> Result<Uuid, UserFlagLogicError> {
        if client_id.is_empty() || client_secret.is_empty() {
            return Err(UserFlagLogicError::InvalidInput(
                "client_id and client_secret are required".to_string(),
            ));
        }
        let cid = Self::parse_uuid("client_id", client_id)?;
        let client = self
            .client_repo
            .get_client_by_id(cid)
            .await
            .map_err(|e| match e {
                Error::NotFound(id) => UserFlagLogicError::NotFound(id),
                other => UserFlagLogicError::DatabaseError(other),
            })?;
        if !client.enabled {
            return Err(UserFlagLogicError::PermissionDenied(
                "client is disabled".to_string(),
            ));
        }
        if client.api_key != client_secret {
            return Err(UserFlagLogicError::Unauthenticated(
                "invalid client_secret".to_string(),
            ));
        }
        Ok(client.team_id)
    }

    async fn upsert_after_auth(
        &self,
        user_id: &str,
        feature_id: &str,
        environment_id: &str,
        assigned: bool,
        variant: Option<String>,
    ) -> Result<(), UserFlagLogicError> {
        if user_id.is_empty() || feature_id.is_empty() || environment_id.is_empty() {
            // no-op consistent with gRPC code that simply skipped empty ones
            return Ok(());
        }
        let fid = Self::parse_uuid("feature_id", feature_id)?;
        let eid = Self::parse_uuid("environment_id", environment_id)?;
        self.user_flag_repo
            .upsert(user_id, fid, eid, assigned, variant)
            .await
            .map_err(UserFlagLogicError::DatabaseError)
    }

    async fn list_user_assignments(
        &self,
        team_id: Uuid,
        feature_id: Option<String>,
        environment_id: Option<String>,
    ) -> Result<Vec<UserFlagAssignmentRow>, UserFlagLogicError> {
        let fid = match feature_id {
            Some(s) if !s.is_empty() => Some(Self::parse_uuid("feature_id", &s)?),
            _ => None,
        };
        let eid = match environment_id {
            Some(s) if !s.is_empty() => Some(Self::parse_uuid("environment_id", &s)?),
            _ => None,
        };
        let rows = self
            .user_flag_repo
            .list(team_id, fid, eid)
            .await
            .map_err(UserFlagLogicError::DatabaseError)?;
        Ok(rows)
    }

    fn clone_box(&self) -> Box<dyn UserFlagLogic> {
        Box::new(Self::new(
            self.client_repo.clone(),
            self.user_flag_repo.clone(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::client::MockClientRepository;
    use crate::database::entity::ClientType;
    use crate::database::user_flag_assignment::{
        MockUserFlagAssignmentRepository, UserFlagAssignmentRow,
    };

    fn sample_client(enabled: bool, api_key: &str) -> crate::database::entity::Client {
        crate::database::entity::Client {
            id: Uuid::new_v4(),
            team_id: Uuid::new_v4(),
            name: "c".into(),
            description: None,
            enabled,
            client_type: ClientType::Web,
            api_key: api_key.into(),
            web_origins: None,
        }
    }

    #[tokio::test]
    async fn authenticate_client_happy_path() {
        let mut mock_client = MockClientRepository::new();
        let client = sample_client(true, "secret");
        let id = client.id;
        let expected_team = client.team_id;
        mock_client
            .expect_get_client_by_id()
            .returning(move |_| Ok(client.clone()));

        let uf_repo = MockUserFlagAssignmentRepository::new();
        let logic = UserFlagLogicImpl::new(Box::new(mock_client), Box::new(uf_repo));
        let team_id = logic
            .authenticate_client(&id.to_string(), "secret")
            .await
            .unwrap();
        assert_eq!(team_id, expected_team);
    }

    #[tokio::test]
    async fn authenticate_client_invalid_uuid() {
        let mock_client = MockClientRepository::new();
        let uf_repo = MockUserFlagAssignmentRepository::new();
        let logic = UserFlagLogicImpl::new(Box::new(mock_client), Box::new(uf_repo));
        let err = logic
            .authenticate_client("not-a-uuid", "s")
            .await
            .err()
            .unwrap();
        matches!(err, UserFlagLogicError::InvalidInput(_));
    }

    #[tokio::test]
    async fn authenticate_client_not_found() {
        let mut mock_client = MockClientRepository::new();
        let missing = Uuid::new_v4();
        mock_client
            .expect_get_client_by_id()
            .returning(move |_| Err(Error::NotFound(missing)));
        let uf_repo = MockUserFlagAssignmentRepository::new();
        let logic = UserFlagLogicImpl::new(Box::new(mock_client), Box::new(uf_repo));
        let err = logic
            .authenticate_client(&missing.to_string(), "x")
            .await
            .err()
            .unwrap();
        assert!(matches!(err, UserFlagLogicError::NotFound(id) if id==missing));
    }

    #[tokio::test]
    async fn authenticate_client_disabled() {
        let mut mock_client = MockClientRepository::new();
        let client = sample_client(false, "secret");
        let id = client.id;
        mock_client
            .expect_get_client_by_id()
            .returning(move |_| Ok(client.clone()));
        let uf_repo = MockUserFlagAssignmentRepository::new();
        let logic = UserFlagLogicImpl::new(Box::new(mock_client), Box::new(uf_repo));
        let err = logic
            .authenticate_client(&id.to_string(), "secret")
            .await
            .err()
            .unwrap();
        assert!(matches!(err, UserFlagLogicError::PermissionDenied(_)));
    }

    #[tokio::test]
    async fn authenticate_client_invalid_secret() {
        let mut mock_client = MockClientRepository::new();
        let client = sample_client(true, "secret");
        let id = client.id;
        mock_client
            .expect_get_client_by_id()
            .returning(move |_| Ok(client.clone()));
        let uf_repo = MockUserFlagAssignmentRepository::new();
        let logic = UserFlagLogicImpl::new(Box::new(mock_client), Box::new(uf_repo));
        let err = logic
            .authenticate_client(&id.to_string(), "wrong")
            .await
            .err()
            .unwrap();
        assert!(matches!(err, UserFlagLogicError::Unauthenticated(_)));
    }

    #[tokio::test]
    async fn upsert_after_auth_happy_path() {
        let mock_client = MockClientRepository::new();
        let mut uf_repo = MockUserFlagAssignmentRepository::new();
        uf_repo.expect_upsert().returning(|_, _, _, _, _| Ok(()));
        let logic = UserFlagLogicImpl::new(Box::new(mock_client), Box::new(uf_repo));
        let fid = Uuid::new_v4().to_string();
        let eid = Uuid::new_v4().to_string();
        let res = logic.upsert_after_auth("user", &fid, &eid, true, Some("variant-a".into())).await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn upsert_after_auth_invalid_ids() {
        let mock_client = MockClientRepository::new();
        let uf_repo = MockUserFlagAssignmentRepository::new();
        let logic = UserFlagLogicImpl::new(Box::new(mock_client), Box::new(uf_repo));
        let err = logic
            .upsert_after_auth("user", "bad", &Uuid::new_v4().to_string(), true, None)
            .await
            .err()
            .unwrap();
        assert!(matches!(err, UserFlagLogicError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn upsert_after_auth_db_error() {
        let mock_client = MockClientRepository::new();
        let mut uf_repo = MockUserFlagAssignmentRepository::new();
        uf_repo
            .expect_upsert()
            .returning(|_, _, _, _, _| Err(Error::InvalidInput("x".into())));
        let logic = UserFlagLogicImpl::new(Box::new(mock_client), Box::new(uf_repo));
        let err = logic
            .upsert_after_auth(
                "user",
                &Uuid::new_v4().to_string(),
                &Uuid::new_v4().to_string(),
                true,
                None,
            )
            .await
            .err()
            .unwrap();
        assert!(matches!(err, UserFlagLogicError::DatabaseError(_)));
    }

    #[tokio::test]
    async fn list_user_assignments_happy_path() {
        let mock_client = MockClientRepository::new();
        let mut uf_repo = MockUserFlagAssignmentRepository::new();
        let team_id = Uuid::new_v4();
        let sample = UserFlagAssignmentRow {
            user_id: "u".into(),
            feature_id: Uuid::new_v4(),
            environment_id: Uuid::new_v4(),
            assigned: true,
            variant: None,
        };
        uf_repo
            .expect_list()
            .returning(move |_, _, _| Ok(vec![sample.clone()]));
        let logic = UserFlagLogicImpl::new(Box::new(mock_client), Box::new(uf_repo));
        let rows = logic
            .list_user_assignments(team_id, None, None)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[tokio::test]
    async fn list_user_assignments_invalid_filters() {
        let mock_client = MockClientRepository::new();
        let uf_repo = MockUserFlagAssignmentRepository::new();
        let logic = UserFlagLogicImpl::new(Box::new(mock_client), Box::new(uf_repo));
        let err = logic
            .list_user_assignments(Uuid::new_v4(), Some("bad".to_string()), None)
            .await
            .err()
            .unwrap();
        assert!(matches!(err, UserFlagLogicError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn list_user_assignments_db_error() {
        let mock_client = MockClientRepository::new();
        let mut uf_repo = MockUserFlagAssignmentRepository::new();
        uf_repo
            .expect_list()
            .returning(|_, _, _| Err(Error::InvalidInput("x".into())));
        let logic = UserFlagLogicImpl::new(Box::new(mock_client), Box::new(uf_repo));
        let err = logic
            .list_user_assignments(Uuid::new_v4(), None, None)
            .await
            .err()
            .unwrap();
        assert!(matches!(err, UserFlagLogicError::DatabaseError(_)));
    }
}
