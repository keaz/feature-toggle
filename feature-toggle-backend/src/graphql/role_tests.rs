use super::*;
use crate::graphql::query::Query as GqlQuery;
use crate::logic::role::MockRoleLogic;
use async_graphql::{EmptySubscription, Request, Schema};
use uuid::Uuid;

#[tokio::test]
async fn test_assign_user_roles_mutation() {
    let mut mock = MockRoleLogic::new();
    let user_id = ID::from(Uuid::new_v4());
    let role_id = ID::from(Uuid::new_v4());
    let role_ids = vec![role_id.clone()];
    
    // Mock the assign operation to return assigned roles
    let expected_role = crate::logic::role::GqlRole {
        id: role_id.clone(),
        name: "Approver".to_string(),
        description: "Can approve deployment requests".to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
    };
    
    mock.expect_assign_user_roles()
        .times(1)
        .return_once(move |_, _, _| Ok(vec![expected_role]));

    let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
        .data::<Box<dyn crate::logic::role::RoleLogic>>(Box::new(mock))
        .finish();

    let gql = r#"
        mutation($userId: ID!, $roleIds: [ID!]!) {
            assignUserRoles(userId: $userId, input: { roleIds: $roleIds }) {
                id
                name
                description
            }
        }
    "#;
    let mut req = Request::new(gql);
    req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
        "userId": user_id.to_string(),
        "roleIds": [role_id.to_string()]
    })));
    let resp = schema.execute(req).await;
    assert!(
        resp.errors.is_empty(),
        "{}",
        serde_json::to_string(&resp.errors).unwrap()
    );
    let data = resp.data.into_json().unwrap();
    assert_eq!(data["assignUserRoles"][0]["name"], "Approver");
}

#[tokio::test]
async fn test_roles_query() {
    let mut mock = MockRoleLogic::new();
    let expected_roles = vec![
        crate::logic::role::GqlRole {
            id: ID::from(Uuid::new_v4()),
            name: "Approver".to_string(),
            description: "Can approve deployment requests".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        },
        crate::logic::role::GqlRole {
            id: ID::from(Uuid::new_v4()),
            name: "Requester".to_string(),
            description: "Can request deployments".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        },
    ];

    mock.expect_get_all_roles()
        .times(1)
        .return_once(move || Ok(expected_roles));

    let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
        .data::<Box<dyn crate::logic::role::RoleLogic>>(Box::new(mock))
        .finish();

    let gql = r#"
        query {
            roles {
                id
                name
                description
            }
        }
    "#;
    let resp = schema.execute(Request::new(gql)).await;
    assert!(
        resp.errors.is_empty(),
        "{}",
        serde_json::to_string(&resp.errors).unwrap()
    );
    let data = resp.data.into_json().unwrap();
    assert_eq!(data["roles"].as_array().unwrap().len(), 2);
    assert_eq!(data["roles"][0]["name"], "Approver");
    assert_eq!(data["roles"][1]["name"], "Requester");
}
