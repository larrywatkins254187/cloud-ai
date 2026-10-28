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
    /// Token breakdown per model used in this turn (sub-agents can use other models).
    pub usage: Vec<ModelUsage>,
}

/// Tokens one model used in one turn. `input` excludes cached tokens;
/// `reasoning` is already counted in `output`.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct ModelUsage {
    pub model: String,
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub reasoning: i64,
    pub web_search: i64,
    /// Cost reported by the CLI; None when it reports none (Codex), so it is estimated later.
    pub cost_usd: Option<f64>,
}

impl ModelUsage {
    pub fn is_empty(&self) -> bool {
        self.input == 0 && self.output == 0 && self.cache_read == 0 && self.cache_write == 0 && self.cost_usd.unwrap_or(0.0) <= 0.0
    }

    /// Claude's per-model totals (`modelUsage` entries) are cumulative for the process.
    pub fn from_claude(model: &str, m: &Value) -> Self {
        let n = |k: &str| m[k].as_i64().unwrap_or(0);
        ModelUsage {
            model: model.to_string(),
            input: n("inputTokens"),
            output: n("outputTokens"),
            cache_read: n("cacheReadInputTokens"),
            cache_write: n("cacheCreationInputTokens"),
            reasoning: n("thinkingTokens"),
            web_search: n("webSearchRequests"),
            cost_usd: m["costUSD"].as_f64(),
        }
    }

    /// `self - prev`, clamped at zero.
    pub fn minus(&self, prev: Option<&ModelUsage>) -> Self {
        let Some(p) = prev else { return self.clone() };
        let d = |a: i64, b: i64| (a - b).max(0);
        ModelUsage {
            model: self.model.clone(),
            input: d(self.input, p.input),
            output: d(self.output, p.output),
            cache_read: d(self.cache_read, p.cache_read),
            cache_write: d(self.cache_write, p.cache_write),
            reasoning: d(self.reasoning, p.reasoning),
            web_search: d(self.web_search, p.web_search),
            cost_usd: self.cost_usd.map(|c| (c - p.cost_usd.unwrap_or(0.0)).max(0.0)),
        }
    }
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

/// Claude's `get_context_usage` answer, trimmed to what the panel shows.
pub fn claude_context(v: &Value) -> Value {
    let n = |x: &Value| x.as_i64().unwrap_or(0);
    let list = |arr: &Value, f: &dyn Fn(&Value) -> Value| -> Vec<Value> {
        let mut items: Vec<Value> = arr.as_array().map(|a| a.iter().map(f).collect()).unwrap_or_default();
        items.sort_by_key(|i| -n(&i["tokens"]));
        items.truncate(60);
        items
    };
    let skills = &v["skills"];
    let mb = &v["messageBreakdown"];
    json!({
        "tool": "claude",
        "model": v["model"],
        "used": n(&v["totalTokens"]),
        "max": n(&v["maxTokens"]),
        "percentage": v["percentage"],
        "auto_compact_threshold": v["autoCompactThreshold"],
        "auto_compact": v["isAutoCompactEnabled"],
        "categories": v["categories"].as_array().map(|a| a.iter().map(|c| json!({
            "id": c["id"], "name": c["name"], "tokens": n(&c["tokens"]), "kind": c["kind"], "deferred": c["isDeferred"].as_bool().unwrap_or(false),
        })).collect::<Vec<_>>()).unwrap_or_default(),
        "mcp_tools": list(&v["mcpTools"], &|t| json!({"name": t["name"], "server": t["serverName"].as_str().or(t["server_name"].as_str()), "tokens": n(&t["tokens"])})),
        "memory_files": list(&v["memoryFiles"], &|t| json!({"name": t["path"], "type": t["type"], "tokens": n(&t["tokens"])})),
        "agents": list(&v["agents"], &|t| json!({"name": t["agentType"].as_str().or(t["name"].as_str()), "source": t["source"], "tokens": n(&t["tokens"])})),
        "skills": list(&skills["skillFrontmatter"], &|t| json!({"name": t["name"], "source": t["source"], "tokens": n(&t["tokens"])})),
        "messages": if mb.is_object() { json!({
            "tool_calls": n(&mb["toolCallTokens"]),
            "tool_results": n(&mb["toolResultTokens"]),
            "attachments": n(&mb["attachmentTokens"]),
            "assistant": n(&mb["assistantMessageTokens"]),
            "user": n(&mb["userMessageTokens"]),
            "other": n(&mb["unattributedTokens"]) + n(&mb["redirectedContextTokens"]),
            "by_tool": list(&mb["toolCallsByType"], &|t| json!({"name": t["name"], "tokens": n(&t["callTokens"]) + n(&t["resultTokens"])})),
        }) } else { Value::Null },
        "updated_at": crate::util::now(),
    })
}

/// Codex only reports the size of the last request against the model's window.
pub fn codex_context(model: &str, used: i64, max: i64) -> Value {
    let max = max.max(0);
    json!({
        "tool": "codex",
        "model": model,
        "used": used,
        "max": max,
        "percentage": if max > 0 { (used as f64 * 100.0 / max as f64).round() } else { 0.0 },
        "categories": [
            {"id": "used", "name": "Conversation", "tokens": used, "kind": "used"},
            {"id": "free_space", "name": "Free space", "tokens": (max - used).max(0), "kind": "free"},
        ],
        "updated_at": crate::util::now(),
    })
}

/// Save a chat's context snapshot and tell the browsers.
pub fn save_context(st: &St, chat_id: i64, project_id: i64, ctx: Value) {
    let _ = st.db.conn().execute("UPDATE chats SET context=?2 WHERE id=?1", rusqlite::params![chat_id, ctx.to_string()]);
    st.emit_project(project_id, crate::events::Event::ChatContext { chat_id, context: ctx });
}

pub fn load_context(st: &St, chat_id: i64) -> Value {
    let s: Option<String> = st.db.conn().query_row("SELECT context FROM chats WHERE id=?1", [chat_id], |r| r.get(0)).ok().flatten();
    s.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null)
}

/// Added to the system prompt of read-only chats.
pub const READ_ONLY_PROMPT: &str = "This chat is READ-ONLY. Do not create, edit, move or delete files, do not commit, push or change git state, do not install packages, and do not run commands that change anything on this machine or other servers. Read files and run read-only commands only. When changes would help, describe them (with code) instead of making them.";

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

/// True when a shell command only reads: every part of it starts a program known to
/// be read-only (git only with reading subcommands), with no output redirection or
/// command substitution. Used to let read-only chats run commands Claude asks about.
pub fn is_read_only_command(cmd: &str) -> bool {
    const PROGRAMS: &[&str] = &[
        "ls", "cat", "head", "tail", "grep", "egrep", "fgrep", "rg", "ag", "wc", "du", "df", "pwd", "echo", "printf", "which", "whereis", "type",
        "stat", "file", "tree", "less", "more", "sort", "uniq", "cut", "tr", "column", "diff", "cmp", "md5sum", "sha256sum", "basename", "dirname",
        "realpath", "readlink", "date", "uname", "whoami", "id", "hostname", "uptime", "free", "ps", "top", "jq", "yq", "nl", "od", "xxd", "hexdump",
        "strings", "true", "false", "test", "[", "env", "printenv", "awk", "sed", "find", "git", "node", "python3", "python", "npm", "cargo", "go",
        "lsof", "ss", "netstat", "systemctl", "journalctl", "docker", "pm2", "curl",
    ];
    if cmd.contains('`') || cmd.contains("$(") || cmd.contains("<(") || cmd.contains(">(") {
        return false;
    }
    let cleaned = cmd.replace("2>&1", " ").replace(">/dev/null", " ").replace("> /dev/null", " ").replace("2>/dev/null", " ");
    if cleaned.contains('>') {
        return false;
    }
    let parts = cleaned.replace("&&", "\n").replace("||", "\n").replace([';', '|', '&'], "\n");
    for part in parts.lines() {
        let words: Vec<&str> = part.split_whitespace().skip_while(|w| w.contains('=') && !w.starts_with('-')).collect();
        let Some(prog) = words.first() else { continue };
        let prog = prog.rsplit('/').next().unwrap_or(prog);
        if !PROGRAMS.contains(&prog) {
            return false;
        }
        let args = &words[1..];
        let has = |a: &str| args.iter().any(|x| *x == a || x.starts_with(&format!("{a}=")));
        let ok = match prog {
            "find" => !["-delete", "-exec", "-execdir", "-ok", "-okdir", "-fprint", "-fprintf", "-fls"].iter().any(|f| has(f)),
            "sed" => !args.iter().any(|a| a.starts_with("-i") || *a == "--in-place") && !args.iter().any(|a| a.contains("w ") || a.ends_with('w')),
            "awk" => !args.iter().any(|a| a.contains("system") || a.contains("print >") || a.contains('>')),
            "git" => git_reads(args),
            "node" | "python3" | "python" | "cargo" | "go" => args.len() == 1 && (args[0] == "--version" || args[0] == "-v" || args[0] == "-V" || args[0] == "version"),
            "npm" => matches!(args.first(), Some(&"ls") | Some(&"list") | Some(&"view") | Some(&"outdated") | Some(&"--version") | Some(&"-v") | Some(&"config") if !has("set") && !has("delete")),
            "systemctl" => matches!(args.iter().find(|a| !a.starts_with('-')), Some(&"status") | Some(&"list-units") | Some(&"list-unit-files") | Some(&"show") | Some(&"cat") | Some(&"is-active") | Some(&"is-enabled")),
            "docker" => matches!(args.first(), Some(&"ps") | Some(&"images") | Some(&"logs") | Some(&"inspect") | Some(&"version") | Some(&"info") | Some(&"stats")),
            "pm2" => matches!(args.first(), Some(&"ls") | Some(&"list") | Some(&"status") | Some(&"logs") | Some(&"show") | Some(&"describe") | Some(&"jlist")),
            "curl" => !args.iter().any(|a| ["-X", "--request", "-d", "--data", "--data-raw", "--data-binary", "-F", "--form", "-T", "--upload-file", "-o", "--output", "-O", "--remote-name"].contains(a) || a.starts_with("-d") || a.starts_with("-X")),
            "env" | "printenv" => args.iter().all(|a| !a.contains('=') && (a.starts_with('-') || args.len() <= 1)) && args.len() <= 1,
            _ => true,
        };
        if !ok {
            return false;
        }
    }
    true
}

fn git_reads(args: &[&str]) -> bool {
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "-C" | "-c" | "--git-dir" | "--work-tree" => i += 2,
            a if a.starts_with("--git-dir=") || a.starts_with("--work-tree=") || a == "--no-pager" || a == "-P" => i += 1,
            _ => break,
        }
    }
    let Some(sub) = args.get(i) else { return true };
    let rest = &args[i + 1..];
    let has = |f: &str| rest.iter().any(|a| *a == f || a.starts_with(&format!("{f}=")));
    match *sub {
        "log" | "show" | "status" | "diff" | "blame" | "rev-parse" | "ls-files" | "ls-tree" | "cat-file" | "describe" | "shortlog" | "grep" | "rev-list"
        | "merge-base" | "name-rev" | "whatchanged" | "count-objects" | "version" | "help" => !has("--output") && !has("-o"),
        // Only listing: no names (which would create) and no changing flags.
        "branch" | "tag" => {
            const LIST: &[&str] = &["-a", "--all", "-r", "--remotes", "-v", "-vv", "--verbose", "-l", "--list", "--show-current", "--contains", "--merged", "--no-merged", "--sort", "--format", "-n"];
            rest.iter().all(|a| LIST.iter().any(|f| a == f || a.starts_with(&format!("{f}="))))
        }
        "remote" => rest.iter().all(|a| ["-v", "--verbose"].contains(a)) || matches!(rest.first(), Some(&"show") | Some(&"get-url")),
        "config" => has("--get") || has("--list") || has("-l") || has("--get-all") || has("--get-regexp"),
        "stash" => rest.first() == Some(&"list") || rest.first() == Some(&"show"),
        "reflog" => rest.is_empty() || rest.first() == Some(&"show"),
        "worktree" => rest.first() == Some(&"list"),
        _ => false,
    }
}

#[cfg(test)]
mod read_only_tests {
    use super::is_read_only_command as ro;

    #[test]
    fn reads_are_allowed() {
        for c in [
            "git -C /srv/app log --oneline",
            "git status && git diff HEAD~1",
            "ls -la | head -20",
            "grep -rn foo src 2>/dev/null | wc -l",
            "find . -name '*.rs' -type f",
            "cat package.json",
            "sed -n 1,20p src/main.rs",
            "git branch -a",
            "git remote -v",
            "node --version",
            "systemctl status nginx",
            "curl -s http://127.0.0.1:3000/health",
            "FOO=1 cat x",
        ] {
            assert!(ro(c), "{c}");
        }
    }

    #[test]
    fn writes_are_refused() {
        for c in [
            "touch a.txt",
            "echo hi > a.txt",
            "cat a >> b",
            "rm -rf build",
            "git commit -am x",
            "git checkout main",
            "git branch -D old",
            "git push",
            "find . -name x -delete",
            "find . -exec rm {} +",
            "sed -i s/a/b/ f",
            "ls; rm x",
            "ls && mv a b",
            "echo $(rm x)",
            "npm install",
            "node script.js",
            "python3 -c 'open(\"x\",\"w\")'",
            "systemctl restart api",
            "docker rm -f web",
            "curl -X POST http://x",
            "curl -o out http://x",
            "git config user.name x",
            "git stash",
            "awk '{print > \"f\"}' x",
        ] {
            assert!(!ro(c), "{c}");
        }
    }
}
