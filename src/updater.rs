//! Keeps Claude Code, Codex and cloud-ai itself up to date (only while nothing
//! is running), and handles logging the CLIs in from the panel.
use crate::agents::codex::CodexProc;
use crate::auth::AdminUser;
use crate::settings;
use crate::state::St;
use crate::store::{get_cli_info, set_cli_info};
use crate::util::{now, tail, ApiError, ApiResult};
use anyhow::{anyhow, bail, Context, Result};
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Timelike;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;

#[derive(Default)]
pub struct UpdaterState {
    running: tokio::sync::Mutex<()>,
    codex_login: tokio::sync::Mutex<Option<Arc<CodexProc>>>,
}

pub fn router() -> Router<St> {
    Router::new()
        .route("/api/cli/status", get(status))
        .route("/api/cli/check", post(check))
        .route("/api/cli/update", post(update))
        .route("/api/cli/login/{tool}", post(login))
        .route("/api/cli/logout/{tool}", post(logout))
}

async fn sh(cmd: &str, timeout: Duration) -> Result<(bool, String)> {
    let fut = Command::new("bash").arg("-lc").arg(cmd).env("IS_SANDBOX", "1").kill_on_drop(true).output();
    let out = tokio::time::timeout(timeout, fut).await.map_err(|_| anyhow!("`{cmd}` timed out"))??;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    Ok((out.status.success(), text))
}

fn bin(st: &St, tool: &str) -> String {
    settings::get(&st.db, if tool == "codex" { "codex_path" } else { "claude_path" })
}

/// "2.1.310 (Claude Code)" / "codex-cli 0.164.0" → "2.1.310" / "0.164.0"
fn parse_version(s: &str) -> Option<String> {
    s.split_whitespace()
        .find(|w| w.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) && w.contains('.'))
        .map(|w| w.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '-').to_string())
}

pub async fn installed_version(st: &St, tool: &str) -> Option<String> {
    let (ok, out) = sh(&format!("{} --version", crate::util::shell_quote(&bin(st, tool))), Duration::from_secs(30)).await.ok()?;
    if !ok {
        return None;
    }
    let v = parse_version(&out);
    if let Some(v) = &v {
        set_cli_info(st, &format!("{tool}_version"), &json!(v));
    }
    v
}

async fn latest_version(st: &St, tool: &str) -> Option<String> {
    let pkg = if tool == "codex" { "@openai/codex" } else { "@anthropic-ai/claude-code" };
    let v: Value = st.http
        .get(format!("https://registry.npmjs.org/{pkg}/latest"))
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    let ver = v["version"].as_str()?.to_string();
    set_cli_info(st, &format!("{tool}_latest"), &json!(ver));
    Some(ver)
}

fn newer(latest: &str, current: &str) -> bool {
    match (semver::Version::parse(latest), semver::Version::parse(current)) {
        (Ok(l), Ok(c)) => l > c,
        _ => latest != current,
    }
}

/// True when the binary was installed with npm (then npm also handles rollback).
async fn npm_package_installed(pkg: &str) -> bool {
    sh(&format!("npm ls -g --depth=0 {pkg}"), Duration::from_secs(30)).await.map(|(ok, out)| ok && out.contains(pkg)).unwrap_or(false)
}

pub fn idle(st: &St) -> bool {
    let busy: i64 = st
        .db
        .conn()
        .query_row("SELECT (SELECT COUNT(*) FROM runs WHERE status IN ('running','queued')) + (SELECT COUNT(*) FROM deploys WHERE status='running')", [], |r| r.get(0))
        .unwrap_or(1);
    busy == 0
}

/// Update one CLI if a newer version exists. Returns a human readable result.
pub async fn update_cli(st: &St, tool: &str) -> Result<String> {
    let name = if tool == "codex" { "Codex" } else { "Claude Code" };
    let pkg = if tool == "codex" { "@openai/codex" } else { "@anthropic-ai/claude-code" };
    let before = installed_version(st, tool).await.ok_or_else(|| anyhow!("{name} is not installed or not on PATH"))?;
    let latest = latest_version(st, tool).await;
    if let Some(l) = &latest {
        if !newer(l, &before) {
            return Ok(format!("{name} {before} is up to date"));
        }
    }
    // Stop processes that use the old binary (nothing is running at this point).
    if tool == "codex" {
        st.codex.stop().await;
    } else {
        crate::runner::reap_idle(st, Duration::ZERO);
    }
    let via_npm = npm_package_installed(pkg).await;
    let cmd = if via_npm {
        format!("npm install -g {pkg}@latest")
    } else {
        format!("{} update", crate::util::shell_quote(&bin(st, tool)))
    };
    let (ok, out) = sh(&cmd, Duration::from_secs(900)).await?;
    let after = installed_version(st, tool).await;
    match after {
        Some(a) if ok || a != before => {
            if a == before {
                return Ok(format!("{name} {before}: nothing to update ({})", tail(out.trim(), 200)));
            }
            Ok(format!("{name} updated {before} → {a}"))
        }
        _ => {
            // The new version does not run: go back to the previous one.
            let rb = if via_npm {
                format!("npm install -g {pkg}@{before}")
            } else if tool == "claude" {
                format!("{} install {before}", crate::util::shell_quote(&bin(st, tool)))
            } else {
                format!("npm install -g {pkg}@{before}")
            };
            let (rok, rout) = sh(&rb, Duration::from_secs(900)).await?;
            bail!(
                "{name} update failed ({}). Rollback to {before}: {}",
                tail(out.trim(), 400),
                if rok { "ok".to_string() } else { tail(rout.trim(), 300) }
            )
        }
    }
}

// ---------- self update ----------

fn asset_name() -> String {
    format!("cloud-ai-linux-{}", std::env::consts::ARCH)
}

async fn latest_release(st: &St) -> Result<Value> {
    let repo = settings::get(&st.db, "app_update_repo");
    let v: Value = st.http
        .get(format!("https://api.github.com/repos/{repo}/releases/latest"))
        .header("user-agent", "cloud-ai")
        .header("accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(20))
        .send()
        .await?
        .json()
        .await?;
    if v["tag_name"].is_null() {
        bail!("no release found in {repo}");
    }
    set_cli_info(st, "app_latest", &v["tag_name"]);
    Ok(v)
}

/// Download the latest release binary, verify it and replace the running
/// executable; systemd restarts the service afterwards.
pub async fn update_app(st: &St) -> Result<Option<String>> {
    let rel = latest_release(st).await?;
    let tag = rel["tag_name"].as_str().unwrap_or("").trim_start_matches('v').to_string();
    if !newer(&tag, env!("CARGO_PKG_VERSION")) {
        return Ok(None);
    }
    let assets = rel["assets"].as_array().cloned().unwrap_or_default();
    let want = asset_name();
    let url = |name: &str| assets.iter().find(|a| a["name"] == name).and_then(|a| a["browser_download_url"].as_str()).map(String::from);
    let bin_url = url(&want).ok_or_else(|| anyhow!("release {tag} has no {want} asset"))?;
    let client = st.http.clone();
    let bytes = client.get(&bin_url).header("user-agent", "cloud-ai").timeout(Duration::from_secs(600)).send().await?.bytes().await?;
    if let Some(sum_url) = url(&format!("{want}.sha256")) {
        let sum = client.get(&sum_url).header("user-agent", "cloud-ai").send().await?.text().await?;
        let expected = sum.split_whitespace().next().unwrap_or("").to_lowercase();
        let actual = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&bytes));
        if expected != actual {
            bail!("checksum mismatch for {want}");
        }
    }
    let exe = std::env::current_exe()?;
    let new = exe.with_extension("new");
    std::fs::write(&new, &bytes)?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755))?;
    }
    let (ok, out) = sh(&format!("{} version", new.display()), Duration::from_secs(20)).await?;
    if !ok || !out.contains(&tag) {
        let _ = std::fs::remove_file(&new);
        bail!("downloaded binary does not run: {}", tail(out.trim(), 200));
    }
    let _ = std::fs::copy(&exe, exe.with_extension("old"));
    std::fs::rename(&new, &exe).context("replacing the binary")?;
    Ok(Some(tag))
}

/// Run all updates now (only when idle). Returns a report.
pub async fn run_updates(st: &St, include_app: bool) -> Result<Vec<String>> {
    let _guard = st.updater.running.try_lock().map_err(|_| anyhow!("an update is already running"))?;
    if !idle(st) {
        bail!("something is running; updates wait until all prompts and deploys are finished");
    }
    let mut report = Vec::new();
    let mut changed = false;
    for tool in ["claude", "codex"] {
        match update_cli(st, tool).await {
            Ok(msg) => {
                changed |= msg.contains('→');
                report.push(msg);
            }
            Err(e) => report.push(format!("ERROR: {e:#}")),
        }
    }
    if changed {
        crate::catalog::refresh(st).await;
    }
    let _ = st.db.set_setting("last_update_at", &now());
    let mut restart = false;
    if include_app && settings::get_bool(&st.db, "app_auto_update") {
        match update_app(st).await {
            Ok(Some(tag)) => {
                report.push(format!("cloud-ai updated to {tag}; restarting"));
                restart = true;
            }
            Ok(None) => report.push(format!("cloud-ai {} is up to date", env!("CARGO_PKG_VERSION"))),
            Err(e) => report.push(format!("cloud-ai update: {e:#}")),
        }
    }
    let level = if report.iter().any(|r| r.starts_with("ERROR")) { "error" } else { "info" };
    st.notice(level, format!("Updates: {}", report.join(" · ")));
    if restart {
        restart_soon(st);
    }
    Ok(report)
}

/// Update just one thing now: "claude", "codex" or "app" (cloud-ai itself,
/// regardless of the auto-update switch). Only when idle.
pub async fn run_update_one(st: &St, target: &str) -> Result<Vec<String>> {
    let _guard = st.updater.running.try_lock().map_err(|_| anyhow!("an update is already running"))?;
    if !idle(st) {
        bail!("something is running; updates wait until all prompts and deploys are finished");
    }
    let msg = match target {
        "claude" | "codex" => {
            let msg = update_cli(st, target).await?;
            if msg.contains('→') {
                crate::catalog::refresh(st).await;
            }
            msg
        }
        "app" => match update_app(st).await? {
            Some(tag) => {
                restart_soon(st);
                format!("cloud-ai updated to {tag}; restarting")
            }
            None => format!("cloud-ai {} is up to date", env!("CARGO_PKG_VERSION")),
        },
        _ => bail!("unknown update target {target}"),
    };
    st.notice("info", format!("Updates: {msg}"));
    Ok(vec![msg])
}

fn restart_soon(st: &St) {
    let st2 = st.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        crate::shutdown(&st2).await;
        // systemd (Restart=always) starts the new binary.
        std::process::exit(0);
    });
}

pub fn spawn(st: St) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(20)).await;
        for tool in ["claude", "codex"] {
            installed_version(&st, tool).await;
        }
        loop {
            if settings::get_bool(&st.db, "auto_update_enabled") {
                let tz = settings::tz(&st.db);
                let local = chrono::Utc::now().with_timezone(&tz);
                let today = local.format("%Y-%m-%d").to_string();
                let hour = settings::get_i64(&st.db, "update_hour");
                let done_today = st.db.get_setting("update_day").as_deref() == Some(today.as_str());
                if !done_today && local.hour() as i64 >= hour && idle(&st) {
                    let _ = st.db.set_setting("update_day", &today);
                    if let Err(e) = run_updates(&st, true).await {
                        st.notice("warn", format!("Scheduled update skipped: {e:#}"));
                    }
                }
            }
            // Re-checked every minute, so a busy 05:00 simply retries until idle.
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });
}

// ---------- API ----------

async fn claude_auth(st: &St) -> Value {
    match sh(&format!("{} auth status --json", crate::util::shell_quote(&bin(st, "claude"))), Duration::from_secs(30)).await {
        Ok((_, out)) => serde_json::from_str(out.trim()).unwrap_or(json!({"raw": out.trim()})),
        Err(e) => json!({"error": e.to_string()}),
    }
}

async fn codex_auth(st: &St) -> Value {
    match sh(&format!("{} login status", crate::util::shell_quote(&bin(st, "codex"))), Duration::from_secs(30)).await {
        Ok((ok, out)) => {
            let logged_in = ok && !out.to_lowercase().contains("not logged");
            json!({"loggedIn": logged_in, "detail": out.trim(), "email": if logged_in { codex_email() } else { None }})
        }
        Err(e) => json!({"loggedIn": false, "detail": e.to_string()}),
    }
}

/// Email of the ChatGPT account Codex is logged in with, read from the
/// id_token in Codex's auth.json.
fn codex_email() -> Option<String> {
    let home = std::env::var("CODEX_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| std::path::Path::new(&h).join(".codex")))
        .ok()?;
    let auth: Value = serde_json::from_str(&std::fs::read_to_string(home.join("auth.json")).ok()?).ok()?;
    jwt_email(auth["tokens"]["id_token"].as_str()?)
}

fn jwt_email(token: &str) -> Option<String> {
    use base64::Engine;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v["email"].as_str().map(String::from)
}

async fn status(State(st): State<St>, AdminUser(_): AdminUser) -> Json<Value> {
    let (cv, xv, ca, xa) = tokio::join!(
        installed_version(&st, "claude"),
        installed_version(&st, "codex"),
        claude_auth(&st),
        codex_auth(&st)
    );
    let pending_login = st.updater.codex_login.lock().await.is_some();
    Json(json!({
        "claude": {"version": cv, "latest": get_cli_info(&st.db, "claude_latest"), "auth": ca, "account": get_cli_info(&st.db, "claude_account")},
        "codex": {"version": xv, "latest": get_cli_info(&st.db, "codex_latest"), "auth": xa, "account": get_cli_info(&st.db, "codex_account"), "login_pending": pending_login},
        "app": {"version": env!("CARGO_PKG_VERSION"), "latest": get_cli_info(&st.db, "app_latest")},
        "last_update_at": st.db.get_setting("last_update_at"),
        "idle": idle(&st),
    }))
}

/// Look up the latest versions now and compare them with what is installed.
async fn check(State(st): State<St>, AdminUser(_): AdminUser) -> Json<Value> {
    let (a, b, c, ia, ib) = tokio::join!(
        latest_version(&st, "claude"),
        latest_version(&st, "codex"),
        latest_release(&st),
        installed_version(&st, "claude"),
        installed_version(&st, "codex")
    );
    let entry = |installed: Option<String>, latest: Option<String>, err: Option<String>| {
        let available = matches!((&installed, &latest), (Some(i), Some(l)) if newer(l, i));
        json!({"installed": installed, "latest": latest, "update_available": available, "error": err})
    };
    let app_installed = env!("CARGO_PKG_VERSION").to_string();
    let (app_latest, app_err) = match c {
        Ok(r) => (r["tag_name"].as_str().map(|t| t.trim_start_matches('v').to_string()), None),
        Err(e) => (None, Some(format!("{e:#}"))),
    };
    Json(json!({
        "claude": entry(ia, a, None),
        "codex": entry(ib, b, None),
        "app": entry(Some(app_installed), app_latest, app_err),
        "idle": idle(&st),
    }))
}

#[derive(Deserialize)]
struct UpdateBody {
    #[serde(default)]
    app: bool,
    /// Update only this one: "claude", "codex" or "app".
    only: Option<String>,
}

async fn update(State(st): State<St>, AdminUser(_): AdminUser, Json(b): Json<UpdateBody>) -> ApiResult<Json<Value>> {
    let report = match b.only.as_deref() {
        Some(t) => run_update_one(&st, t).await,
        None => run_updates(&st, b.app).await,
    }
    .map_err(|e| ApiError::bad(format!("{e:#}")))?;
    Ok(Json(json!({"report": report})))
}

async fn login(State(st): State<St>, AdminUser(_): AdminUser, Path(tool): Path<String>) -> ApiResult<Json<Value>> {
    match tool.as_str() {
        "claude" => {
            let t = crate::terminal::spawn_term(
                &st,
                serde_json::from_value(json!({"command": format!("{} auth login", bin(&st, "claude")), "title": "Claude Code login"})).unwrap(),
            )?;
            Ok(Json(json!({"terminal": t.id})))
        }
        "codex" => {
            let p = crate::agents::codex::start_proc(&st).await?;
            let r = p.request("account/login/start", json!({"type": "chatgptDeviceCode"})).await?;
            *st.updater.codex_login.lock().await = Some(p.clone());
            // Watch for completion, then shut the helper down.
            let st2 = st.clone();
            tokio::spawn(async move {
                for _ in 0..180 {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    if let Ok(acc) = p.request("account/read", json!({})).await {
                        if !acc["account"].is_null() {
                            set_cli_info(&st2, "codex_account", &acc);
                            st2.notice("info", "Codex is logged in");
                            break;
                        }
                    }
                }
                p.kill();
                *st2.updater.codex_login.lock().await = None;
                st2.codex.stop().await;
            });
            Ok(Json(json!({"verification_url": r["verificationUrl"], "user_code": r["userCode"]})))
        }
        "codex-terminal" => {
            let t = crate::terminal::spawn_term(
                &st,
                serde_json::from_value(json!({"command": format!("{} login --device-auth", bin(&st, "codex")), "title": "Codex login"})).unwrap(),
            )?;
            Ok(Json(json!({"terminal": t.id})))
        }
        _ => Err(ApiError::bad("unknown tool")),
    }
}

async fn logout(State(st): State<St>, AdminUser(_): AdminUser, Path(tool): Path<String>) -> ApiResult<Json<Value>> {
    let cmd = match tool.as_str() {
        "claude" => format!("{} auth logout", crate::util::shell_quote(&bin(&st, "claude"))),
        "codex" => format!("{} logout", crate::util::shell_quote(&bin(&st, "codex"))),
        _ => return Err(ApiError::bad("unknown tool")),
    };
    let (_, out) = sh(&cmd, Duration::from_secs(30)).await?;
    if tool == "codex" {
        st.codex.stop().await;
    }
    Ok(Json(json!({"output": out.trim()})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("2.1.310 (Claude Code)").as_deref(), Some("2.1.310"));
        assert_eq!(parse_version("codex-cli 0.164.0").as_deref(), Some("0.164.0"));
        assert!(newer("2.1.311", "2.1.310"));
        assert!(!newer("2.1.310", "2.1.310"));
    }

    #[test]
    fn email_from_id_token() {
        // header.payload.signature with payload {"email":"a@b.co"}
        let t = "eyJhbGciOiJub25lIn0.eyJlbWFpbCI6ImFAYi5jbyJ9.sig";
        assert_eq!(jwt_email(t).as_deref(), Some("a@b.co"));
        assert_eq!(jwt_email("bad"), None);
    }
}
