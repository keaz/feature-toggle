use std::collections::BTreeMap;

use feature_toggle_backend::Error;
use feature_toggle_backend::database::activity_log::activity_log_repository;
use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::database::jira_integration::{
    jira_integration_repository, jira_integration_repository_tx,
};
use feature_toggle_backend::database::jira_outbound_job::jira_outbound_job_repository_tx;
use feature_toggle_backend::logic::ActorContext;
use feature_toggle_backend::logic::jira_integration::{JiraStatusRuleInput, hash_secret};
use feature_toggle_backend::logic::jira_integration_tx::{
    JiraIntegrationInput, JiraIntegrationPatch, JiraIntegrationWithSecret,
    create_jira_integration_in_tx, delete_jira_integration_in_tx, replace_jira_status_rules_in_tx,
    rotate_jira_integration_secret_in_tx, update_jira_integration_in_tx,
};
use feature_toggle_backend::utils::activity_logger::activity_types::{
    JIRA_INTEGRATION_CREATED, JIRA_INTEGRATION_DELETED, JIRA_INTEGRATION_RULES_REPLACED,
    JIRA_INTEGRATION_SECRET_ROTATED, JIRA_INTEGRATION_UPDATED,
};
use sqlx::PgPool;
use uuid::Uuid;

/// Seeded admin user from `init.sql`.
const SEED_ADMIN_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

fn actor() -> ActorContext {
    ActorContext::new(Uuid::parse_str(SEED_ADMIN_ID).unwrap(), "admin".to_string())
}

/// A team with two environments (QA, Production).
struct Team {
    id: Uuid,
    qa: Uuid,
    prod: Uuid,
}

async fn insert_team(pool: &PgPool) -> Team {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, 'jira test')")
        .bind(id)
        .bind(format!("jira-integration-test-{id}"))
        .execute(pool)
        .await
        .expect("insert team");
    let mut envs = Vec::new();
    for name in ["QA", "Production"] {
        let env_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO environments (id, name, active, team_id, environment_type) \
             VALUES ($1, $2, TRUE, $3, 'Development')",
        )
        .bind(env_id)
        .bind(name)
        .bind(id)
        .execute(pool)
        .await
        .expect("insert environment");
        envs.push(env_id);
    }
    Team {
        id,
        qa: envs[0],
        prod: envs[1],
    }
}

/// Deletes the team; its integrations cascade. Shadow users stay (audit trail),
/// so they are removed here explicitly.
async fn delete_team(pool: &PgPool, team_id: Uuid) {
    let shadow_users: Vec<Uuid> =
        sqlx::query_scalar("SELECT actor_user_id FROM jira_integrations WHERE team_id = $1")
            .bind(team_id)
            .fetch_all(pool)
            .await
            .expect("load shadow users");
    sqlx::query("DELETE FROM teams WHERE id = $1")
        .bind(team_id)
        .execute(pool)
        .await
        .expect("delete team");
    sqlx::query("DELETE FROM users WHERE id = ANY($1)")
        .bind(shadow_users)
        .execute(pool)
        .await
        .expect("delete shadow users");
}

fn input(name: &str, team: &Team) -> JiraIntegrationInput {
    JiraIntegrationInput {
        name: name.to_string(),
        jira_base_url: Some("https://acme.atlassian.net".to_string()),
        environment_field: "customfield_10042".to_string(),
        environment_aliases: BTreeMap::from([("Prod".to_string(), team.prod.to_string())]),
        jira_approved_environment_ids: vec![team.qa.to_string()],
        feature_key_field: None,
        enabled: true,
    }
}

async fn create(
    pool: &PgPool,
    team: &Team,
    name: &str,
) -> Result<JiraIntegrationWithSecret, Error> {
    let repo = jira_integration_repository_tx(pool.clone());
    let activity = activity_log_repository(pool.clone());
    let mut tx = pool.begin().await.expect("begin");
    let created = create_jira_integration_in_tx(
        &mut tx,
        &repo,
        activity.as_ref(),
        team.id,
        input(name, team),
        actor(),
    )
    .await?;
    tx.commit().await.expect("commit");
    Ok(created)
}

async fn activity_metadata(
    pool: &PgPool,
    activity_type: &str,
    integration_id: Uuid,
) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT metadata::TEXT FROM activity_log \
         WHERE activity_type = $1 AND entity_type = 'jira_integration' AND entity_id = $2",
    )
    .bind(activity_type)
    .bind(integration_id.to_string())
    .fetch_all(pool)
    .await
    .expect("load activity")
}

fn rule(status: &str, action: &str, envs: Option<Vec<Uuid>>) -> JiraStatusRuleInput {
    JiraStatusRuleInput {
        jira_status: status.to_string(),
        action: action.to_string(),
        environment_ids: envs.map(|ids| ids.iter().map(Uuid::to_string).collect()),
        enabled: true,
    }
}

#[tokio::test]
async fn create_stores_the_secret_hash_and_a_requester_only_shadow_user() {
    let pool = init_pg_pool().await;
    let team = insert_team(&pool).await;

    let created = create(&pool, &team, "Jira PROJ").await.expect("create");
    let integration = &created.integration;

    assert_eq!(integration.team_id, team.id);
    assert_eq!(integration.name, "Jira PROJ");
    assert_eq!(integration.secret_hash, hash_secret(&created.secret));
    assert_eq!(integration.environment_field, "customfield_10042");
    assert_eq!(
        integration.environment_aliases.0,
        BTreeMap::from([("Prod".to_string(), team.prod)])
    );
    assert_eq!(integration.jira_approved_environment_ids, vec![team.qa]);
    assert!(integration.enabled);

    let (username, auth_source, is_admin, enabled, password_hash): (
        String,
        String,
        bool,
        bool,
        String,
    ) = sqlx::query_as(
        "SELECT username, auth_source, is_admin, enabled, password_hash FROM users WHERE id = $1",
    )
    .bind(integration.actor_user_id)
    .fetch_one(&pool)
    .await
    .expect("shadow user");
    assert_eq!(username, format!("jira-integration-{}", integration.id));
    assert_eq!(auth_source, "system");
    assert!(!is_admin);
    assert!(enabled);
    assert!(!password_hash.starts_with("$argon2"), "no password login");

    let roles: Vec<String> = sqlx::query_scalar(
        "SELECT r.name FROM user_roles ur JOIN roles r ON r.id = ur.role_id WHERE ur.user_id = $1",
    )
    .bind(integration.actor_user_id)
    .fetch_all(&pool)
    .await
    .expect("roles");
    assert_eq!(roles, vec!["Requester".to_string()]);

    let metadata = activity_metadata(&pool, JIRA_INTEGRATION_CREATED, integration.id).await;
    assert_eq!(metadata.len(), 1);
    assert!(!metadata[0].contains(&created.secret));
    assert!(!metadata[0].contains(&integration.secret_hash));

    let listed = jira_integration_repository(pool.clone())
        .list_for_team(team.id)
        .await
        .expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, integration.id);

    delete_team(&pool, team.id).await;
}

#[tokio::test]
async fn create_rejects_a_duplicate_name_and_foreign_environments() {
    let pool = init_pg_pool().await;
    let team = insert_team(&pool).await;
    let other = insert_team(&pool).await;

    create(&pool, &team, "Jira").await.expect("create");
    assert!(matches!(
        create(&pool, &team, "Jira").await,
        Err(Error::RecordAlreadyExists(_))
    ));

    let repo = jira_integration_repository_tx(pool.clone());
    let activity = activity_log_repository(pool.clone());
    let mut tx = pool.begin().await.expect("begin");
    let mut foreign = input("Foreign", &team);
    foreign.jira_approved_environment_ids = vec![other.qa.to_string()];
    let result =
        create_jira_integration_in_tx(&mut tx, &repo, activity.as_ref(), team.id, foreign, actor())
            .await;
    assert!(matches!(result, Err(Error::InvalidInput(_))), "{result:?}");
    tx.rollback().await.expect("rollback");

    delete_team(&pool, team.id).await;
    delete_team(&pool, other.id).await;
}

#[tokio::test]
async fn rotate_secret_replaces_the_hash_and_returns_the_new_secret() {
    let pool = init_pg_pool().await;
    let team = insert_team(&pool).await;
    let created = create(&pool, &team, "Jira").await.expect("create");

    let repo = jira_integration_repository_tx(pool.clone());
    let activity = activity_log_repository(pool.clone());
    let mut tx = pool.begin().await.expect("begin");
    let rotated = rotate_jira_integration_secret_in_tx(
        &mut tx,
        &repo,
        activity.as_ref(),
        created.integration.id,
        actor(),
    )
    .await
    .expect("rotate");
    tx.commit().await.expect("commit");

    assert_ne!(rotated.secret, created.secret);
    assert_eq!(
        rotated.integration.secret_hash,
        hash_secret(&rotated.secret)
    );
    let stored = jira_integration_repository(pool.clone())
        .get(created.integration.id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(stored.secret_hash, hash_secret(&rotated.secret));

    let metadata = activity_metadata(
        &pool,
        JIRA_INTEGRATION_SECRET_ROTATED,
        created.integration.id,
    )
    .await;
    assert_eq!(metadata.len(), 1);
    assert!(!metadata[0].contains(&rotated.secret));
    assert!(!metadata[0].contains(&stored.secret_hash));

    let mut tx = pool.begin().await.expect("begin");
    let missing = rotate_jira_integration_secret_in_tx(
        &mut tx,
        &repo,
        activity.as_ref(),
        Uuid::new_v4(),
        actor(),
    )
    .await;
    assert!(matches!(missing, Err(Error::NotFound(_))), "{missing:?}");

    delete_team(&pool, team.id).await;
}

#[tokio::test]
async fn update_changes_only_the_given_fields() {
    let pool = init_pg_pool().await;
    let team = insert_team(&pool).await;
    let created = create(&pool, &team, "Jira").await.expect("create");

    let repo = jira_integration_repository_tx(pool.clone());
    let outbound = jira_outbound_job_repository_tx(pool.clone());
    let activity = activity_log_repository(pool.clone());
    let mut tx = pool.begin().await.expect("begin");
    let updated = update_jira_integration_in_tx(
        &mut tx,
        &repo,
        &outbound,
        activity.as_ref(),
        created.integration.id,
        JiraIntegrationPatch {
            name: Some(" Jira renamed ".to_string()),
            jira_base_url: Some(String::new()),
            environment_field: Some("labels".to_string()),
            environment_aliases: Some(BTreeMap::from([(
                "QA env".to_string(),
                team.qa.to_string(),
            )])),
            jira_approved_environment_ids: Some(vec![team.prod.to_string(), team.qa.to_string()]),
            feature_key_field: Some("customfield_10050".to_string()),
            enabled: Some(false),
        },
        &feature_toggle_backend::config::JiraConfig::default(),
        actor(),
    )
    .await
    .expect("update");
    tx.commit().await.expect("commit");

    assert_eq!(updated.name, "Jira renamed");
    assert_eq!(updated.jira_base_url, None);
    assert_eq!(updated.environment_field, "labels");
    assert_eq!(
        updated.environment_aliases.0,
        BTreeMap::from([("QA env".to_string(), team.qa)])
    );
    assert_eq!(
        updated.jira_approved_environment_ids,
        vec![team.prod, team.qa]
    );
    assert_eq!(
        updated.feature_key_field.as_deref(),
        Some("customfield_10050")
    );
    assert!(!updated.enabled);
    assert_eq!(updated.secret_hash, created.integration.secret_hash);
    assert!(updated.updated_at >= created.integration.updated_at);

    // An empty patch keeps everything.
    let mut tx = pool.begin().await.expect("begin");
    let unchanged = update_jira_integration_in_tx(
        &mut tx,
        &repo,
        &outbound,
        activity.as_ref(),
        created.integration.id,
        JiraIntegrationPatch::default(),
        &feature_toggle_backend::config::JiraConfig::default(),
        actor(),
    )
    .await
    .expect("empty update");
    tx.commit().await.expect("commit");
    assert_eq!(unchanged.name, "Jira renamed");
    assert_eq!(
        unchanged.feature_key_field.as_deref(),
        Some("customfield_10050")
    );

    let metadata = activity_metadata(&pool, JIRA_INTEGRATION_UPDATED, created.integration.id).await;
    assert_eq!(metadata.len(), 2);

    delete_team(&pool, team.id).await;
}

#[tokio::test]
async fn replace_rules_keeps_order_and_replaces_the_whole_list() {
    let pool = init_pg_pool().await;
    let team = insert_team(&pool).await;
    let created = create(&pool, &team, "Jira").await.expect("create");
    let id = created.integration.id;

    let repo = jira_integration_repository_tx(pool.clone());
    let activity = activity_log_repository(pool.clone());
    let mut tx = pool.begin().await.expect("begin");
    let rules = replace_jira_status_rules_in_tx(
        &mut tx,
        &repo,
        activity.as_ref(),
        id,
        vec![
            rule("Ready for Release", "approve", Some(vec![team.qa])),
            rule("Done", "approve", None),
            rule("Done", "deploy", None),
            rule("Reopened", "rollback", Some(vec![team.prod, team.qa])),
        ],
        actor(),
    )
    .await
    .expect("replace");
    tx.commit().await.expect("commit");

    let summary: Vec<(String, String, i32)> = rules
        .iter()
        .map(|r| (r.jira_status.clone(), r.action.clone(), r.position))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("Ready for Release".to_string(), "approve".to_string(), 0),
            ("Done".to_string(), "approve".to_string(), 1),
            ("Done".to_string(), "deploy".to_string(), 2),
            ("Reopened".to_string(), "rollback".to_string(), 3),
        ]
    );
    assert_eq!(rules[0].environment_ids, Some(vec![team.qa]));
    assert_eq!(rules[1].environment_ids, None);
    assert_eq!(rules[3].environment_ids, Some(vec![team.prod, team.qa]));

    let mut tx = pool.begin().await.expect("begin");
    replace_jira_status_rules_in_tx(
        &mut tx,
        &repo,
        activity.as_ref(),
        id,
        vec![rule("Done", "deploy", None), rule("In QA", "request", None)],
        actor(),
    )
    .await
    .expect("replace again");
    tx.commit().await.expect("commit");

    let listed = jira_integration_repository(pool.clone())
        .list_rules(id)
        .await
        .expect("list rules");
    let summary: Vec<(String, i32)> = listed
        .iter()
        .map(|r| (r.jira_status.clone(), r.position))
        .collect();
    assert_eq!(
        summary,
        vec![("Done".to_string(), 0), ("In QA".to_string(), 1)]
    );

    // A rejected list leaves the stored rules unchanged.
    let mut tx = pool.begin().await.expect("begin");
    let result = replace_jira_status_rules_in_tx(
        &mut tx,
        &repo,
        activity.as_ref(),
        id,
        vec![rule("Done", "deploy", None), rule("done", "deploy", None)],
        actor(),
    )
    .await;
    assert!(matches!(result, Err(Error::InvalidInput(_))), "{result:?}");
    tx.rollback().await.expect("rollback");
    assert_eq!(
        jira_integration_repository(pool.clone())
            .list_rules(id)
            .await
            .expect("list rules")
            .len(),
        2
    );

    let metadata = activity_metadata(&pool, JIRA_INTEGRATION_RULES_REPLACED, id).await;
    assert_eq!(metadata.len(), 2);

    delete_team(&pool, team.id).await;
}

#[tokio::test]
async fn delete_cascades_rules_and_disables_the_shadow_user() {
    let pool = init_pg_pool().await;
    let team = insert_team(&pool).await;
    let created = create(&pool, &team, "Jira").await.expect("create");
    let id = created.integration.id;
    let shadow_user = created.integration.actor_user_id;

    let repo = jira_integration_repository_tx(pool.clone());
    let activity = activity_log_repository(pool.clone());
    let mut tx = pool.begin().await.expect("begin");
    replace_jira_status_rules_in_tx(
        &mut tx,
        &repo,
        activity.as_ref(),
        id,
        vec![rule("Done", "deploy", None)],
        actor(),
    )
    .await
    .expect("rules");
    delete_jira_integration_in_tx(&mut tx, &repo, activity.as_ref(), id, actor())
        .await
        .expect("delete");
    tx.commit().await.expect("commit");

    let read_repo = jira_integration_repository(pool.clone());
    assert!(read_repo.get(id).await.expect("get").is_none());
    let rule_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM jira_status_rules WHERE integration_id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("count rules");
    assert_eq!(rule_count, 0);
    let enabled: bool = sqlx::query_scalar("SELECT enabled FROM users WHERE id = $1")
        .bind(shadow_user)
        .fetch_one(&pool)
        .await
        .expect("shadow user kept");
    assert!(!enabled);
    assert_eq!(
        activity_metadata(&pool, JIRA_INTEGRATION_DELETED, id)
            .await
            .len(),
        1
    );

    let mut tx = pool.begin().await.expect("begin");
    let missing =
        delete_jira_integration_in_tx(&mut tx, &repo, activity.as_ref(), id, actor()).await;
    assert!(matches!(missing, Err(Error::NotFound(_))), "{missing:?}");

    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(shadow_user)
        .execute(&pool)
        .await
        .expect("delete shadow user");
    delete_team(&pool, team.id).await;
}
