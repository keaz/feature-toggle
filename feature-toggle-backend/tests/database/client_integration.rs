use feature_toggle_backend::database::client::{CreateClient, client_repository};
use feature_toggle_backend::database::entity::ClientType;
use feature_toggle_backend::database::init_pg_pool;
use sqlx::PgPool;
use uuid::Uuid;

async fn pool() -> PgPool {
    init_pg_pool().await
}

#[tokio::test]
async fn test_get_clients_seeded() {
    let pool = pool().await;
    let repo = client_repository(pool);
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();

    let list = repo
        .get_clients(team_id, None, None, None)
        .await
        .expect("get_clients ok");

    assert!(list.len() >= 2);
    let web = list
        .iter()
        .find(|c| c.name == "Web Client 1")
        .expect("web client present");
    assert!(matches!(web.client_type, ClientType::Web));
    assert_eq!(web.web_origins.as_ref().map(|v| v.len()).unwrap_or(0), 2);
}

#[tokio::test]
async fn test_create_and_delete_client() {
    let pool = pool().await;
    let repo = client_repository(pool);
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();

    let name = format!("it-client-{}", Uuid::new_v4());
    let create = CreateClient {
        name: name.clone(),
        description: Some("integ test".into()),
        enabled: true,
        client_type: ClientType::Web,
        web_origins: Some(vec!["http://test.local".into()]),
    };

    let created = repo
        .create_client(team_id, create)
        .await
        .expect("create ok");
    assert_eq!(created.name, name);
    assert_eq!(created.team_id, team_id);
    assert!(matches!(created.client_type, ClientType::Web));
    assert_eq!(created.web_origins.as_ref().unwrap().len(), 1);
    assert_eq!(
        created.web_origins.as_ref().unwrap()[0],
        "http://test.local"
    );
    assert_eq!(created.api_key.len(), 48);

    // cleanup
    repo.delete_client(created.id).await.expect("delete ok");
}
