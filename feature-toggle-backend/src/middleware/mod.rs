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

/// Whether `path` (the routed path) is one of the public SSO login routes:
/// `GET /api/v1/auth/sso/providers`, `GET /api/v1/auth/sso/{slug}/authorize`,
/// `GET /api/v1/auth/sso/{slug}/callback` and `POST /api/v1/auth/sso/exchange`.
/// Segments are matched exactly and `{slug}` must be a valid provider slug, so no
/// other path under `/api/v1/auth/sso/` becomes public.
pub(crate) fn is_public_sso_path(path: &str, method: &actix_web::http::Method) -> bool {
    use actix_web::http::Method;
    let Some(rest) = path.strip_prefix("/api/v1/auth/sso/") else {
        return false;
    };
    let segments: Vec<&str> = rest.split('/').collect();
    match segments.as_slice() {
        ["providers"] => method == Method::GET,
        ["exchange"] => method == Method::POST,
        [slug, "authorize" | "callback"] => {
            method == Method::GET && crate::logic::sso_provider::validate_slug(slug).is_ok()
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::is_public_sso_path;
    use actix_web::http::Method;

    #[test]
    fn sso_public_paths_match_exact_segments() {
        for (method, path) in [
            (Method::GET, "/api/v1/auth/sso/providers"),
            (Method::GET, "/api/v1/auth/sso/okta/authorize"),
            (Method::GET, "/api/v1/auth/sso/my-idp-2/callback"),
            (Method::POST, "/api/v1/auth/sso/exchange"),
        ] {
            assert!(is_public_sso_path(path, &method), "{method} {path}");
        }
        for (method, path) in [
            (Method::POST, "/api/v1/auth/sso/providers"),
            (Method::GET, "/api/v1/auth/sso/exchange"),
            (Method::POST, "/api/v1/auth/sso/okta/callback"),
            (Method::GET, "/api/v1/auth/sso/okta/authorize/x"),
            (Method::GET, "/api/v1/auth/sso/okta/other"),
            (Method::GET, "/api/v1/auth/sso//authorize"),
            (Method::GET, "/api/v1/auth/sso/Okta/authorize"),
            (Method::GET, "/api/v1/auth/sso/a/b/authorize"),
            (Method::GET, "/api/v1/auth/sso/providers/x"),
            (Method::GET, "/api/v1/sso/providers"),
            (Method::GET, "/api/v1/auth/sso/okta%2Fx/authorize"),
            (Method::GET, "/api/v1/auth/ssox/okta/authorize"),
        ] {
            assert!(!is_public_sso_path(path, &method), "{method} {path}");
        }
    }
}
