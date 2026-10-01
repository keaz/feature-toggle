use crate::Error;
use crate::database::client::ClientRepository;
use crate::database::user_flag_assignment::{UserFlagAssignmentRepository, UserFlagAssignmentRow};
use mockall::automock;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
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

/// One assignment row as received from a client, before validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserAssignmentInput {
    pub user_id: String,
    pub feature_id: String,
    pub environment_id: String,
    pub assigned: bool,
    pub variant: Option<String>,
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

    // Upsert a chunk of assignments after successful authentication, in one DB
    // write. Every feature and environment must belong to `team_id`, the
    // authenticated client's team; otherwise nothing in the chunk is written.
    async fn upsert_many_after_auth(
        &self,
        team_id: Uuid,
        rows: Vec<UserAssignmentInput>,
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

    async fn upsert_many_after_auth(
        &self,
        team_id: Uuid,
        rows: Vec<UserAssignmentInput>,
    ) -> Result<(), UserFlagLogicError> {
        // Validate, skip rows with empty ids (as before), and dedupe by key with
        // the last occurrence winning: Postgres cannot upsert the same row twice
        // in one statement, and last-write-wins is the per-row behavior.
        let mut deduped: Vec<UserFlagAssignmentRow> = Vec::with_capacity(rows.len());
        let mut index_by_key: HashMap<(String, Uuid, Uuid), usize> =
            HashMap::with_capacity(rows.len());
        for row in rows {
            if row.user_id.is_empty() || row.feature_id.is_empty() || row.environment_id.is_empty()
            {
                continue;
            }
            let fid = Self::parse_uuid("feature_id", &row.feature_id)?;
            let eid = Self::parse_uuid("environment_id", &row.environment_id)?;
            let parsed = UserFlagAssignmentRow {
                user_id: row.user_id,
                feature_id: fid,
                environment_id: eid,
                assigned: row.assigned,
                variant: row.variant,
            };
            match index_by_key.entry((parsed.user_id.clone(), fid, eid)) {
                Entry::Occupied(slot) => deduped[*slot.get()] = parsed,
                Entry::Vacant(slot) => {
                    slot.insert(deduped.len());
                    deduped.push(parsed);
                }
            }
        }
        if deduped.is_empty() {
            return Ok(());
        }

        let feature_ids: Vec<Uuid> = deduped.iter().map(|r| r.feature_id).collect();
        let environment_ids: Vec<Uuid> = deduped.iter().map(|r| r.environment_id).collect();
        let owned = self
            .user_flag_repo
            .all_owned_by_team(team_id, &feature_ids, &environment_ids)
            .await
            .map_err(UserFlagLogicError::DatabaseError)?;
        if !owned {
            return Err(UserFlagLogicError::PermissionDenied(
                "feature or environment does not belong to the client's team".to_string(),
            ));
        }
        self.user_flag_repo
            .upsert_many(&deduped)
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
            environment_id: Uuid::new_v4(),
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

    fn input(
        user_id: &str,
        feature_id: &str,
        environment_id: &str,
        variant: Option<&str>,
    ) -> UserAssignmentInput {
        UserAssignmentInput {
            user_id: user_id.to_string(),
            feature_id: feature_id.to_string(),
            environment_id: environment_id.to_string(),
            assigned: true,
            variant: variant.map(str::to_string),
        }
    }

    #[tokio::test]
    async fn upsert_many_after_auth_writes_chunk_in_one_call() {
        let mock_client = MockClientRepository::new();
        let mut uf_repo = MockUserFlagAssignmentRepository::new();
        let team_id = Uuid::new_v4();
        let fid = Uuid::new_v4();
        let eid_a = Uuid::new_v4();
        let eid_b = Uuid::new_v4();
        uf_repo
            .expect_all_owned_by_team()
            .withf(move |t, f, e| *t == team_id && f == [fid, fid] && e == [eid_a, eid_b])
            .times(1)
            .returning(|_, _, _| Ok(true));
        uf_repo
            .expect_upsert_many()
            .withf(move |rows| {
                rows.len() == 2
                    && rows[0].user_id == "u1"
                    && rows[0].feature_id == fid
                    && rows[0].environment_id == eid_a
                    && rows[0].variant.as_deref() == Some("variant-a")
                    && rows[1].user_id == "u2"
                    && rows[1].environment_id == eid_b
                    && rows[1].variant.is_none()
            })
            .times(1)
            .returning(|_| Ok(()));
        uf_repo.expect_upsert().never();
        let logic = UserFlagLogicImpl::new(Box::new(mock_client), Box::new(uf_repo));
        let res = logic
            .upsert_many_after_auth(
                team_id,
                vec![
                    input(
                        "u1",
                        &fid.to_string(),
                        &eid_a.to_string(),
                        Some("variant-a"),
                    ),
                    input("u2", &fid.to_string(), &eid_b.to_string(), None),
                ],
            )
            .await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn upsert_many_after_auth_keeps_last_occurrence_of_a_key() {
        let mock_client = MockClientRepository::new();
        let mut uf_repo = MockUserFlagAssignmentRepository::new();
        let fid = Uuid::new_v4().to_string();
        let eid = Uuid::new_v4().to_string();
        uf_repo
            .expect_all_owned_by_team()
            .returning(|_, _, _| Ok(true));
        uf_repo
            .expect_upsert_many()
            .withf(|rows| {
                let mut seen: Vec<(&str, Option<&str>)> = rows
                    .iter()
                    .map(|r| (r.user_id.as_str(), r.variant.as_deref()))
                    .collect();
                seen.sort();
                seen == vec![("u1", Some("second")), ("u2", Some("only"))]
            })
            .times(1)
            .returning(|_| Ok(()));
        let logic = UserFlagLogicImpl::new(Box::new(mock_client), Box::new(uf_repo));
        let res = logic
            .upsert_many_after_auth(
                Uuid::new_v4(),
                vec![
                    input("u1", &fid, &eid, Some("first")),
                    input("u2", &fid, &eid, Some("only")),
                    input("u1", &fid, &eid, Some("second")),
                ],
            )
            .await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn upsert_many_after_auth_rejects_feature_of_another_team() {
        let mock_client = MockClientRepository::new();
        let mut uf_repo = MockUserFlagAssignmentRepository::new();
        uf_repo
            .expect_all_owned_by_team()
            .times(1)
            .returning(|_, _, _| Ok(false));
        uf_repo.expect_upsert_many().never();
        uf_repo.expect_upsert().never();
        let logic = UserFlagLogicImpl::new(Box::new(mock_client), Box::new(uf_repo));
        let err = logic
            .upsert_many_after_auth(
                Uuid::new_v4(),
                vec![input(
                    "user",
                    &Uuid::new_v4().to_string(),
                    &Uuid::new_v4().to_string(),
                    Some("attacker-variant"),
                )],
            )
            .await
            .err()
            .unwrap();
        assert!(matches!(err, UserFlagLogicError::PermissionDenied(_)));
    }

    #[tokio::test]
    async fn upsert_many_after_auth_skips_rows_with_empty_ids() {
        let mock_client = MockClientRepository::new();
        let mut uf_repo = MockUserFlagAssignmentRepository::new();
        let fid = Uuid::new_v4();
        let eid = Uuid::new_v4();
        uf_repo
            .expect_all_owned_by_team()
            .withf(move |_, f, e| f == [fid] && e == [eid])
            .times(1)
            .returning(|_, _, _| Ok(true));
        uf_repo
            .expect_upsert_many()
            .withf(|rows| rows.len() == 1 && rows[0].user_id == "kept")
            .times(1)
            .returning(|_| Ok(()));
        let logic = UserFlagLogicImpl::new(Box::new(mock_client), Box::new(uf_repo));
        let res = logic
            .upsert_many_after_auth(
                Uuid::new_v4(),
                vec![
                    input("", &fid.to_string(), &eid.to_string(), None),
                    input("kept", &fid.to_string(), &eid.to_string(), None),
                    input("no-feature", "", &eid.to_string(), None),
                    input("no-env", &fid.to_string(), "", None),
                ],
            )
            .await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn upsert_many_after_auth_without_rows_makes_no_db_calls() {
        let mock_client = MockClientRepository::new();
        let mut uf_repo = MockUserFlagAssignmentRepository::new();
        uf_repo.expect_all_owned_by_team().never();
        uf_repo.expect_upsert_many().never();
        let logic = UserFlagLogicImpl::new(Box::new(mock_client), Box::new(uf_repo));
        let res = logic
            .upsert_many_after_auth(Uuid::new_v4(), vec![input("", "", "", None)])
            .await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn upsert_many_after_auth_invalid_ids() {
        let mock_client = MockClientRepository::new();
        let mut uf_repo = MockUserFlagAssignmentRepository::new();
        uf_repo.expect_all_owned_by_team().never();
        uf_repo.expect_upsert_many().never();
        let logic = UserFlagLogicImpl::new(Box::new(mock_client), Box::new(uf_repo));
        let eid = Uuid::new_v4().to_string();
        let err = logic
            .upsert_many_after_auth(
                Uuid::new_v4(),
                vec![
                    input("user", &Uuid::new_v4().to_string(), &eid, None),
                    input("user", "bad", &eid, None),
                ],
            )
            .await
            .err()
            .unwrap();
        assert!(matches!(err, UserFlagLogicError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn upsert_many_after_auth_db_error() {
        let mock_client = MockClientRepository::new();
        let mut uf_repo = MockUserFlagAssignmentRepository::new();
        uf_repo
            .expect_all_owned_by_team()
            .returning(|_, _, _| Ok(true));
        uf_repo
            .expect_upsert_many()
            .returning(|_| Err(Error::InvalidInput("x".into())));
        let logic = UserFlagLogicImpl::new(Box::new(mock_client), Box::new(uf_repo));
        let err = logic
            .upsert_many_after_auth(
                Uuid::new_v4(),
                vec![input(
                    "user",
                    &Uuid::new_v4().to_string(),
                    &Uuid::new_v4().to_string(),
                    None,
                )],
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
