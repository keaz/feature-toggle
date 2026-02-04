pub mod cluster;
mod config;
pub mod database;
pub mod graphql;
pub mod grpc;
pub mod logic;
mod middleware;
pub mod rest;
pub mod scheduler;
pub mod utils;

use crate::database::init_pg_pool;
use crate::graphql::mutation::MutationRoot;
use crate::graphql::query::Query;
use crate::graphql::subscription::FeatureEvaluationSubscription;
use crate::middleware::access_log::AccessLogger;
use crate::middleware::admin_guard::{AdminGuard, AdminState};
use crate::middleware::jwt_guard::JwtGuard;
use actix_cors::Cors;
use actix_web::error::{
    ErrorBadRequest, ErrorConflict, ErrorForbidden, ErrorInternalServerError, ErrorUnauthorized,
};
use actix_web::{App, HttpMessage, HttpRequest, HttpResponse, HttpServer, Result, guard, web};
use async_graphql::Schema;
use async_graphql::http::GraphiQLSource;
use async_graphql_actix_web::{GraphQLRequest, GraphQLResponse, GraphQLSubscription};
use chrono::{DateTime, Utc};
use log::error;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Record does not exists for the id {0}")]
    NotFound(Uuid),
    #[error("Database error occurred")]
    DatabaseError(#[source] sqlx::Error),
    #[error("Record {0} already exists")]
    RecordAlreadyExists(String),
    #[error("Invalid input: {0}")]
    InvalidInput(String),
}

pub async fn run() -> std::io::Result<()> {
    setup_logger().unwrap();

    // Load configuration (from TOML or defaults)
    let cfg = crate::config::Config::load();

    let db_pool = init_pg_pool().await;

    // Initialize activity log repository (shared across all logic layers)
    let activity_log_repository = database::activity_log::activity_log_repository(db_pool.clone());
    // Wrap in Arc for thread-safe cloning in HttpServer closure
    let activity_log_repository_arc = Arc::new(activity_log_repository.clone_box());

    // Initialize feature repository (needed for entity resolution in activity logs)
    let feature_repository = database::feature::feature_repository(db_pool.clone());
    let feature_repository_arc = Arc::new(feature_repository.clone_box());

    let environment_repository = database::environment::environment_repository(db_pool.clone());
    let variant_allocations_repository =
        database::variant_allocations::variant_allocations_repository(db_pool.clone());
    let compound_rules_repository =
        database::compound_rules::compound_rules_repository(db_pool.clone());
    let environment_logic = logic::environment::environment_logic(
        environment_repository.clone(),
        activity_log_repository.clone_box(),
    );
    let approval_repository = database::approval::approval_repository(db_pool.clone());
    let role_repository = database::role::role_repository(db_pool.clone());
    let (approval_events_tx, _approval_events_rx) =
        tokio::sync::broadcast::channel::<logic::approval::ApprovalRequestEvent>(128);

    // Create a broadcast channel for feature updates shared between GraphQL mutations and gRPC streaming
    let (updates_tx, _updates_rx) =
        tokio::sync::broadcast::channel::<crate::grpc::pb::FeatureUpdate>(128);

    let approval_logic = logic::approval::approval_logic_with_pool(
        db_pool.clone(),
        approval_repository.clone(),
        feature_repository.clone_box(),
        environment_logic.clone(),
        role_repository.clone(),
        approval_events_tx.clone(),
        updates_tx.clone(),
    );
    let team_logic = logic::team::team_logic(
        database::team::team_repository(db_pool.clone()),
        activity_log_repository.clone_box(),
    );
    let pipeline_logic = logic::pipeline::pipeline_logic(
        database::pipeline::pipeline_repository(db_pool.clone()),
        environment_logic.clone(),
        activity_log_repository.clone_box(),
    );
    let feature_logic = logic::feature::feature_logic_with_approval(
        feature_repository.clone_box(),
        environment_logic.clone(),
        activity_log_repository.clone_box(),
        database::user::user_repository(db_pool.clone()),
        Some(approval_logic.clone()),
    );

    let client_logic = logic::client::client_logic(
        database::client::client_repository(db_pool.clone()),
        activity_log_repository.clone_box(),
    );
    let metric_logic = logic::metrics::metric_logic(
        database::metrics::metric_repository(db_pool.clone()),
        database::client::client_repository(db_pool.clone()),
    );
    let context_logic = logic::context::context_logic(
        database::context::context_repository(db_pool.clone()),
        database::feature::feature_repository(db_pool.clone()),
        updates_tx.clone(),
    );
    let user_logic = logic::user::user_logic(
        database::user::user_repository(db_pool.clone()),
        activity_log_repository.clone_box(),
    );
    let role_logic = logic::role::role_logic(
        database::role::role_repository(db_pool.clone()),
        activity_log_repository.clone_box(),
    );
    let jwt_secret_logic = logic::jwt_secret::jwt_secret_logic(db_pool.clone());
    let jwt_token_logic = logic::jwt_token::jwt_token_logic(
        database::jwt_token::jwt_token_repository(db_pool.clone()),
        user_logic.clone(),
        role_logic.clone(),
        jwt_secret_logic.clone(),
    );
    // Broadcast channel for feature evaluation events powering GraphQL subscriptions.
    let (evaluation_events_tx, _evaluation_events_rx_unused) =
        tokio::sync::broadcast::channel::<logic::feature_evaluation::FeatureEvaluationEvent>(512);

    let _cluster_guard = cluster::start(
        &cfg.cluster,
        Some(db_pool.clone()),
        updates_tx.clone(),
        evaluation_events_tx.clone(),
    );
    let feature_evaluation_logic = logic::feature_evaluation::feature_evaluation_logic_with_events(
        database::feature_evaluation::feature_evaluation_repository(db_pool.clone()),
        evaluation_events_tx.clone(),
    );

    // Initialize JWT secret on startup (called once)
    jwt_secret_logic
        .initialize_secret()
        .await
        .expect("Failed to initialize JWT secret");
    log::info!("JWT secret initialized successfully");

    let grpc_pool = db_pool.clone();
    let grpc_updates_tx = updates_tx.clone();
    // Clone evaluation events sender for gRPC so original can be used later in HttpServer closure
    let evaluation_events_tx_for_grpc = evaluation_events_tx.clone();
    // Spawn gRPC server on separate task
    let grpc_addr: std::net::SocketAddr = cfg
        .grpc_socket_addr()
        .unwrap_or_else(|_| "0.0.0.0:50051".parse().unwrap());
    tokio::spawn(async move {
        if let Err(e) = crate::grpc::serve(
            grpc_pool,
            grpc_addr,
            grpc_updates_tx,
            evaluation_events_tx_for_grpc,
        )
        .await
        {
            error!("gRPC server error: {e}");
        }
    });

    // Start kill switch rollback scheduler
    let scheduler_feature_logic = feature_logic.clone();
    let scheduler_feature_repo = database::feature::feature_repository(db_pool.clone());
    let scheduler_pool = db_pool.clone();
    let scheduler_updates_tx = updates_tx.clone();
    tokio::spawn(async move {
        let scheduler = scheduler::KillSwitchRollbackScheduler::new(
            scheduler_feature_logic,
            scheduler_feature_repo,
            scheduler_pool,
            scheduler_updates_tx,
        );
        scheduler.start_scheduler().await;
    });

    // Metrics aggregation scheduler (hourly)
    let metrics_logic_for_scheduler = metric_logic.clone();
    tokio::spawn(async move {
        let aggregator = scheduler::MetricsAggregator::new(
            metrics_logic_for_scheduler,
            std::time::Duration::from_secs(3600),
        );
        aggregator.start().await;
    });

    let auto_approval_scheduler = scheduler::AutoApprovalScheduler::new(
        approval_repository.clone_box(),
        approval_logic.clone(),
        Duration::from_secs(60),
    );
    tokio::spawn(async move {
        auto_approval_scheduler.start().await;
    });

    // Clone values for use in the HttpServer closure
    let jwt_secret_logic_for_server = jwt_secret_logic.clone();
    let jwt_token_logic_for_server = jwt_token_logic.clone();

    HttpServer::new(move || {
        let admin_state = AdminState::new();

        let schema = Schema::build(Query, MutationRoot, FeatureEvaluationSubscription)
            .data(db_pool.clone())
            .data(updates_tx.clone())
            // Channel for evaluation events consumed by subscriptions
            .data(evaluation_events_tx.clone())
            .data(activity_log_repository_arc.clone())
            .data(activity_log_repository_arc.clone_box())
            .data(feature_repository_arc.clone())
            .data(feature_repository.clone())
            .data(approval_repository.clone_box())
            .data(environment_logic.clone())
            .data(team_logic.clone())
            .data(pipeline_logic.clone())
            .data(feature_logic.clone())
            .data(approval_logic.clone())
            .data(approval_events_tx.clone())
            .data(client_logic.clone())
            .data(context_logic.clone())
            .data(user_logic.clone())
            .data(role_logic.clone())
            .data(jwt_secret_logic_for_server.clone())
            .data(jwt_token_logic_for_server.clone())
            .data(feature_evaluation_logic.clone())
            .data(metric_logic.clone())
            .data(admin_state.clone())
            // .extension(ApolloTracing)
            .finish();

        let cors = Cors::default()
            .allowed_origin(&cfg.allowed_origin) // configured frontend origin
            .allowed_methods(vec!["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"]) // Or your allowed methods
            .allowed_headers(vec!["content-type", "authorization"]) // Or your allowed headers
            .supports_credentials()
            .max_age(3600);

        App::new()
            // Order of wraps: last registered runs first. We want AdminGuard first, then JwtGuard, then AccessLogger.
            .wrap(JwtGuard::new(
                cfg.allowed_origin.clone(),
                jwt_secret_logic_for_server.clone(),
                db_pool.clone(),
            ))
            .wrap(AdminGuard::new(
                db_pool.clone(),
                cfg.allowed_origin.clone(),
                admin_state.clone(),
            ))
            .wrap(AccessLogger)
            .wrap(cors)
            .app_data(web::Data::new(db_pool.clone()))
            .app_data(web::Data::new(schema.clone()))
            .app_data(web::Data::new(metric_logic.clone()))
            .app_data(web::Data::new(feature_evaluation_logic.clone()))
            .app_data(web::Data::new(environment_logic.clone()))
            .app_data(web::Data::new(pipeline_logic.clone()))
            .app_data(web::Data::new(team_logic.clone()))
            .app_data(web::Data::new(context_logic.clone()))
            .app_data(web::Data::new(client_logic.clone()))
            .app_data(web::Data::new(feature_logic.clone()))
            .app_data(web::Data::new(approval_logic.clone()))
            .app_data(web::Data::new(approval_repository.clone_box()))
            .app_data(web::Data::new(activity_log_repository.clone_box()))
            .app_data(web::Data::new(role_logic.clone()))
            .app_data(web::Data::new(user_logic.clone()))
            .app_data(web::Data::new(jwt_token_logic_for_server.clone()))
            .app_data(web::Data::new(jwt_secret_logic_for_server.clone()))
            .app_data(web::Data::new(evaluation_events_tx.clone()))
            .app_data(web::Data::new(approval_events_tx.clone()))
            .app_data(web::Data::new(admin_state.clone()))
            .app_data(web::Data::new(feature_repository.clone_box()))
            .app_data(web::Data::new(variant_allocations_repository.clone()))
            .app_data(web::Data::new(compound_rules_repository.clone()))
            .app_data(web::Data::new(updates_tx.clone()))
            .service(
                web::resource("/graphql")
                    .guard(guard::Post())
                    .to(graphql_handler),
            )
            .service(
                web::resource("/graphql")
                    .guard(guard::Get())
                    .guard(guard::Header("upgrade", "websocket"))
                    .app_data(web::Data::new(schema))
                    .to(index_ws),
            )
            .service(
                web::resource("/graphql")
                    .guard(guard::Get())
                    .to(index_graphiql),
            )
            .service(
                web::resource("/metrics/track")
                    .guard(guard::Post())
                    .to(track_metrics_http),
            )
            .configure(rest::configure)
            .service(rest::swagger_ui())
    })
    .bind(&cfg.http_addr)?
    .run()
    .await
}

#[derive(Clone)]
pub struct JwtUser {
    pub id: uuid::Uuid,
    pub username: String,
    pub is_admin: bool,
    pub roles: Vec<String>,
    pub token_hash: String, // SHA256 hash of the current token for logout
}

async fn graphql_handler(
    schema: web::Data<Schema<Query, MutationRoot, FeatureEvaluationSubscription>>,
    req: HttpRequest,
    gql_req: GraphQLRequest,
) -> GraphQLResponse {
    let mut inner = gql_req.into_inner();

    // Inject JWT user data into the GraphQL request data (if present from middleware)
    if let Some(jwt_user) = req.extensions().get::<JwtUser>() {
        inner = inner.data(jwt_user.clone());
    }

    let is_login = inner.query.contains("mutation") && inner.query.contains("login");

    let resp = schema.execute(inner).await;

    // If this was a login mutation and it succeeded, we don't need to set session anymore
    // The JWT token will be returned in the response data
    if is_login && resp.errors.is_empty() {
        // The login mutation should return the JWT token in its response
        // No additional session handling needed
    }

    resp.into()
}

async fn index_graphiql() -> Result<HttpResponse> {
    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(
            GraphiQLSource::build()
                .endpoint("/graphql")
                .subscription_endpoint("/graphql")
                .finish(),
        ))
}

async fn index_ws(
    schema: web::Data<Schema<Query, MutationRoot, FeatureEvaluationSubscription>>,
    req: HttpRequest,
    payload: web::Payload,
) -> Result<HttpResponse> {
    GraphQLSubscription::new(Schema::clone(&*schema)).start(&req, payload)
}

async fn track_metrics_http(
    metric_logic: web::Data<Box<dyn crate::logic::metrics::MetricLogic>>,
    payload: web::Json<crate::rest::metrics::TrackMetricsRequest>,
) -> Result<HttpResponse, actix_web::Error> {
    let body = payload.into_inner();
    let mut events = Vec::with_capacity(body.events.len());

    for ev in body.events {
        let environment_id = match ev.environment_id {
            Some(ref env) if !env.is_empty() => {
                Some(Uuid::parse_str(env).map_err(|_| ErrorBadRequest("invalid environment_id"))?)
            }
            _ => None,
        };

        let timestamp = match ev.timestamp_unix_ms {
            Some(ts) if ts > 0 => Some(
                DateTime::<Utc>::from_timestamp_millis(ts)
                    .ok_or_else(|| ErrorBadRequest("invalid timestamp_unix_ms"))?,
            ),
            _ => None,
        };

        events.push(crate::logic::metrics::TrackMetricInput {
            metric_key: ev.metric_key,
            feature_key: ev.feature_key,
            environment_id,
            user_context: ev.user_context,
            variant: ev.variant,
            value: ev.value,
            metadata: ev.metadata,
            timestamp,
        });
    }

    let processed = metric_logic
        .track_metrics(&body.client_id, &body.client_secret, events)
        .await
        .map_err(map_metric_logic_error_to_http)?;

    Ok(HttpResponse::Ok().json(serde_json::json!({ "processed": processed })))
}

fn map_metric_logic_error_to_http(
    err: crate::logic::metrics::MetricLogicError,
) -> actix_web::Error {
    match err {
        crate::logic::metrics::MetricLogicError::InvalidInput(msg) => ErrorBadRequest(msg),
        crate::logic::metrics::MetricLogicError::NotFound(msg) => ErrorBadRequest(msg),
        crate::logic::metrics::MetricLogicError::RecordAlreadyExists(msg) => ErrorConflict(msg),
        crate::logic::metrics::MetricLogicError::Unauthenticated(msg) => ErrorUnauthorized(msg),
        crate::logic::metrics::MetricLogicError::PermissionDenied(msg) => ErrorForbidden(msg),
        crate::logic::metrics::MetricLogicError::Database(e) => {
            ErrorInternalServerError(format!("database error: {e}"))
        }
    }
}

fn setup_logger() -> Result<(), Box<dyn std::error::Error>> {
    log4rs::init_file("log4rs.yaml", Default::default())?;
    Ok(())
}
