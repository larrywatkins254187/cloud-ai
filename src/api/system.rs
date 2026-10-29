use crate::auth::{AdminUser, AuthUser};
use crate::settings;
use crate::state::St;
use crate::store::get_cli_info;
use crate::util::{ApiError, ApiResult};
use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

pub async fn get_settings(State(st): State<St>, AdminUser(_): AdminUser) -> ApiResult<Json<Value>> {
    Ok(Json(settings::all_for_ui(&st.db)))
}

pub async fn put_settings(State(st): State<St>, AdminUser(_): AdminUser, Json(b): Json<Value>) -> ApiResult<Json<Value>> {
    let old_tz = settings::get(&st.db, "timezone");
    settings::update_from_ui(&st.db, &b).map_err(|e| ApiError::bad(e.to_string()))?;
    if settings::get(&st.db, "timezone") != old_tz {
        crate::scheduler::recompute_all(&st);
    }
    st.runner.wake.notify_one();
    Ok(Json(settings::all_for_ui(&st.db)))
}

pub async fn status(State(st): State<St>, AuthUser(u): AuthUser) -> ApiResult<Json<Value>> {
    let mut v = json!({
        "system": crate::sysmon::snapshot(&st),
        "counts": crate::runner::active_counts(&st),
        "limits": {
            "max_claude": settings::get_i64(&st.db, "max_claude"),
            "max_codex": settings::get_i64(&st.db, "max_codex"),
            "ram_guard_mb": settings::get_i64(&st.db, "ram_guard_mb"),
        },
        "rate_limits": crate::runner::rate_limits(&st),
        "usage_errors": crate::plan_usage::errors(),
        "version": env!("CARGO_PKG_VERSION"),
        "timezone": settings::get(&st.db, "timezone"),
    });
    if u.is_admin() {
        v["cli"] = json!({
            "claude_version": get_cli_info(&st.db, "claude_version"),
            "claude_account": get_cli_info(&st.db, "claude_account"),
            "codex_version": get_cli_info(&st.db, "codex_version"),
            "codex_account": get_cli_info(&st.db, "codex_account"),
            "logins": logins(),
        });
        v["telegram_configured"] = json!(!settings::get(&st.db, "telegram_token").is_empty());
    }
    Ok(Json(v))
}

/// Fetch fresh plan usage from both CLIs now (each at most every 30 seconds,
/// right away after an account change).
pub async fn refresh_usage(State(st): State<St>, AuthUser(_): AuthUser) -> ApiResult<Json<Value>> {
    let changed = crate::plan_usage::check_accounts(&st).await;
    let (a, b) = tokio::join!(
        crate::plan_usage::refresh(&st, "claude", changed.contains(&"claude")),
        crate::plan_usage::refresh(&st, "codex", changed.contains(&"codex"))
    );
    let errors = crate::plan_usage::errors();
    let error = [a.err(), b.err()].into_iter().flatten().map(|e| format!("{e:#}")).collect::<Vec<_>>().join("; ");
    Ok(Json(json!({
        "rate_limits": crate::runner::rate_limits(&st),
        "usage_errors": errors,
        "logins": logins(),
        "error": if error.is_empty() { Value::Null } else { json!(error) },
    })))
}

/// Who each CLI is logged in as, read from the CLIs' own login files.
fn logins() -> Value {
    json!({"claude": crate::plan_usage::claude_login().json(), "codex": crate::plan_usage::codex_login().json()})
}

pub async fn events(State(st): State<St>, AdminUser(_): AdminUser) -> ApiResult<Json<Value>> {
    let c = st.db.conn();
    let mut q = c.prepare("SELECT id, level, message, created_at FROM events ORDER BY id DESC LIMIT 200")?;
    let rows: Vec<Value> = q
        .query_map([], |r| {
            Ok(json!({"id": r.get::<_, i64>(0)?, "level": r.get::<_, String>(1)?, "message": r.get::<_, String>(2)?, "created_at": r.get::<_, String>(3)?}))
        })?
        .collect::<Result<_, _>>()?;
    Ok(Json(Value::Array(rows)))
}

pub async fn models(State(st): State<St>, AuthUser(_): AuthUser) -> ApiResult<Json<Value>> {
    Ok(Json(crate::catalog::list(&st)))
}

pub async fn refresh_models(State(st): State<St>, AuthUser(_): AuthUser) -> ApiResult<Json<Value>> {
    let report = crate::catalog::refresh(&st).await;
    let mut out = crate::catalog::list(&st);
    out["report"] = report;
    Ok(Json(out))
}

pub async fn telegram_test(State(st): State<St>, AdminUser(_): AdminUser) -> ApiResult<Json<Value>> {
    match crate::telegram::test(&st).await {
        Ok(me) => Ok(Json(json!({"ok": true, "bot": me}))),
        Err(e) => Ok(Json(json!({"ok": false, "error": format!("{e:#}")}))),
    }
}

/// The price table's update status (admins).
pub async fn prices_status(State(st): State<St>, AdminUser(_): AdminUser) -> ApiResult<Json<Value>> {
    Ok(Json(crate::prices::status(&st)))
}

/// Download the latest prices now (admins). Manual rows are kept.
pub async fn prices_update(State(st): State<St>, AdminUser(_): AdminUser) -> ApiResult<Json<Value>> {
    crate::prices::update(&st).await.map_err(|e| ApiError::bad(format!("{e:#}")))?;
    let mut v = crate::prices::status(&st);
    v["usage_prices"] = json!(settings::get(&st.db, "usage_prices"));
    Ok(Json(v))
}
