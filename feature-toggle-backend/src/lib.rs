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
use crate::middleware::session_guard::SessionGuard;
use actix_cors::Cors;
use actix_session::{storage::CookieSessionStore, Session, SessionMiddleware};
use actix_web::cookie::{Key, SameSite};
use actix_web::{guard, web, App, HttpRequest, HttpResponse, HttpServer, Result};
use async_graphql::http::GraphiQLSource;
use async_graphql::{EmptyMutation, EmptySubscription, Schema};
use async_graphql_actix_web::{GraphQL, GraphQLRequest, GraphQLResponse, GraphQLSubscription};
use base64::Engine;
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
            .supports_credentials()
            .max_age(3600);

        //#FIXME: Move this to a config or database.
        let session_key = Key::from(&base64::engine::general_purpose::STANDARD
            .decode("J06X7Bb28hc0bT7kn+OZoLaUQPV5tD/rNIBsSJsP6Ler0K/HHRkEnmu29fVFhefyOV6X096t+te3bnQi3yMwlw==").unwrap());

        App::new()
            // Order of wraps: last registered runs first. We want AdminGuard first, then SessionGuard, then AccessLogger.
            .wrap(SessionGuard::new(cfg.allowed_origin.clone()))
            .wrap(AdminGuard::new(db_pool.clone(), cfg.allowed_origin.clone(), admin_state.clone()))
            .wrap(SessionMiddleware::builder(CookieSessionStore::default(), session_key.clone())
                .cookie_name("d".to_string())
                .cookie_secure(false) // This should be changed to true in prod
                .cookie_http_only(true)
                .cookie_same_site(SameSite::Lax) // This should be changed to None in prod
                .build())
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

async fn graphql_handler(
    schema: web::Data<Schema<Query, MutationRoot, EmptySubscription>>,
    session: Session,
    req: GraphQLRequest,
) -> GraphQLResponse {
    let inner = req.into_inner();
    let is_login = inner.query.contains("mutation") && inner.query.contains("login");

    let resp = schema.execute(inner).await;

    // If this was a login mutation and it succeeded, set the session
    if is_login && resp.errors.is_empty() {
        // Try to extract the user id from the data payload: { "login": { "id": "..." } }
        let v = serde_json::to_value(&resp.data).unwrap_or(serde_json::json!({}));
        let user_id = v.get("login").and_then(|l| l.get("id")).and_then(|id| id.as_str());
        if let Some(uid) = user_id {
            let _ = session.insert("user_id", uid);
        }
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
