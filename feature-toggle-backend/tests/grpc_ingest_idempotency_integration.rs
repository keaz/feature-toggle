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
    let feature_id = Uuid::new_v4();
    let environment_id = Uuid::new_v4();
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
