use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use rand::RngCore;
use sha2::{Digest, Sha256};

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

pub fn random_token(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

/// Cut a string to at most `max` bytes on a char boundary, appending a marker.
pub fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… [{} more bytes]", &s[..end], s.len() - end)
}

/// Keep only the last `max` bytes of a string (for log tails).
pub fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &s[start..])
}

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, msg: impl Into<String>) -> Self {
        ApiError { status, message: msg.into() }
    }
    pub fn bad(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, msg)
    }
    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not found")
    }
    pub fn forbidden() -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden")
    }
    pub fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(serde_json::json!({ "error": self.message }))).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        tracing::warn!("api error: {e:#}");
        ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"))
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        match e {
            rusqlite::Error::QueryReturnedNoRows => ApiError::not_found(),
            e => {
                tracing::warn!("db error: {e}");
                ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        }
    }
}

impl From<std::io::Error> for ApiError {
    fn from(e: std::io::Error) -> Self {
        ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

/// Read `MemAvailable`/`MemTotal`/swap from /proc/meminfo, in MiB.
pub fn meminfo() -> MemInfo {
    let mut m = MemInfo::default();
    if let Ok(s) = std::fs::read_to_string("/proc/meminfo") {
        for line in s.lines() {
            let mut parts = line.split_whitespace();
            let key = parts.next().unwrap_or("");
            let kb: u64 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            let mb = kb / 1024;
            match key {
                "MemTotal:" => m.total_mb = mb,
                "MemAvailable:" => m.available_mb = mb,
                "SwapTotal:" => m.swap_total_mb = mb,
                "SwapFree:" => m.swap_free_mb = mb,
                _ => {}
            }
        }
    }
    m
}

#[derive(Default, Clone, Copy, serde::Serialize)]
pub struct MemInfo {
    pub total_mb: u64,
    pub available_mb: u64,
    pub swap_total_mb: u64,
    pub swap_free_mb: u64,
}

pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let t = out.trim_matches('-').to_string();
    if t.is_empty() { "x".into() } else { t }
}
