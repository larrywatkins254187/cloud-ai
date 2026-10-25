use crate::crypto;
use crate::state::St;
use crate::store::{get_user, User};
use crate::util::{now, random_token, sha256_hex, ApiError, ApiResult};
use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const COOKIE: &str = "cai_session";
const PENDING_TTL_MIN: i64 = 10;
const SESSION_TTL_DAYS: i64 = 30;

/// Logged-in user that passed both the password and the TOTP step.
pub struct AuthUser(pub User);
pub struct AdminUser(pub User);

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|kv| {
        let (k, v) = kv.trim().split_once('=')?;
        (k == COOKIE).then(|| v.to_string())
    })
}

/// Look up a session by cookie; returns (user, stage).
pub fn session_user(st: &St, headers: &HeaderMap) -> Option<(User, String)> {
    let token = cookie_token(headers)?;
    let hash = sha256_hex(&token);
    let row: Option<(i64, String, String)> = st
        .db
        .conn()
        .query_row(
            "SELECT user_id, stage, expires_at FROM sessions WHERE token_hash=?1",
            [&hash],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .ok()
        .flatten();
    let (uid, stage, expires) = row?;
    if expires < now() {
        let _ = st.db.conn().execute("DELETE FROM sessions WHERE token_hash=?1", [&hash]);
        return None;
    }
    let user = get_user(&st.db, uid).ok()?;
    if user.disabled {
        return None;
    }
    Some((user, stage))
}

impl FromRequestParts<St> for AuthUser {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, st: &St) -> Result<Self, Self::Rejection> {
        match session_user(st, &parts.headers) {
            Some((u, stage)) if stage == "full" => Ok(AuthUser(u)),
            _ => Err(ApiError::unauthorized()),
        }
    }
}

impl FromRequestParts<St> for AdminUser {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, st: &St) -> Result<Self, Self::Rejection> {
        let AuthUser(u) = AuthUser::from_request_parts(parts, st).await?;
        if !u.is_admin() {
            return Err(ApiError::forbidden());
        }
        Ok(AdminUser(u))
    }
}

/// Simple in-memory brute-force protection: 5 failures per key → 5 minute lock.
#[derive(Default)]
pub struct LoginLimiter(Mutex<HashMap<String, (u32, Instant)>>);

impl LoginLimiter {
    fn check(&self, key: &str) -> bool {
        let m = self.0.lock().unwrap();
        match m.get(key) {
            Some((n, t)) if *n >= 5 && t.elapsed() < Duration::from_secs(300) => false,
            _ => true,
        }
    }
    fn fail(&self, key: &str) {
        let mut m = self.0.lock().unwrap();
        let e = m.entry(key.to_string()).or_insert((0, Instant::now()));
        if e.1.elapsed() > Duration::from_secs(300) {
            *e = (0, Instant::now());
        }
        e.0 += 1;
        e.1 = Instant::now();
    }
    fn ok(&self, key: &str) {
        self.0.lock().unwrap().remove(key);
    }
}

fn client_ip(headers: &HeaderMap) -> String {
    headers
        .get("x-real-ip")
        .or_else(|| headers.get("x-forwarded-for"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or("").trim().to_string())
        .unwrap_or_else(|| "local".into())
}

fn is_https(headers: &HeaderMap) -> bool {
    headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()) == Some("https")
}

fn set_cookie(token: &str, max_age_secs: i64, secure: bool) -> String {
    format!(
        "{COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age_secs}{}",
        if secure { "; Secure" } else { "" }
    )
}

fn create_session(st: &St, user_id: i64, stage: &str, ttl: chrono::Duration, ua: &str) -> anyhow::Result<String> {
    let token = random_token(32);
    let expires = (chrono::Utc::now() + ttl).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    st.db.conn().execute(
        "INSERT INTO sessions(token_hash,user_id,stage,created_at,expires_at,user_agent) VALUES(?1,?2,?3,?4,?5,?6)",
        params![sha256_hex(&token), user_id, stage, now(), expires, ua],
    )?;
    Ok(token)
}

#[derive(Deserialize)]
pub struct LoginBody {
    username: String,
    password: String,
}

pub async fn login(State(st): State<St>, headers: HeaderMap, Json(b): Json<LoginBody>) -> ApiResult<Response> {
    let key = format!("{}|{}", client_ip(&headers), b.username.to_lowercase());
    if !st.login_limiter.check(&key) {
        return Err(ApiError::new(StatusCode::TOO_MANY_REQUESTS, "Too many attempts, try again in 5 minutes"));
    }
    let row: Option<(i64, String, i64, i64)> = st
        .db
        .conn()
        .query_row(
            "SELECT id,password_hash,totp_enabled,disabled FROM users WHERE username=?1",
            [&b.username],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((uid, hash, totp_enabled, disabled)) = row else {
        st.login_limiter.fail(&key);
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "Wrong username or password"));
    };
    if disabled != 0 || !crypto::verify_password(&b.password, &hash) {
        st.login_limiter.fail(&key);
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "Wrong username or password"));
    }
    st.login_limiter.ok(&key);
    let ua = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("");
    let token = create_session(&st, uid, "totp", chrono::Duration::minutes(PENDING_TTL_MIN), ua)?;
    let mut body = json!({ "stage": "totp" });
    if totp_enabled == 0 {
        // First login: enrol an authenticator app (mandatory for everyone).
        let secret = crypto::new_totp_secret();
        st.db.conn().execute("UPDATE users SET totp_secret=?2 WHERE id=?1", params![uid, secret])?;
        let uri = crypto::totp_uri(&secret, &b.username, "cloud-ai");
        body = json!({ "stage": "totp_setup", "secret": secret, "otpauth_url": uri, "qr_svg": crypto::qr_svg(&uri) });
    }
    Ok((
        [(header::SET_COOKIE, set_cookie(&token, PENDING_TTL_MIN * 60, is_https(&headers)))],
        Json(body),
    )
        .into_response())
}

#[derive(Deserialize)]
pub struct TotpBody {
    code: String,
}

pub async fn verify_totp(State(st): State<St>, headers: HeaderMap, Json(b): Json<TotpBody>) -> ApiResult<Response> {
    let Some((user, stage)) = session_user(&st, &headers) else { return Err(ApiError::unauthorized()) };
    if stage != "totp" {
        return Err(ApiError::bad("not waiting for a code"));
    }
    let key = format!("totp|{}", user.id);
    if !st.login_limiter.check(&key) {
        return Err(ApiError::new(StatusCode::TOO_MANY_REQUESTS, "Too many attempts, try again in 5 minutes"));
    }
    let secret: Option<String> =
        st.db.conn().query_row("SELECT totp_secret FROM users WHERE id=?1", [user.id], |r| r.get(0))?;
    let Some(secret) = secret else { return Err(ApiError::bad("no 2FA secret, log in again")) };
    if !crypto::verify_totp(&secret, &b.code) {
        st.login_limiter.fail(&key);
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "Invalid code"));
    }
    st.login_limiter.ok(&key);
    // Replace the pending session with a full one.
    if let Some(old) = cookie_token(&headers) {
        st.db.conn().execute("DELETE FROM sessions WHERE token_hash=?1", [sha256_hex(&old)])?;
    }
    st.db.conn().execute("UPDATE users SET totp_enabled=1 WHERE id=?1", [user.id])?;
    let ua = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("");
    let token = create_session(&st, user.id, "full", chrono::Duration::days(SESSION_TTL_DAYS), ua)?;
    let user = get_user(&st.db, user.id)?;
    Ok((
        [(header::SET_COOKIE, set_cookie(&token, SESSION_TTL_DAYS * 86400, is_https(&headers)))],
        Json(json!({ "user": user })),
    )
        .into_response())
}

pub async fn logout(State(st): State<St>, headers: HeaderMap) -> ApiResult<Response> {
    if let Some(t) = cookie_token(&headers) {
        st.db.conn().execute("DELETE FROM sessions WHERE token_hash=?1", [sha256_hex(&t)])?;
    }
    Ok(([(header::SET_COOKIE, set_cookie("", 0, is_https(&headers)))], Json(json!({"ok": true}))).into_response())
}

pub async fn me(State(st): State<St>, headers: HeaderMap) -> ApiResult<Json<serde_json::Value>> {
    let setup_needed: i64 = st.db.conn().query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?;
    match session_user(&st, &headers) {
        Some((u, stage)) if stage == "full" => Ok(Json(json!({ "user": u, "version": env!("CARGO_PKG_VERSION") }))),
        Some((_, stage)) => Ok(Json(json!({ "user": null, "stage": stage }))),
        None => Ok(Json(json!({ "user": null, "setup_needed": setup_needed == 0 }))),
    }
}

#[derive(Deserialize)]
pub struct PasswordBody {
    old_password: String,
    new_password: String,
}

pub async fn change_password(State(st): State<St>, AuthUser(u): AuthUser, Json(b): Json<PasswordBody>) -> ApiResult<Json<serde_json::Value>> {
    let hash: String = st.db.conn().query_row("SELECT password_hash FROM users WHERE id=?1", [u.id], |r| r.get(0))?;
    if !crypto::verify_password(&b.old_password, &hash) {
        return Err(ApiError::bad("Current password is wrong"));
    }
    validate_password(&b.new_password)?;
    st.db.conn().execute("UPDATE users SET password_hash=?2 WHERE id=?1", params![u.id, crypto::hash_password(&b.new_password)?])?;
    Ok(Json(json!({"ok": true})))
}

pub fn validate_password(pw: &str) -> ApiResult<()> {
    if pw.chars().count() < 8 {
        return Err(ApiError::bad("Password must be at least 8 characters"));
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct SetupBody {
    token: String,
    username: String,
    password: String,
}

/// First-run: create the admin account. Requires the one-time token written to
/// `<data>/setup-token` so nobody else can claim a freshly installed panel.
pub async fn setup(State(st): State<St>, Json(b): Json<SetupBody>) -> ApiResult<Json<serde_json::Value>> {
    let count: i64 = st.db.conn().query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?;
    if count > 0 {
        return Err(ApiError::bad("already set up"));
    }
    let expected = std::fs::read_to_string(st.cfg.setup_token_path()).unwrap_or_default();
    if expected.trim().is_empty() || expected.trim() != b.token.trim() {
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "Wrong setup token"));
    }
    if b.username.trim().is_empty() {
        return Err(ApiError::bad("username required"));
    }
    validate_password(&b.password)?;
    st.db.conn().execute(
        "INSERT INTO users(username,password_hash,role,created_at) VALUES(?1,?2,'admin',?3)",
        params![b.username.trim(), crypto::hash_password(&b.password)?, now()],
    )?;
    let _ = std::fs::remove_file(st.cfg.setup_token_path());
    Ok(Json(json!({"ok": true})))
}

/// Make sure a setup token exists while there are no users; print it to the log.
pub fn ensure_setup_token(st: &St) {
    let count: i64 = st.db.conn().query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0)).unwrap_or(1);
    if count > 0 {
        return;
    }
    let path = st.cfg.setup_token_path();
    let token = match std::fs::read_to_string(&path) {
        Ok(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => {
            let t = random_token(12);
            let _ = std::fs::write(&path, &t);
            crypto::set_mode_600(&path);
            t
        }
    };
    tracing::warn!("No users yet. Open the panel and create the admin with setup token: {token}");
}
