mod mutation;

use actix_web::{App, HttpRequest, HttpResponse, HttpServer, guard, web, Result};
use async_graphql::{Context, EmptyMutation, EmptySubscription, Object, Schema, SimpleObject};
use async_graphql_actix_web::{GraphQL, GraphQLRequest, GraphQLResponse, GraphQLSubscription};
use std::net::SocketAddr;
use async_graphql::http::GraphiQLSource;
use crate::mutation::MutationRoot;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    HttpServer::new(|| {
        let schema = Schema::build(Query, MutationRoot, EmptySubscription).finish();
        App::new()
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

async fn index(
    // Schema now accessible here
    schema: web::Data<Schema<Query, EmptyMutation, EmptySubscription>>,
    request: GraphQLRequest,
) -> web::Json<GraphQLResponse> {
    web::Json(schema.execute(request.into_inner()).await.into())
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
