use actix_web::{get, patch, post, web, HttpMessage, HttpRequest, HttpResponse, Responder};
use crate::model::ID;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::database::activity_log::ActivityLogRepository;
use crate::database::role::role_repository_tx;
use crate::database::user::user_repository_tx;
use crate::logic::role::RoleLogic;
use crate::logic::user::{RegisterUserInput, UpdateGqlUserInput, UserLogic};
use crate::logic::ActorContext;
use crate::middleware::admin_guard::AdminState;
use crate::rest::error::RestError;
use crate::rest::pagination::{normalize_pagination, PageMeta, PaginationQuery};
use crate::rest::role::RoleResponse;
use crate::rest::team::TeamResponse;
use crate::JwtUser;

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserListQuery {
    pub team_id: Option<String>,
    pub name: Option<String>,
    pub offset: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserResponse {
    pub id: String,
    pub username: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub is_admin: bool,
    pub created_at: String,
    pub updated_at: String,
    pub last_login: Option<String>,
    pub is_temporary_password: bool,
    pub team_ids: Option<Vec<String>>,
    pub teams: Option<Vec<TeamResponse>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UsersResponse {
    pub items: Vec<UserResponse>,
    pub meta: PageMeta,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub is_admin: Option<bool>,
    pub is_temporary_password: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateUserRequest {
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub email: Option<String>,
    pub is_admin: Option<bool>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AssignUserTeamsRequest {
    pub team_ids: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AssignUserRolesRequest {
    pub role_ids: Vec<String>,
}

impl UserResponse {
    fn from_gql(user: crate::logic::user::GqlUser) -> Self {
        Self {
            id: user.id.to_string(),
            username: user.username,
            first_name: user.first_name,
            last_name: user.last_name,
            email: user.email,
            is_admin: user.is_admin,
            created_at: user.created_at.to_rfc3339(),
            updated_at: user.updated_at.to_rfc3339(),
            last_login: user.last_login.map(|value| value.to_rfc3339()),
            is_temporary_password: user.is_temporary_password,
            team_ids: None,
            teams: None,
        }
    }
}

impl From<crate::logic::user::GqlUser> for UserResponse {
    fn from(user: crate::logic::user::GqlUser) -> Self {
        UserResponse::from_gql(user)
    }
}

fn parse_uuid(value: &str, field: &str) -> Result<Uuid, RestError> {
    Uuid::parse_str(value).map_err(|_| RestError::invalid_input(format!("invalid {field}")))
}

fn actor_from_request(req: &HttpRequest) -> Option<ActorContext> {
    req.extensions()
        .get::<JwtUser>()
        .map(|jwt| ActorContext::new(jwt.id, jwt.username.clone()))
}

fn jwt_user(req: &HttpRequest) -> Result<JwtUser, RestError> {
    req.extensions()
        .get::<JwtUser>()
        .cloned()
        .ok_or_else(|| RestError::unauthorized("User authentication not found"))
}

fn validate_email(value: &str) -> Result<(), RestError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(RestError::invalid_input("invalid email"));
    }

    let mut parts = trimmed.split('@');
    let local = parts.next().unwrap_or("");
    let domain = parts.next().unwrap_or("");

    if local.is_empty() || domain.is_empty() || parts.next().is_some() {
        return Err(RestError::invalid_input("invalid email"));
    }

    if !domain.contains('.') || domain.starts_with('.') || domain.ends_with('.') {
        return Err(RestError::invalid_input("invalid email"));
    }

    Ok(())
}

#[utoipa::path(
    get,
    path = "/api/v1/users",
    params(
        ("teamId" = Option<String>, Query, description = "Filter by team"),
        ("name" = Option<String>, Query, description = "Filter by name or username"),
        ("offset" = Option<i64>, Query, description = "Pagination offset"),
        ("limit" = Option<i64>, Query, description = "Pagination limit")
    ),
    responses(
        (status = 200, description = "Users list", body = UsersResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Users"
)]
#[get("/users")]
pub(crate) async fn list_users(
    logic: web::Data<Box<dyn UserLogic>>,
    query: web::Query<UserListQuery>,
) -> Result<impl Responder, RestError> {
    let (offset, limit) = normalize_pagination(&PaginationQuery {
        offset: query.offset,
        limit: query.limit,
    });
    let page_number = ((offset / limit) + 1) as i32;
    let page_size = limit as i32;

    let team_id = match query.team_id.as_deref() {
        Some(value) => Some(ID::from(parse_uuid(value, "team_id")?)),
        None => None,
    };

    let name = query
        .name
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());

    let (users, total) = logic
        .search_users(team_id, name, page_number, page_size)
        .await
        .map_err(RestError::from)?;

    let items = users
        .into_iter()
        .map(UserResponse::from)
        .collect::<Vec<_>>();

    Ok(HttpResponse::Ok().json(UsersResponse {
        items,
        meta: PageMeta {
            offset,
            limit,
            total,
        },
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/users/{id}",
    params(
        ("id" = String, Path, description = "User ID")
    ),
    responses(
        (status = 200, description = "User", body = UserResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Users"
)]
#[get("/users/{id}")]
pub(crate) async fn get_user(
    logic: web::Data<Box<dyn UserLogic>>,
    user_id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let user_uuid = parse_uuid(&user_id, "user_id")?;
    let gql_user = logic
        .get_user_by_id(ID::from(user_uuid))
        .await
        .map_err(RestError::from)?;

    let teams = logic
        .get_user_teams(ID::from(user_uuid))
        .await
        .map_err(RestError::from)?;

    let team_ids = teams.iter().map(|team| team.id.to_string()).collect();
    let team_responses = teams.into_iter().map(TeamResponse::from).collect();

    let mut response = UserResponse::from(gql_user);
    response.team_ids = Some(team_ids);
    response.teams = Some(team_responses);

    Ok(HttpResponse::Ok().json(response))
}

#[utoipa::path(
    post,
    path = "/api/v1/users",
    request_body = CreateUserRequest,
    responses(
        (status = 201, description = "User created", body = UserResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 409, description = "Conflict", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Users"
)]
#[post("/users")]
pub(crate) async fn create_user(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    payload: web::Json<CreateUserRequest>,
) -> Result<impl Responder, RestError> {
    validate_email(&payload.email)?;

    let actor = actor_from_request(&req);
    let input = RegisterUserInput {
        username: payload.username.trim().to_string(),
        password: payload.password.clone(),
        first_name: payload.first_name.trim().to_string(),
        last_name: payload.last_name.trim().to_string(),
        email: payload.email.trim().to_string(),
        is_admin: payload.is_admin.unwrap_or(false),
        is_temporary_password: payload.is_temporary_password.unwrap_or(true),
    };

    let repo = user_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;

    let result = crate::logic::user_tx::register_user_in_tx(
        &mut tx,
        &repo,
        activity_repo.as_ref().as_ref(),
        input,
        actor,
    )
    .await;

    match result {
        Ok(created) => {
            tx.commit().await.map_err(|e| {
                RestError::internal(format!("Failed to commit transaction: {e}"))
            })?;
            Ok(HttpResponse::Created().json(UserResponse::from(created)))
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/admins",
    request_body = CreateUserRequest,
    responses(
        (status = 201, description = "Admin created", body = UserResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 409, description = "Conflict", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Users"
)]
#[post("/admins")]
pub(crate) async fn create_admin(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    admin_state: web::Data<AdminState>,
    req: HttpRequest,
    payload: web::Json<CreateUserRequest>,
) -> Result<impl Responder, RestError> {
    validate_email(&payload.email)?;

    let actor = actor_from_request(&req);
    let input = RegisterUserInput {
        username: payload.username.trim().to_string(),
        password: payload.password.clone(),
        first_name: payload.first_name.trim().to_string(),
        last_name: payload.last_name.trim().to_string(),
        email: payload.email.trim().to_string(),
        is_admin: true,
        is_temporary_password: false,
    };

    let repo = user_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;

    let result = crate::logic::user_tx::register_user_in_tx(
        &mut tx,
        &repo,
        activity_repo.as_ref().as_ref(),
        input,
        actor,
    )
    .await;

    match result {
        Ok(created) => {
            tx.commit().await.map_err(|e| {
                RestError::internal(format!("Failed to commit transaction: {e}"))
            })?;

            if created.is_admin {
                admin_state.set_exists(true);
            }

            Ok(HttpResponse::Created().json(UserResponse::from(created)))
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

#[utoipa::path(
    patch,
    path = "/api/v1/users/{id}",
    request_body = UpdateUserRequest,
    params(
        ("id" = String, Path, description = "User ID")
    ),
    responses(
        (status = 200, description = "User updated", body = UserResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Users"
)]
#[patch("/users/{id}")]
pub(crate) async fn update_user(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    user_id: web::Path<String>,
    payload: web::Json<UpdateUserRequest>,
) -> Result<impl Responder, RestError> {
    let user_uuid = parse_uuid(&user_id, "user_id")?;

    if let Some(email) = payload.email.as_deref() {
        validate_email(email)?;
    }

    let actor = actor_from_request(&req);
    let input = UpdateGqlUserInput {
        first_name: payload.first_name.as_ref().map(|value| value.trim().to_string()),
        last_name: payload.last_name.as_ref().map(|value| value.trim().to_string()),
        email: payload.email.as_ref().map(|value| value.trim().to_string()),
        is_admin: payload.is_admin,
        enabled: payload.enabled,
    };

    let repo = user_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;

    let result = crate::logic::user_tx::update_user_in_tx(
        &mut tx,
        &repo,
        activity_repo.as_ref().as_ref(),
        ID::from(user_uuid),
        input,
        actor,
    )
    .await;

    match result {
        Ok(updated) => {
            tx.commit().await.map_err(|e| {
                RestError::internal(format!("Failed to commit transaction: {e}"))
            })?;
            Ok(HttpResponse::Ok().json(UserResponse::from(updated)))
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/users/{id}/teams",
    request_body = AssignUserTeamsRequest,
    params(
        ("id" = String, Path, description = "User ID")
    ),
    responses(
        (status = 200, description = "Teams assigned", body = [TeamResponse]),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Users"
)]
#[post("/users/{id}/teams")]
pub(crate) async fn assign_user_teams(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    logic: web::Data<Box<dyn UserLogic>>,
    req: HttpRequest,
    user_id: web::Path<String>,
    payload: web::Json<AssignUserTeamsRequest>,
) -> Result<impl Responder, RestError> {
    let user_uuid = parse_uuid(&user_id, "user_id")?;
    let team_ids = payload
        .team_ids
        .iter()
        .map(|value| parse_uuid(value, "team_id"))
        .collect::<Result<Vec<_>, _>>()?;

    let actor = actor_from_request(&req);
    let repo = user_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;

    let result = crate::logic::user_tx::assign_user_teams_in_tx(
        &mut tx,
        &repo,
        activity_repo.as_ref().as_ref(),
        ID::from(user_uuid),
        team_ids.into_iter().map(ID::from).collect(),
        actor,
    )
    .await;

    match result {
        Ok(_) => {
            tx.commit().await.map_err(|e| {
                RestError::internal(format!("Failed to commit transaction: {e}"))
            })?;
        }
        Err(err) => {
            let _ = tx.rollback().await;
            return Err(RestError::from(err));
        }
    }

    let teams = logic
        .get_user_teams(ID::from(user_uuid))
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(
        teams.into_iter().map(TeamResponse::from).collect::<Vec<_>>(),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/users/{id}/roles",
    params(
        ("id" = String, Path, description = "User ID")
    ),
    responses(
        (status = 200, description = "User roles", body = [RoleResponse]),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Users"
)]
#[get("/users/{id}/roles")]
pub(crate) async fn get_user_roles(
    role_logic: web::Data<Box<dyn RoleLogic>>,
    user_id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let user_uuid = parse_uuid(&user_id, "user_id")?;
    let roles = role_logic
        .get_user_roles(ID::from(user_uuid))
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(
        roles.into_iter().map(RoleResponse::from).collect::<Vec<_>>(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/users/{id}/roles",
    request_body = AssignUserRolesRequest,
    params(
        ("id" = String, Path, description = "User ID")
    ),
    responses(
        (status = 200, description = "Roles assigned", body = [RoleResponse]),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Users"
)]
#[post("/users/{id}/roles")]
pub(crate) async fn assign_user_roles(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    user_id: web::Path<String>,
    payload: web::Json<AssignUserRolesRequest>,
) -> Result<impl Responder, RestError> {
    let _ = jwt_user(&req)?;
    let user_uuid = parse_uuid(&user_id, "user_id")?;
    let role_ids = payload
        .role_ids
        .iter()
        .map(|value| parse_uuid(value, "role_id"))
        .collect::<Result<Vec<_>, _>>()?;

    let actor = actor_from_request(&req);
    let repo = role_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;

    let result = crate::logic::role_tx::assign_user_roles_in_tx(
        &mut tx,
        &repo,
        activity_repo.as_ref().as_ref(),
        ID::from(user_uuid),
        role_ids.into_iter().map(ID::from).collect(),
        actor,
    )
    .await;

    match result {
        Ok(roles) => {
            tx.commit().await.map_err(|e| {
                RestError::internal(format!("Failed to commit transaction: {e}"))
            })?;
            Ok(HttpResponse::Ok().json(
                roles.into_iter().map(RoleResponse::from).collect::<Vec<_>>(),
            ))
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(list_users)
        .service(get_user)
        .service(create_user)
        .service(create_admin)
        .service(update_user)
        .service(assign_user_teams)
        .service(get_user_roles)
        .service(assign_user_roles);
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{App, http::StatusCode, test, web};
    use crate::database::activity_log::PgActivityLogRepository;
    use crate::logic::role::MockRoleLogic;
    use crate::logic::user::MockUserLogic;
    use sqlx::postgres::PgPoolOptions;

    fn sample_user(user_id: Uuid) -> crate::logic::user::GqlUser {
        crate::logic::user::GqlUser {
            id: ID::from(user_id),
            username: "jdoe".to_string(),
            first_name: "Jane".to_string(),
            last_name: "Doe".to_string(),
            email: "jane@example.com".to_string(),
            is_admin: false,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_login: None,
            is_temporary_password: false,
        }
    }

    #[actix_web::test]
    async fn list_users_returns_items_and_meta() {
        let user_id = Uuid::new_v4();
        let user = sample_user(user_id);
        let user_clone = user.clone();

        let mut mock_user_logic = MockUserLogic::new();
        mock_user_logic
            .expect_search_users()
            .times(1)
            .returning(move |_, _, _, _| Ok((vec![user_clone.clone()], 1)));

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(
                    Box::new(mock_user_logic) as Box<dyn UserLogic>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let req = test::TestRequest::get()
            .uri("/api/v1/users?offset=0&limit=10")
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["items"][0]["id"], user_id.to_string());
        assert_eq!(json["meta"]["total"], 1);
    }

    #[actix_web::test]
    async fn create_user_invalid_email_returns_bad_request() {
        let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL not set");
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&db_url)
            .await
            .expect("Failed to connect to database");
        let activity_repo = PgActivityLogRepository::new(pool.clone());

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool))
                .app_data(web::Data::new(
                    Box::new(activity_repo) as Box<dyn ActivityLogRepository>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/api/v1/users")
            .set_json(CreateUserRequest {
                username: "jdoe".to_string(),
                password: "secret".to_string(),
                first_name: "Jane".to_string(),
                last_name: "Doe".to_string(),
                email: "not-an-email".to_string(),
                is_admin: None,
                is_temporary_password: None,
            })
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[actix_web::test]
    async fn get_user_roles_returns_items() {
        let user_id = Uuid::new_v4();
        let role = crate::logic::role::GqlRole {
            id: ID::from(Uuid::new_v4()),
            name: "Approver".to_string(),
            description: "Role".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        };

        let mut mock_role_logic = MockRoleLogic::new();
        mock_role_logic
            .expect_get_user_roles()
            .times(1)
            .returning(move |_| Ok(vec![role.clone()]));

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(
                    Box::new(mock_role_logic) as Box<dyn RoleLogic>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/users/{user_id}/roles");
        let req = test::TestRequest::get().uri(&uri).to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::OK);
    }
}
