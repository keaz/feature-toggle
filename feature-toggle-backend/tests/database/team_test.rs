use feature_toggle_backend::database::team::{CreateTeam, UpdateTeam};
use feature_toggle_backend::database::{init_pg_pool, team};
use uuid::Uuid;

#[tokio::test]
async fn test_get_existing_team() {
    let pool = init_pg_pool().await;
    let repository = team::team_repository(pool);

    let id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let result = repository.get_team_by_id(id).await;

    assert!(result.is_ok());
    let team = result.unwrap();
    assert_eq!(team.id, id);
    assert_eq!(team.name, "Test Team");
}

#[tokio::test]
async fn test_get_not_found_team() {
    let pool = init_pg_pool().await;
    let repository = team::team_repository(pool);

    let id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98fca").unwrap();
    let result = repository.get_team_by_id(id).await;

    assert!(result.is_err());
    let error = result.err().unwrap();
    assert!(matches!(error, feature_toggle_backend::Error::NotFound(_)));
}

#[tokio::test]
async fn test_create_team() {
    let pool = init_pg_pool().await;
    let repository = team::team_repository(pool);

    let name = format!("New Team {}", Uuid::new_v4());
    let input = CreateTeam {
        name: name.clone(),
        description: "Description of the new environment".to_string(),
    };
    let result = repository.create_team(input).await;

    assert!(result.is_ok());
    let environment = result.unwrap();
    assert_eq!(environment.name, name);
    assert_eq!(
        environment.description,
        "Description of the new environment".to_string()
    );
}

#[tokio::test]
async fn test_update_team() {
    let pool = init_pg_pool().await;
    let repository = team::team_repository(pool);

    let input = UpdateTeam {
        id: Uuid::parse_str("3eef17bc-9e06-411d-b5f4-7a786e68bb96").unwrap(),
        name: Some("Updated Team".to_string()),
        description: Some("Updated description".to_string()),
    };
    let result = repository.update_team(input).await;

    assert!(result.is_ok());
    let environment = result.unwrap();
    assert_eq!(environment.name, "Updated Team");
    assert_eq!(environment.description, "Updated description".to_string());
}

#[tokio::test]
async fn test_not_found_update_team() {
    let pool = init_pg_pool().await;
    let repository = team::team_repository(pool);

    let input = UpdateTeam {
        id: Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98fca").unwrap(),
        name: Some("Non-existent Environment".to_string()),
        description: Some("This environment does not exist".to_string()),
    };
    let result = repository.update_team(input).await;

    assert!(result.is_err());
    let error = result.err().unwrap();
    assert!(matches!(error, feature_toggle_backend::Error::NotFound(_)));
}

#[tokio::test]
async fn test_delete_team() {
    let pool = init_pg_pool().await;
    let repository = team::team_repository(pool);

    let created = repository
        .create_team(CreateTeam {
            name: format!("Delete Team {}", Uuid::new_v4()),
            description: "temp team".to_string(),
        })
        .await
        .expect("team should be created");

    let result = repository.delete_team(created.id).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_not_found_delete_test() {
    let pool = init_pg_pool().await;
    let repository = team::team_repository(pool);

    let id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98fca").unwrap();
    let result = repository.delete_team(id).await;

    assert!(result.is_err());
    let error = result.err().unwrap();
    assert!(matches!(error, feature_toggle_backend::Error::NotFound(_)));
}

#[tokio::test]
async fn test_non_name_get_environments() {
    let pool = init_pg_pool().await;
    let repository = team::team_repository(pool);

    let result = repository.get_teams(None).await;

    assert!(result.is_ok());
    let teams = result.unwrap();
    assert!(!teams.is_empty());
    assert!(teams.iter().any(|env| env.name == "Test Team"));
}

#[tokio::test]
async fn test_name_param_get_team() {
    let pool = init_pg_pool().await;
    let repository = team::team_repository(pool);

    let result = repository.get_teams(Some("Test".to_string())).await;

    assert!(result.is_ok());
    let teams = result.unwrap();
    assert!(!teams.is_empty());
    assert!(
        teams
            .iter()
            .all(|env| env.name.to_lowercase().contains("test"))
    );
}
