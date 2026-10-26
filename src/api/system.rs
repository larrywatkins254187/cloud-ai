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
        "version": env!("CARGO_PKG_VERSION"),
        "timezone": settings::get(&st.db, "timezone"),
    });
    if u.is_admin() {
        v["cli"] = json!({
            "claude_version": get_cli_info(&st.db, "claude_version"),
            "claude_account": get_cli_info(&st.db, "claude_account"),
            "codex_version": get_cli_info(&st.db, "codex_version"),
            "codex_account": get_cli_info(&st.db, "codex_account"),
        });
        v["telegram_configured"] = json!(!settings::get(&st.db, "telegram_token").is_empty());
    }
    Ok(Json(v))
}

/// Fetch fresh plan usage from Codex now (at most once a minute).
pub async fn refresh_usage(State(st): State<St>, AuthUser(_): AuthUser) -> ApiResult<Json<Value>> {
    static LAST: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    let recent = {
        let mut l = LAST.lock().unwrap();
        let recent = l.map(|t| t.elapsed() < std::time::Duration::from_secs(60)).unwrap_or(false);
        if !recent {
            *l = Some(std::time::Instant::now());
        }
        recent
    };
    if !recent {
        if let Err(e) = crate::agents::codex::refresh_rate_limits(&st).await {
            return Ok(Json(json!({"rate_limits": crate::runner::rate_limits(&st), "error": format!("{e:#}")})));
        }
    }
    Ok(Json(json!({"rate_limits": crate::runner::rate_limits(&st)})))
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
