//! Claude Code driven through its stream-json stdio protocol (the same one the
//! Agent SDK uses): one long-lived `claude -p` process per active chat.
use super::{claude_permission_mode, prompt_with_attachments, question, system_prompt, Attachment, ModelUsage, TurnOutcome};
use crate::events::Event;
use crate::runner;
use crate::settings;
use crate::state::St;
use crate::store::{self, insert_message, Chat, Project, Run};
use crate::util::{tail, truncate};
use anyhow::{Context, Result};
use base64::Engine;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};

struct Turn {
    run_id: i64,
    done: Option<oneshot::Sender<TurnOutcome>>,
    last_text: String,
}

pub struct ClaudeSession {
    chat_id: i64,
    project_id: i64,
    /// model|effort|mode the process was started with; a change restarts it.
    spawn_key: String,
    stdin: mpsc::UnboundedSender<String>,
    child: Mutex<Option<Child>>,
    turn: Mutex<Option<Turn>>,
    alive: AtomicBool,
    last_used: Mutex<Instant>,
    total_cost: Mutex<f64>,
    /// Last cumulative `modelUsage` per model, to get each turn's share.
    model_usage: Mutex<HashMap<String, ModelUsage>>,
    /// Model named in the latest assistant message (fallback when `modelUsage` is missing).
    last_model: Mutex<String>,
    stderr: Mutex<String>,
    ctrl_seq: AtomicU64,
    /// Sub-agent tool_use id ↔ Claude task id (used by `stop_task`).
    tasks: Mutex<Tasks>,
    /// PID of the Claude process (to find the processes of its background tasks).
    pid: Option<u32>,
    /// Background Bash / Monitor calls waiting for their task id: tool_use id → (tool, command, description).
    bg_calls: Mutex<HashMap<String, (String, String, String)>>,
    /// Background shells and monitors of this process: task id → info.
    bg: Mutex<HashMap<String, BgTask>>,
}

/// A background shell command or monitor started by Claude Code.
#[derive(Clone, Debug)]
pub struct BgTask {
    pub command: String,
    pub running: bool,
    pub output_file: Option<String>,
}

#[derive(Default)]
struct Tasks {
    by_ref: HashMap<String, String>,
    by_task: HashMap<String, String>,
}

impl Tasks {
    fn link(&mut self, tool_use_id: &str, task_id: &str) {
        if tool_use_id.is_empty() || task_id.is_empty() {
            return;
        }
        self.by_ref.insert(tool_use_id.to_string(), task_id.to_string());
        self.by_task.insert(task_id.to_string(), tool_use_id.to_string());
    }
}

/// The agent id Claude reports in a Task/Agent tool result ("agentId: a1b2c3…").
fn agent_id_in(output: &str) -> Option<&str> {
    let i = output.find("agentId: ")? + "agentId: ".len();
    let id: &str = &output[i..];
    let end = id.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_')).unwrap_or(id.len());
    (end > 0).then(|| &id[..end])
}

fn s_id(b: &Value) -> String {
    b["id"].as_str().unwrap_or("").to_string()
}

/// Claude task status → the status shown in the panel.
fn task_status(s: &str) -> &'static str {
    match s {
        "completed" => "completed",
        "failed" => "failed",
        "stopped" | "killed" | "cancelled" => "stopped",
        _ => "running",
    }
}

fn spawn_key(chat: &Chat) -> String {
    format!("{}|{}|{}|{}", chat.model, chat.effort, chat.permission_mode, chat.cwd)
}

impl ClaudeSession {
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }
    pub fn is_busy(&self) -> bool {
        self.turn.lock().unwrap().is_some()
    }
    pub fn idle_for(&self) -> Duration {
        self.last_used.lock().unwrap().elapsed()
    }

    pub fn kill(&self) {
        self.alive.store(false, Ordering::SeqCst);
        // Background commands run in their own process groups and would outlive Claude.
        for (task_id, t) in self.bg.lock().unwrap().iter().filter(|(_, t)| t.running) {
            let out = crate::background::output_path(task_id, t.output_file.as_deref());
            crate::background::kill_task(self.pid, &t.command, out.as_deref());
        }
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.start_kill();
        }
    }

    /// The process ended: its background tasks are gone too (kill any survivors).
    fn end_background(&self, st: &St) {
        let running: Vec<(String, BgTask)> = self.bg.lock().unwrap().iter().filter(|(_, t)| t.running).map(|(k, t)| (k.clone(), t.clone())).collect();
        for (task_id, t) in running {
            let out = crate::background::output_path(&task_id, t.output_file.as_deref());
            crate::background::kill_task(self.pid, &t.command, out.as_deref());
            self.bg_finished(st, &task_id, "stopped", &json!("Stopped because the Claude Code process ended"), None);
        }
    }

    /// True while a background shell or monitor of this process is running.
    pub fn has_background(&self) -> bool {
        self.bg.lock().unwrap().values().any(|t| t.running)
    }

    pub fn running_background(&self) -> Vec<String> {
        self.bg.lock().unwrap().iter().filter(|(_, t)| t.running).map(|(k, _)| k.clone()).collect()
    }

    pub fn background_task(&self, task_id: &str) -> Option<BgTask> {
        self.bg.lock().unwrap().get(task_id).cloned()
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// Ask Claude Code to stop a task by its id (sub-agent, shell or monitor).
    pub fn stop_task_id(&self, task_id: &str) {
        let id = format!("stop-{}", self.ctrl_seq.fetch_add(1, Ordering::SeqCst));
        self.send(json!({"type":"control_request","request_id": id, "request": {"subtype":"stop_task","task_id": task_id}}));
    }

    /// Record a background task's end once and tell the panel.
    fn bg_finished(&self, st: &St, task_id: &str, status: &str, summary: &Value, output_file: Option<&str>) {
        let mut data = json!({"task_id": task_id, "status": status});
        {
            let mut bg = self.bg.lock().unwrap();
            let Some(t) = bg.get_mut(task_id) else { return };
            if let Some(f) = output_file.filter(|f| !f.is_empty()) {
                t.output_file = Some(f.to_string());
                data["output_file"] = json!(f);
            }
            if !t.running && summary.is_null() && output_file.is_none() {
                return;
            }
            t.running = false;
        }
        if !summary.is_null() {
            data["summary"] = summary.clone();
        }
        self.store(st, "shell", data);
    }

    /// Stop one sub-agent. Returns false when its task id is unknown (older CLI or
    /// already finished), so the caller can fall back to interrupting the turn.
    pub fn stop_task(&self, tool_use_id: &str) -> bool {
        let task = self.tasks.lock().unwrap().by_ref.get(tool_use_id).cloned();
        let Some(task_id) = task else { return false };
        let id = format!("stop-{}", self.ctrl_seq.fetch_add(1, Ordering::SeqCst));
        self.send(json!({"type":"control_request","request_id": id, "request": {"subtype":"stop_task","task_id": task_id}}));
        true
    }

    pub fn interrupt(&self) {
        let id = format!("int-{}", self.ctrl_seq.fetch_add(1, Ordering::SeqCst));
        let _ = self.stdin.send(json!({"type":"control_request","request_id": id, "request": {"subtype":"interrupt"}}).to_string());
    }

    pub async fn get_or_spawn(st: &St, chat: &Chat, project: &Project) -> Result<Arc<ClaudeSession>> {
        let existing = st.runner.claude.lock().unwrap().get(&chat.id).cloned();
        if let Some(s) = existing {
            if s.is_alive() && s.spawn_key == spawn_key(chat) {
                return Ok(s);
            }
            s.kill();
        }
        let s = Self::spawn(st, chat, project).await?;
        st.runner.claude.lock().unwrap().insert(chat.id, s.clone());
        Ok(s)
    }

    async fn spawn(st: &St, chat: &Chat, project: &Project) -> Result<Arc<ClaudeSession>> {
        let bin = settings::get(&st.db, "claude_path");
        let mut args: Vec<String> = [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--permission-prompt-tool",
            "stdio",
            "--permission-mode",
            claude_permission_mode(&chat.permission_mode),
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        if !chat.model.is_empty() && chat.model != "default" {
            args.push("--model".into());
            args.push(chat.model.clone());
        }
        if !chat.effort.is_empty() {
            args.push("--effort".into());
            args.push(chat.effort.clone());
        }
        match &chat.cli_session_id {
            Some(sid) => {
                args.push("--resume".into());
                args.push(sid.clone());
            }
            None => {
                let sid = uuid::Uuid::new_v4().to_string();
                st.db.conn().execute("UPDATE chats SET cli_session_id=?2 WHERE id=?1", rusqlite::params![chat.id, sid])?;
                args.push("--session-id".into());
                args.push(sid);
            }
        }
        let sys = system_prompt(st, project);
        if !sys.is_empty() {
            args.push("--append-system-prompt".into());
            args.push(sys);
        }
        let mut child = Command::new(&bin)
            .args(&args)
            .current_dir(&chat.cwd)
            // Claude refuses bypassPermissions as root unless it is told it runs sandboxed.
            .env("IS_SANDBOX", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("starting `{bin}` (is Claude Code installed? set its path in Settings)"))?;
        let child_pid = child.id();
        let mut stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let session = Arc::new(ClaudeSession {
            chat_id: chat.id,
            project_id: project.id,
            spawn_key: spawn_key(chat),
            stdin: tx,
            child: Mutex::new(Some(child)),
            turn: Mutex::new(None),
            alive: AtomicBool::new(true),
            last_used: Mutex::new(Instant::now()),
            total_cost: Mutex::new(0.0),
            model_usage: Mutex::new(HashMap::new()),
            last_model: Mutex::new(chat.model.clone()),
            stderr: Mutex::new(String::new()),
            tasks: Mutex::new(Tasks::default()),
            pid: child_pid,
            bg_calls: Mutex::new(HashMap::new()),
            bg: Mutex::new(HashMap::new()),
            ctrl_seq: AtomicU64::new(1),
        });

        tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err() || stdin.write_all(b"\n").await.is_err() {
                    break;
                }
                let _ = stdin.flush().await;
            }
        });
        {
            let s = session.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    let mut e = s.stderr.lock().unwrap();
                    e.push_str(&l);
                    e.push('\n');
                    if e.len() > 8192 {
                        *e = tail(&e, 4096);
                    }
                }
            });
        }
        {
            let s = session.clone();
            let st = st.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::with_capacity(1 << 16, stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if line.trim().is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<Value>(&line) {
                        Ok(v) => s.handle(&st, v),
                        Err(_) => tracing::debug!("claude non-json: {}", truncate(&line, 200)),
                    }
                }
                s.alive.store(false, Ordering::SeqCst);
                s.end_background(&st);
                let err = s.stderr.lock().unwrap().clone();
                let turn = s.turn.lock().unwrap().take();
                if let Some(mut t) = turn {
                    if let Some(done) = t.done.take() {
                        let msg = if err.trim().is_empty() {
                            "Claude Code exited unexpectedly".to_string()
                        } else {
                            format!("Claude Code exited: {}", tail(err.trim(), 1500))
                        };
                        let _ = done.send(TurnOutcome::error(msg));
                    }
                }
                let mut map = st.runner.claude.lock().unwrap();
                if map.get(&s.chat_id).map(|x| Arc::ptr_eq(x, &s)).unwrap_or(false) {
                    map.remove(&s.chat_id);
                }
            });
        }
        session.send(json!({"type":"control_request","request_id":"init","request":{"subtype":"initialize"}}));
        Ok(session)
    }

    fn send(&self, v: Value) {
        let _ = self.stdin.send(v.to_string());
    }

    pub async fn run_turn(&self, st: &St, run: &Run, atts: &[Attachment]) -> TurnOutcome {
        let (tx, rx) = oneshot::channel();
        {
            let mut t = self.turn.lock().unwrap();
            if t.is_some() {
                return TurnOutcome::error("a turn is already running in this chat");
            }
            *t = Some(Turn { run_id: run.id, done: Some(tx), last_text: String::new() });
        }
        *self.last_used.lock().unwrap() = Instant::now();
        let mut content = vec![json!({"type":"text","text": prompt_with_attachments(&run.prompt, atts)})];
        for a in atts {
            if a.mime.starts_with("image/") && a.size < 5 * 1024 * 1024 {
                if let Ok(bytes) = tokio::fs::read(&a.path).await {
                    content.push(json!({
                        "type": "image",
                        "source": {"type":"base64","media_type": a.mime, "data": base64::engine::general_purpose::STANDARD.encode(bytes)}
                    }));
                }
            }
        }
        self.send(json!({
            "type": "user",
            "message": {"role": "user", "content": content},
            "parent_tool_use_id": null,
            "session_id": ""
        }));
        let out = rx.await.unwrap_or_else(|_| TurnOutcome::error("Claude Code stopped"));
        *self.last_used.lock().unwrap() = Instant::now();
        let _ = st;
        out
    }

    fn current_run(&self) -> Option<i64> {
        self.turn.lock().unwrap().as_ref().map(|t| t.run_id)
    }

    /// This turn's tokens per model. `modelUsage` is cumulative for the process,
    /// so each turn is the difference from the previous snapshot; `usage` is the
    /// fallback (main model only) for CLIs that do not report `modelUsage`.
    fn turn_usage(&self, v: &Value, cost: f64) -> Vec<ModelUsage> {
        if let Some(per_model) = v["modelUsage"].as_object().filter(|m| !m.is_empty()) {
            let mut prev = self.model_usage.lock().unwrap();
            let mut out = Vec::new();
            for (model, m) in per_model {
                let now = ModelUsage::from_claude(model, m);
                let delta = now.minus(prev.get(model));
                prev.insert(model.clone(), now);
                if !delta.is_empty() {
                    out.push(delta);
                }
            }
            return out;
        }
        let u = &v["usage"];
        let n = |k: &str| u[k].as_i64().unwrap_or(0);
        let mu = ModelUsage {
            model: self.last_model.lock().unwrap().clone(),
            input: n("input_tokens"),
            output: n("output_tokens"),
            cache_read: n("cache_read_input_tokens"),
            cache_write: n("cache_creation_input_tokens"),
            reasoning: u["output_tokens_details"]["thinking_tokens"].as_i64().unwrap_or(0),
            web_search: u["server_tool_use"]["web_search_requests"].as_i64().unwrap_or(0),
            cost_usd: Some(cost),
        };
        if mu.is_empty() { vec![] } else { vec![mu] }
    }

    fn store(&self, st: &St, kind: &str, data: Value) {
        if let Ok(m) = insert_message(&st.db, self.chat_id, self.current_run(), kind, &data) {
            st.emit_project(self.project_id, Event::Message { message: m });
        }
    }

    fn delta(&self, st: &St, kind: &str, text: &str) {
        if let Some(run_id) = self.current_run() {
            st.emit_project(
                self.project_id,
                Event::Delta { chat_id: self.chat_id, run_id, kind: kind.into(), text: text.into() },
            );
        }
    }

    fn handle(self: &Arc<Self>, st: &St, v: Value) {
        let parent = v.get("parent_tool_use_id").cloned().unwrap_or(Value::Null);
        match v["type"].as_str().unwrap_or("") {
            "system" => match v["subtype"].as_str().unwrap_or("") {
                "init" => {
                    if let Some(sid) = v["session_id"].as_str() {
                        let _ = st.db.conn().execute(
                            "UPDATE chats SET cli_session_id=?2 WHERE id=?1",
                            rusqlite::params![self.chat_id, sid],
                        );
                    }
                }
                "compact_boundary" => self.store(st, "system", json!({"text": "Conversation compacted to free context"})),
                "task_started" if v["task_type"] == "local_bash" => {
                    let task_id = v["task_id"].as_str().unwrap_or("");
                    let r = v["tool_use_id"].as_str().unwrap_or("");
                    let (tool, command, description) = self.bg_calls.lock().unwrap().remove(r).unwrap_or_default();
                    self.bg.lock().unwrap().insert(task_id.to_string(), BgTask { command: command.clone(), running: true, output_file: None });
                    self.store(
                        st,
                        "shell",
                        json!({
                            "task_id": task_id, "ref": r, "status": "running",
                            "kind": if tool == "Monitor" { "monitor" } else { "shell" },
                            "command": command,
                            "description": v["description"].as_str().filter(|d| !d.is_empty()).map(String::from).unwrap_or(description),
                        }),
                    );
                }
                "task_updated" => {
                    let task_id = v["task_id"].as_str().unwrap_or("");
                    if let Some(s) = v["patch"]["status"].as_str() {
                        if task_status(s) != "running" && self.bg.lock().unwrap().contains_key(task_id) {
                            self.bg_finished(st, task_id, task_status(s), &Value::Null, None);
                        }
                    }
                }
                "task_started" => {
                    let task_id = v["task_id"].as_str().unwrap_or("");
                    let r = v["tool_use_id"].as_str().unwrap_or("");
                    self.tasks.lock().unwrap().link(r, task_id);
                    self.store(
                        st,
                        "agent",
                        json!({"ref": r, "task_id": task_id, "status": "running", "description": v["description"], "task_type": v["task_type"]}),
                    );
                }
                "task_progress" => {
                    let task_id = v["task_id"].as_str().unwrap_or("");
                    let r = v["tool_use_id"].as_str().map(String::from).or_else(|| self.tasks.lock().unwrap().by_task.get(task_id).cloned());
                    let u = &v["usage"];
                    let p = json!({
                        "ref": r, "task_id": task_id, "last_tool": v["last_tool_name"], "description": v["description"],
                        "tokens": u["total_tokens"], "tool_uses": u["tool_uses"], "duration_ms": u["duration_ms"],
                    });
                    self.delta(st, "agent", &p.to_string());
                }
                "task_notification" if self.bg.lock().unwrap().contains_key(v["task_id"].as_str().unwrap_or("")) => {
                    let task_id = v["task_id"].as_str().unwrap_or("");
                    self.bg_finished(st, task_id, task_status(v["status"].as_str().unwrap_or("")), &v["summary"], v["output_file"].as_str());
                }
                "task_notification" => {
                    let task_id = v["task_id"].as_str().unwrap_or("");
                    let r = v["tool_use_id"].as_str().map(String::from).or_else(|| self.tasks.lock().unwrap().by_task.get(task_id).cloned());
                    self.store(
                        st,
                        "agent",
                        json!({"ref": r, "task_id": task_id, "status": task_status(v["status"].as_str().unwrap_or("")), "summary": v["summary"]}),
                    );
                }
                _ => {}
            },
            "stream_event" => {
                if !parent.is_null() {
                    return;
                }
                let ev = &v["event"];
                match ev["type"].as_str().unwrap_or("") {
                    "content_block_delta" => {
                        let d = &ev["delta"];
                        match d["type"].as_str().unwrap_or("") {
                            "text_delta" => self.delta(st, "text", d["text"].as_str().unwrap_or("")),
                            "thinking_delta" => self.delta(st, "thinking", d["thinking"].as_str().unwrap_or("")),
                            _ => {}
                        }
                    }
                    "content_block_start" => {
                        let b = &ev["content_block"];
                        if b["type"] == "tool_use" {
                            self.delta(st, "activity", &format!("Using {}", b["name"].as_str().unwrap_or("tool")));
                        }
                    }
                    _ => {}
                }
            }
            "assistant" => {
                if v["parent_tool_use_id"].is_null() {
                    if let Some(m) = v["message"]["model"].as_str().filter(|m| !m.is_empty() && *m != "<synthetic>") {
                        *self.last_model.lock().unwrap() = m.to_string();
                    }
                }
                let blocks = v["message"]["content"].as_array().cloned().unwrap_or_default();
                for b in blocks {
                    match b["type"].as_str().unwrap_or("") {
                        "text" => {
                            let text = b["text"].as_str().unwrap_or("").to_string();
                            if text.trim().is_empty() {
                                continue;
                            }
                            if parent.is_null() {
                                if let Some(t) = self.turn.lock().unwrap().as_mut() {
                                    t.last_text = text.clone();
                                }
                            }
                            self.store(st, "assistant", json!({"text": text, "parent": parent}));
                        }
                        "thinking" => {
                            let text = b["thinking"].as_str().unwrap_or("");
                            if !text.trim().is_empty() {
                                self.store(st, "thinking", json!({"text": text, "parent": parent}));
                            }
                        }
                        "tool_use" => {
                            let name = b["name"].as_str().unwrap_or("");
                            if name == "Monitor" || (name == "Bash" && b["input"]["run_in_background"] == true) {
                                let s = |k: &str| b["input"][k].as_str().unwrap_or("").to_string();
                                self.bg_calls.lock().unwrap().insert(s_id(&b), (name.to_string(), s("command"), s("description")));
                            }
                            self.store(st, "tool_use", json!({"id": b["id"], "name": b["name"], "input": b["input"], "parent": parent}))
                        }
                        _ => {}
                    }
                }
            }
            "user" => {
                if let Some(blocks) = v["message"]["content"].as_array() {
                    for b in blocks {
                        if b["type"] == "tool_result" {
                            let output = match &b["content"] {
                                Value::String(s) => s.clone(),
                                Value::Array(items) => items
                                    .iter()
                                    .map(|i| match i["type"].as_str() {
                                        Some("text") => i["text"].as_str().unwrap_or("").to_string(),
                                        Some(t) => format!("[{t}]"),
                                        None => String::new(),
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n"),
                                other => other.to_string(),
                            };
                            let mut launched = None;
                            if parent.is_null() {
                                // Background agents report their id right away; it is also their task id.
                                if let (Some(r), Some(aid)) = (b["tool_use_id"].as_str(), agent_id_in(&output)) {
                                    self.tasks.lock().unwrap().link(r, aid);
                                    if output.contains("Async agent launched") {
                                        launched = Some(json!({"ref": r, "task_id": aid, "status": "running", "background": true}));
                                    }
                                }
                            }
                            self.store(
                                st,
                                "tool_result",
                                json!({
                                    "tool_use_id": b["tool_use_id"],
                                    "output": truncate(&output, 30_000),
                                    "is_error": b["is_error"].as_bool().unwrap_or(false),
                                    "parent": parent,
                                }),
                            );
                            if let Some(l) = launched {
                                self.store(st, "agent", l);
                            }
                        }
                    }
                }
            }
            "result" => {
                if self.turn.lock().unwrap().is_none() {
                    // A turn Claude started by itself (e.g. after a background task finished).
                    // Keep the cumulative counters as they are, so the next prompt's cost and
                    // usage include this turn instead of losing it.
                    *self.last_used.lock().unwrap() = Instant::now();
                    return;
                }
                let total = v["total_cost_usd"].as_f64().unwrap_or(0.0);
                let cost = {
                    let mut prev = self.total_cost.lock().unwrap();
                    let c = (total - *prev).max(0.0);
                    *prev = total;
                    c
                };
                let u = &v["usage"];
                let input = u["input_tokens"].as_i64().unwrap_or(0)
                    + u["cache_creation_input_tokens"].as_i64().unwrap_or(0)
                    + u["cache_read_input_tokens"].as_i64().unwrap_or(0);
                let output = u["output_tokens"].as_i64().unwrap_or(0);
                let usage = self.turn_usage(&v, cost);
                let is_error = v["is_error"].as_bool().unwrap_or(false) || v["subtype"] != "success";
                if is_error && v["num_turns"].as_i64() == Some(0) && v["result"].as_str().unwrap_or("").is_empty() {
                    // Failed before doing anything (e.g. the session to resume is gone): the reason
                    // is on stderr, which may still be arriving.
                    let s = self.clone();
                    let subtype = v["subtype"].as_str().unwrap_or("error").to_string();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(400)).await;
                        let err = s.stderr.lock().unwrap().trim().to_string();
                        let turn = s.turn.lock().unwrap().take();
                        if let Some(mut t) = turn {
                            if let Some(done) = t.done.take() {
                                let msg = if err.is_empty() { format!("Stopped: {subtype}") } else { tail(&err, 1500) };
                                let _ = done.send(TurnOutcome::error(msg));
                            }
                        }
                    });
                    return;
                }
                let turn = self.turn.lock().unwrap().take();
                if let Some(mut t) = turn {
                    let result_text = v["result"].as_str().map(|s| s.to_string()).or(Some(t.last_text.clone()));
                    let outcome = TurnOutcome {
                        status: if is_error { "error".into() } else { "done".into() },
                        error: if is_error {
                            Some(
                                v["result"]
                                    .as_str()
                                    .filter(|s| !s.is_empty())
                                    .map(|s| s.to_string())
                                    .unwrap_or_else(|| format!("Stopped: {}", v["subtype"].as_str().unwrap_or("error"))),
                            )
                        } else {
                            None
                        },
                        result_text,
                        cost_usd: Some(cost),
                        input_tokens: input,
                        output_tokens: output,
                        usage,
                    };
                    if let Some(done) = t.done.take() {
                        let _ = done.send(outcome);
                    }
                }
                *self.last_used.lock().unwrap() = Instant::now();
            }
            "rate_limit_event" => {
                crate::runner::set_rate_limits(st, "claude", v["rate_limit_info"].clone());
            }
            "control_request" => {
                let s = self.clone();
                let st = st.clone();
                tokio::spawn(async move { s.handle_control(&st, v).await });
            }
            _ => {}
        }
    }

    async fn handle_control(&self, st: &St, v: Value) {
        let request_id = v["request_id"].clone();
        let req = &v["request"];
        let reply = |response: Value| {
            json!({"type":"control_response","response":{"subtype":"success","request_id": request_id, "response": response}})
        };
        if req["subtype"] != "can_use_tool" {
            self.send(json!({"type":"control_response","response":{
                "subtype":"error","request_id": request_id,
                "error": format!("{} is not supported by cloud-ai", req["subtype"].as_str().unwrap_or("request"))
            }}));
            return;
        }
        let tool = req["tool_name"].as_str().unwrap_or("").to_string();
        let input = req["input"].clone();
        let run_id = self.current_run();
        if tool == "AskUserQuestion" {
            let qs: Vec<Value> = input["questions"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .enumerate()
                .map(|(i, q)| {
                    question(
                        &i.to_string(),
                        q["header"].as_str().unwrap_or(""),
                        q["question"].as_str().unwrap_or(""),
                        q["multiSelect"].as_bool().unwrap_or(false),
                        true,
                        q["options"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default()
                            .iter()
                            .map(|o| {
                                (o["label"].as_str().unwrap_or("").to_string(), o["description"].as_str().unwrap_or("").to_string())
                            })
                            .collect(),
                    )
                })
                .collect();
            let ans = runner::ask(st, self.chat_id, run_id, "question", json!({"tool": "claude", "questions": qs})).await;
            if ans["allow"] == false {
                self.send(reply(json!({"behavior":"deny","message": ans["message"].as_str().unwrap_or("The user declined to answer")})));
                return;
            }
            let mut answers = serde_json::Map::new();
            for (i, q) in input["questions"].as_array().cloned().unwrap_or_default().iter().enumerate() {
                let picked: Vec<String> = ans["answers"][i.to_string()]
                    .as_array()
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                answers.insert(q["question"].as_str().unwrap_or("").to_string(), json!(picked.join(", ")));
            }
            let mut updated = input.clone();
            updated["answers"] = Value::Object(answers);
            self.send(reply(json!({"behavior":"allow","updatedInput": updated})));
            return;
        }
        let summary = match tool.as_str() {
            "Bash" => input["command"].as_str().unwrap_or("").to_string(),
            "Edit" | "Write" | "Read" | "NotebookEdit" => input["file_path"].as_str().unwrap_or("").to_string(),
            "WebFetch" => input["url"].as_str().unwrap_or("").to_string(),
            "ExitPlanMode" => input["plan"].as_str().unwrap_or("").to_string(),
            _ => truncate(&input.to_string(), 600),
        };
        let data = json!({
            "tool": tool,
            "title": req["title"].as_str().or(req["display_name"].as_str()).unwrap_or(&tool),
            "summary": truncate(&summary, 4000),
            "reason": req["decision_reason"],
            "input": input,
        });
        let ans = runner::ask(st, self.chat_id, run_id, "approval", data).await;
        if ans["allow"] == true {
            self.send(reply(json!({"behavior":"allow","updatedInput": input})));
        } else {
            self.send(reply(json!({
                "behavior":"deny",
                "message": ans["message"].as_str().filter(|m| !m.is_empty()).unwrap_or("The user denied this action")
            })));
        }
    }
}

/// Start Claude once to read the model catalog from its initialize response.
pub async fn fetch_models(st: &St) -> Result<Vec<Value>> {
    let bin = settings::get(&st.db, "claude_path");
    let mut child = Command::new(&bin)
        .args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"])
        .current_dir(std::env::temp_dir())
        .env("IS_SANDBOX", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("starting {bin}"))?;
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(b"{\"type\":\"control_request\",\"request_id\":\"m\",\"request\":{\"subtype\":\"initialize\"}}\n")
        .await?;
    let stdout = child.stdout.take().unwrap();
    let mut lines = BufReader::with_capacity(1 << 16, stdout).lines();
    let res = tokio::time::timeout(Duration::from_secs(60), async {
        while let Ok(Some(l)) = lines.next_line().await {
            if let Ok(v) = serde_json::from_str::<Value>(&l) {
                if v["type"] == "control_response" && v["response"]["request_id"] == "m" {
                    let r = &v["response"]["response"];
                    if let Some(acc) = r.get("account") {
                        store::set_cli_info(st, "claude_account", acc);
                    }
                    if let Some(ver) = r.get("claude_code_version") {
                        store::set_cli_info(st, "claude_version", ver);
                    }
                    return Ok(r["models"].as_array().cloned().unwrap_or_default());
                }
            }
        }
        anyhow::bail!("claude exited without answering")
    })
    .await;
    let _ = child.start_kill();
    res.unwrap_or_else(|_| anyhow::bail!("timed out waiting for claude"))
}
