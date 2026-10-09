//! The admin UI, served from the `ui_dir` folder on the backend's HTTP port.
//! One process and one origin carry both the UI and `/api/v1`, so the UI needs
//! no CORS and no backend address. Off unless `ui_dir` is set.
//!
//! `/config.js` points the UI at the origin it was loaded from. Other paths are
//! files of the folder; a path that is no file and looks like a UI route (no
//! extension, not under `/api/`) gets `index.html`, so client-side routes such
//! as `/settings/sso` work on reload.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use actix_files::{Files, NamedFile};
use actix_web::dev::{ServiceRequest, ServiceResponse, fn_service};
use actix_web::http::Method;
use actix_web::http::header::{CACHE_CONTROL, HeaderValue};
use actix_web::{HttpRequest, HttpResponse, web};

/// The UI's runtime settings. The Docker UI image writes the same file with a
/// fixed backend address; here the backend is the origin of the page.
const CONFIG_JS: &str = r#"window.ENV = {
    REST_HTTP_URL: window.location.origin + "/api/v1",
    REST_WS_URL: (window.location.protocol === "https:" ? "wss://" : "ws://") + window.location.host + "/api/v1/ws"
};
"#;

/// The folder the UI is served from. It is in the app data only when the
/// backend serves the UI; the guards use that to let UI requests through.
#[derive(Clone, Debug)]
pub struct UiDir(Arc<PathBuf>);

impl UiDir {
    /// Checks that `dir` holds the built UI (an `index.html`).
    pub fn open(dir: impl AsRef<Path>) -> io::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        if !dir.join("index.html").is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("ui_dir {} has no index.html", dir.display()),
            ));
        }
        Ok(Self(Arc::new(dir)))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// A UI folder in the temp directory, for tests of code that checks
    /// whether the UI is served.
    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        let dir = std::env::temp_dir().join(format!("fluxgate-ui-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), "<html></html>").unwrap();
        Self::open(dir).unwrap()
    }

    fn index(&self) -> PathBuf {
        self.0.join("index.html")
    }
}

/// Whether a request is for the UI rather than the API: a `GET` or `HEAD`
/// outside `/api/`. The guards treat these as public when the UI is served;
/// they reach only static files and `index.html`.
pub fn is_ui_request(path: &str, method: &Method) -> bool {
    (method == Method::GET || method == Method::HEAD)
        && path != "/api"
        && !path.starts_with("/api/")
}

/// Whether a path that matched no file should get `index.html`: a UI request
/// whose last segment has no extension. A missing `/assets/x.js` stays a 404.
fn serves_index(path: &str, method: &Method) -> bool {
    let last = path.rsplit('/').next().unwrap_or_default();
    is_ui_request(path, method) && !last.contains('.')
}

/// Registers the UI routes. Call it after every API route: the file service
/// is mounted at `/` and answers every path the API did not match.
pub fn configure(cfg: &mut web::ServiceConfig, ui: &UiDir) {
    let fallback = ui.clone();
    cfg.app_data(web::Data::new(ui.clone()))
        .route("/config.js", web::get().to(config_js))
        .route("/", web::get().to(index))
        .service(Files::new("/", ui.path()).default_handler(fn_service(
            move |req: ServiceRequest| {
                let ui = fallback.clone();
                async move { not_a_file(ui, req).await }
            },
        )));
}

async fn config_js() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("application/javascript; charset=utf-8")
        .insert_header((CACHE_CONTROL, "no-store"))
        .body(CONFIG_JS)
}

async fn index(req: HttpRequest, ui: web::Data<UiDir>) -> actix_web::Result<HttpResponse> {
    index_response(&req, &ui).await
}

/// `index.html`, revalidated on every load so an upgrade reaches browsers at
/// once. The assets it loads have hashed names and can be cached.
async fn index_response(req: &HttpRequest, ui: &UiDir) -> actix_web::Result<HttpResponse> {
    let mut res = NamedFile::open(ui.index())?.into_response(req);
    res.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    Ok(res)
}

async fn not_a_file(ui: UiDir, req: ServiceRequest) -> actix_web::Result<ServiceResponse> {
    let (req, _) = req.into_parts();
    let res = if serves_index(req.path(), req.method()) {
        index_response(&req, &ui).await?
    } else {
        HttpResponse::NotFound().finish()
    };
    Ok(ServiceResponse::new(req, res))
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::App;
    use actix_web::body::to_bytes;
    use actix_web::http::StatusCode;
    use actix_web::test as actix_test;

    /// A folder with an `index.html` and one asset, removed on drop.
    struct UiFixture(PathBuf);

    impl UiFixture {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("fluxgate-ui-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(dir.join("assets")).unwrap();
            std::fs::write(dir.join("index.html"), "<html>index</html>").unwrap();
            std::fs::write(dir.join("assets/app-1a2b.js"), "console.log('app')").unwrap();
            Self(dir)
        }
    }

    impl Drop for UiFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    async fn get(path: &str) -> (StatusCode, Option<String>, String) {
        let fixture = UiFixture::new();
        let ui = UiDir::open(&fixture.0).unwrap();
        let app = actix_test::init_service(
            App::new()
                .service(web::scope("/api/v1").route(
                    "/health",
                    web::get().to(|| async { HttpResponse::Ok().body("ok") }),
                ))
                .configure(|cfg| configure(cfg, &ui)),
        )
        .await;
        let res =
            actix_test::call_service(&app, actix_test::TestRequest::get().uri(path).to_request())
                .await;
        let status = res.status();
        let cache = res
            .headers()
            .get(CACHE_CONTROL)
            .map(|value| value.to_str().unwrap().to_string());
        let body = to_bytes(res.into_body()).await.unwrap();
        (status, cache, String::from_utf8(body.to_vec()).unwrap())
    }

    #[actix_web::test]
    async fn root_and_ui_routes_get_index_html_revalidated() {
        for path in ["/", "/login", "/settings/sso", "/features/abc-123"] {
            let (status, cache, body) = get(path).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            assert_eq!(body, "<html>index</html>", "{path}");
            assert_eq!(cache.as_deref(), Some("no-cache"), "{path}");
        }
    }

    #[actix_web::test]
    async fn assets_are_served_and_missing_assets_are_not_found() {
        let (status, _, body) = get("/assets/app-1a2b.js").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "console.log('app')");
        let (status, _, _) = get("/assets/missing.js").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[actix_web::test]
    async fn config_js_points_the_ui_at_its_own_origin() {
        let (status, cache, body) = get("/config.js").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(cache.as_deref(), Some("no-store"));
        assert!(body.contains("window.location.origin + \"/api/v1\""));
        assert!(body.contains("/api/v1/ws"));
    }

    #[actix_web::test]
    async fn api_paths_are_never_answered_with_the_ui() {
        let (status, _, body) = get("/api/v1/health").await;
        assert_eq!((status, body.as_str()), (StatusCode::OK, "ok"));
        for path in ["/api/v1/nope", "/api/v2/teams", "/api"] {
            let (status, _, body) = get(path).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
            assert!(!body.contains("index"), "{path}");
        }
    }

    #[test]
    fn open_requires_an_index_html() {
        let fixture = UiFixture::new();
        assert!(UiDir::open(&fixture.0).is_ok());
        assert!(UiDir::open(fixture.0.join("assets")).is_err());
    }

    #[test]
    fn ui_requests_are_reads_outside_the_api() {
        assert!(is_ui_request("/", &Method::GET));
        assert!(is_ui_request("/settings/sso", &Method::HEAD));
        assert!(is_ui_request("/apis", &Method::GET));
        assert!(!is_ui_request("/api", &Method::GET));
        assert!(!is_ui_request("/api/v1/teams", &Method::GET));
        assert!(!is_ui_request("/login", &Method::POST));
    }
}
