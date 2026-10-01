use feature_toggle_backend::database::{init_pg_pool, run_migrations};
use feature_toggle_backend::grpc::pb;
use feature_toggle_backend::grpc::pb::feature_evaluation_client::FeatureEvaluationClient;
use feature_toggle_backend::grpc::{
    FeatureEvaluationSvc, feature_evaluation_server::FeatureEvaluationServer,
};
use std::net::{Ipv4Addr, SocketAddr};
use tokio::sync::broadcast;
use tokio::time::{Duration, Instant};
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;
use uuid::Uuid;

const SEEDED_CLIENT_ID: &str = "a1b2c3d4-0000-4000-8000-000000000001";
const SEEDED_CLIENT_SECRET: &str = "TEST_WEB_KEY_1";
const SEEDED_ENV_ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98f27";
// "Test Feature", owned by the seeded client's team (51ecc366-...).
const SEEDED_FEATURE_ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98f27";
// A seeded team other than the seeded client's team.
const OTHER_TEAM_ID: &str = "3eef17bc-9e06-411d-b5f4-7a786e68bb96";

async fn start_server(pool: sqlx::PgPool) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");
    let incoming = TcpListenerStream::new(listener);

    let (updates_tx, _) = broadcast::channel::<pb::FeatureUpdate>(64);
    let (evaluation_events_tx, _) = broadcast::channel::<
        feature_toggle_backend::logic::feature_evaluation::FeatureEvaluationEvent,
    >(64);
    let svc = FeatureEvaluationSvc::new(pool, updates_tx, evaluation_events_tx);
    let router = Server::builder().add_service(FeatureEvaluationServer::new(svc));

    let handle = tokio::spawn(async move {
        router
            .serve_with_incoming(incoming)
            .await
            .expect("grpc server should run");
    });

    (addr, handle)
}

async fn wait_for_evaluation_count(
    pool: &sqlx::PgPool,
    feature_key: &str,
    user_context: &str,
    expected_count: i64,
    timeout: Duration,
) -> i64 {
    let deadline = Instant::now() + timeout;
    loop {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::bigint FROM feature_evaluations WHERE feature_key = $1 AND user_context = $2",
        )
        .bind(feature_key)
        .bind(user_context)
        .fetch_one(pool)
        .await
        .expect("count query should succeed");

        if count == expected_count || Instant::now() >= deadline {
            return count;
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_assignment_count(
    pool: &sqlx::PgPool,
    user_id: &str,
    feature_id: Uuid,
    environment_id: Uuid,
    expected_count: i64,
    timeout: Duration,
) -> i64 {
    let deadline = Instant::now() + timeout;
    loop {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::bigint FROM user_flag_assignments WHERE user_id = $1 AND feature_id = $2 AND environment_id = $3",
        )
        .bind(user_id)
        .bind(feature_id)
        .bind(environment_id)
        .fetch_one(pool)
        .await
        .expect("count query should succeed");

        if count == expected_count || Instant::now() >= deadline {
            return count;
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn push_evaluation_events_dedupes_retries_with_reordered_context() {
    if std::env::var("DATABASE_URL").is_err() {
        eprintln!("Skipping test: DATABASE_URL is not set");
        return;
    }

    let pool = init_pg_pool().await;
    run_migrations(&pool)
        .await
        .expect("feature evaluation migrations should be applied");
    let (addr, server_handle) = start_server(pool.clone()).await;
    let endpoint = format!("http://{}", addr);
    let mut client = FeatureEvaluationClient::connect(endpoint)
        .await
        .expect("connect grpc client");

    let test_suffix = Uuid::new_v4().to_string();
    let feature_key = format!("grpc-idempotency-feature-{test_suffix}");
    let user_context = format!("grpc-idempotency-user-{test_suffix}");
    let evaluated_at_ms = chrono::Utc::now().timestamp_millis();

    let req_a = pb::PushEvaluationEventsRequest {
        events: vec![pb::FeatureEvaluationEvent {
            feature_key: feature_key.clone(),
            environment_id: SEEDED_ENV_ID.to_string(),
            client_id: SEEDED_CLIENT_ID.to_string(),
            client_secret: SEEDED_CLIENT_SECRET.to_string(),
            evaluation_result: true,
            evaluation_context: vec![
                pb::Context {
                    key: "region".to_string(),
                    value: "us".to_string(),
                },
                pb::Context {
                    key: "tier".to_string(),
                    value: "beta".to_string(),
                },
            ],
            user_context: user_context.clone(),
            evaluated_at_unix_ms: evaluated_at_ms,
            prior_assignment: false,
            variant: "control".to_string(),
            variant_value: "{\"enabled\":true}".to_string(),
        }],
    };

    // Same logical payload as req_a, but with evaluation_context entries reordered.
    let req_b = pb::PushEvaluationEventsRequest {
        events: vec![pb::FeatureEvaluationEvent {
            evaluation_context: vec![
                pb::Context {
                    key: "tier".to_string(),
                    value: "beta".to_string(),
                },
                pb::Context {
                    key: "region".to_string(),
                    value: "us".to_string(),
                },
            ],
            ..req_a.events[0].clone()
        }],
    };

    let first = client
        .push_evaluation_events(req_a)
        .await
        .expect("first push should succeed")
        .into_inner();
    assert_eq!(first.processed_count, 1);

    let second = client
        .push_evaluation_events(req_b)
        .await
        .expect("duplicate push should succeed")
        .into_inner();
    assert_eq!(second.processed_count, 1);

    let persisted = wait_for_evaluation_count(
        &pool,
        &feature_key,
        &user_context,
        1,
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(
        persisted, 1,
        "duplicate ingest should not create a second persisted row"
    );

    sqlx::query("DELETE FROM feature_evaluations WHERE feature_key = $1 AND user_context = $2")
        .bind(&feature_key)
        .bind(&user_context)
        .execute(&pool)
        .await
        .expect("cleanup should succeed");

    server_handle.abort();
}

#[tokio::test]
async fn push_user_assignments_upserts_duplicate_deliveries() {
    if std::env::var("DATABASE_URL").is_err() {
        eprintln!("Skipping test: DATABASE_URL is not set");
        return;
    }

    let pool = init_pg_pool().await;
    run_migrations(&pool)
        .await
        .expect("feature evaluation migrations should be applied");
    let (addr, server_handle) = start_server(pool.clone()).await;
    let endpoint = format!("http://{}", addr);
    let mut client = FeatureEvaluationClient::connect(endpoint)
        .await
        .expect("connect grpc client");

    let test_suffix = Uuid::new_v4().to_string();
    let user_id = format!("grpc-idempotency-user-{test_suffix}");
    let feature_id = Uuid::parse_str(SEEDED_FEATURE_ID).unwrap();
    let environment_id = Uuid::parse_str(SEEDED_ENV_ID).unwrap();
    let assignment = pb::UserFlagAssignment {
        user_id: user_id.clone(),
        feature_id: feature_id.to_string(),
        environment_id: environment_id.to_string(),
        assigned: true,
        client_id: SEEDED_CLIENT_ID.to_string(),
        client_secret: SEEDED_CLIENT_SECRET.to_string(),
        variant: "variant-a".to_string(),
    };

    let first = client
        .push_user_assignments(tokio_stream::iter(vec![assignment.clone()]))
        .await
        .expect("first push should succeed")
        .into_inner();
    assert!(!first.message_id.is_empty());

    let second = client
        .push_user_assignments(tokio_stream::iter(vec![assignment]))
        .await
        .expect("duplicate push should succeed")
        .into_inner();
    assert!(!second.message_id.is_empty());

    let persisted = wait_for_assignment_count(
        &pool,
        &user_id,
        feature_id,
        environment_id,
        1,
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(
        persisted, 1,
        "duplicate delivery should not create a second assignment row"
    );

    sqlx::query(
        "DELETE FROM user_flag_assignments WHERE user_id = $1 AND feature_id = $2 AND environment_id = $3",
    )
    .bind(&user_id)
    .bind(feature_id)
    .bind(environment_id)
    .execute(&pool)
    .await
    .expect("cleanup should succeed");

    server_handle.abort();
}

async fn count_assignments_for_user(pool: &sqlx::PgPool, user_id: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::bigint FROM user_flag_assignments WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("count query should succeed")
}

#[tokio::test]
async fn push_user_assignments_rejects_feature_of_another_team() {
    if std::env::var("DATABASE_URL").is_err() {
        eprintln!("Skipping test: DATABASE_URL is not set");
        return;
    }

    let pool = init_pg_pool().await;
    run_migrations(&pool)
        .await
        .expect("feature evaluation migrations should be applied");

    let test_suffix = Uuid::new_v4().to_string();
    let other_team_id = Uuid::parse_str(OTHER_TEAM_ID).unwrap();
    let foreign_feature_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO features (id, key, feature_type, team_id) VALUES ($1, $2, 'Simple', $3)",
    )
    .bind(foreign_feature_id)
    .bind(format!("b11-foreign-feature-{test_suffix}"))
    .bind(other_team_id)
    .execute(&pool)
    .await
    .expect("insert other-team feature");

    let (addr, server_handle) = start_server(pool.clone()).await;
    let mut client = FeatureEvaluationClient::connect(format!("http://{}", addr))
        .await
        .expect("connect grpc client");

    let user_id = format!("b11-foreign-feature-user-{test_suffix}");
    let assignment = pb::UserFlagAssignment {
        user_id: user_id.clone(),
        feature_id: foreign_feature_id.to_string(),
        environment_id: SEEDED_ENV_ID.to_string(),
        assigned: true,
        client_id: SEEDED_CLIENT_ID.to_string(),
        client_secret: SEEDED_CLIENT_SECRET.to_string(),
        variant: "attacker-variant".to_string(),
    };

    let result = client
        .push_user_assignments(tokio_stream::iter(vec![assignment]))
        .await;
    let rows = count_assignments_for_user(&pool, &user_id).await;

    sqlx::query("DELETE FROM user_flag_assignments WHERE user_id = $1")
        .bind(&user_id)
        .execute(&pool)
        .await
        .expect("cleanup assignments");
    sqlx::query("DELETE FROM features WHERE id = $1")
        .bind(foreign_feature_id)
        .execute(&pool)
        .await
        .expect("cleanup feature");
    server_handle.abort();

    let status = result.expect_err("push for another team's feature must be rejected");
    assert_eq!(status.code(), tonic::Code::PermissionDenied, "{status:?}");
    assert_eq!(
        rows, 0,
        "no assignment row may be written for another team's feature"
    );
}

#[tokio::test]
async fn push_user_assignments_rejects_environment_of_another_team() {
    if std::env::var("DATABASE_URL").is_err() {
        eprintln!("Skipping test: DATABASE_URL is not set");
        return;
    }

    let pool = init_pg_pool().await;
    run_migrations(&pool)
        .await
        .expect("feature evaluation migrations should be applied");

    let test_suffix = Uuid::new_v4().to_string();
    let other_team_id = Uuid::parse_str(OTHER_TEAM_ID).unwrap();
    let foreign_env_id = Uuid::new_v4();
    sqlx::query("INSERT INTO environments (id, name, active, team_id) VALUES ($1, $2, true, $3)")
        .bind(foreign_env_id)
        .bind(format!("b11-foreign-env-{test_suffix}"))
        .bind(other_team_id)
        .execute(&pool)
        .await
        .expect("insert other-team environment");

    let (addr, server_handle) = start_server(pool.clone()).await;
    let mut client = FeatureEvaluationClient::connect(format!("http://{}", addr))
        .await
        .expect("connect grpc client");

    let user_id = format!("b11-foreign-env-user-{test_suffix}");
    let own_row = pb::UserFlagAssignment {
        user_id: user_id.clone(),
        feature_id: SEEDED_FEATURE_ID.to_string(),
        environment_id: foreign_env_id.to_string(),
        assigned: true,
        client_id: SEEDED_CLIENT_ID.to_string(),
        client_secret: SEEDED_CLIENT_SECRET.to_string(),
        variant: String::new(),
    };
    let unknown_ids_user = format!("b11-unknown-ids-user-{test_suffix}");
    let unknown_ids = pb::UserFlagAssignment {
        user_id: unknown_ids_user.clone(),
        feature_id: Uuid::new_v4().to_string(),
        environment_id: Uuid::new_v4().to_string(),
        assigned: true,
        client_id: SEEDED_CLIENT_ID.to_string(),
        client_secret: SEEDED_CLIENT_SECRET.to_string(),
        variant: String::new(),
    };

    let foreign_env_result = client
        .push_user_assignments(tokio_stream::iter(vec![own_row]))
        .await;
    let unknown_ids_result = client
        .push_user_assignments(tokio_stream::iter(vec![unknown_ids]))
        .await;
    let foreign_env_rows = count_assignments_for_user(&pool, &user_id).await;
    let unknown_ids_rows = count_assignments_for_user(&pool, &unknown_ids_user).await;

    sqlx::query("DELETE FROM user_flag_assignments WHERE user_id = ANY($1)")
        .bind(vec![user_id.clone(), unknown_ids_user.clone()])
        .execute(&pool)
        .await
        .expect("cleanup assignments");
    sqlx::query("DELETE FROM environments WHERE id = $1")
        .bind(foreign_env_id)
        .execute(&pool)
        .await
        .expect("cleanup environment");
    server_handle.abort();

    let status =
        foreign_env_result.expect_err("push for another team's environment must be rejected");
    assert_eq!(status.code(), tonic::Code::PermissionDenied, "{status:?}");
    assert_eq!(foreign_env_rows, 0);

    let status =
        unknown_ids_result.expect_err("push for unknown feature/environment must be rejected");
    assert_eq!(status.code(), tonic::Code::PermissionDenied, "{status:?}");
    assert_eq!(unknown_ids_rows, 0);
}

fn seeded_assignment(
    user_id: &str,
    variant: &str,
    with_credentials: bool,
) -> pb::UserFlagAssignment {
    pb::UserFlagAssignment {
        user_id: user_id.to_string(),
        feature_id: SEEDED_FEATURE_ID.to_string(),
        environment_id: SEEDED_ENV_ID.to_string(),
        assigned: true,
        client_id: if with_credentials {
            SEEDED_CLIENT_ID.to_string()
        } else {
            String::new()
        },
        client_secret: if with_credentials {
            SEEDED_CLIENT_SECRET.to_string()
        } else {
            String::new()
        },
        variant: variant.to_string(),
    }
}

#[tokio::test]
async fn push_user_assignments_keeps_last_write_for_duplicate_keys_in_one_stream() {
    if std::env::var("DATABASE_URL").is_err() {
        eprintln!("Skipping test: DATABASE_URL is not set");
        return;
    }

    let pool = init_pg_pool().await;
    run_migrations(&pool)
        .await
        .expect("feature evaluation migrations should be applied");
    let (addr, server_handle) = start_server(pool.clone()).await;
    let mut client = FeatureEvaluationClient::connect(format!("http://{}", addr))
        .await
        .expect("connect grpc client");

    let user_id = format!("p03-last-write-user-{}", Uuid::new_v4());
    let result = client
        .push_user_assignments(tokio_stream::iter(vec![
            seeded_assignment(&user_id, "variant-a", true),
            seeded_assignment(&user_id, "variant-b", false),
        ]))
        .await;

    let stored = sqlx::query_as::<_, (Option<String>,)>(
        "SELECT variant FROM user_flag_assignments WHERE user_id = $1",
    )
    .bind(&user_id)
    .fetch_all(&pool)
    .await
    .expect("query stored assignments");

    sqlx::query("DELETE FROM user_flag_assignments WHERE user_id = $1")
        .bind(&user_id)
        .execute(&pool)
        .await
        .expect("cleanup assignments");
    server_handle.abort();

    result.expect("push with a repeated key should succeed");
    assert_eq!(stored, vec![(Some("variant-b".to_string()),)]);
}

#[tokio::test]
async fn push_user_assignments_stores_every_row_of_a_large_stream() {
    if std::env::var("DATABASE_URL").is_err() {
        eprintln!("Skipping test: DATABASE_URL is not set");
        return;
    }

    let pool = init_pg_pool().await;
    run_migrations(&pool)
        .await
        .expect("feature evaluation migrations should be applied");
    let (addr, server_handle) = start_server(pool.clone()).await;
    let mut client = FeatureEvaluationClient::connect(format!("http://{}", addr))
        .await
        .expect("connect grpc client");

    // More than two backend chunks, with a partial last chunk.
    const ROWS: usize = 1500;
    let prefix = format!("p03-large-stream-{}-", Uuid::new_v4());
    let messages: Vec<_> = (0..ROWS)
        .map(|i| seeded_assignment(&format!("{prefix}{i}"), "variant-a", i == 0))
        .collect();

    let result = client
        .push_user_assignments(tokio_stream::iter(messages))
        .await;

    let count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::bigint FROM user_flag_assignments WHERE user_id LIKE $1",
    )
    .bind(format!("{prefix}%"))
    .fetch_one(&pool)
    .await
    .expect("count stored assignments");

    sqlx::query("DELETE FROM user_flag_assignments WHERE user_id LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(&pool)
        .await
        .expect("cleanup assignments");
    server_handle.abort();

    result.expect("large push should succeed");
    assert_eq!(count, ROWS as i64);
}
