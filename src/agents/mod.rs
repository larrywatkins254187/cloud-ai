pub mod claude;
pub mod codex;

use crate::settings;
use crate::state::St;
use crate::store::Project;
use serde::Serialize;
use serde_json::{json, Value};

/// What a finished turn reports back to the runner.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TurnOutcome {
    /// done | error | cancelled
    pub status: String,
    pub error: Option<String>,
    pub result_text: Option<String>,
    pub cost_usd: Option<f64>,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

impl TurnOutcome {
    pub fn error(msg: impl Into<String>) -> Self {
        TurnOutcome { status: "error".into(), error: Some(msg.into()), ..Default::default() }
    }
}

/// An uploaded file attached to a prompt.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct Attachment {
    pub id: i64,
    pub filename: String,
    pub path: String,
    pub mime: String,
    pub size: i64,
}

pub fn parse_attachments(v: &Value) -> Vec<Attachment> {
    serde_json::from_value(v.clone()).unwrap_or_default()
}

/// Prompt text plus a list of attached file paths, so the agent can open them.
pub fn prompt_with_attachments(prompt: &str, atts: &[Attachment]) -> String {
    if atts.is_empty() {
        return prompt.to_string();
    }
    let mut s = prompt.to_string();
    s.push_str("\n\nAttached files (saved on this server):\n");
    for a in atts {
        s.push_str(&format!("- {} ({}, {} bytes)\n", a.path, a.mime, a.size));
    }
    s
}

/// Extra instructions appended to the agent's system prompt: the admin's global
/// text plus how to reach the project's deploy server over SSH.
pub fn system_prompt(st: &St, project: &Project) -> String {
    let mut parts = Vec::new();
    let extra = settings::get(&st.db, "extra_system_prompt");
    if !extra.trim().is_empty() {
        parts.push(extra.trim().to_string());
    }
    if project.deploy_target == "remote" {
        if let Some(sid) = project.server_id {
            if let Ok(server) = crate::store::get_server(&st.db, sid) {
                let cfg = st.cfg.ssh_dir().join("config");
                let mut s = format!(
                    "This project runs in production on another server ({}). The code you edit here is deployed there by the cloud-ai panel",
                    server.name
                );
                if !project.remote_path.is_empty() {
                    s.push_str(&format!(" into `{}`", project.remote_path));
                }
                s.push('.');
                if server.auth_type != "password" {
                    s.push_str(&format!(
                        " You can inspect that server (logs, services, files) with `ssh -F {} {}`.",
                        cfg.display(),
                        server.alias()
                    ));
                }
                parts.push(s);
            }
        }
    }
    if !project.prod_url.is_empty() {
        parts.push(format!("The production website of this project is {}.", project.prod_url));
    }
    parts.join("\n\n")
}

/// App-level permission modes mapped to each CLI.
pub fn claude_permission_mode(mode: &str) -> &'static str {
    match mode {
        "edits" => "acceptEdits",
        "ask" => "default",
        "plan" => "plan",
        _ => "bypassPermissions",
    }
}

pub fn codex_policy(mode: &str) -> (&'static str, &'static str) {
    match mode {
        "edits" => ("on-request", "workspace-write"),
        "ask" => ("untrusted", "workspace-write"),
        "plan" => ("never", "read-only"),
        _ => ("never", "danger-full-access"),
    }
}

/// Unified question format shown in the UI and Telegram.
pub fn question(id: &str, header: &str, text: &str, multi: bool, other: bool, options: Vec<(String, String)>) -> Value {
    json!({
        "id": id,
        "header": header,
        "question": text,
        "multi": multi,
        "allow_other": other,
        "options": options.into_iter().map(|(l, d)| json!({"label": l, "description": d})).collect::<Vec<_>>(),
    })
}
