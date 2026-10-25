use crate::db::Db;
use serde_json::{json, Map, Value};

/// Every admin-editable setting with its default. Values are stored as strings.
pub const DEFAULTS: &[(&str, &str)] = &[
    ("timezone", "Asia/Tehran"),
    ("public_url", ""),
    ("max_claude", "3"),
    ("max_codex", "3"),
    ("ram_guard_mb", "300"),
    ("idle_kill_minutes", "5"),
    ("claude_path", "claude"),
    ("codex_path", "codex"),
    ("default_permission_mode", "bypass"),
    ("extra_system_prompt", ""),
    ("telegram_token", ""),
    ("telegram_chat_id", ""),
    ("telegram_allowed_users", ""),
    ("telegram_send_prompts", "1"),
    ("telegram_send_results", "1"),
    ("auto_update_enabled", "1"),
    ("update_hour", "5"),
    ("app_auto_update", "1"),
    ("app_update_repo", "larrywatkins254187/cloud-ai"),
    ("browser_path", ""),
    ("browser_idle_minutes", "10"),
];

/// Settings never sent back to the browser in clear text.
pub const SECRET_KEYS: &[&str] = &["telegram_token"];

pub fn get(db: &Db, key: &str) -> String {
    if key == "public_url" {
        // The installer passes the domain so Telegram links work before anyone opens Settings.
        let stored = db.get_setting(key).unwrap_or_default();
        if stored.is_empty() {
            return std::env::var("CLOUD_AI_PUBLIC_URL").unwrap_or_default();
        }
        return stored;
    }
    db.get_setting(key).unwrap_or_else(|| {
        DEFAULTS.iter().find(|(k, _)| *k == key).map(|(_, v)| v.to_string()).unwrap_or_default()
    })
}

pub fn get_i64(db: &Db, key: &str) -> i64 {
    get(db, key).trim().parse().unwrap_or_else(|_| {
        DEFAULTS.iter().find(|(k, _)| *k == key).and_then(|(_, v)| v.parse().ok()).unwrap_or(0)
    })
}

pub fn get_bool(db: &Db, key: &str) -> bool {
    matches!(get(db, key).trim(), "1" | "true" | "yes" | "on")
}

pub fn tz(db: &Db) -> chrono_tz::Tz {
    get(db, "timezone").parse().unwrap_or(chrono_tz::Asia::Tehran)
}

pub fn all_for_ui(db: &Db) -> Value {
    let mut m = Map::new();
    for (k, _) in DEFAULTS {
        let v = get(db, k);
        if SECRET_KEYS.contains(k) {
            m.insert(k.to_string(), json!(if v.is_empty() { "" } else { "********" }));
        } else {
            m.insert(k.to_string(), json!(v));
        }
    }
    Value::Object(m)
}

pub fn update_from_ui(db: &Db, v: &Value) -> anyhow::Result<()> {
    let Some(obj) = v.as_object() else { anyhow::bail!("expected object") };
    for (k, val) in obj {
        if !DEFAULTS.iter().any(|(d, _)| d == k) {
            continue;
        }
        let s = match val {
            Value::String(s) => s.clone(),
            Value::Bool(b) => if *b { "1".into() } else { "0".into() },
            Value::Number(n) => n.to_string(),
            Value::Null => String::new(),
            other => other.to_string(),
        };
        if SECRET_KEYS.contains(&k.as_str()) && s == "********" {
            continue;
        }
        if k == "timezone" && s.parse::<chrono_tz::Tz>().is_err() {
            anyhow::bail!("unknown timezone {s}");
        }
        db.set_setting(k, s.trim())?;
    }
    Ok(())
}
