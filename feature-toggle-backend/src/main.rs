mod mutation;

use actix_web::{App, HttpRequest, HttpResponse, HttpServer, guard, web, Result};
use async_graphql::{Context, EmptyMutation, EmptySubscription, Object, Schema, SimpleObject};
use async_graphql_actix_web::{GraphQL, GraphQLRequest, GraphQLResponse, GraphQLSubscription};
use std::net::SocketAddr;
use actix_cors::Cors;
use async_graphql::http::GraphiQLSource;
use crate::mutation::MutationRoot;

#[tokio::main]
async fn main() -> std::io::Result<()> {

    HttpServer::new(|| {
        let schema = Schema::build(Query, MutationRoot, EmptySubscription).finish();
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
            .service(web::resource("/graphql").guard(guard::Get()).to(index_graphiql))
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
) -> actix_web::Result<HttpResponse> {
    GraphQLSubscription::new(Schema::clone(&*schema)).start(&req, payload)
}

struct Query;

#[Object]
impl Query {
    /// Returns the sum of a and b
    async fn add(&self, a: i32, b: i32) -> i32 {
        a + b
    }

    async fn value_from_db(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of object")] id: i64
    ) -> String {

        String::from("Hello World")
    }
}
