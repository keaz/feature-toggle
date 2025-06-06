pub mod database;
mod graphql;
mod logic;

use crate::database::init_pg_pool;
use crate::graphql::mutation::MutationRoot;
use crate::graphql::query::Query;
use actix_cors::Cors;
use actix_web::{App, HttpRequest, HttpResponse, HttpServer, Result, guard, web};
use async_graphql::http::GraphiQLSource;
use async_graphql::{EmptyMutation, EmptySubscription, Schema};
use async_graphql_actix_web::{GraphQL, GraphQLSubscription};

pub async fn run() -> std::io::Result<()> {
    let db_pool = init_pg_pool().await;
    let environment_repository = database::environment::environment_repository(db_pool.clone());
    let environment_logic = logic::environment::environment_logic(environment_repository.clone());

    HttpServer::new(move || {
        let schema = Schema::build(Query, MutationRoot, EmptySubscription)
            .data(db_pool.clone())
            .data(environment_logic.clone())
            .finish();

        let cors = Cors::default()
            .allowed_origin("http://localhost:5173") // Or your frontend's domain
            .allowed_methods(vec!["GET", "POST", "PUT", "DELETE", "OPTIONS"]) // Or your allowed methods
            .allowed_headers(vec!["content-type", "authorization"]) // Or your allowed headers
            .max_age(3600);

        App::new()
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
    .bind(("127.0.0.1", 8080))?
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
