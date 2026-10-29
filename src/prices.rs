//! Keeps the Usage price table up to date.
//!
//! Once a day the panel downloads LiteLLM's public price list (it covers every
//! Anthropic and OpenAI model, including cache prices) and rebuilds the
//! automatic rows of the `usage_prices` setting: one row per Claude / GPT model,
//! plus wildcard rows for models that are not listed yet. Rows the admin added
//! or edited by hand are marked `manual` and kept as they are.
use crate::api::usage::{parse_prices, Price};
use crate::settings;
use crate::state::St;
use crate::util::now;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::time::Duration;

pub const SOURCE_URL: &str = "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
pub const SOURCE_NAME: &str = "LiteLLM public price list";

/// How often the prices are refreshed.
const EVERY: Duration = Duration::from_secs(24 * 3600);
/// Wait before retrying a failed download.
const RETRY: Duration = Duration::from_secs(3600);

/// Wildcard rows for models the list does not have yet, in match order. Each one
/// takes the price of the newest listed model it matches.
const FALLBACKS: &[&str] = &[
    "claude-opus*",
    "claude-sonnet*",
    "claude-haiku*",
    "claude-fable*",
    "claude-*",
    "*-codex-mini*",
    "*-mini*",
    "*-nano*",
    "gpt-*",
    "*",
];

/// The table that ships with this release (a snapshot of the downloaded list).
pub const BUILTIN: &str = include_str!("prices_builtin.json");

/// Rows of the price table before automatic updates existed. A stored row that is
/// still exactly one of these was never edited, so updates may replace it.
const LEGACY_DEFAULTS: &str = r#"[
  {"model":"*-mini*","input":0.25,"cache_read":0.025,"cache_write":0,"output":2},
  {"model":"gpt-5.2*","input":1.75,"cache_read":0.175,"cache_write":0,"output":14},
  {"model":"gpt-5.1*","input":1.25,"cache_read":0.125,"cache_write":0,"output":10},
  {"model":"gpt-5*","input":1.25,"cache_read":0.125,"cache_write":0,"output":10},
  {"model":"*","input":1.75,"cache_read":0.175,"cache_write":0,"output":14}
]"#;

fn is_relevant(name: &str) -> bool {
    let n = name.to_lowercase();
    if n.contains('/') || n.starts_with("ft:") {
        return false;
    }
    if ["audio", "realtime", "image", "tts", "transcribe", "embedding"].iter().any(|w| n.contains(w)) {
        return false;
    }
    n.starts_with("claude-")
        || n.starts_with("gpt-5")
        || n.starts_with("codex")
        || (n.len() >= 2 && n.starts_with('o') && n.as_bytes()[1].is_ascii_digit())
}

/// Version numbers in a model name (dates left out), used to tell which model of a
/// family is newest.
fn version(name: &str) -> Vec<u32> {
    name.split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        // A 4- or 8-digit number starts a release date; the rest is not a version.
        .take_while(|s| s.len() <= 3)
        .filter_map(|s| s.parse().ok())
        .collect()
}

/// A name ending in a release date (2025-08-07 or 20250929) is a pinned snapshot.
fn is_snapshot(name: &str) -> bool {
    name.split(|c: char| !c.is_ascii_digit()).any(|s| s.len() == 8) || {
        let parts: Vec<&str> = name.rsplitn(4, '-').collect();
        parts.len() == 4 && parts[0].len() == 2 && parts[1].len() == 2 && parts[2].len() == 4 && parts[..3].iter().all(|p| p.chars().all(|c| c.is_ascii_digit()))
    }
}

fn per_million(v: &Value, key: &str) -> f64 {
    let x = v[key].as_f64().unwrap_or(0.0) * 1_000_000.0;
    // Drop float noise such as 2.9999999999999996.
    (x * 10_000.0).round() / 10_000.0
}

/// Build the automatic rows from LiteLLM's price list.
pub fn from_litellm(list: &Value) -> Result<Vec<Price>> {
    let Some(obj) = list.as_object() else { bail!("the price list is not a JSON object") };
    let mut rows: Vec<Price> = Vec::new();
    for (name, v) in obj {
        let provider = v["litellm_provider"].as_str().unwrap_or("");
        if provider != "anthropic" && provider != "openai" {
            continue;
        }
        if !matches!(v["mode"].as_str(), Some("chat") | Some("responses") | None) || !is_relevant(name) {
            continue;
        }
        if v["input_cost_per_token"].as_f64().is_none() && v["output_cost_per_token"].as_f64().is_none() {
            continue;
        }
        rows.push(Price {
            model: name.to_lowercase(),
            input: per_million(v, "input_cost_per_token"),
            cache_read: per_million(v, "cache_read_input_token_cost"),
            cache_write: per_million(v, "cache_creation_input_token_cost"),
            output: per_million(v, "output_cost_per_token"),
            source: Some("auto".into()),
        });
    }
    if !rows.iter().any(|r| r.model.starts_with("claude-")) || !rows.iter().any(|r| r.model.starts_with("gpt-")) {
        bail!("the price list has no Claude or GPT models");
    }
    rows.sort_by(|a, b| {
        let family = |m: &str| if m.starts_with("claude-") { 0 } else { 1 };
        (family(&a.model), version(&b.model), &a.model).cmp(&(family(&b.model), version(&a.model), &b.model))
    });
    rows.dedup_by(|a, b| a.model == b.model);

    let mut fallbacks = Vec::new();
    for pat in FALLBACKS {
        let newest = rows
            .iter()
            .filter(|r| !is_snapshot(&r.model) && (*pat != "*" || r.model.starts_with("gpt-")) && crate::api::usage::glob(pat, &r.model))
            // Newest version first; on a tie the plain name (gpt-5.6 over gpt-5.6-pro).
            .max_by(|a, b| (version(&a.model), std::cmp::Reverse(a.model.len())).cmp(&(version(&b.model), std::cmp::Reverse(b.model.len()))));
        if let Some(n) = newest {
            fallbacks.push(Price { model: pat.to_string(), ..n.clone() });
        }
    }
    rows.extend(fallbacks);
    Ok(rows)
}

/// True for a row the admin added or edited (kept by automatic updates).
fn is_manual(p: &Price, legacy: &[Price], builtin: &[Price]) -> bool {
    match p.source.as_deref() {
        Some("manual") => true,
        Some(_) => false,
        // Stored before rows were marked: manual unless it is an untouched default.
        None => !legacy.iter().chain(builtin).any(|d| d.same_price(p)),
    }
}

/// The admin's manual rows first, then the fresh automatic rows.
pub fn merge(current: &[Price], fresh: Vec<Price>) -> Vec<Price> {
    let legacy = parse_prices(LEGACY_DEFAULTS).unwrap_or_default();
    let builtin = parse_prices(BUILTIN).unwrap_or_default();
    let mut out: Vec<Price> = current
        .iter()
        .filter(|p| is_manual(p, &legacy, &builtin))
        .cloned()
        .map(|mut p| {
            p.source = Some("manual".into());
            p
        })
        .collect();
    out.extend(fresh);
    out
}

fn current(st: &St) -> Vec<Price> {
    let s = settings::get(&st.db, "usage_prices");
    if s.trim().is_empty() {
        return Vec::new();
    }
    parse_prices(&s).unwrap_or_default()
}

/// Download the latest prices and save the merged table.
pub async fn update(st: &St) -> Result<usize> {
    let res = async {
        let list: Value = st
            .http
            .get(SOURCE_URL)
            .header("user-agent", "cloud-ai")
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .context("downloading the price list")?
            .error_for_status()
            .context("downloading the price list")?
            .json()
            .await
            .context("reading the price list")?;
        let fresh = from_litellm(&list)?;
        let n = fresh.iter().filter(|p| !p.model.contains('*')).count();
        let merged = merge(&current(st), fresh);
        st.db.set_setting("usage_prices", &serde_json::to_string(&merged)?)?;
        Ok::<_, anyhow::Error>(n)
    }
    .await;
    let _ = st.db.set_setting("usage_prices_checked_at", &now());
    match &res {
        Ok(_) => {
            let _ = st.db.set_setting("usage_prices_updated_at", &now());
            let _ = st.db.set_setting("usage_prices_error", "");
        }
        Err(e) => {
            let _ = st.db.set_setting("usage_prices_error", &format!("{e:#}"));
        }
    }
    res
}

/// When the prices were last updated, and the last error, for the Settings page.
pub fn status(st: &St) -> Value {
    let get = |k: &str| st.db.get_setting(k).filter(|s| !s.is_empty());
    json!({
        "source": SOURCE_NAME,
        "source_url": SOURCE_URL,
        "updated_at": get("usage_prices_updated_at"),
        "checked_at": get("usage_prices_checked_at"),
        "error": get("usage_prices_error"),
        "builtin": parse_prices(BUILTIN).unwrap_or_default(),
    })
}

fn age(st: &St, key: &str) -> Option<Duration> {
    let at = st.db.get_setting(key)?;
    let t = chrono::DateTime::parse_from_rfc3339(&at).ok()?;
    (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).to_std().ok()
}

/// Refresh the prices once a day (and an hour after a failed try).
pub fn spawn(st: St) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(45)).await;
        loop {
            let due = age(&st, "usage_prices_updated_at").is_none_or(|a| a >= EVERY)
                && age(&st, "usage_prices_checked_at").is_none_or(|a| a >= RETRY);
            if due {
                match update(&st).await {
                    Ok(n) => tracing::info!("usage prices updated ({n} models)"),
                    Err(e) => tracing::warn!("usage price update failed: {e:#}"),
                }
            }
            tokio::time::sleep(Duration::from_secs(600)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        json!({
            "sample_spec": {"litellm_provider": "openai"},
            "claude-opus-5-5": {"litellm_provider": "anthropic", "mode": "chat", "input_cost_per_token": 4e-6, "output_cost_per_token": 2e-5, "cache_read_input_token_cost": 2e-7, "cache_creation_input_token_cost": 5e-6},
            "claude-opus-4-7-20260416": {"litellm_provider": "anthropic", "mode": "chat", "input_cost_per_token": 5e-6, "output_cost_per_token": 2.5e-5},
            "claude-sonnet-5-5": {"litellm_provider": "anthropic", "mode": "chat", "input_cost_per_token": 2e-6, "output_cost_per_token": 1e-5},
            "bedrock/claude-opus-5-5": {"litellm_provider": "bedrock", "mode": "chat", "input_cost_per_token": 9e-6},
            "gpt-5.6": {"litellm_provider": "openai", "mode": "chat", "input_cost_per_token": 4e-6, "output_cost_per_token": 2e-5},
            "gpt-5.6-pro": {"litellm_provider": "openai", "mode": "responses", "input_cost_per_token": 3e-5, "output_cost_per_token": 1.8e-4},
            "gpt-5.1-codex-mini": {"litellm_provider": "openai", "mode": "responses", "input_cost_per_token": 2.5e-7, "output_cost_per_token": 2e-6},
            "gpt-4o-realtime-preview": {"litellm_provider": "openai", "mode": "chat", "input_cost_per_token": 5e-6},
            "text-embedding-3-large": {"litellm_provider": "openai", "mode": "embedding", "input_cost_per_token": 1e-7}
        })
    }

    #[test]
    fn builds_rows_and_fallbacks() {
        let rows = from_litellm(&sample()).unwrap();
        let names: Vec<&str> = rows.iter().map(|r| r.model.as_str()).collect();
        assert!(names.contains(&"claude-opus-5-5"));
        assert!(!names.iter().any(|n| n.contains('/') || n.contains("realtime") || n.contains("embedding")));
        let opus = rows.iter().find(|r| r.model == "claude-opus-5-5").unwrap();
        assert_eq!((opus.input, opus.cache_read, opus.cache_write, opus.output), (4.0, 0.2, 5.0, 20.0));
        // Wildcards take the newest model of the family, not a dated snapshot.
        assert_eq!(rows.iter().find(|r| r.model == "claude-opus*").unwrap().input, 4.0);
        assert_eq!(rows.iter().find(|r| r.model == "gpt-*").unwrap().input, 4.0);
        assert_eq!(rows.iter().find(|r| r.model == "*").unwrap().output, 20.0);
        assert_eq!(rows.iter().find(|r| r.model == "*-mini*").unwrap().input, 0.25);
        assert_eq!(rows.last().unwrap().model, "*");
        assert!(rows.iter().all(|r| r.source.as_deref() == Some("auto")));
        assert!(from_litellm(&json!({})).is_err());
    }

    #[test]
    fn merge_keeps_manual_rows() {
        let fresh = from_litellm(&sample()).unwrap();
        let current = parse_prices(
            r#"[
              {"model":"*-mini*","input":0.25,"cache_read":0.025,"cache_write":0,"output":2},
              {"model":"my-model","input":1,"cache_read":0,"cache_write":0,"output":2},
              {"model":"gpt-5.6","input":9,"cache_read":0,"cache_write":0,"output":9,"source":"manual"},
              {"model":"gpt-5.5","input":5,"cache_read":0.5,"cache_write":0,"output":30,"source":"auto"}
            ]"#,
        )
        .unwrap();
        let merged = merge(&current, fresh.clone());
        // Untouched legacy defaults and old automatic rows are replaced.
        assert_eq!(merged[0].model, "my-model");
        assert_eq!(merged[1].model, "gpt-5.6");
        assert!(merged[..2].iter().all(|p| p.source.as_deref() == Some("manual")));
        assert_eq!(merged.len(), fresh.len() + 2);
        assert!(!merged.iter().any(|p| p.model == "gpt-5.5"));
    }

    #[test]
    fn builtin_table_is_valid() {
        let rows = parse_prices(BUILTIN).unwrap();
        assert!(rows.iter().any(|r| r.model.starts_with("claude-")));
        assert!(rows.iter().any(|r| r.model.starts_with("gpt-")));
        assert_eq!(rows.last().unwrap().model, "*");
    }

    #[test]
    fn versions_ignore_dates() {
        assert_eq!(version("gpt-5-search-api-2025-10-14"), vec![5]);
        assert_eq!(version("claude-opus-4-7-20260416"), vec![4, 7]);
        assert_eq!(version("gpt-5.6-luna"), vec![5, 6]);
    }

    #[test]
    fn snapshots() {
        assert!(is_snapshot("claude-opus-4-7-20260416"));
        assert!(is_snapshot("gpt-5-2025-08-07"));
        assert!(!is_snapshot("gpt-5.6"));
        assert!(!is_snapshot("claude-opus-5-5"));
    }

    /// Regenerates src/prices_builtin.json from a downloaded list:
    /// `PRICES_FILE=/path/list.json cargo test write_builtin -- --ignored`
    #[test]
    #[ignore]
    fn write_builtin() {
        let path = std::env::var("PRICES_FILE").expect("PRICES_FILE");
        let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let rows = from_litellm(&v).unwrap();
        let lines: Vec<String> = rows.iter().map(|r| format!("  {}", serde_json::to_string(r).unwrap())).collect();
        std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/src/prices_builtin.json"), format!("[\n{}\n]\n", lines.join(",\n"))).unwrap();
    }
}
