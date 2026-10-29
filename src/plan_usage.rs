//! Plan usage (5-hour and weekly limits) of the accounts the Claude Code and Codex
//! CLIs are logged in with, and which accounts those are.
//!
//! - Claude: read live from Anthropic with the CLI's own login token (the request
//!   Claude Code's `/usage` makes), and updated from `rate_limit_event`s during runs.
//! - Codex: asked from its app-server (`account/rateLimits/read`).
//!
//! Every stored usage snapshot carries the id of the account it belongs to, so a
//! snapshot of a previous account is never shown. A watcher notices logins and
//! logouts within seconds and fetches the new account's usage right away.
use crate::state::St;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How often usage is refreshed in the background.
const REFRESH_EVERY: Duration = Duration::from_secs(5 * 60);
/// How often the login files are checked for an account change.
const WATCH_EVERY: Duration = Duration::from_secs(15);
/// Minimum time between two usage requests for one CLI (unless the account changed).
const MIN_GAP: Duration = Duration::from_secs(30);

pub const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

/// The account a CLI is logged in with, read from its own login files.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Login {
    /// "subscription" (Claude.ai / ChatGPT login), "api_key", or "none".
    pub method: &'static str,
    pub email: Option<String>,
    /// Plan name such as "max", "pro", "plus".
    pub plan: Option<String>,
    /// Stable id of the account; empty when logged out.
    pub id: String,
}

impl Login {
    pub fn json(&self) -> Value {
        json!({"method": self.method, "email": self.email, "plan": self.plan, "logged_in": self.method != "none"})
    }
}

fn home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/root"))
}

fn read_json(p: PathBuf) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

fn claude_dir() -> PathBuf {
    std::env::var("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|_| home().join(".claude"))
}

/// Claude Code keeps its account details in `.claude.json` (inside the config dir
/// when CLAUDE_CONFIG_DIR is set, else in the home folder).
fn claude_global_config() -> Option<Value> {
    match std::env::var("CLAUDE_CONFIG_DIR") {
        Ok(d) => read_json(PathBuf::from(d).join(".claude.json")),
        Err(_) => read_json(home().join(".claude.json")),
    }
}

fn claude_oauth() -> Option<Value> {
    let v = read_json(claude_dir().join(".credentials.json"))?;
    let o = v.get("claudeAiOauth")?.clone();
    o["accessToken"].is_string().then_some(o)
}

fn short_hash(s: &str) -> String {
    use sha2::Digest;
    hex::encode(&sha2::Sha256::digest(s.as_bytes())[..8])
}

pub fn claude_login() -> Login {
    let cfg = claude_global_config().unwrap_or(Value::Null);
    let acc = &cfg["oauthAccount"];
    if let Some(o) = claude_oauth() {
        let email = acc["emailAddress"].as_str().map(String::from);
        let id = acc["accountUuid"]
            .as_str()
            .map(String::from)
            .or_else(|| email.clone())
            .unwrap_or_else(|| short_hash(o["refreshToken"].as_str().or(o["accessToken"].as_str()).unwrap_or("")));
        return Login { method: "subscription", email, plan: o["subscriptionType"].as_str().map(String::from), id: format!("claude:{id}") };
    }
    let key = std::env::var("ANTHROPIC_API_KEY").ok().filter(|k| !k.is_empty()).or_else(|| cfg["primaryApiKey"].as_str().map(String::from));
    match key {
        Some(k) => Login { method: "api_key", email: None, plan: None, id: format!("claude-key:{}", short_hash(&k)) },
        None => Login { method: "none", ..Default::default() },
    }
}

fn jwt_claims(token: &str) -> Option<Value> {
    use base64::Engine;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn codex_home() -> PathBuf {
    std::env::var("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|_| home().join(".codex"))
}

pub fn codex_login() -> Login {
    let Some(auth) = read_json(codex_home().join("auth.json")) else { return Login { method: "none", ..Default::default() } };
    if let Some(tok) = auth["tokens"]["id_token"].as_str() {
        let c = jwt_claims(tok).unwrap_or(Value::Null);
        let a = &c["https://api.openai.com/auth"];
        let email = c["email"].as_str().map(String::from);
        let id = a["chatgpt_user_id"]
            .as_str()
            .or(a["user_id"].as_str())
            .or(auth["tokens"]["account_id"].as_str())
            .map(String::from)
            .or_else(|| email.clone())
            .unwrap_or_else(|| short_hash(tok));
        // One ChatGPT user can belong to several workspaces with separate limits.
        let workspace = auth["tokens"]["account_id"].as_str().or(a["chatgpt_account_id"].as_str()).unwrap_or("");
        return Login { method: "subscription", email, plan: a["chatgpt_plan_type"].as_str().map(String::from), id: format!("codex:{id}:{workspace}") };
    }
    match auth["OPENAI_API_KEY"].as_str().filter(|k| !k.is_empty()) {
        Some(k) => Login { method: "api_key", email: None, plan: None, id: format!("codex-key:{}", short_hash(k)) },
        None => Login { method: "none", ..Default::default() },
    }
}

pub fn login(tool: &str) -> Login {
    if tool == "claude" { claude_login() } else { codex_login() }
}

// ---------- Claude usage ----------

fn epoch_secs(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => chrono::DateTime::parse_from_rfc3339(s).ok().map(|t| t.timestamp()),
        _ => None,
    }
}

/// Anthropic's usage answer (utilization in percent, ISO reset times) in the shape
/// the dashboard reads: `unifiedWindows.<window> = {utilization: 0..1, resetsAt: secs}`.
pub fn claude_windows_from_api(v: &Value) -> Value {
    let mut w = serde_json::Map::new();
    for key in ["five_hour", "seven_day", "seven_day_sonnet", "seven_day_opus"] {
        let x = &v[key];
        if let Some(u) = x["utilization"].as_f64() {
            w.insert(key.into(), json!({"utilization": u / 100.0, "resetsAt": epoch_secs(&x["resets_at"])}));
        }
    }
    let mut out = json!({"unifiedWindows": w, "source": "api"});
    if v["extra_usage"].is_object() {
        out["extraUsage"] = v["extra_usage"].clone();
    }
    out
}

/// Fold a `rate_limit_event` from a Claude run into the latest snapshot. Events
/// carry either all windows (`unifiedWindows`) or one (`rateLimitType`).
pub fn merge_claude_event(prev: Option<&Value>, info: &Value) -> Value {
    let mut out = prev.cloned().filter(|p| p.is_object()).unwrap_or_else(|| json!({}));
    if !out["unifiedWindows"].is_object() {
        out["unifiedWindows"] = json!({});
    }
    if let Some(w) = info["unifiedWindows"].as_object() {
        for (k, x) in w {
            out["unifiedWindows"][k] = x.clone();
        }
    } else if let (Some(kind), Some(u)) = (info["rateLimitType"].as_str(), info["utilization"].as_f64()) {
        out["unifiedWindows"][kind] = json!({"utilization": u, "resetsAt": epoch_secs(&info["resetsAt"])});
    }
    for (k, x) in info.as_object().into_iter().flatten() {
        if k != "unifiedWindows" {
            out[k] = x.clone();
        }
    }
    out
}

async fn fetch_claude(st: &St) -> Result<Value> {
    let mut o = claude_oauth().ok_or_else(|| anyhow!("Claude Code is not logged in with a Claude account"))?;
    let expired = |o: &Value| o["expiresAt"].as_i64().is_some_and(|ms| ms <= chrono::Utc::now().timestamp_millis() + 30_000);
    if expired(&o) {
        // Any Claude Code command renews an expired token; `auth status` is the cheapest.
        let bin = crate::settings::get(&st.db, "claude_path");
        let _ = tokio::time::timeout(
            Duration::from_secs(30),
            tokio::process::Command::new(&bin).args(["auth", "status", "--json"]).stdin(std::process::Stdio::null()).output(),
        )
        .await;
        o = claude_oauth().ok_or_else(|| anyhow!("Claude Code is not logged in with a Claude account"))?;
        if expired(&o) {
            bail!("the Claude login token has expired; it renews on the next Claude run");
        }
    }
    let token = o["accessToken"].as_str().unwrap_or_default();
    let r = st
        .http
        .get(CLAUDE_USAGE_URL)
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("content-type", "application/json")
        .header("user-agent", "cloud-ai")
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .context("asking Anthropic for Claude usage")?;
    let status = r.status();
    let body: Value = r.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let msg = body["error"]["message"].as_str().unwrap_or("").to_string();
        bail!("Anthropic answered {status}{}", if msg.is_empty() { String::new() } else { format!(": {msg}") });
    }
    Ok(claude_windows_from_api(&body))
}

// ---------- refresh and watch ----------

struct Seen {
    accounts: HashMap<&'static str, String>,
    last_try: HashMap<&'static str, Instant>,
    errors: HashMap<&'static str, String>,
}

static SEEN: Mutex<Option<Seen>> = Mutex::new(None);

fn with_seen<T>(f: impl FnOnce(&mut Seen) -> T) -> T {
    let mut g = SEEN.lock().unwrap();
    let s = g.get_or_insert_with(|| Seen { accounts: HashMap::new(), last_try: HashMap::new(), errors: HashMap::new() });
    f(s)
}

/// The last refresh error per CLI (cleared by a successful refresh).
pub fn errors() -> Value {
    with_seen(|s| json!(s.errors.iter().map(|(k, v)| (k.to_string(), v.clone())).collect::<HashMap<_, _>>()))
}

/// Notice logins, logouts and account switches. A changed account's old usage is
/// dropped and its new usage fetched. Returns the CLIs whose account changed.
pub async fn check_accounts(st: &St) -> Vec<&'static str> {
    let mut changed = Vec::new();
    for tool in ["claude", "codex"] {
        let now = login(tool);
        let before = with_seen(|s| s.accounts.insert(tool, now.id.clone()));
        if before.is_some_and(|b| b != now.id) {
            changed.push(tool);
        }
    }
    for tool in &changed {
        let l = login(tool);
        tracing::info!("{tool} account changed ({})", l.email.as_deref().unwrap_or(l.method));
        crate::runner::clear_rate_limits(st, tool);
        with_seen(|s| {
            s.last_try.remove(tool);
            s.errors.remove(tool);
        });
        if *tool == "codex" {
            st.codex.stop_if_idle().await;
        }
        if l.method == "subscription" {
            let _ = refresh(st, tool, true).await;
        }
    }
    changed
}

/// Fetch the plan usage of one CLI now. Without `force`, at most once per MIN_GAP.
pub async fn refresh(st: &St, tool: &'static str, force: bool) -> Result<()> {
    let l = login(tool);
    if l.method != "subscription" {
        let msg = if l.method == "api_key" { "plan usage is only reported for a subscription login, not an API key" } else { "not logged in" };
        with_seen(|s| s.errors.insert(tool, msg.into()));
        bail!("{tool}: {msg}");
    }
    let skip = with_seen(|s| {
        let recent = s.last_try.get(tool).is_some_and(|t| t.elapsed() < MIN_GAP);
        if recent && !force {
            return true;
        }
        s.last_try.insert(tool, Instant::now());
        false
    });
    if skip {
        return Ok(());
    }
    let res = match tool {
        "claude" => fetch_claude(st).await.map(|v| crate::runner::set_rate_limits(st, "claude", v)),
        _ => crate::agents::codex::refresh_rate_limits(st, force).await.map(|_| ()),
    };
    with_seen(|s| match &res {
        Ok(()) => {
            s.errors.remove(tool);
        }
        Err(e) => {
            s.errors.insert(tool, format!("{e:#}"));
        }
    });
    res
}

/// Watch for account changes and keep usage fresh while the CLIs are logged in.
pub fn spawn(st: St) {
    tokio::spawn(async move {
        // Remember the current accounts without treating startup as a change.
        for tool in ["claude", "codex"] {
            let id = login(tool).id;
            with_seen(|s| s.accounts.insert(tool, id));
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
        let mut last_refresh: Option<Instant> = None;
        loop {
            check_accounts(&st).await;
            if last_refresh.is_none_or(|t| t.elapsed() >= REFRESH_EVERY) {
                last_refresh = Some(Instant::now());
                for tool in ["claude", "codex"] {
                    if login(tool).method == "subscription" {
                        if let Err(e) = refresh(&st, tool, false).await {
                            tracing::debug!("{tool} usage refresh: {e:#}");
                        }
                    }
                }
            }
            tokio::time::sleep(WATCH_EVERY).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_usage_becomes_windows() {
        let v = json!({
            "five_hour": {"utilization": 42.0, "resets_at": "2026-10-29T15:00:00+00:00"},
            "seven_day": {"utilization": 7.5, "resets_at": "2026-11-01T00:00:00Z"},
            "seven_day_opus": null
        });
        let w = claude_windows_from_api(&v);
        assert_eq!(w["unifiedWindows"]["five_hour"]["utilization"], json!(0.42));
        assert_eq!(w["unifiedWindows"]["five_hour"]["resetsAt"], json!(1793286000));
        assert_eq!(w["unifiedWindows"]["seven_day"]["utilization"], json!(0.075));
        assert!(w["unifiedWindows"]["seven_day_opus"].is_null());
    }

    #[test]
    fn run_events_merge_into_snapshot() {
        let prev = json!({"unifiedWindows": {"five_hour": {"utilization": 0.1, "resetsAt": 1}, "seven_day": {"utilization": 0.2, "resetsAt": 2}}});
        let one = merge_claude_event(Some(&prev), &json!({"rateLimitType": "five_hour", "utilization": 0.5, "resetsAt": 9, "status": "allowed"}));
        assert_eq!(one["unifiedWindows"]["five_hour"], json!({"utilization": 0.5, "resetsAt": 9}));
        assert_eq!(one["unifiedWindows"]["seven_day"]["utilization"], json!(0.2));
        assert_eq!(one["status"], "allowed");
        let all = merge_claude_event(None, &json!({"unifiedWindows": {"seven_day": {"utilization": 0.3, "resetsAt": 3}}}));
        assert_eq!(all["unifiedWindows"]["seven_day"]["utilization"], json!(0.3));
    }
}
