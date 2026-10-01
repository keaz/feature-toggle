pub mod access_log;
pub mod admin_guard;
pub mod jwt_guard;

/// The request path exactly as actix's router will match it.
///
/// The router percent-decodes most escapes before routing (`/system%2Dclients/x`
/// reaches the `/system-clients/{id}` handler), so every authorization, scope and
/// public-path decision must use this and never the raw `req.path()`.
pub(crate) fn routed_path(req: &actix_web::dev::ServiceRequest) -> String {
    req.match_info().as_str().to_string()
}
