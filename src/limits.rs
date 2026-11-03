//! Plan usage limits (Claude's 5-hour window, Codex's short window…).
//! When a run fails because the limit was reached we work out when the window
//! resets, so the chat can offer to continue automatically at that time.
use crate::settings;
use crate::state::St;
use chrono::{DateTime, Duration, TimeZone, Utc};
use serde_json::{json, Value};

/// Extra wait after the reported reset, so the first prompt isn't rejected again.
const RESET_MARGIN_MINUTES: i64 = 2;
/// Used when the CLI didn't tell us when the window resets.
const FALLBACK_HOURS: i64 = 5;

/// Does this error text look like "you've hit your plan's usage limit"?
pub fn looks_like_limit(error: &str) -> bool {
    let e = error.to_lowercase();
    [
        "usage limit",
        "hit your limit",
        "limit reached",
        "limit will reset",
        "usagelimitexceeded",
        "out of extra usage",
        "rate_limit_exceeded",
        "5-hour limit",
        "weekly limit",
    ]
    .iter()
    .any(|p| e.contains(p))
}

/// Read a timestamp that may be unix seconds, unix milliseconds or an RFC 3339 string.
fn ts(v: &Value) -> Option<DateTime<Utc>> {
    if let Some(n) = v.as_f64() {
        let secs = if n > 1e12 { n / 1000.0 } else { n };
        return Utc.timestamp_opt(secs as i64, 0).single();
    }
    v.as_str().and_then(|s| {
        DateTime::parse_from_rfc3339(s)
            .map(|d| d.with_timezone(&Utc))
            .ok()
            .or_else(|| s.parse::<f64>().ok().and_then(|n| ts(&json!(n))))
    })
}

/// Old Claude CLIs report "Claude AI usage limit reached|1751234567".
fn reset_from_text(error: &str) -> Option<DateTime<Utc>> {
    let (_, rest) = error.split_once('|')?;
    let digits: String = rest.trim().chars().take_while(|c| c.is_ascii_digit()).collect();
    ts(&json!(digits.parse::<i64>().ok()?))
}

/// Latest reset time among the windows that are exhausted (or the given fallback window).
fn reset_from_info(tool: &str, info: &Value) -> (Option<DateTime<Utc>>, bool) {
    let mut exhausted: Vec<DateTime<Utc>> = Vec::new();
    let mut any: Vec<DateTime<Utc>> = Vec::new();
    if tool == "codex" {
        for w in ["primary", "secondary"] {
            let win = &info[w];
            if let Some(t) = ts(&win["resetsAt"]) {
                any.push(t);
                if win["usedPercent"].as_f64().unwrap_or(0.0) >= 99.5 {
                    exhausted.push(t);
                }
            }
        }
    } else {
        // Claude `rate_limit_event`: { status, resetsAt, rateLimitType, utilization, … }
        let rejected = info["status"].as_str() == Some("rejected");
        if let Some(t) = ts(&info["resetsAt"]) {
            any.push(t);
            if rejected {
                exhausted.push(t);
            }
        }
        if let Some(wins) = info["unifiedWindows"].as_object() {
            for win in wins.values() {
                if let Some(t) = ts(&win["resetsAt"]) {
                    any.push(t);
                    if win["utilization"].as_f64().unwrap_or(0.0) >= 0.995 || win["status"].as_str() == Some("rejected") {
                        exhausted.push(t);
                    }
                }
            }
        }
    }
    let now = Utc::now();
    let pick = |v: Vec<DateTime<Utc>>| v.into_iter().filter(|t| *t > now).max();
    // Stale info (window already reset) doesn't count.
    let blocked = exhausted.iter().any(|t| *t > now);
    (pick(exhausted).or_else(|| pick(any)), blocked)
}

/// If `error` is a usage-limit failure, describe it for the chat UI:
/// `{tool, resets_at, resume_at, resume_local, estimated}`.
pub fn detect(st: &St, tool: &str, error: &str) -> Option<Value> {
    let info = crate::runner::rate_limits(st)[tool].clone();
    let (from_info, blocked) = reset_from_info(tool, &info);
    if !looks_like_limit(error) && !blocked {
        return None;
    }
    let now = Utc::now();
    let reset = reset_from_text(error).filter(|t| *t > now).or(from_info);
    let estimated = reset.is_none();
    let reset = reset.unwrap_or_else(|| now + Duration::hours(FALLBACK_HOURS));
    let resume = reset + Duration::minutes(RESET_MARGIN_MINUTES);
    let tz = settings::tz(&st.db);
    Some(json!({
        "tool": tool,
        "resets_at": reset.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "resume_at": resume.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        // Panel-local time in the same format the schedule form uses.
        "resume_local": resume.with_timezone(&tz).format("%Y-%m-%dT%H:%M").to_string(),
        "estimated": estimated,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_limit_messages() {
        assert!(looks_like_limit("Claude AI usage limit reached|1751234567"));
        assert!(looks_like_limit("5-hour limit reached ∙ resets 3pm"));
        assert!(looks_like_limit("You've hit your limit · resets 5pm (Asia/Tehran)"));
        assert!(looks_like_limit("You've hit your usage limit. Upgrade to Pro or try again at 8:05 PM."));
        assert!(!looks_like_limit("No conversation found with session ID"));
        assert!(!looks_like_limit("cargo build failed"));
    }

    #[test]
    fn reads_reset_from_old_claude_text() {
        let t = reset_from_text("Claude AI usage limit reached|1751234567").unwrap();
        assert_eq!(t.timestamp(), 1751234567);
        assert!(reset_from_text("no pipe here").is_none());
    }

    #[test]
    fn picks_exhausted_codex_window() {
        let soon = Utc::now().timestamp() + 3600;
        let later = soon + 86400;
        let info = json!({
            "primary": {"usedPercent": 100.0, "resetsAt": soon},
            "secondary": {"usedPercent": 40.0, "resetsAt": later},
        });
        let (t, blocked) = reset_from_info("codex", &info);
        assert!(blocked);
        assert_eq!(t.unwrap().timestamp(), soon);
    }

    #[test]
    fn claude_rejected_event() {
        let at = Utc::now().timestamp() + 7200;
        let info = json!({"status": "rejected", "resetsAt": at, "rateLimitType": "five_hour"});
        let (t, blocked) = reset_from_info("claude", &info);
        assert!(blocked);
        assert_eq!(t.unwrap().timestamp(), at);
        let (_, blocked) = reset_from_info("claude", &json!({"status": "allowed", "resetsAt": at}));
        assert!(!blocked);
    }
}
