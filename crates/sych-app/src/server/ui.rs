//! The web UI's static build at `/ui`: files from `http.ui_dir`, and `index.html` for every other
//! page path so the client router answers it. Headers keep the page to its own origin.

use std::path::Path;

use axum::Router;
use axum::extract::Request;
use axum::http::{HeaderValue, header};
use axum::middleware::{self, Next};
use axum::response::{Redirect, Response};
use axum::routing::get;
use tower_http::services::{ServeDir, ServeFile};

/// Scripts, styles and fonts from this origin only; avatars from GitHub; never framed.
const CSP: &str = "default-src 'self'; img-src 'self' data: https://avatars.githubusercontent.com; \
    style-src 'self' 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'";

/// `/` and `/ui/…` over the build in `dir`.
pub fn routes<S: Clone + Send + Sync + 'static>(dir: &Path) -> Router<S> {
    let pages = ServeDir::new(dir).fallback(ServeFile::new(dir.join("index.html")));
    Router::new()
        .route("/", get(|| async { Redirect::temporary("/ui/") }))
        // Hashed assets never fall back to the page: a missing one is a 404, not HTML.
        .nest_service("/ui/assets", ServeDir::new(dir.join("assets")))
        .nest_service("/ui", pages)
        .layer(middleware::from_fn(headers))
}

async fn headers(request: Request, next: Next) -> Response {
    let hashed = request.uri().path().starts_with("/ui/assets/");
    let mut response = next.run(request).await;
    let cache = if hashed && response.status().is_success() {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let h = response.headers_mut();
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    response
}
