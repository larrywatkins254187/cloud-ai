//! Codex CLI driven through `codex app-server` (JSON-RPC over stdio). A single
//! app-server process hosts every Codex chat's thread, which keeps RAM low.
use super::{codex_policy, prompt_with_attachments, question, system_prompt, Attachment, ModelUsage, TurnOutcome};
use crate::events::Event;
use crate::runner;
use crate::settings;
use crate::state::St;
use crate::store::{insert_message, Chat, Project, Run};
use crate::util::{tail, truncate};
use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};

struct TurnCtx {
    chat_id: i64,
    project_id: i64,
    run_id: i64,
    turn_id: Option<String>,
    done: Option<oneshot::Sender<TurnOutcome>>,
    last_text: String,
    input_tokens: i64,
    output_tokens: i64,
    cached_tokens: i64,
    reasoning_tokens: i64,
    model: String,
    failure: Option<String>,
    /// Size of the latest request and the model's context window.
    ctx_used: i64,
    ctx_max: i64,
}

impl TurnCtx {
    /// Codex counts cached tokens inside `inputTokens`; split them out.
    fn usage(&self) -> Vec<ModelUsage> {
        let mu = ModelUsage {
            model: self.model.clone(),
            input: (self.input_tokens - self.cached_tokens).max(0),
            output: self.output_tokens,
            cache_read: self.cached_tokens,
            reasoning: self.reasoning_tokens,
            ..Default::default()
        };
        if mu.is_empty() { vec![] } else { vec![mu] }
    }
}

/// A sub-agent thread (Codex collab `spawnAgent`) and where its output goes.
#[derive(Clone)]
struct ChildCtx {
    chat_id: i64,
    project_id: i64,
    run_id: i64,
    /// Item id of the spawnAgent call; the child's messages nest under it.
    parent: String,
    turn_id: Option<String>,
    /// The chat's own thread (the top of the agent tree).
    root_thread: String,
    running: bool,
}

/// A chat turn that ended while its sub-agents were still working. The prompt
/// finishes when they are done (or the parent continues and then ends).
struct Waiting {
    ctx: TurnCtx,
    outcome: TurnOutcome,
    since: Instant,
}

/// Codex collab agent status → the status shown in the panel.
fn collab_status(s: &str) -> &'static str {
    match s {
        "completed" => "completed",
        "errored" | "notFound" => "failed",
        "interrupted" | "shutdown" => "stopped",
        _ => "running",
    }
}

pub struct CodexProc {
    stdin: mpsc::UnboundedSender<String>,
    child: Mutex<Option<Child>>,
    next_id: AtomicI64,
    pending: Mutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>,
    loaded: Mutex<HashSet<String>>,
    /// Model each loaded thread runs on, as reported by thread/start|resume.
    thread_models: Mutex<HashMap<String, String>>,
    /// Active turn per thread id.
    turns: Mutex<HashMap<String, TurnCtx>>,
    /// Sub-agent threads spawned by a chat's turn, by thread id.
    children: Mutex<HashMap<String, ChildCtx>>,
    /// Chat turns waiting for their sub-agents, by thread id.
    waiting: Mutex<HashMap<String, Waiting>>,
    alive: AtomicBool,
    last_active: Mutex<Instant>,
    stderr: Mutex<String>,
}

#[derive(Default)]
pub struct CodexManager {
    proc_: tokio::sync::Mutex<Option<Arc<CodexProc>>>,
}

impl CodexProc {
    pub fn send(&self, v: Value) {
        let _ = self.stdin.send(v.to_string());
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        self.send(json!({"id": id, "method": method, "params": params}));
        match tokio::time::timeout(Duration::from_secs(120), rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(anyhow!("{method}: {e}")),
            Ok(Err(_)) => Err(anyhow!("codex app-server stopped")),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(anyhow!("{method}: timed out"))
            }
        }
    }

    pub fn kill(&self) {
        self.alive.store(false, Ordering::SeqCst);
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.start_kill();
        }
    }

    fn busy(&self) -> bool {
        !self.turns.lock().unwrap().is_empty() || !self.waiting.lock().unwrap().is_empty()
    }

    fn running_children(&self, root: &str) -> usize {
        self.children.lock().unwrap().values().filter(|c| c.running && c.root_thread == root).count()
    }

    /// Finish a waiting turn now. Returns false when nothing was waiting.
    fn release_thread(&self, st: &St, thread: &str, note: Option<&str>) -> bool {
        let Some(mut w) = self.waiting.lock().unwrap().remove(thread) else { return false };
        if let Some(n) = note {
            store_msg(st, w.ctx.chat_id, w.ctx.project_id, w.ctx.run_id, "system", json!({"text": n}));
        }
        if let Some(done) = w.ctx.done.take() {
            let _ = done.send(w.outcome);
        }
        true
    }

    /// A sub-agent stopped. When none of the chat's agents is left running, give the
    /// parent a moment to continue on its own, then finish the prompt.
    fn child_stopped(self: &Arc<Self>, st: &St, root: &str) {
        if self.running_children(root) > 0 || !self.waiting.lock().unwrap().contains_key(root) {
            return;
        }
        let p = self.clone();
        let st = st.clone();
        let root = root.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(8)).await;
            if p.running_children(&root) == 0 {
                p.release_thread(&st, &root, None);
            }
        });
    }
}

impl CodexManager {
    /// The running app-server, if any (does not start one).
    pub async fn current(&self) -> Option<Arc<CodexProc>> {
        self.proc_.lock().await.as_ref().filter(|p| p.alive.load(Ordering::SeqCst)).cloned()
    }

    pub async fn get(&self, st: &St) -> Result<Arc<CodexProc>> {
        let mut guard = self.proc_.lock().await;
        if let Some(p) = guard.as_ref() {
            if p.alive.load(Ordering::SeqCst) {
                *p.last_active.lock().unwrap() = Instant::now();
                return Ok(p.clone());
            }
        }
        let p = start_proc(st).await?;
        *guard = Some(p.clone());
        Ok(p)
    }

    pub async fn reap_idle(&self, idle: Duration) {
        let mut guard = self.proc_.lock().await;
        if let Some(p) = guard.as_ref() {
            if !p.alive.load(Ordering::SeqCst) || (!p.busy() && p.last_active.lock().unwrap().elapsed() >= idle) {
                p.kill();
                *guard = None;
            }
        }
    }

    pub async fn stop(&self) {
        if let Some(p) = self.proc_.lock().await.take() {
            p.kill();
        }
    }

    pub async fn interrupt(&self, chat_id: i64) {
        let p = self.proc_.lock().await.clone();
        let Some(p) = p else { return };
        let target = {
            let turns = p.turns.lock().unwrap();
            turns
                .iter()
                .find(|(_, t)| t.chat_id == chat_id)
                .map(|(thread, t)| (thread.clone(), t.turn_id.clone()))
        };
        if let Some((thread, Some(turn))) = target {
            let _ = p.request("turn/interrupt", json!({"threadId": thread, "turnId": turn})).await;
        }
    }
}

impl CodexManager {
    /// Finish a chat's prompt that only waits for its sub-agents. Returns false when it was not waiting.
    pub async fn release(&self, st: &St, chat_id: i64, note: Option<&str>) -> bool {
        let p = self.proc_.lock().await.clone();
        let Some(p) = p else { return false };
        let thread = p.waiting.lock().unwrap().iter().find(|(_, w)| w.ctx.chat_id == chat_id).map(|(t, _)| t.clone());
        thread.map(|t| p.release_thread(st, &t, note)).unwrap_or(false)
    }

    pub async fn is_waiting(&self, chat_id: i64) -> bool {
        let p = self.proc_.lock().await.clone();
        p.map(|p| p.waiting.lock().unwrap().values().any(|w| w.ctx.chat_id == chat_id)).unwrap_or(false)
    }

    /// Interrupt one sub-agent thread. Returns false when its turn isn't known.
    pub async fn interrupt_agent(&self, chat_id: i64, spawn_id: &str) -> bool {
        let p = self.proc_.lock().await.clone();
        let Some(p) = p else { return false };
        let targets: Vec<(String, String)> = p
            .children
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, c)| c.chat_id == chat_id && c.parent == spawn_id)
            .filter_map(|(t, c)| c.turn_id.clone().map(|turn| (t.clone(), turn)))
            .collect();
        let mut any = false;
        for (thread, turn) in targets {
            any |= p.request("turn/interrupt", json!({"threadId": thread, "turnId": turn})).await.is_ok();
        }
        any
    }
}

pub async fn start_proc(st: &St) -> Result<Arc<CodexProc>> {
    let bin = settings::get(&st.db, "codex_path");
    let mut child = Command::new(&bin)
        .arg("app-server")
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("starting `{bin} app-server` (is Codex CLI installed? set its path in Settings)"))?;
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let p = Arc::new(CodexProc {
        stdin: tx,
        child: Mutex::new(Some(child)),
        next_id: AtomicI64::new(1),
        pending: Mutex::new(HashMap::new()),
        loaded: Mutex::new(HashSet::new()),
        thread_models: Mutex::new(HashMap::new()),
        turns: Mutex::new(HashMap::new()),
        children: Mutex::new(HashMap::new()),
        waiting: Mutex::new(HashMap::new()),
        alive: AtomicBool::new(true),
        last_active: Mutex::new(Instant::now()),
        stderr: Mutex::new(String::new()),
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
        let p = p.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                let mut e = p.stderr.lock().unwrap();
                e.push_str(&l);
                e.push('\n');
                if e.len() > 8192 {
                    *e = tail(&e, 4096);
                }
            }
        });
    }
    {
        let p = p.clone();
        let st = st.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::with_capacity(1 << 16, stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                handle(&st, &p, v);
            }
            p.alive.store(false, Ordering::SeqCst);
            let err = tail(p.stderr.lock().unwrap().trim(), 1500);
            for (_, tx) in p.pending.lock().unwrap().drain() {
                let _ = tx.send(Err("codex app-server exited".into()));
            }
            for (_, mut t) in p.turns.lock().unwrap().drain() {
                if let Some(done) = t.done.take() {
                    let _ = done.send(TurnOutcome::error(format!("Codex exited unexpectedly. {err}")));
                }
            }
            for (_, mut w) in p.waiting.lock().unwrap().drain() {
                if let Some(done) = w.ctx.done.take() {
                    let _ = done.send(w.outcome);
                }
            }
        });
    }
    p.request(
        "initialize",
        json!({"clientInfo": {"name": "cloud-ai", "title": "cloud-ai", "version": env!("CARGO_PKG_VERSION")}, "capabilities": null}),
    )
    .await?;
    p.send(json!({"method": "initialized"}));
    Ok(p)
}

fn handle(st: &St, p: &Arc<CodexProc>, v: Value) {
    let method = v["method"].as_str().map(|s| s.to_string());
    let has_id = v.get("id").map(|i| !i.is_null()).unwrap_or(false);
    match (method, has_id) {
        (None, true) => {
            let id = v["id"].as_i64().unwrap_or(-1);
            if let Some(tx) = p.pending.lock().unwrap().remove(&id) {
                if let Some(err) = v.get("error") {
                    let _ = tx.send(Err(err["message"].as_str().unwrap_or(&err.to_string()).to_string()));
                } else {
                    let _ = tx.send(Ok(v["result"].clone()));
                }
            }
        }
        (Some(m), true) => {
            let p = p.clone();
            let st = st.clone();
            tokio::spawn(async move { server_request(&st, &p, &m, v).await });
        }
        (Some(m), false) => notification(st, p, &m, &v["params"]),
        _ => {}
    }
}

fn store_msg(st: &St, chat_id: i64, project_id: i64, run_id: i64, kind: &str, data: Value) {
    if let Ok(m) = insert_message(&st.db, chat_id, Some(run_id), kind, &data) {
        st.emit_project(project_id, Event::Message { message: m });
    }
}

fn notification(st: &St, p: &Arc<CodexProc>, method: &str, params: &Value) {
    if method == "account/rateLimits/updated" {
        runner::set_rate_limits(st, "codex", params["rateLimits"].clone());
        return;
    }
    let Some(thread) = params["threadId"].as_str() else { return };
    if method == "turn/started" && !p.turns.lock().unwrap().contains_key(thread) {
        // The parent continues after its sub-agents reported back: the prompt goes on.
        if let Some(w) = p.waiting.lock().unwrap().remove(thread) {
            let mut ctx = w.ctx;
            ctx.turn_id = params["turn"]["id"].as_str().map(String::from);
            p.turns.lock().unwrap().insert(thread.to_string(), ctx);
        }
    }
    let mut turns = p.turns.lock().unwrap();
    let Some(t) = turns.get_mut(thread) else {
        drop(turns);
        child_notification(st, p, thread, method, params);
        return;
    };
    *p.last_active.lock().unwrap() = Instant::now();
    let (chat_id, project_id, run_id) = (t.chat_id, t.project_id, t.run_id);
    let delta = |kind: &str, text: &str| {
        st.emit_project(project_id, Event::Delta { chat_id, run_id, kind: kind.into(), text: text.into() });
    };
    match method {
        "turn/started" => {
            if t.turn_id.is_none() {
                t.turn_id = params["turn"]["id"].as_str().map(String::from);
            }
        }
        "item/agentMessage/delta" => delta("text", params["delta"].as_str().unwrap_or("")),
        "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
            delta("thinking", params["delta"].as_str().unwrap_or(""))
        }
        "item/started" => {
            let item = &params["item"];
            match item["type"].as_str().unwrap_or("") {
                "commandExecution" => delta("activity", &format!("$ {}", item["command"].as_str().unwrap_or(""))),
                "fileChange" => delta("activity", "Editing files"),
                "mcpToolCall" => delta("activity", &format!("Using {}", item["tool"].as_str().unwrap_or("tool"))),
                "webSearch" => delta("activity", "Searching the web"),
                _ => {}
            }
        }
        "item/completed" => {
            if let Some(text) = store_item(st, p, (chat_id, project_id, run_id), thread, &params["item"], &Value::Null) {
                t.last_text = text;
            }
        }
        "turn/plan/updated" => {
            store_msg(st, chat_id, project_id, run_id, "todo", json!({"plan": params["plan"], "explanation": params["explanation"]}));
        }
        "thread/tokenUsage/updated" => {
            let last = &params["tokenUsage"]["last"];
            t.input_tokens += last["inputTokens"].as_i64().unwrap_or(0);
            t.output_tokens += last["outputTokens"].as_i64().unwrap_or(0);
            t.cached_tokens += last["cachedInputTokens"].as_i64().unwrap_or(0);
            t.reasoning_tokens += last["reasoningOutputTokens"].as_i64().unwrap_or(0);
            let used = last["totalTokens"].as_i64().unwrap_or_else(|| last["inputTokens"].as_i64().unwrap_or(0) + last["outputTokens"].as_i64().unwrap_or(0));
            if used > 0 {
                t.ctx_used = used;
            }
            if let Some(w) = params["tokenUsage"]["modelContextWindow"].as_i64() {
                t.ctx_max = w;
            }
        }
        "error" => {
            if params["willRetry"] != true {
                t.failure = params["error"]["message"].as_str().map(String::from);
            }
        }
        "turn/completed" => {
            let turn = &params["turn"];
            let status = turn["status"].as_str().unwrap_or("completed");
            let mut ctx = turns.remove(thread).unwrap();
            if ctx.ctx_used > 0 {
                super::save_context(st, chat_id, project_id, super::codex_context(&ctx.model, ctx.ctx_used, ctx.ctx_max));
            }
            let outcome = TurnOutcome {
                status: match status {
                    "completed" => "done",
                    "interrupted" => "cancelled",
                    _ => "error",
                }
                .into(),
                error: if status == "failed" {
                    Some(
                        turn["error"]["message"]
                            .as_str()
                            .map(String::from)
                            .or(ctx.failure.clone())
                            .unwrap_or_else(|| "Codex turn failed".into()),
                    )
                } else {
                    None
                },
                result_text: Some(ctx.last_text.clone()),
                cost_usd: None,
                input_tokens: ctx.input_tokens,
                output_tokens: ctx.output_tokens,
                usage: ctx.usage(),
            };
            let waiting_for = p.running_children(thread);
            if outcome.status == "done" && waiting_for > 0 {
                // The sub-agents this turn started are still working.
                let msg = format!("Waiting for {waiting_for} sub-agent{} before this prompt is done", if waiting_for == 1 { "" } else { "s" });
                store_msg(st, chat_id, project_id, run_id, "system", json!({"text": msg}));
                delta("activity", &format!("Waiting for {waiting_for} sub-agent{} to finish…", if waiting_for == 1 { "" } else { "s" }));
                let since = Instant::now();
                p.waiting.lock().unwrap().insert(thread.to_string(), Waiting { ctx, outcome, since });
                let limit = settings::get_i64(&st.db, "background_wait_minutes").max(1) as u64;
                let (p2, st2, th) = (p.clone(), st.clone(), thread.to_string());
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(limit * 60)).await;
                    let same = p2.waiting.lock().unwrap().get(&th).map(|w| w.since == since).unwrap_or(false);
                    if same {
                        p2.release_thread(&st2, &th, Some(&format!("Stopped waiting for sub-agents after {limit} minutes")));
                    }
                });
                return;
            }
            if let Some(done) = ctx.done.take() {
                let _ = done.send(outcome);
            }
        }
        _ => {}
    }
}

/// Store a finished item as chat messages (nested under `parent` for sub-agents).
/// Returns the text of an agent message.
fn store_item(st: &St, p: &CodexProc, ctx: (i64, i64, i64), root_thread: &str, item: &Value, parent: &Value) -> Option<String> {
    let (chat_id, project_id, run_id) = ctx;
    let id = item["id"].clone();
    let store = |kind: &str, mut data: Value| {
        if !parent.is_null() {
            data["parent"] = parent.clone();
        }
        store_msg(st, chat_id, project_id, run_id, kind, data);
    };
    match item["type"].as_str().unwrap_or("") {
        "agentMessage" => {
            let text = item["text"].as_str().unwrap_or("").to_string();
            if !text.trim().is_empty() {
                store("assistant", json!({"text": text}));
                return Some(text);
            }
        }
        "reasoning" => {
            let parts: Vec<String> = item["summary"]
                .as_array()
                .or(item["content"].as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let text = parts.join("\n\n");
            if !text.trim().is_empty() {
                store("thinking", json!({"text": text}));
            }
        }
        "commandExecution" => {
            store("tool_use", json!({"id": id, "name": "Bash", "input": {"command": item["command"], "cwd": item["cwd"]}}));
            let exit = item["exitCode"].as_i64();
            store("tool_result", json!({
                "tool_use_id": id,
                "output": truncate(item["aggregatedOutput"].as_str().unwrap_or(""), 30_000),
                "is_error": exit.map(|c| c != 0).unwrap_or(false) || item["status"] == "failed" || item["status"] == "declined",
                "exit_code": exit,
            }));
        }
        "fileChange" => {
            store("tool_use", json!({"id": id, "name": "Edit", "input": {"changes": item["changes"]}}));
            store("tool_result", json!({
                "tool_use_id": id, "output": item["status"], "is_error": item["status"] == "failed" || item["status"] == "declined"
            }));
        }
        "mcpToolCall" => {
            store("tool_use", json!({
                "id": id,
                "name": format!("mcp__{}__{}", item["server"].as_str().unwrap_or(""), item["tool"].as_str().unwrap_or("")),
                "input": item["arguments"],
            }));
            let out = if item["error"].is_null() { item["result"].to_string() } else { item["error"].to_string() };
            store("tool_result", json!({"tool_use_id": id, "output": truncate(&out, 30_000), "is_error": !item["error"].is_null()}));
        }
        "webSearch" => store("tool_use", json!({"id": id, "name": "WebSearch", "input": item})),
        "plan" => store("plan", json!({"text": item["text"]})),
        "contextCompaction" => store("system", json!({"text": "Conversation compacted to free context"})),
        "collabAgentToolCall" => {
            let tool = item["tool"].as_str().unwrap_or("");
            let receivers: Vec<String> =
                item["receiverThreadIds"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
            let states = &item["agentsStates"];
            let summary: Vec<String> = receivers
                .iter()
                .map(|r| {
                    let s = &states[r];
                    let msg = s["message"].as_str().unwrap_or("");
                    format!("{}: {}{}", r, s["status"].as_str().unwrap_or("?"), if msg.is_empty() { String::new() } else { format!(" — {msg}") })
                })
                .collect();
            let prompt = item["prompt"].as_str().unwrap_or("");
            if tool == "spawnAgent" {
                let description: String = prompt.lines().next().unwrap_or("Sub-agent").chars().take(80).collect();
                store("tool_use", json!({
                    "id": id, "name": "Agent",
                    "input": {"description": description, "prompt": prompt, "model": item["model"], "subagent_type": "codex"}
                }));
                store("tool_result", json!({"tool_use_id": id, "output": summary.join("\n"), "is_error": item["status"] == "failed"}));
                let parent_id = id.as_str().unwrap_or("").to_string();
                for r in &receivers {
                    let status = states[r]["status"].as_str().map(collab_status).unwrap_or("running");
                    p.children.lock().unwrap().insert(
                        r.clone(),
                        ChildCtx {
                            chat_id,
                            project_id,
                            run_id,
                            parent: parent_id.clone(),
                            turn_id: None,
                            root_thread: root_thread.to_string(),
                            running: status == "running",
                        },
                    );
                    store("agent", json!({"ref": parent_id, "task_id": r, "status": status, "background": true}));
                }
            } else {
                store("tool_use", json!({"id": id, "name": format!("Agent·{tool}"), "input": {"agents": receivers, "prompt": prompt}}));
                store("tool_result", json!({"tool_use_id": id, "output": summary.join("\n"), "is_error": item["status"] == "failed"}));
                // Waits and closes report the agents' latest states.
                for r in &receivers {
                    let child = p.children.lock().unwrap().get(r).cloned();
                    if let (Some(c), Some(s)) = (child, states[r]["status"].as_str()) {
                        if collab_status(s) != "running" {
                            if let Some(ch) = p.children.lock().unwrap().get_mut(r) {
                                ch.running = false;
                            }
                        }
                        store_msg(st, c.chat_id, c.project_id, c.run_id, "agent", json!({
                            "ref": c.parent, "task_id": r, "status": collab_status(s), "summary": states[r]["message"], "background": true
                        }));
                    }
                }
            }
        }
        _ => {}
    }
    None
}

/// Events of a sub-agent thread: its items nest under the spawning call.
fn child_notification(st: &St, p: &Arc<CodexProc>, thread: &str, method: &str, params: &Value) {
    let Some(c) = p.children.lock().unwrap().get(thread).cloned() else { return };
    *p.last_active.lock().unwrap() = Instant::now();
    let parent = json!(c.parent);
    match method {
        "turn/started" => {
            if let Some(ch) = p.children.lock().unwrap().get_mut(thread) {
                ch.turn_id = params["turn"]["id"].as_str().map(String::from);
                ch.running = true;
            }
            store_msg(st, c.chat_id, c.project_id, c.run_id, "agent", json!({"ref": c.parent, "task_id": thread, "status": "running", "background": true}));
        }
        "item/completed" => {
            store_item(st, p, (c.chat_id, c.project_id, c.run_id), &c.root_thread, &params["item"], &parent);
        }
        "turn/completed" => {
            let status = match params["turn"]["status"].as_str().unwrap_or("completed") {
                "completed" => "completed",
                "interrupted" => "stopped",
                _ => "failed",
            };
            if let Some(ch) = p.children.lock().unwrap().get_mut(thread) {
                ch.turn_id = None;
                ch.running = false;
            }
            store_msg(st, c.chat_id, c.project_id, c.run_id, "agent", json!({
                "ref": c.parent, "task_id": thread, "status": status, "summary": params["turn"]["error"]["message"], "background": true
            }));
            p.child_stopped(st, &c.root_thread);
        }
        _ => {}
    }
}

async fn server_request(st: &St, p: &Arc<CodexProc>, method: &str, v: Value) {
    let id = v["id"].clone();
    let params = &v["params"];
    let ctx = params["threadId"]
        .as_str()
        .and_then(|th| p.turns.lock().unwrap().get(th).map(|t| (t.chat_id, t.run_id)));
    let Some((chat_id, run_id)) = ctx else {
        p.send(json!({"id": id, "error": {"code": -32000, "message": "no active turn for this thread"}}));
        return;
    };
    match method {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
            let is_cmd = method.contains("commandExecution");
            let data = json!({
                "tool": if is_cmd { "Bash" } else { "Edit" },
                "title": if is_cmd { "Run command" } else { "Edit files" },
                "summary": if is_cmd { params["command"].as_str().unwrap_or("").to_string() } else { params["grantRoot"].as_str().unwrap_or("").to_string() },
                "reason": params["reason"],
                "input": params,
            });
            let ans = runner::ask(st, chat_id, Some(run_id), "approval", data).await;
            let decision = if ans["allow"] == true { "accept" } else { "decline" };
            p.send(json!({"id": id, "result": {"decision": decision}}));
        }
        "item/tool/requestUserInput" => {
            let qs: Vec<Value> = params["questions"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|q| {
                    question(
                        q["id"].as_str().unwrap_or(""),
                        q["header"].as_str().unwrap_or(""),
                        q["question"].as_str().unwrap_or(""),
                        false,
                        q["isOther"].as_bool().unwrap_or(true) || q["options"].is_null(),
                        q["options"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default()
                            .iter()
                            .map(|o| (o["label"].as_str().unwrap_or("").into(), o["description"].as_str().unwrap_or("").into()))
                            .collect(),
                    )
                })
                .collect();
            let ans = runner::ask(st, chat_id, Some(run_id), "question", json!({"tool": "codex", "questions": qs})).await;
            let mut answers = serde_json::Map::new();
            if let Some(obj) = ans["answers"].as_object() {
                for (k, a) in obj {
                    answers.insert(k.clone(), json!({"answers": a}));
                }
            }
            p.send(json!({"id": id, "result": {"answers": answers}}));
        }
        "item/permissions/requestApproval" => {
            let data = json!({
                "tool": "Permissions",
                "title": "Grant extra permissions",
                "summary": params["permissions"].to_string(),
                "reason": params["reason"],
                "input": params,
            });
            let ans = runner::ask(st, chat_id, Some(run_id), "approval", data).await;
            if ans["allow"] == true {
                let perms = params["permissions"].clone();
                let mut granted = serde_json::Map::new();
                if !perms["network"].is_null() {
                    granted.insert("network".into(), perms["network"].clone());
                }
                if !perms["fileSystem"].is_null() {
                    granted.insert("fileSystem".into(), perms["fileSystem"].clone());
                }
                p.send(json!({"id": id, "result": {"permissions": granted, "scope": "turn"}}));
            } else {
                p.send(json!({"id": id, "result": {"permissions": {}, "scope": "turn"}}));
            }
        }
        "mcpServer/elicitation/request" => {
            p.send(json!({"id": id, "result": {"action": "decline", "content": null, "_meta": null}}));
        }
        _ => {
            p.send(json!({"id": id, "error": {"code": -32601, "message": format!("{method} is not supported by cloud-ai")}}));
        }
    }
}

pub async fn run_turn(st: &St, chat: &Chat, project: &Project, run: &Run, atts: &[Attachment]) -> TurnOutcome {
    match run_turn_inner(st, chat, project, run, atts).await {
        Ok(o) => o,
        Err(e) => TurnOutcome::error(format!("{e:#}")),
    }
}

async fn run_turn_inner(st: &St, chat: &Chat, project: &Project, run: &Run, atts: &[Attachment]) -> Result<TurnOutcome> {
    let p = st.codex.get(st).await?;
    let (approval, sandbox) = if chat.read_only { ("never", "read-only") } else { codex_policy(&chat.permission_mode) };
    let model = (!chat.model.is_empty() && chat.model != "default").then(|| chat.model.clone());
    let mut sys = system_prompt(st, project);
    if chat.read_only {
        sys = format!("{}{}{}", super::READ_ONLY_PROMPT, if sys.is_empty() { "" } else { "\n\n" }, sys);
    }
    let mut thread_id = chat.cli_session_id.clone();
    if let Some(tid) = &thread_id {
        if !p.loaded.lock().unwrap().contains(tid) {
            let res = p
                .request(
                    "thread/resume",
                    json!({"threadId": tid, "cwd": chat.cwd, "approvalPolicy": approval, "sandbox": sandbox, "model": model,
                           "developerInstructions": if sys.is_empty() { Value::Null } else { json!(sys) }}),
                )
                .await;
            match res {
                Ok(res) => {
                    if let Some(m) = res["model"].as_str() {
                        p.thread_models.lock().unwrap().insert(tid.clone(), m.to_string());
                    }
                    p.loaded.lock().unwrap().insert(tid.clone());
                }
                Err(e) => {
                    tracing::warn!("codex resume {tid} failed ({e}); starting a new thread");
                    store_msg(st, chat.id, chat.project_id, run.id, "system", json!({"text": format!("Could not resume the previous Codex thread ({e}); started a new one.")}));
                    thread_id = None;
                }
            }
        }
    }
    let thread_id = match thread_id {
        Some(t) => t,
        None => {
            let res = p
                .request(
                    "thread/start",
                    json!({"cwd": chat.cwd, "approvalPolicy": approval, "sandbox": sandbox, "model": model,
                           "developerInstructions": if sys.is_empty() { Value::Null } else { json!(sys) }}),
                )
                .await?;
            let tid = res["thread"]["id"].as_str().ok_or_else(|| anyhow!("thread/start returned no id"))?.to_string();
            st.db.conn().execute("UPDATE chats SET cli_session_id=?2 WHERE id=?1", rusqlite::params![chat.id, tid])?;
            if let Some(m) = res["model"].as_str() {
                p.thread_models.lock().unwrap().insert(tid.clone(), m.to_string());
            }
            p.loaded.lock().unwrap().insert(tid.clone());
            tid
        }
    };
    let turn_model = model
        .clone()
        .or_else(|| p.thread_models.lock().unwrap().get(&thread_id).cloned())
        .unwrap_or_else(|| "default".into());
    let (tx, rx) = oneshot::channel();
    p.turns.lock().unwrap().insert(
        thread_id.clone(),
        TurnCtx {
            chat_id: chat.id,
            project_id: chat.project_id,
            run_id: run.id,
            turn_id: None,
            done: Some(tx),
            last_text: String::new(),
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: 0,
            reasoning_tokens: 0,
            model: turn_model,
            failure: None,
            ctx_used: 0,
            ctx_max: 0,
        },
    );
    let mut input = vec![json!({"type": "text", "text": prompt_with_attachments(&run.prompt, atts), "text_elements": []})];
    for a in atts {
        if a.mime.starts_with("image/") {
            input.push(json!({"type": "localImage", "path": a.path}));
        }
    }
    let mut params = json!({"threadId": thread_id, "input": input, "approvalPolicy": approval});
    if let Some(m) = &model {
        params["model"] = json!(m);
    }
    if !chat.effort.is_empty() {
        params["effort"] = json!(chat.effort);
    }
    match p.request("turn/start", params).await {
        Ok(res) => {
            if let Some(tid) = res["turn"]["id"].as_str() {
                if let Some(t) = p.turns.lock().unwrap().get_mut(&thread_id) {
                    t.turn_id = Some(tid.to_string());
                }
            }
        }
        Err(e) => {
            p.turns.lock().unwrap().remove(&thread_id);
            return Err(e);
        }
    }
    Ok(rx.await.unwrap_or_else(|_| TurnOutcome::error("Codex stopped")))
}

/// Ask Codex for the account's current plan usage. Uses the running app-server
/// when there is one, otherwise a short-lived one.
pub async fn refresh_rate_limits(st: &St) -> Result<Value> {
    let shared = st.codex.current().await;
    let (p, temp) = match shared {
        Some(p) => (p, false),
        None => (start_proc(st).await?, true),
    };
    let res = p.request("account/rateLimits/read", json!({})).await;
    if temp {
        p.kill();
    }
    let v = res?["rateLimits"].clone();
    if v.is_null() {
        return Err(anyhow!("Codex returned no usage (is it logged in with ChatGPT?)"));
    }
    runner::set_rate_limits(st, "codex", v.clone());
    Ok(v)
}

/// Keep Codex plan usage fresh while Codex is logged in with ChatGPT.
pub fn spawn_usage_refresh(st: St) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(20)).await;
        loop {
            let logged_in = !crate::store::get_cli_info(&st.db, "codex_account")["account"].is_null();
            if logged_in {
                if let Err(e) = refresh_rate_limits(&st).await {
                    tracing::debug!("codex usage refresh: {e:#}");
                }
            }
            tokio::time::sleep(Duration::from_secs(15 * 60)).await;
        }
    });
}

/// Read the model catalog (and account) from a short-lived app-server.
pub async fn fetch_models(st: &St) -> Result<Vec<Value>> {
    let p = start_proc(st).await?;
    let res = async {
        let mut out = Vec::new();
        let mut cursor = Value::Null;
        loop {
            let r = p.request("model/list", json!({"cursor": cursor, "includeHidden": false})).await?;
            out.extend(r["data"].as_array().cloned().unwrap_or_default());
            cursor = r["nextCursor"].clone();
            if cursor.is_null() {
                break;
            }
        }
        if let Ok(acc) = p.request("account/read", json!({})).await {
            crate::store::set_cli_info(st, "codex_account", &acc);
        }
        Ok::<_, anyhow::Error>(out)
    }
    .await;
    p.kill();
    res
}
