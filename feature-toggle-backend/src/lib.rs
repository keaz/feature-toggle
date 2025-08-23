pub mod database;
mod graphql;
pub mod grpc;
mod logic;
mod middleware;
mod config;

use crate::database::init_pg_pool;
use crate::graphql::mutation::MutationRoot;
use crate::graphql::query::Query;
use crate::middleware::access_log::AccessLogger;
use crate::middleware::admin_guard::{AdminGuard, AdminState};
use actix_cors::Cors;
use actix_web::{guard, web, App, HttpRequest, HttpResponse, HttpServer, Result};
use async_graphql::http::GraphiQLSource;
use async_graphql::{EmptyMutation, EmptySubscription, Schema};
use async_graphql_actix_web::{GraphQL, GraphQLSubscription};
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
    let environment_repository = database::environment::environment_repository(db_pool.clone());
    let environment_logic = logic::environment::environment_logic(environment_repository.clone());
    let team_logic = logic::team::team_logic(database::team::team_repository(db_pool.clone()));
    let pipeline_logic = logic::pipeline::pipeline_logic(
        database::pipeline::pipeline_repository(db_pool.clone()),
        environment_logic.clone(),
    );
    let feature_logic = logic::feature::feature_logic(
        database::feature::feature_repository(db_pool.clone()),
        environment_logic.clone(),
    );
    let client_logic =
        logic::client::client_logic(database::client::client_repository(db_pool.clone()));
    let context_logic =
        logic::context::context_logic(database::context::context_repository(db_pool.clone()));
    let user_logic =
        logic::user::user_logic(database::user::user_repository(db_pool.clone()));

    // Create a broadcast channel for feature updates shared between GraphQL mutations and gRPC streaming
    let (updates_tx, _updates_rx) =
        tokio::sync::broadcast::channel::<crate::grpc::pb::FeatureUpdate>(128);

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

    HttpServer::new(move || {
        let admin_state = AdminState::new();

        let schema = Schema::build(Query, MutationRoot, EmptySubscription)
            .data(db_pool.clone())
            .data(updates_tx.clone())
            .data(environment_logic.clone())
            .data(team_logic.clone())
            .data(pipeline_logic.clone())
            .data(feature_logic.clone())
            .data(client_logic.clone())
            .data(context_logic.clone())
            .data(user_logic.clone())
            .data(admin_state.clone())
            // .extension(ApolloTracing)
            .finish();

        let cors = Cors::default()
            .allowed_origin(&cfg.allowed_origin) // configured frontend origin
            .allowed_methods(vec!["GET", "POST", "PUT", "DELETE", "OPTIONS"]) // Or your allowed methods
            .allowed_headers(vec!["content-type", "authorization"]) // Or your allowed headers
            .max_age(3600);

        App::new()
            // Order of wraps: last registered runs first. We want AccessLogger first, then AdminGuard.
            .wrap(AdminGuard::new(db_pool.clone(), cfg.allowed_origin.clone(), admin_state.clone()))
            .wrap(AccessLogger)
            .wrap(cors)
            .service(
                web::resource("/graphql")
                    .guard(guard::Post())
                    .to(GraphQL::new(schema.clone())),
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
    schema: web::Data<Schema<Query, EmptyMutation, EmptySubscription>>,
    req: HttpRequest,
    payload: web::Payload,
) -> Result<HttpResponse> {
    GraphQLSubscription::new(Schema::clone(&*schema)).start(&req, payload)
}

fn setup_logger() -> Result<(), Box<dyn std::error::Error>> {
    log4rs::init_file("log4rs.yaml", Default::default())?;
    Ok(())
}
