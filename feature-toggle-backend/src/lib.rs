mod config;
pub mod database;
pub mod graphql;
pub mod grpc;
pub mod logic;
mod middleware;
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
use actix_web::{App, HttpMessage, HttpRequest, HttpResponse, HttpServer, Result, guard, web};
use async_graphql::Schema;
use async_graphql::http::GraphiQLSource;
use async_graphql_actix_web::{GraphQLRequest, GraphQLResponse, GraphQLSubscription};
use log::error;
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

    let environment_repository = database::environment::environment_repository(db_pool.clone());
    let environment_logic = logic::environment::environment_logic(
        environment_repository.clone(),
        activity_log_repository.clone_box(),
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
    let feature_logic = logic::feature::feature_logic(
        database::feature::feature_repository(db_pool.clone()),
        environment_logic.clone(),
        activity_log_repository.clone_box(),
    );
    // Create a broadcast channel for feature updates shared between GraphQL mutations and gRPC streaming
    let (updates_tx, _updates_rx) =
        tokio::sync::broadcast::channel::<crate::grpc::pb::FeatureUpdate>(128);

    let client_logic = logic::client::client_logic(
        database::client::client_repository(db_pool.clone()),
        activity_log_repository.clone_box(),
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
    let feature_evaluation_logic = logic::feature_evaluation::feature_evaluation_logic(
        database::feature_evaluation::feature_evaluation_repository(db_pool.clone()),
    );

    // Initialize JWT secret on startup
    jwt_secret_logic
        .initialize_secret()
        .await
        .expect("Failed to initialize JWT secret");
    log::info!("JWT secret initialized successfully");

    // Initialize JWT secret on startup
    jwt_secret_logic
        .initialize_secret()
        .await
        .expect("Failed to initialize JWT secret");
    log::info!("JWT secret initialized successfully");

    let grpc_pool = db_pool.clone();
    let grpc_updates_tx = updates_tx.clone();
    // Spawn gRPC server on separate task
    let grpc_addr: std::net::SocketAddr = cfg
        .grpc_socket_addr()
        .unwrap_or_else(|_| "0.0.0.0:50051".parse().unwrap());
    tokio::spawn(async move {
        if let Err(e) = crate::grpc::serve(grpc_pool, grpc_addr, grpc_updates_tx).await {
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

    // Clone values for use in the HttpServer closure
    let jwt_secret_logic_for_server = jwt_secret_logic.clone();
    let jwt_token_logic_for_server = jwt_token_logic.clone();

    HttpServer::new(move || {
        let admin_state = AdminState::new();

        let schema = Schema::build(Query, MutationRoot, FeatureEvaluationSubscription)
            .data(db_pool.clone())
            .data(updates_tx.clone())
            .data(environment_logic.clone())
            .data(team_logic.clone())
            .data(pipeline_logic.clone())
            .data(feature_logic.clone())
            .data(client_logic.clone())
            .data(context_logic.clone())
            .data(user_logic.clone())
            .data(role_logic.clone())
            .data(jwt_secret_logic_for_server.clone())
            .data(jwt_token_logic_for_server.clone())
            .data(feature_evaluation_logic.clone())
            .data(admin_state.clone())
            // .extension(ApolloTracing)
            .finish();

        let cors = Cors::default()
            .allowed_origin(&cfg.allowed_origin) // configured frontend origin
            .allowed_methods(vec!["GET", "POST", "PUT", "DELETE", "OPTIONS"]) // Or your allowed methods
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
            .app_data(web::Data::new(schema.clone()))
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

fn setup_logger() -> Result<(), Box<dyn std::error::Error>> {
    log4rs::init_file("log4rs.yaml", Default::default())?;
    Ok(())
}
