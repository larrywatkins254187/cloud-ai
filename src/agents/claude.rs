//! Claude Code driven through its stream-json stdio protocol (the same one the
//! Agent SDK uses): one long-lived `claude -p` process per active chat.
use super::{claude_permission_mode, prompt_with_attachments, question, system_prompt, Attachment, TurnOutcome};
use crate::events::Event;
use crate::runner;
use crate::settings;
use crate::state::St;
use crate::store::{self, insert_message, Chat, Project, Run};
use crate::util::{tail, truncate};
use anyhow::{Context, Result};
use base64::Engine;
use serde_json::{json, Value};
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
    stderr: Mutex<String>,
    ctrl_seq: AtomicU64,
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
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.start_kill();
        }
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
            stderr: Mutex::new(String::new()),
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
                        "tool_use" => self.store(
                            st,
                            "tool_use",
                            json!({"id": b["id"], "name": b["name"], "input": b["input"], "parent": parent}),
                        ),
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
                        }
                    }
                }
            }
            "result" => {
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
                let is_error = v["is_error"].as_bool().unwrap_or(false) || v["subtype"] != "success";
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
                    };
                    if let Some(done) = t.done.take() {
                        let _ = done.send(outcome);
                    }
                }
                *self.last_used.lock().unwrap() = Instant::now();
            }
            "rate_limit_event" => {
                st.runner.rate_limits.lock().unwrap().insert("claude".into(), v["rate_limit_info"].clone());
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
