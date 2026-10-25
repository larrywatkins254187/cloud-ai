//! The React app, embedded into the binary at build time.
use axum::body::Body;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "web/dist"]
struct Assets;

pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    if let Some(f) = Assets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        let cache = if path.starts_with("assets/") {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };
        return Response::builder()
            .header(header::CONTENT_TYPE, mime.as_ref())
            .header(header::CACHE_CONTROL, cache)
            .body(Body::from(f.data.into_owned()))
            .unwrap();
    }
    if path.starts_with("assets/") || path.contains('.') && !path.ends_with(".html") {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    // SPA route: serve index.html.
    match Assets::get("index.html") {
        Some(f) => Response::builder()
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::from(f.data.into_owned()))
            .unwrap(),
        None => (StatusCode::NOT_FOUND, "frontend not built").into_response(),
    }
}
