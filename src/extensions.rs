//! Skills, plugins, marketplaces, MCP servers and instruction files
//! (CLAUDE.md / AGENTS.md) for both CLIs, globally or per project.
use crate::auth::AuthUser;
use crate::settings;
use crate::state::St;
use crate::store::{can_access_project, get_project, User};
use crate::util::{tail, ApiError, ApiResult};
use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

pub fn router() -> Router<St> {
    Router::new()
        .route("/api/ext/skills", get(list_skills))
        .route("/api/ext/skill", get(read_skill).put(write_skill).delete(delete_skill))
        .route("/api/ext/plugins", get(list_plugins).post(plugin_action))
        .route("/api/ext/marketplaces", post(marketplace_action))
        .route("/api/ext/mcp", get(list_mcp).post(mcp_action))
        .route("/api/ext/instructions", get(read_instructions).put(write_instructions))
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/root".into()))
}

#[derive(Deserialize, Default)]
pub struct Scope {
    #[serde(default)]
    tool: String,
    /// "user" (global) or "project"
    #[serde(default)]
    scope: String,
    project: Option<i64>,
}

fn project_dir(st: &St, u: &User, project: Option<i64>) -> ApiResult<PathBuf> {
    let pid = project.ok_or_else(|| ApiError::bad("project required"))?;
    if !can_access_project(&st.db, u, pid) {
        return Err(ApiError::forbidden());
    }
    Ok(PathBuf::from(get_project(&st.db, pid)?.path))
}

/// Folders a tool reads skills from, for the user scope and optionally a project.
fn skill_roots(tool: &str, project: Option<&Path>) -> Vec<(String, PathBuf)> {
    let h = home();
    let mut v = Vec::new();
    if tool == "claude" {
        v.push(("user".into(), h.join(".claude/skills")));
        if let Some(p) = project {
            v.push(("project".into(), p.join(".claude/skills")));
        }
    } else {
        v.push(("user".into(), h.join(".codex/skills")));
        v.push(("user".into(), h.join(".agents/skills")));
        if let Some(p) = project {
            v.push(("project".into(), p.join(".codex/skills")));
            v.push(("project".into(), p.join(".agents/skills")));
        }
    }
    v
}

fn frontmatter(text: &str) -> (String, String) {
    let mut name = String::new();
    let mut desc = String::new();
    if let Some(rest) = text.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            for line in rest[..end].lines() {
                if let Some(v) = line.strip_prefix("name:") {
                    name = v.trim().trim_matches('"').to_string();
                } else if let Some(v) = line.strip_prefix("description:") {
                    desc = v.trim().trim_matches('"').to_string();
                }
            }
        }
    }
    (name, desc)
}

async fn list_skills(State(st): State<St>, AuthUser(u): AuthUser, Query(q): Query<Scope>) -> ApiResult<Json<Value>> {
    let pdir = match q.project {
        Some(_) => Some(project_dir(&st, &u, q.project)?),
        None => None,
    };
    let mut out = Vec::new();
    for tool in ["claude", "codex"] {
        for (scope, root) in skill_roots(tool, pdir.as_deref()) {
            let Ok(rd) = std::fs::read_dir(&root) else { continue };
            for e in rd.flatten() {
                let dir = e.path();
                let file = dir.join("SKILL.md");
                if e.file_name().to_string_lossy().starts_with('.') || !file.exists() {
                    continue;
                }
                let text = std::fs::read_to_string(&file).unwrap_or_default();
                let (name, desc) = frontmatter(&text);
                out.push(json!({
                    "tool": tool, "scope": scope,
                    "name": if name.is_empty() { e.file_name().to_string_lossy().to_string() } else { name },
                    "description": desc, "path": file.to_string_lossy(), "dir": dir.to_string_lossy(),
                }));
            }
        }
    }
    Ok(Json(Value::Array(out)))
}

/// A skill path must live directly under one of the known skill roots.
fn check_skill_path(st: &St, u: &User, path: &Path) -> ApiResult<()> {
    let mut roots = skill_roots("claude", None);
    roots.extend(skill_roots("codex", None));
    let projects = crate::store::list_projects(&st.db, u)?;
    for p in &projects {
        roots.extend(skill_roots("claude", Some(Path::new(&p.path))));
        roots.extend(skill_roots("codex", Some(Path::new(&p.path))));
    }
    let ok = roots.iter().any(|(_, r)| {
        path.strip_prefix(r).map(|rest| rest.components().count() >= 1 && !rest.to_string_lossy().contains("..")).unwrap_or(false)
    });
    if !ok {
        return Err(ApiError::bad("not a skill path"));
    }
    Ok(())
}

#[derive(Deserialize)]
struct PathQ {
    path: String,
}

async fn read_skill(State(st): State<St>, AuthUser(u): AuthUser, Query(q): Query<PathQ>) -> ApiResult<Json<Value>> {
    let p = PathBuf::from(&q.path);
    check_skill_path(&st, &u, &p)?;
    Ok(Json(json!({"path": q.path, "content": std::fs::read_to_string(&p)?})))
}

#[derive(Deserialize)]
struct WriteSkill {
    #[serde(flatten)]
    scope: Scope,
    name: String,
    content: String,
}

async fn write_skill(State(st): State<St>, AuthUser(u): AuthUser, Json(b): Json<WriteSkill>) -> ApiResult<Json<Value>> {
    let slug = crate::util::slug(&b.name);
    let base = if b.scope.scope == "project" {
        let p = project_dir(&st, &u, b.scope.project)?;
        if b.scope.tool == "codex" { p.join(".agents/skills") } else { p.join(".claude/skills") }
    } else if b.scope.tool == "codex" {
        home().join(".agents/skills")
    } else {
        home().join(".claude/skills")
    };
    let dir = base.join(&slug);
    std::fs::create_dir_all(&dir)?;
    let file = dir.join("SKILL.md");
    std::fs::write(&file, &b.content)?;
    Ok(Json(json!({"path": file.to_string_lossy()})))
}

async fn delete_skill(State(st): State<St>, AuthUser(u): AuthUser, Query(q): Query<PathQ>) -> ApiResult<Json<Value>> {
    let p = PathBuf::from(&q.path);
    let dir = if p.file_name().map(|f| f == "SKILL.md").unwrap_or(false) { p.parent().unwrap().to_path_buf() } else { p };
    check_skill_path(&st, &u, &dir)?;
    std::fs::remove_dir_all(&dir)?;
    Ok(Json(json!({"ok": true})))
}

// ---------- CLI helpers ----------

async fn cli(st: &St, tool: &str, args: &[String], cwd: Option<&Path>) -> ApiResult<(bool, String)> {
    let bin = settings::get(&st.db, if tool == "codex" { "codex_path" } else { "claude_path" });
    let mut cmd = Command::new(&bin);
    cmd.args(args).env("IS_SANDBOX", "1").stdin(std::process::Stdio::null()).kill_on_drop(true);
    cmd.current_dir(cwd.map(|p| p.to_path_buf()).unwrap_or_else(home));
    let out = tokio::time::timeout(Duration::from_secs(300), cmd.output())
        .await
        .map_err(|_| ApiError::bad("command timed out"))?
        .map_err(|e| ApiError::bad(format!("could not run {bin}: {e}")))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    Ok((out.status.success(), text))
}

fn json_or_raw(s: &str) -> Value {
    serde_json::from_str(s.trim()).unwrap_or_else(|_| json!({"raw": s.trim()}))
}

fn check_tool(t: &str) -> ApiResult<()> {
    if t != "claude" && t != "codex" {
        return Err(ApiError::bad("tool must be claude or codex"));
    }
    Ok(())
}

async fn list_plugins(State(st): State<St>, AuthUser(u): AuthUser, Query(q): Query<Scope>) -> ApiResult<Json<Value>> {
    check_tool(&q.tool)?;
    let cwd = match q.project {
        Some(_) => Some(project_dir(&st, &u, q.project)?),
        None => None,
    };
    let args: Vec<String> = ["plugin", "list", "--json", "--available"].iter().map(|s| s.to_string()).collect();
    let (_, plugins) = cli(&st, &q.tool, &args, cwd.as_deref()).await?;
    let margs: Vec<String> = if q.tool == "claude" {
        ["plugin", "marketplace", "list", "--json"].iter().map(|s| s.to_string()).collect()
    } else {
        ["plugin", "marketplace", "list"].iter().map(|s| s.to_string()).collect()
    };
    let (_, markets) = cli(&st, &q.tool, &margs, cwd.as_deref()).await?;
    Ok(Json(json!({"plugins": json_or_raw(&plugins), "marketplaces": json_or_raw(&markets)})))
}

#[derive(Deserialize)]
struct PluginAction {
    #[serde(flatten)]
    scope: Scope,
    action: String,
    plugin: String,
}

async fn plugin_action(State(st): State<St>, AuthUser(u): AuthUser, Json(b): Json<PluginAction>) -> ApiResult<Json<Value>> {
    check_tool(&b.scope.tool)?;
    let cwd = if b.scope.scope == "project" { Some(project_dir(&st, &u, b.scope.project)?) } else { None };
    let mut args: Vec<String> = vec!["plugin".into()];
    if b.scope.tool == "claude" {
        let verb = match b.action.as_str() {
            "install" | "uninstall" | "enable" | "disable" | "update" => b.action.clone(),
            _ => return Err(ApiError::bad("unknown action")),
        };
        args.push(verb);
        args.push(b.plugin.clone());
        if b.action != "update" {
            args.push("--scope".into());
            args.push(if b.scope.scope == "project" { "project".into() } else { "user".into() });
        }
        if b.action == "install" || b.action == "uninstall" {
            args.push("-y".into());
        }
    } else {
        let verb = match b.action.as_str() {
            "install" => "add",
            "uninstall" => "remove",
            _ => return Err(ApiError::bad("Codex plugins support install and uninstall")),
        };
        args.push(verb.into());
        args.push(b.plugin.clone());
    }
    let (ok, out) = cli(&st, &b.scope.tool, &args, cwd.as_deref()).await?;
    if !ok {
        return Err(ApiError::bad(tail(out.trim(), 1500)));
    }
    Ok(Json(json!({"ok": true, "output": out.trim()})))
}

#[derive(Deserialize)]
struct MarketAction {
    tool: String,
    action: String,
    /// source for add (URL, path or owner/repo), name for remove/update
    value: String,
}

async fn marketplace_action(State(st): State<St>, AuthUser(_): AuthUser, Json(b): Json<MarketAction>) -> ApiResult<Json<Value>> {
    check_tool(&b.tool)?;
    let verb = match (b.tool.as_str(), b.action.as_str()) {
        (_, "add") => "add",
        (_, "remove") => "remove",
        ("claude", "update") => "update",
        ("codex", "update") => "upgrade",
        _ => return Err(ApiError::bad("unknown action")),
    };
    let args = vec!["plugin".to_string(), "marketplace".into(), verb.into(), b.value.trim().to_string()];
    let (ok, out) = cli(&st, &b.tool, &args, None).await?;
    if !ok {
        return Err(ApiError::bad(tail(out.trim(), 1500)));
    }
    Ok(Json(json!({"ok": true, "output": out.trim()})))
}

async fn list_mcp(State(st): State<St>, AuthUser(u): AuthUser, Query(q): Query<Scope>) -> ApiResult<Json<Value>> {
    check_tool(&q.tool)?;
    let cwd = match q.project {
        Some(_) => Some(project_dir(&st, &u, q.project)?),
        None => None,
    };
    if q.tool == "codex" {
        let (_, out) = cli(&st, "codex", &["mcp".into(), "list".into(), "--json".into()], cwd.as_deref()).await?;
        return Ok(Json(json!({"servers": json_or_raw(&out)})));
    }
    // `claude mcp list` has no JSON output: read the config files it uses.
    let mut servers = Vec::new();
    let cfg: Value = std::fs::read_to_string(home().join(".claude.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
    if let Some(m) = cfg["mcpServers"].as_object() {
        for (name, v) in m {
            servers.push(json!({"name": name, "scope": "user", "config": v}));
        }
    }
    if let Some(p) = &cwd {
        if let Some(m) = cfg["projects"][p.to_string_lossy().as_ref()]["mcpServers"].as_object() {
            for (name, v) in m {
                servers.push(json!({"name": name, "scope": "local", "config": v}));
            }
        }
        let pj: Value = std::fs::read_to_string(p.join(".mcp.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
        if let Some(m) = pj["mcpServers"].as_object() {
            for (name, v) in m {
                servers.push(json!({"name": name, "scope": "project", "config": v}));
            }
        }
    }
    Ok(Json(json!({"servers": servers})))
}

#[derive(Deserialize)]
struct McpAction {
    #[serde(flatten)]
    scope: Scope,
    action: String,
    name: String,
    /// stdio: command + args; http: url
    #[serde(default)]
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    url: String,
    #[serde(default)]
    env: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    headers: std::collections::BTreeMap<String, String>,
}

async fn mcp_action(State(st): State<St>, AuthUser(u): AuthUser, Json(b): Json<McpAction>) -> ApiResult<Json<Value>> {
    check_tool(&b.scope.tool)?;
    let project_scoped = b.scope.scope == "project" || b.scope.scope == "local";
    let cwd = if project_scoped { Some(project_dir(&st, &u, b.scope.project)?) } else { None };
    let name = b.name.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::bad("name required"));
    }
    let args: Vec<String> = if b.scope.tool == "claude" {
        let scope = if project_scoped { b.scope.scope.clone() } else { "user".into() };
        match b.action.as_str() {
            "add" => {
                let cfg = if !b.url.trim().is_empty() {
                    json!({"type": "http", "url": b.url.trim(), "headers": b.headers})
                } else {
                    json!({"type": "stdio", "command": b.command.trim(), "args": b.args, "env": b.env})
                };
                vec!["mcp".into(), "add-json".into(), name, cfg.to_string(), "-s".into(), scope]
            }
            "remove" => vec!["mcp".into(), "remove".into(), name, "-s".into(), scope],
            _ => return Err(ApiError::bad("unknown action")),
        }
    } else {
        match b.action.as_str() {
            "add" => {
                let mut a: Vec<String> = vec!["mcp".into(), "add".into(), name];
                for (k, v) in &b.env {
                    a.push("--env".into());
                    a.push(format!("{k}={v}"));
                }
                if !b.url.trim().is_empty() {
                    a.push("--url".into());
                    a.push(b.url.trim().into());
                } else {
                    a.push("--".into());
                    a.push(b.command.trim().into());
                    a.extend(b.args.iter().cloned());
                }
                a
            }
            "remove" => vec!["mcp".into(), "remove".into(), name],
            _ => return Err(ApiError::bad("unknown action")),
        }
    };
    let (ok, out) = cli(&st, &b.scope.tool, &args, cwd.as_deref()).await?;
    if !ok {
        return Err(ApiError::bad(tail(out.trim(), 1500)));
    }
    if b.scope.tool == "codex" {
        st.codex.stop().await;
    }
    Ok(Json(json!({"ok": true, "output": out.trim()})))
}

fn instructions_path(st: &St, u: &User, q: &Scope) -> ApiResult<PathBuf> {
    check_tool(&q.tool)?;
    let file = if q.tool == "claude" { "CLAUDE.md" } else { "AGENTS.md" };
    Ok(if q.scope == "project" {
        project_dir(st, u, q.project)?.join(file)
    } else if q.tool == "claude" {
        home().join(".claude").join(file)
    } else {
        home().join(".codex").join(file)
    })
}

async fn read_instructions(State(st): State<St>, AuthUser(u): AuthUser, Query(q): Query<Scope>) -> ApiResult<Json<Value>> {
    let p = instructions_path(&st, &u, &q)?;
    Ok(Json(json!({"path": p.to_string_lossy(), "content": std::fs::read_to_string(&p).unwrap_or_default(), "exists": p.exists()})))
}

#[derive(Deserialize)]
struct WriteInstr {
    #[serde(flatten)]
    scope: Scope,
    content: String,
}

async fn write_instructions(State(st): State<St>, AuthUser(u): AuthUser, Json(b): Json<WriteInstr>) -> ApiResult<Json<Value>> {
    let p = instructions_path(&st, &u, &b.scope)?;
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&p, b.content)?;
    Ok(Json(json!({"ok": true, "path": p.to_string_lossy()})))
}
