use crate::graphql::schema::User;
use crate::logic::user::GqlUser;
use async_graphql::Result as GqlResult;

pub mod mutation;
pub mod query;
pub mod schema;
pub mod subscription;
pub mod validator;


pub fn create_user(u: GqlUser) -> GqlResult<User> {
    Ok(User {
        id: u.id,
        username: u.username,
        first_name: u.first_name,
        last_name: u.last_name,
        email: u.email,
        is_admin: u.is_admin,
        created_at: u.created_at.to_rfc3339(),
        updated_at: u.updated_at.to_rfc3339(),
        last_login: u.last_login.map(|d| d.to_rfc3339()),
        is_temporary_password: u.is_temporary_password,
    })
}