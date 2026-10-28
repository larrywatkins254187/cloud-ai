//! "Set up deploy with AI": Claude Code or Codex reads the project (read-only),
//! together with a snapshot of the server it runs on, and proposes a deploy
//! command and a health check URL. `check` then verifies a configuration
//! without deploying anything.
use crate::auth::AdminUser;
use crate::settings;
use crate::state::St;
use crate::store::{self, get_project, Project, Server};
use crate::util::{shell_quote, tail, truncate, ApiError, ApiResult};
use anyhow::{anyhow, Result};
use axum::extract::{Path as AxPath, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

pub fn router() -> Router<St> {
    Router::new()
        .route("/api/projects/{id}/deploy-assist", post(start))
        .route("/api/projects/{id}/deploy-check", post(check))
        .route("/api/deploy-assist/{job}", get(job_status))
        .route("/api/deploy-assist/{job}/stop", post(stop))
}

/// The deploy settings being edited (they may not be saved yet).
#[derive(Deserialize, Clone, Debug, Default)]
pub struct Target {
    #[serde(default)]
    pub deploy_target: String,
    pub server_id: Option<i64>,
    #[serde(default)]
    pub remote_path: String,
    #[serde(default)]
    pub deploy_method: String,
    #[serde(default)]
    pub deploy_command: String,
    #[serde(default)]
    pub health_url: String,
    #[serde(default)]
    pub prod_url: String,
}

#[derive(Serialize, Clone, Default)]
struct Job {
    id: u64,
    project_id: i64,
    tool: String,
    /// running | done | error | stopped
    status: String,
    log: Vec<String>,
    result: Option<Value>,
    error: Option<String>,
    #[serde(skip)]
    pid: Option<u32>,
}

fn jobs() -> &'static Mutex<HashMap<u64, Job>> {
    static J: OnceLock<Mutex<HashMap<u64, Job>>> = OnceLock::new();
    J.get_or_init(Default::default)
}

static NEXT: AtomicU64 = AtomicU64::new(1);

fn update(id: u64, f: impl FnOnce(&mut Job)) {
    if let Some(j) = jobs().lock().unwrap().get_mut(&id) {
        f(j);
    }
}

fn log(id: u64, line: impl Into<String>) {
    let line = line.into();
    if line.trim().is_empty() {
        return;
    }
    update(id, |j| {
        j.log.push(truncate(&line, 400));
        if j.log.len() > 300 {
            j.log.drain(..50);
        }
    });
}

#[derive(Deserialize)]
pub struct StartBody {
    tool: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    effort: String,
    #[serde(flatten)]
    target: Target,
    /// Anything the admin wants the agent to know ("it runs with pm2 as api").
    #[serde(default)]
    notes: String,
}

async fn start(State(st): State<St>, AdminUser(_u): AdminUser, AxPath(pid): AxPath<i64>, Json(b): Json<StartBody>) -> ApiResult<Json<Value>> {
    let project = get_project(&st.db, pid)?;
    crate::chats::validate_tool(&b.tool).map_err(|e| ApiError::bad(e.to_string()))?;
    if !["local", "remote"].contains(&b.target.deploy_target.as_str()) {
        return Err(ApiError::bad("choose where the project runs first"));
    }
    if b.target.deploy_target == "remote" && (b.target.server_id.is_none() || b.target.remote_path.trim().is_empty()) {
        return Err(ApiError::bad("choose the server and the path on it first"));
    }
    let running = jobs().lock().unwrap().values().any(|j| j.project_id == pid && j.status == "running");
    if running {
        return Err(ApiError::bad("the AI is already working on this project's deploy settings"));
    }
    let id = NEXT.fetch_add(1, Ordering::SeqCst);
    jobs().lock().unwrap().insert(id, Job { id, project_id: pid, tool: b.tool.clone(), status: "running".into(), ..Default::default() });
    let st2 = st.clone();
    tokio::spawn(async move {
        let res = run_job(&st2, id, &project, &b).await;
        update(id, |j| {
            if j.status != "running" {
                return;
            }
            match res {
                Ok(v) => {
                    j.status = "done".into();
                    j.result = Some(v);
                }
                Err(e) => {
                    j.status = "error".into();
                    j.error = Some(format!("{e:#}"));
                }
            }
        });
    });
    Ok(Json(json!({"id": id})))
}

async fn job_status(AdminUser(_u): AdminUser, AxPath(id): AxPath<u64>) -> ApiResult<Json<Value>> {
    let j = jobs().lock().unwrap().get(&id).cloned().ok_or_else(|| ApiError::bad("this AI run is gone (the panel restarted?)"))?;
    Ok(Json(serde_json::to_value(j).unwrap()))
}

async fn stop(AdminUser(_u): AdminUser, AxPath(id): AxPath<u64>) -> ApiResult<Json<Value>> {
    let mut pid = None;
    update(id, |j| {
        if j.status == "running" {
            j.status = "stopped".into();
            pid = j.pid;
        }
    });
    if let Some(p) = pid {
        let _ = std::process::Command::new("kill").arg(p.to_string()).status();
    }
    Ok(Json(json!({"ok": true})))
}

/// Read-only look at the machine the project runs on: tools, services, ports and
/// web server config. Collected by the panel, so the agent needs no SSH access.
/// Lines that may hold secrets (unit `Environment=`, passwords, tokens) are left out.
fn snapshot_script(path: &str) -> String {
    let p = shell_quote(path);
    format!(
        r###"exec 2>/dev/null
echo "## host"; uname -a; head -3 /etc/os-release
echo "## folder {p}"; if cd {p}; then pwd; ls -la | head -60; echo "-- git"; git remote -v | head -4; git log -1 --oneline; git status --short | head -20; else echo "missing"; fi
echo "## tools"; for t in node npm pnpm yarn bun deno python3 pip uv poetry go cargo java mvn gradle php composer ruby bundle docker docker-compose podman pm2 systemctl supervisorctl nginx caddy apache2 pg_ctl mysql redis-server; do command -v $t >/dev/null && echo "$t $(command -v $t)"; done
echo "## running services"; systemctl list-units --type=service --state=running --no-pager --no-legend | awk '{{print $1}}' | head -80
echo "## pm2"; pm2 ls --no-color | head -40
echo "## docker"; docker ps --format '{{{{.Names}}}}  {{{{.Image}}}}  {{{{.Ports}}}}  {{{{.Status}}}}' | head -40
echo "## listening ports"; ss -ltnp | head -60
echo "## nginx/caddy"; grep -RhE '^\s*(server_name|listen|proxy_pass|root)\s' /etc/nginx/sites-enabled /etc/nginx/conf.d | head -80; grep -RhE 'reverse_proxy|^\S+\s*\{{' /etc/caddy/Caddyfile | head -30
echo "## systemd units using this folder"; for f in $(grep -rl {p} /etc/systemd/system | head -5); do echo "--- $f"; head -40 "$f" | grep -viE 'environment|password|secret|token|apikey|api_key'; done
echo "## pm2 ecosystem / compose files"; ls {p}/ecosystem.config.* {p}/docker-compose*.y*ml {p}/compose*.y*ml {p}/Dockerfile {p}/Procfile
true"###
    )
}

async fn snapshot(st: &St, t: &Target, project: &Project) -> String {
    let (script, label) = if t.deploy_target == "remote" {
        (snapshot_script(&t.remote_path), "the deploy server")
    } else {
        (snapshot_script(&project.path), "this server")
    };
    let out = if t.deploy_target == "remote" {
        let Some(server) = t.server_id.and_then(|id| store::get_server(&st.db, id).ok()) else {
            return "(no server selected)".into();
        };
        match crate::deploy::ssh_cmd(st, &server, &script) {
            Ok(mut c) => tokio::time::timeout(Duration::from_secs(60), c.stdin(Stdio::null()).output()).await,
            Err(e) => return format!("(could not connect: {e})"),
        }
    } else {
        tokio::time::timeout(Duration::from_secs(60), Command::new("bash").arg("-c").arg(&script).stdin(Stdio::null()).output()).await
    };
    match out {
        Ok(Ok(o)) => {
            let mut s = String::from_utf8_lossy(&o.stdout).to_string();
            if !o.status.success() && s.trim().is_empty() {
                s = format!("(could not read {label}: {})", String::from_utf8_lossy(&o.stderr).trim());
            }
            truncate(&s, 24_000)
        }
        Ok(Err(e)) => format!("(could not read {label}: {e})"),
        Err(_) => format!("(reading {label} timed out)"),
    }
}

fn prompt(st: &St, project: &Project, t: &Target, server: Option<&Server>, snap: &str, notes: &str) -> String {
    let how = if t.deploy_target == "remote" {
        let s = server.map(|s| format!("{} ({}@{})", s.name, s.username, s.host)).unwrap_or_default();
        let ship = if t.deploy_method == "rsync" {
            "the panel copies this folder to the server with `rsync --delete` (excluded paths are kept), then runs the deploy command there"
        } else {
            "the panel runs `git push origin <branch>` here, then on the server runs `cd <path> && git fetch --all --prune && git reset --hard <commit>`, then the deploy command"
        };
        format!(
            "The project runs on ANOTHER server, {s}, in `{}`. On each deploy {ship}. The deploy command runs there with bash (`set -e`, in `{}`, $DEPLOY_REF = commit).",
            t.remote_path, t.remote_path
        )
    } else {
        format!(
            "The project runs on THIS server, from this folder (`{}`). On each deploy the panel optionally commits pending changes, then runs the deploy command with `bash -lc` in this folder ($DEPLOY_REF = commit). Nothing is copied anywhere: the command itself must build and (re)start the running app.",
            project.path
        )
    };
    let prod = if t.prod_url.is_empty() { project.prod_url.clone() } else { t.prod_url.clone() };
    let current = if t.deploy_command.trim().is_empty() { "(none yet)".to_string() } else { t.deploy_command.clone() };
    let extra = settings::get(&st.db, "extra_system_prompt");
    format!(
        r#"You are configuring automatic deploys for the project in the current folder ("{name}"), for the cloud-ai panel.

{how}

After the command finishes, the panel sends an HTTP GET to the health check URL FROM THIS PANEL SERVER and retries for about a minute; a 2xx/3xx answer means healthy, anything else can trigger an automatic rollback.
Production URL (if known): {prod}
Current deploy command: {current}
Current health check URL: {health}
Notes from the admin: {notes}
{extra}

Work in read-only mode: read files and run read-only commands only; do not change, install, build, start or stop anything.
1. Understand how the project is built and run: package.json scripts, lockfiles, Dockerfile/compose, Makefile, Procfile, pm2 ecosystem, systemd units, CI workflows, framework config, README, env examples.
2. Use the snapshot of the machine it runs on (below) to see how it is ACTUALLY run there now (which service, pm2 process, container or port), and keep that the same.
3. Write a deploy command that is safe to run repeatedly: install dependencies from the lockfile, build, apply migrations if the project has them, and restart the existing service/process (by its real name). Use `&&` or new lines; it runs with `set -e` on remote servers. Do not use sudo when it is not needed (it runs as {user} on the target).
4. Pick a health check URL that this panel server can reach: a health/status endpoint if the app has one, otherwise the home page or an API route that returns 200 without login. For an app on this server, http://127.0.0.1:<port>/... is fine; for another server use its public URL.

Snapshot of the machine the project runs on:
```
{snap}
```

Finish with ONLY this JSON object in a ```json code block (no text after it):
{{"deploy_command": "…", "health_url": "…", "summary": "two or three sentences: how the app is run and what the command does", "notes": ["assumptions, risks or things the admin should check"]}}"#,
        name = project.name,
        health = if t.health_url.is_empty() { "(none)" } else { &t.health_url },
        notes = if notes.trim().is_empty() { "(none)" } else { notes.trim() },
        extra = if extra.trim().is_empty() { String::new() } else { format!("\nPanel instructions: {}\n", extra.trim()) },
        user = server.map(|s| s.username.as_str()).unwrap_or("the panel's user"),
    )
}

/// The last JSON object in the agent's answer (preferably inside a ```json block).
pub fn extract_json(text: &str) -> Option<Value> {
    if let Some(i) = text.rfind("```json") {
        let rest = &text[i + 7..];
        let body = rest.split("```").next().unwrap_or(rest);
        if let Ok(v) = serde_json::from_str::<Value>(body.trim()) {
            return Some(v);
        }
    }
    let mut start = text.rfind('{');
    while let Some(s) = start {
        if let Some(end) = text.rfind('}') {
            if end > s {
                if let Ok(v) = serde_json::from_str::<Value>(&text[s..=end]) {
                    if v.get("deploy_command").is_some() {
                        return Some(v);
                    }
                }
            }
        }
        start = text[..s].rfind('{');
    }
    None
}

fn describe_claude_tool(b: &Value) -> Option<String> {
    let i = &b["input"];
    let name = b["name"].as_str()?;
    let arg = i["file_path"].as_str().or(i["command"].as_str()).or(i["pattern"].as_str()).or(i["path"].as_str()).unwrap_or("");
    Some(format!("{name} {}", truncate(arg, 160)))
}

async fn run_job(st: &St, id: u64, project: &Project, b: &StartBody) -> Result<Value> {
    let t = &b.target;
    let server = t.server_id.and_then(|sid| store::get_server(&st.db, sid).ok());
    log(id, if t.deploy_target == "remote" { "Reading the deploy server (read-only)…" } else { "Reading this server (read-only)…" });
    let snap = snapshot(st, t, project).await;
    log(id, format!("Snapshot: {} lines", snap.lines().count()));
    let prompt = prompt(st, project, t, server.as_ref(), &snap, &b.notes);
    let model = (!b.model.is_empty() && b.model != "default").then(|| b.model.clone());
    let text = if b.tool == "codex" {
        run_codex(st, id, project, &prompt, model.as_deref(), &b.effort).await?
    } else {
        run_claude(st, id, project, &prompt, model.as_deref(), &b.effort).await?
    };
    let v = extract_json(&text).ok_or_else(|| anyhow!("the AI did not return a deploy command. Its answer:\n{}", tail(&text, 2000)))?;
    let cmd = v["deploy_command"].as_str().unwrap_or("").trim().to_string();
    if cmd.is_empty() {
        return Err(anyhow!("the AI returned an empty deploy command. {}", v["summary"].as_str().unwrap_or("")));
    }
    log(id, "Done.");
    Ok(json!({
        "deploy_command": cmd,
        "health_url": v["health_url"].as_str().unwrap_or("").trim(),
        "summary": v["summary"],
        "notes": v["notes"],
    }))
}

async fn run_claude(st: &St, id: u64, project: &Project, prompt: &str, model: Option<&str>, effort: &str) -> Result<String> {
    let bin = settings::get(&st.db, "claude_path");
    let mut c = Command::new(&bin);
    c.args(["-p", "--output-format", "stream-json", "--verbose", "--permission-mode", "default"])
        .args(["--disallowedTools", crate::agents::claude::READ_ONLY_DISALLOWED])
        .args(["--append-system-prompt", crate::agents::READ_ONLY_PROMPT]);
    if let Some(m) = model {
        c.args(["--model", m]);
    }
    if !effort.is_empty() {
        c.args(["--effort", effort]);
    }
    c.arg(prompt).current_dir(&project.path).env("IS_SANDBOX", "1");
    let mut result = String::new();
    let mut last_text = String::new();
    stream_json(id, c, &bin, |v| {
        match v["type"].as_str().unwrap_or("") {
            "assistant" => {
                for b in v["message"]["content"].as_array().cloned().unwrap_or_default() {
                    match b["type"].as_str() {
                        Some("tool_use") => {
                            if let Some(d) = describe_claude_tool(&b) {
                                log(id, d);
                            }
                        }
                        Some("text") => last_text = b["text"].as_str().unwrap_or("").to_string(),
                        _ => {}
                    }
                }
            }
            "result" => result = v["result"].as_str().unwrap_or("").to_string(),
            _ => {}
        }
    })
    .await?;
    Ok(if result.trim().is_empty() { last_text } else { result })
}

async fn run_codex(st: &St, id: u64, project: &Project, prompt: &str, model: Option<&str>, effort: &str) -> Result<String> {
    let bin = settings::get(&st.db, "codex_path");
    let out_file = std::env::temp_dir().join(format!("cloud-ai-deploy-assist-{id}.txt"));
    let mut c = Command::new(&bin);
    c.args(["exec", "--json", "--sandbox", "read-only", "--skip-git-repo-check", "-C", &project.path])
        .arg("--output-last-message")
        .arg(&out_file);
    if let Some(m) = model {
        c.args(["-m", m]);
    }
    if !effort.is_empty() {
        c.args(["-c", &format!("model_reasoning_effort=\"{effort}\"")]);
    }
    c.arg(format!("{}\n\n{}", crate::agents::READ_ONLY_PROMPT, prompt)).current_dir(&project.path);
    let mut last = String::new();
    stream_json(id, c, &bin, |v| {
        let item = &v["item"];
        if v["type"] == "item.started" || v["type"] == "item.completed" {
            match item["type"].as_str().unwrap_or("") {
                "command_execution" if v["type"] == "item.started" => log(id, format!("$ {}", truncate(item["command"].as_str().unwrap_or(""), 160))),
                "agent_message" if v["type"] == "item.completed" => last = item["text"].as_str().unwrap_or("").to_string(),
                _ => {}
            }
        }
    })
    .await?;
    let text = tokio::fs::read_to_string(&out_file).await.unwrap_or_default();
    let _ = tokio::fs::remove_file(&out_file).await;
    Ok(if text.trim().is_empty() { last } else { text })
}

/// Run an agent CLI that prints JSON lines, feeding each to `on`, for up to 15 minutes.
async fn stream_json(id: u64, mut c: Command, bin: &str, mut on: impl FnMut(&Value)) -> Result<()> {
    c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = c.spawn().map_err(|e| anyhow!("starting `{bin}`: {e} (is it installed? set its path in Settings)"))?;
    let pid = child.id();
    update(id, |j| j.pid = pid);
    let stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let err_task = tokio::spawn(async move {
        let mut s = String::new();
        let _ = tokio::io::AsyncReadExt::read_to_string(&mut stderr, &mut s).await;
        s
    });
    let mut lines = BufReader::with_capacity(1 << 16, stdout).lines();
    let read = async {
        while let Ok(Some(l)) = lines.next_line().await {
            if let Ok(v) = serde_json::from_str::<Value>(&l) {
                on(&v);
            }
        }
    };
    if tokio::time::timeout(Duration::from_secs(15 * 60), read).await.is_err() {
        let _ = child.start_kill();
        return Err(anyhow!("the AI took longer than 15 minutes"));
    }
    let status = child.wait().await?;
    let err = err_task.await.unwrap_or_default();
    let stopped = jobs().lock().unwrap().get(&id).map(|j| j.status == "stopped").unwrap_or(false);
    if stopped {
        return Err(anyhow!("stopped"));
    }
    if !status.success() {
        return Err(anyhow!("`{bin}` failed: {}", tail(err.trim(), 1500)));
    }
    Ok(())
}

// ---------- checking a configuration without deploying ----------

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Check {
    pub name: String,
    /// ok | warn | fail
    pub status: String,
    pub detail: String,
}

fn ck(name: &str, status: &str, detail: impl Into<String>) -> Check {
    Check { name: name.into(), status: status.into(), detail: detail.into() }
}

/// Programs a deploy command starts (first word of each simple command),
/// skipping shell builtins and variable assignments.
pub fn command_programs(cmd: &str) -> Vec<String> {
    const BUILTINS: &[&str] = &[
        "cd", "export", "set", "unset", "echo", "printf", "source", ".", "true", "false", "test", "[", "[[", "if", "then", "else", "elif", "fi",
        "for", "do", "done", "while", "until", "case", "esac", "exit", "return", "local", "read", "eval", "exec", "trap", "shift", "wait", "sleep",
        "{", "}", "(", ")", "!", "function", "in", "time",
    ];
    let mut out: Vec<String> = Vec::new();
    let normalized = cmd.replace("&&", "\n").replace("||", "\n").replace(['|', ';'], "\n");
    for part in normalized.lines() {
        let mut words = part.split_whitespace().peekable();
        while let Some(w) = words.peek() {
            let w = w.trim_start_matches(['(', '{']);
            if w.contains('=') && !w.starts_with('-') && !w.starts_with('=') || w == "sudo" || w == "env" || w == "nohup" || w.is_empty() {
                words.next();
            } else {
                break;
            }
        }
        if let Some(w) = words.next() {
            let w = w.trim_start_matches(['(', '{']).to_string();
            if w.is_empty() || w.starts_with('#') || w.starts_with('$') || w.starts_with('-') || BUILTINS.contains(&w.as_str()) {
                continue;
            }
            if !out.contains(&w) {
                out.push(w);
            }
        }
    }
    out
}

fn checks_script(path: &str, cmd: &str, git: bool) -> String {
    let progs = command_programs(cmd).iter().filter(|p| !p.contains('/')).map(|p| shell_quote(p)).collect::<Vec<_>>().join(" ");
    let mut s = format!("cd {} 2>/dev/null && echo DIR_OK || echo DIR_MISSING\n", shell_quote(path));
    s.push_str(&format!("bash -n -c {} 2>&1 && echo SYNTAX_OK || echo SYNTAX_BAD\n", shell_quote(cmd)));
    if git {
        s.push_str("git rev-parse --is-inside-work-tree >/dev/null 2>&1 && echo GIT_OK || echo GIT_MISSING\n");
        s.push_str("git status --porcelain 2>/dev/null | head -5 | sed 's/^/DIRTY /'\n");
    }
    if !progs.is_empty() {
        s.push_str(&format!("for p in {progs}; do command -v \"$p\" >/dev/null 2>&1 && echo \"HAS $p\" || echo \"MISSING $p\"; done\n"));
    }
    s
}

fn parse_checks(out: &str, git: bool, where_: &str) -> Vec<Check> {
    let mut v = Vec::new();
    v.push(if out.contains("DIR_OK") { ck("Folder", "ok", format!("exists on {where_}")) } else { ck("Folder", "fail", format!("not found on {where_}")) });
    if out.contains("SYNTAX_OK") {
        v.push(ck("Command syntax", "ok", "bash can parse the deploy command"));
    } else {
        let err: Vec<&str> = out.lines().filter(|l| l.contains("syntax") || l.contains("unexpected")).collect();
        v.push(ck("Command syntax", "fail", if err.is_empty() { "bash cannot parse the deploy command".into() } else { err.join("\n") }));
    }
    if git {
        if out.contains("GIT_OK") {
            let dirty: Vec<&str> = out.lines().filter_map(|l| l.strip_prefix("DIRTY ")).collect();
            v.push(if dirty.is_empty() {
                ck("Git checkout", "ok", "the folder is a clean git checkout")
            } else {
                ck("Git checkout", "warn", format!("has local changes that `git reset --hard` will discard: {}", dirty.join(", ")))
            });
        } else {
            v.push(ck("Git checkout", "fail", "the folder is not a git checkout, so `git pull` deploys cannot work (use rsync or clone it there)"));
        }
    }
    let missing: Vec<&str> = out.lines().filter_map(|l| l.strip_prefix("MISSING ")).collect();
    let has: Vec<&str> = out.lines().filter_map(|l| l.strip_prefix("HAS ")).collect();
    if !missing.is_empty() {
        v.push(ck("Programs", "fail", format!("not found on {where_}: {}", missing.join(", "))));
    } else if !has.is_empty() {
        v.push(ck("Programs", "ok", format!("found: {}", has.join(", "))));
    }
    v
}

async fn health(url: &str) -> Check {
    if url.trim().is_empty() {
        return ck("Health check", "warn", "no URL: failed deploys cannot be detected or rolled back");
    }
    let client = match reqwest::Client::builder().timeout(Duration::from_secs(10)).build() {
        Ok(c) => c,
        Err(e) => return ck("Health check", "fail", e.to_string()),
    };
    match client.get(url.trim()).send().await {
        Ok(r) if r.status().is_success() || r.status().is_redirection() => ck("Health check", "ok", format!("{url} answered HTTP {}", r.status().as_u16())),
        Ok(r) => ck("Health check", "warn", format!("{url} answered HTTP {} now; it must answer 2xx/3xx after a deploy", r.status().as_u16())),
        Err(e) => ck("Health check", "warn", format!("{url} is not reachable now ({e}); fine if the app is not running yet")),
    }
}

pub async fn run_checks(st: &St, project: &Project, t: &Target) -> Result<Vec<Check>> {
    let mut out = Vec::new();
    match t.deploy_target.as_str() {
        "local" => {
            let script = checks_script(&project.path, &t.deploy_command, false);
            let o = Command::new("bash").arg("-lc").arg(&script).stdin(Stdio::null()).output().await?;
            out.extend(parse_checks(&String::from_utf8_lossy(&o.stdout), false, "this server"));
        }
        "remote" => {
            let server = t.server_id.and_then(|id| store::get_server(&st.db, id).ok()).ok_or_else(|| anyhow!("choose a server first"))?;
            match crate::deploy::test_server(st, &server).await {
                Ok(_) => out.push(ck("SSH", "ok", format!("connected to {}@{}", server.username, server.host))),
                Err(e) => {
                    out.push(ck("SSH", "fail", format!("{e:#}")));
                    out.push(health(&t.health_url).await);
                    return Ok(out);
                }
            }
            if t.remote_path.trim().is_empty() {
                out.push(ck("Folder", "fail", "the path on the server is not set"));
            } else {
                let git = t.deploy_method != "rsync";
                let script = checks_script(&t.remote_path, &t.deploy_command, git);
                let o = tokio::time::timeout(Duration::from_secs(60), crate::deploy::ssh_cmd(st, &server, &script)?.stdin(Stdio::null()).output())
                    .await
                    .map_err(|_| anyhow!("the server did not answer in time"))??;
                out.extend(parse_checks(&String::from_utf8_lossy(&o.stdout), git, &server.name));
                if git {
                    let root = std::path::Path::new(&project.path);
                    let has_origin = crate::git::run(root, &["remote", "get-url", "origin"]).await.map(|o| o.ok).unwrap_or(false);
                    out.push(if has_origin {
                        ck("git push", "ok", "this project has an `origin` remote to push to")
                    } else {
                        ck("git push", "fail", "this project has no `origin` remote, so git deploys cannot push")
                    });
                }
            }
        }
        _ => return Err(anyhow!("choose where the project runs first")),
    }
    if t.deploy_command.trim().is_empty() {
        out.push(ck("Deploy command", "warn", "empty: the deploy only ships code and restarts nothing"));
    }
    out.push(health(&t.health_url).await);
    Ok(out)
}

async fn check(State(st): State<St>, AdminUser(_u): AdminUser, AxPath(pid): AxPath<i64>, Json(t): Json<Target>) -> ApiResult<Json<Value>> {
    let project = get_project(&st.db, pid)?;
    let checks = run_checks(&st, &project, &t).await.map_err(|e| ApiError::bad(format!("{e:#}")))?;
    let ok = checks.iter().all(|c| c.status != "fail");
    Ok(Json(json!({"ok": ok, "checks": checks})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programs_of_a_command() {
        let p = command_programs("cd app && npm ci && NODE_ENV=production npm run build\nsudo systemctl restart api; pm2 reload web || true");
        assert_eq!(p, vec!["npm", "systemctl", "pm2"]);
        assert_eq!(command_programs("docker compose up -d --build | tee log"), vec!["docker", "tee"]);
        assert!(command_programs("# just a comment").is_empty());
    }

    #[test]
    fn json_from_answer() {
        let t = "I looked around.\n```json\n{\"deploy_command\": \"make deploy\", \"health_url\": \"http://127.0.0.1:3000/\"}\n```";
        assert_eq!(extract_json(t).unwrap()["deploy_command"], "make deploy");
        let t = "Result: {\"deploy_command\": \"x\", \"notes\": [\"{a}\"]} done";
        assert_eq!(extract_json(t).unwrap()["deploy_command"], "x");
        assert!(extract_json("no json here").is_none());
    }

    #[test]
    fn checks_parse() {
        let out = "DIR_OK\nSYNTAX_OK\nGIT_OK\nDIRTY  M a.txt\nHAS npm\nMISSING pm2\n";
        let c = parse_checks(out, true, "prod");
        assert_eq!(c[0].status, "ok");
        assert_eq!(c[1].status, "ok");
        assert_eq!(c[2].status, "warn");
        assert_eq!(c[3].status, "fail");
        assert!(c[3].detail.contains("pm2"));
    }
}

#[cfg(test)]
mod script_tests {
    #[test]
    fn snapshot_script_runs() {
        let s = super::snapshot_script("/tmp/it's here");
        let o = std::process::Command::new("bash").arg("-n").arg("-c").arg(&s).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let c = super::checks_script("/tmp", "npm ci && pm2 reload x", true);
        let o = std::process::Command::new("bash").arg("-c").arg(&c).output().unwrap();
        let out = String::from_utf8_lossy(&o.stdout);
        assert!(out.contains("DIR_OK") && out.contains("SYNTAX_OK"), "{out}");
    }
}
