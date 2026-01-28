use async_graphql::{EmptySubscription, ID, Request, Schema};
use feature_toggle_backend::graphql::mutation::MutationRoot;
use feature_toggle_backend::graphql::query::Query as GqlQuery;
use feature_toggle_backend::logic::role::MockRoleLogic;
use uuid::Uuid;

#[tokio::test]
async fn test_assign_user_roles_mutation() {
    let pool = feature_toggle_backend::database::init_pg_pool().await;
    let activity_repo: Box<
        dyn feature_toggle_backend::database::activity_log::ActivityLogRepository,
    > = Box::new(feature_toggle_backend::database::activity_log::PgActivityLogRepository::new(
        pool.clone(),
    ));

    let user_repo = feature_toggle_backend::database::user::user_repository(pool.clone());
    let role_repo = feature_toggle_backend::database::role::role_repository(pool.clone());

    let unique_suffix = Uuid::new_v4();
    let created_user = user_repo
        .create_user(feature_toggle_backend::database::user::CreateUser {
            username: format!("role_user_{unique_suffix}"),
            password_hash: "hashed_password".to_string(),
            first_name: "Role".to_string(),
            last_name: "User".to_string(),
            email: format!("role_user_{unique_suffix}@example.com"),
            is_admin: false,
            is_temporary_password: false,
        })
        .await
        .expect("create user");

    let role_name = format!("Approver-{}", Uuid::new_v4());
    let role = role_repo
        .create_role(&role_name, "Can approve deployment requests")
        .await
        .expect("create role");

    let user_id = ID::from(created_user.id);
    let role_id = ID::from(role.id);
    let admin_id =
        Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").expect("admin user id");

    let schema = Schema::build(GqlQuery, MutationRoot, EmptySubscription)
        .data(pool.clone())
        .data::<Box<dyn feature_toggle_backend::database::activity_log::ActivityLogRepository>>(
            activity_repo,
        )
        .data(feature_toggle_backend::JwtUser {
            id: admin_id,
            username: "admin".to_string(),
            is_admin: true,
            roles: vec![],
            token_hash: "hash".to_string(),
        })
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
    assert_eq!(data["assignUserRoles"][0]["name"], role_name);
}

#[tokio::test]
async fn test_roles_query() {
    let mut mock = MockRoleLogic::new();
    let expected_roles = vec![
        feature_toggle_backend::logic::role::GqlRole {
            id: ID::from(Uuid::new_v4()),
            name: "Approver".to_string(),
            description: "Can approve deployment requests".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        },
        feature_toggle_backend::logic::role::GqlRole {
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

    let schema = Schema::build(GqlQuery, MutationRoot, EmptySubscription)
        .data::<Box<dyn feature_toggle_backend::logic::role::RoleLogic>>(Box::new(mock))
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
