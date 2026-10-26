//! Run queue: picks queued prompts, respects per-tool concurrency limits and
//! the RAM guard, drives the agent and records the outcome.
use crate::agents::claude::ClaudeSession;
use crate::agents::{parse_attachments, TurnOutcome};
use crate::events::Event;
use crate::settings;
use crate::state::St;
use crate::store::{self, get_chat, get_run, insert_message, Request, Run};
use crate::util::{meminfo, now};
use anyhow::Result;
use rusqlite::params;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{oneshot, Notify};

#[derive(Default)]
pub struct RunnerState {
    pub wake: Notify,
    /// Live Claude processes, by chat id.
    pub claude: Mutex<HashMap<i64, Arc<ClaudeSession>>>,
    /// Requests (approvals/questions) waiting for an answer, by request id.
    pub pending: Mutex<HashMap<i64, oneshot::Sender<Value>>>,
    /// Latest plan usage / rate limit info reported by each CLI.
    pub rate_limits: Mutex<HashMap<String, Value>>,
    /// Runs being cancelled by the user.
    pub cancelling: Mutex<HashSet<i64>>,
    pub ram_blocked_since: Mutex<Option<Instant>>,
}

/// Remember the latest plan usage reported by a CLI, in memory and in the DB
/// so the dashboard still has it after a restart.
pub fn set_rate_limits(st: &St, tool: &str, mut v: Value) {
    if v.is_null() {
        return;
    }
    if let Some(o) = v.as_object_mut() {
        o.insert("updatedAt".into(), json!(crate::util::now()));
    }
    crate::store::set_cli_info(st, &format!("rate_limits_{tool}"), &v);
    st.runner.rate_limits.lock().unwrap().insert(tool.into(), v);
}

/// Plan usage per CLI: what the CLIs reported since start, else the saved copy.
pub fn rate_limits(st: &St) -> Value {
    let mut out = serde_json::Map::new();
    let mem = st.runner.rate_limits.lock().unwrap().clone();
    for tool in ["claude", "codex"] {
        let v = mem.get(tool).cloned().unwrap_or_else(|| crate::store::get_cli_info(&st.db, &format!("rate_limits_{tool}")));
        if !v.is_null() {
            out.insert(tool.into(), v);
        }
    }
    Value::Object(out)
}

pub struct NewRun<'a> {
    pub chat_id: i64,
    pub user_id: Option<i64>,
    pub source: &'a str,
    pub schedule_id: Option<i64>,
    pub prompt: &'a str,
    pub attachments: Value,
    pub chain: Vec<String>,
    /// Sender shown instead of the panel user (Telegram display name).
    pub author: Option<String>,
}

pub fn enqueue(st: &St, r: NewRun) -> Result<Run> {
    let id = {
        let c = st.db.conn();
        c.execute(
            "INSERT INTO runs(chat_id,user_id,source,schedule_id,prompt,attachments,chain,status,created_at,author) VALUES(?1,?2,?3,?4,?5,?6,?7,'queued',?8,?9)",
            params![r.chat_id, r.user_id, r.source, r.schedule_id, r.prompt, r.attachments.to_string(), json!(r.chain).to_string(), now(), r.author],
        )?;
        c.last_insert_rowid()
    };
    let chat = get_chat(&st.db, r.chat_id)?;
    if chat.status == "idle" || chat.status == "error" {
        store::set_chat_status(&st.db, chat.id, "queued")?;
        emit_chat(st, chat.id);
    }
    let run = get_run(&st.db, id)?;
    st.emit_project(chat.project_id, Event::RunQueued { run: run.clone() });
    st.runner.wake.notify_one();
    Ok(run)
}

pub fn emit_chat(st: &St, chat_id: i64) {
    if let Ok(chat) = get_chat(&st.db, chat_id) {
        st.emit_project(chat.project_id, Event::ChatUpdated { chat });
    }
}

/// On startup nothing is running any more: fail interrupted runs and expire requests.
pub fn recover(st: &St) {
    let c = st.db.conn();
    let _ = c.execute(
        "UPDATE runs SET status='error', error='Server restarted while this prompt was running', finished_at=?1 WHERE status='running'",
        [now()],
    );
    let _ = c.execute("UPDATE requests SET status='expired' WHERE status='pending'", []);
    let _ = c.execute("UPDATE chats SET status='idle' WHERE status IN ('running','waiting')", []);
    let _ = c.execute(
        "UPDATE chats SET status='queued' WHERE id IN (SELECT chat_id FROM runs WHERE status='queued')",
        [],
    );
}

pub fn spawn(st: St) {
    let s = st.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = s.runner.wake.notified() => {}
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            }
            if let Err(e) = dispatch(&s).await {
                tracing::warn!("dispatch: {e:#}");
            }
        }
    });
    // Reaper: stop idle agent processes so they don't hold RAM.
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            let idle = Duration::from_secs(settings::get_i64(&st.db, "idle_kill_minutes").max(1) as u64 * 60);
            reap_idle(&st, idle);
            st.codex.reap_idle(idle).await;
        }
    });
}

pub fn reap_idle(st: &St, idle: Duration) -> usize {
    let victims: Vec<Arc<ClaudeSession>> = {
        let map = st.runner.claude.lock().unwrap();
        map.values().filter(|s| !s.is_busy() && s.idle_for() >= idle).cloned().collect()
    };
    for s in &victims {
        s.kill();
    }
    let mut map = st.runner.claude.lock().unwrap();
    map.retain(|_, s| s.is_alive());
    victims.len()
}

async fn dispatch(st: &St) -> Result<()> {
    let queued: Vec<(i64, i64, String)> = {
        let c = st.db.conn();
        let mut q = c.prepare(
            "SELECT r.id, r.chat_id, c.tool FROM runs r JOIN chats c ON c.id=r.chat_id WHERE r.status='queued' ORDER BY r.id",
        )?;
        let rows = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
        rows
    };
    if queued.is_empty() {
        return Ok(());
    }
    let (mut counts, mut busy): (HashMap<String, i64>, HashSet<i64>) = {
        let c = st.db.conn();
        let mut q = c.prepare("SELECT r.chat_id, c.tool FROM runs r JOIN chats c ON c.id=r.chat_id WHERE r.status='running'")?;
        let mut counts = HashMap::new();
        let mut busy = HashSet::new();
        for row in q.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
            let (chat, tool) = row?;
            *counts.entry(tool).or_insert(0) += 1;
            busy.insert(chat);
        }
        (counts, busy)
    };
    for (run_id, chat_id, tool) in queued {
        if busy.contains(&chat_id) {
            continue;
        }
        let max = settings::get_i64(&st.db, if tool == "codex" { "max_codex" } else { "max_claude" }).max(1);
        if *counts.get(&tool).unwrap_or(&0) >= max {
            continue;
        }
        if !ram_ok(st) {
            // Free memory held by idle processes, then retry once.
            reap_idle(st, Duration::ZERO);
            st.codex.reap_idle(Duration::ZERO).await;
            if !ram_ok(st) {
                let mut since = st.runner.ram_blocked_since.lock().unwrap();
                if since.is_none() {
                    *since = Some(Instant::now());
                    drop(since);
                    st.notice("warn", format!(
                        "Low memory: queued prompts wait until at least {} MB RAM is free",
                        settings::get_i64(&st.db, "ram_guard_mb")
                    ));
                }
                return Ok(());
            }
        }
        *st.runner.ram_blocked_since.lock().unwrap() = None;
        let updated = st.db.conn().execute(
            "UPDATE runs SET status='running', started_at=?2 WHERE id=?1 AND status='queued'",
            params![run_id, now()],
        )?;
        if updated == 0 {
            continue;
        }
        busy.insert(chat_id);
        *counts.entry(tool).or_insert(0) += 1;
        let s = st.clone();
        tokio::spawn(async move {
            if let Err(e) = execute(&s, run_id).await {
                tracing::warn!("run {run_id}: {e:#}");
                let _ = finish(&s, run_id, TurnOutcome::error(format!("{e:#}")));
            }
            s.runner.wake.notify_one();
        });
    }
    Ok(())
}

fn ram_ok(st: &St) -> bool {
    let guard = settings::get_i64(&st.db, "ram_guard_mb").max(0) as u64;
    let m = meminfo();
    m.total_mb == 0 || m.available_mb >= guard
}

async fn execute(st: &St, run_id: i64) -> Result<()> {
    let run = get_run(&st.db, run_id)?;
    let chat = get_chat(&st.db, run.chat_id)?;
    let project = store::get_project(&st.db, chat.project_id)?;
    store::set_chat_status(&st.db, chat.id, "running")?;
    emit_chat(st, chat.id);
    st.emit_project(chat.project_id, Event::RunStarted { run: run.clone() });

    let user_msg = insert_message(
        &st.db,
        chat.id,
        Some(run.id),
        "user",
        &json!({
            "text": run.prompt,
            "attachments": run.attachments,
            "source": run.source,
            "user": run.author.clone().unwrap_or_else(|| store::username(&st.db, run.user_id)),
        }),
    )?;
    st.emit_project(chat.project_id, Event::Message { message: user_msg });

    let atts = parse_attachments(&run.attachments);
    let outcome = if chat.tool == "codex" {
        crate::agents::codex::run_turn(st, &chat, &project, &run, &atts).await
    } else {
        let session = ClaudeSession::get_or_spawn(st, &chat, &project).await?;
        let out = session.run_turn(st, &run, &atts).await;
        if out.error.as_deref().map(|e| e.contains("No conversation found")).unwrap_or(false) {
            // The saved session is gone (deleted transcript, other machine…): start a fresh one.
            drop_session(st, chat.id);
            st.db.conn().execute("UPDATE chats SET cli_session_id=NULL WHERE id=?1", [chat.id])?;
            let note = insert_message(&st.db, chat.id, Some(run.id), "system", &json!({"text": "The previous Claude session could not be resumed, so a new one was started."}))?;
            st.emit_project(chat.project_id, Event::Message { message: note });
            let chat = get_chat(&st.db, chat.id)?;
            let session = ClaudeSession::get_or_spawn(st, &chat, &project).await?;
            session.run_turn(st, &run, &atts).await
        } else {
            out
        }
    };
    finish(st, run_id, outcome)
}

fn finish(st: &St, run_id: i64, mut outcome: TurnOutcome) -> Result<()> {
    if st.runner.cancelling.lock().unwrap().remove(&run_id) && outcome.status != "done" {
        outcome.status = "cancelled".into();
    }
    let run = get_run(&st.db, run_id)?;
    {
        let c = st.db.conn();
        c.execute(
            "UPDATE runs SET status=?2, error=?3, result_text=?4, cost_usd=?5, input_tokens=?6, output_tokens=?7, finished_at=?8 WHERE id=?1",
            params![
                run_id,
                outcome.status,
                outcome.error,
                outcome.result_text,
                outcome.cost_usd,
                outcome.input_tokens,
                outcome.output_tokens,
                now()
            ],
        )?;
        c.execute(
            "UPDATE chats SET cost_usd=cost_usd+?2, input_tokens=input_tokens+?3, output_tokens=output_tokens+?4 WHERE id=?1",
            params![run.chat_id, outcome.cost_usd.unwrap_or(0.0), outcome.input_tokens, outcome.output_tokens],
        )?;
    }
    expire_requests_of_run(st, run_id);
    if let Some(err) = &outcome.error {
        if outcome.status == "error" {
            let m = insert_message(&st.db, run.chat_id, Some(run_id), "error", &json!({ "text": err }))?;
            st.emit_project(get_chat(&st.db, run.chat_id)?.project_id, Event::Message { message: m });
        }
    }
    let has_more: i64 = st.db.conn().query_row(
        "SELECT COUNT(*) FROM runs WHERE chat_id=?1 AND status='queued'",
        [run.chat_id],
        |r| r.get(0),
    )?;
    let next_status = if has_more > 0 {
        "queued"
    } else if outcome.status == "error" {
        "error"
    } else {
        "idle"
    };
    store::set_chat_status(&st.db, run.chat_id, next_status)?;
    let chat = get_chat(&st.db, run.chat_id)?;
    st.emit_project(chat.project_id, Event::ChatUpdated { chat: chat.clone() });
    let run = get_run(&st.db, run_id)?;
    st.emit_project(chat.project_id, Event::RunFinished { run: run.clone() });

    if outcome.status == "done" {
        let chain: Vec<String> = serde_json::from_value(run.chain.clone()).unwrap_or_default();
        if let Some((next, rest)) = chain.split_first() {
            enqueue(
                st,
                NewRun {
                    chat_id: chat.id,
                    user_id: run.user_id,
                    source: &run.source,
                    schedule_id: run.schedule_id,
                    prompt: next,
                    attachments: json!([]),
                    chain: rest.to_vec(),
                    author: run.author.clone(),
                },
            )?;
        } else if chat.auto_deploy {
            if let Ok(project) = store::get_project(&st.db, chat.project_id) {
                if project.deployable() {
                    let label = format!("auto (chat #{})", chat.id);
                    // Report into the chat's Telegram topic when it has one.
                    let thread = store::get_chat(&st.db, chat.id).ok().and_then(|c| c.tg_topic_id).filter(|t| *t > 0);
                    crate::deploy::start(st, project.id, &label, None, thread);
                }
            }
        }
    }
    Ok(())
}

// ---------- approvals and questions ----------

/// Store a request, notify everyone and wait for the answer.
pub async fn ask(st: &St, chat_id: i64, run_id: Option<i64>, kind: &str, data: Value) -> Value {
    let created = (|| -> Result<Request> {
        let id = {
            let c = st.db.conn();
            c.execute(
                "INSERT INTO requests(chat_id,run_id,kind,data,created_at) VALUES(?1,?2,?3,?4,?5)",
                params![chat_id, run_id, kind, data.to_string(), now()],
            )?;
            c.last_insert_rowid()
        };
        Ok(store::get_request(&st.db, id)?)
    })();
    let req = match created {
        Ok(r) => r,
        Err(e) => return json!({ "allow": false, "message": format!("{e}") }),
    };
    let (tx, rx) = oneshot::channel();
    st.runner.pending.lock().unwrap().insert(req.id, tx);
    let _ = store::set_chat_status(&st.db, chat_id, "waiting");
    emit_chat(st, chat_id);
    if let Ok(chat) = get_chat(&st.db, chat_id) {
        st.emit_project(chat.project_id, Event::RequestCreated { request: req.clone() });
    }
    let answer = rx.await.unwrap_or_else(|_| json!({ "allow": false, "message": "Request expired" }));
    let _ = store::set_chat_status(&st.db, chat_id, "running");
    emit_chat(st, chat_id);
    answer
}

/// Answer a pending request (from the web UI or Telegram).
pub fn answer(st: &St, request_id: i64, answer: Value, by: &str) -> Result<Request> {
    let req = store::get_request(&st.db, request_id)?;
    if req.status != "pending" {
        anyhow::bail!("This request was already {}", req.status);
    }
    let tx = st.runner.pending.lock().unwrap().remove(&request_id);
    let Some(tx) = tx else {
        st.db.conn().execute("UPDATE requests SET status='expired' WHERE id=?1", [request_id])?;
        anyhow::bail!("This request is no longer active");
    };
    st.db.conn().execute(
        "UPDATE requests SET status='answered', answer=?2, answered_by=?3, answered_at=?4 WHERE id=?1",
        params![request_id, answer.to_string(), by, now()],
    )?;
    let _ = tx.send(answer);
    let req = store::get_request(&st.db, request_id)?;
    if let Ok(chat) = get_chat(&st.db, req.chat_id) {
        st.emit_project(chat.project_id, Event::RequestResolved { request: req.clone() });
    }
    Ok(req)
}

fn expire_requests_of_run(st: &St, run_id: i64) {
    let ids: Vec<i64> = {
        let c = st.db.conn();
        let mut q = match c.prepare("SELECT id FROM requests WHERE run_id=?1 AND status='pending'") {
            Ok(q) => q,
            Err(_) => return,
        };
        q.query_map([run_id], |r| r.get(0)).map(|it| it.filter_map(Result::ok).collect()).unwrap_or_default()
    };
    for id in ids {
        st.runner.pending.lock().unwrap().remove(&id);
        let _ = st.db.conn().execute("UPDATE requests SET status='expired' WHERE id=?1", [id]);
        if let Ok(req) = store::get_request(&st.db, id) {
            if let Ok(chat) = get_chat(&st.db, req.chat_id) {
                st.emit_project(chat.project_id, Event::RequestResolved { request: req });
            }
        }
    }
}

/// Stop a queued or running prompt.
pub async fn cancel_run(st: &St, run_id: i64) -> Result<()> {
    let run = get_run(&st.db, run_id)?;
    match run.status.as_str() {
        "queued" => {
            st.db.conn().execute("UPDATE runs SET status='cancelled', finished_at=?2 WHERE id=?1", params![run_id, now()])?;
            let remaining: i64 = st.db.conn().query_row(
                "SELECT COUNT(*) FROM runs WHERE chat_id=?1 AND status IN ('queued','running')",
                [run.chat_id],
                |r| r.get(0),
            )?;
            if remaining == 0 {
                store::set_chat_status(&st.db, run.chat_id, "idle")?;
            }
            emit_chat(st, run.chat_id);
            let chat = get_chat(&st.db, run.chat_id)?;
            st.emit_project(chat.project_id, Event::RunFinished { run: get_run(&st.db, run_id)? });
        }
        "running" => {
            st.runner.cancelling.lock().unwrap().insert(run_id);
            let chat = get_chat(&st.db, run.chat_id)?;
            // Deny anything the agent is waiting on so it can stop.
            let pending: Vec<i64> = {
                let c = st.db.conn();
                let mut q = c.prepare("SELECT id FROM requests WHERE run_id=?1 AND status='pending'")?;
                let v = q.query_map([run_id], |r| r.get(0))?.filter_map(Result::ok).collect();
                v
            };
            for id in pending {
                let _ = answer(st, id, json!({ "allow": false, "message": "Stopped by user" }), "system");
            }
            if chat.tool == "codex" {
                st.codex.interrupt(chat.id).await;
            } else {
                let s = st.runner.claude.lock().unwrap().get(&chat.id).cloned();
                if let Some(s) = s {
                    s.interrupt();
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Stop one sub-agent of a chat, identified by the tool call that started it.
/// When the CLI can't stop it on its own, the whole running prompt is stopped.
/// Returns "agent" or "turn" to tell the caller which happened.
pub async fn stop_agent(st: &St, chat_id: i64, tool_use_id: &str) -> Result<&'static str> {
    let chat = get_chat(&st.db, chat_id)?;
    let stopped = if chat.tool == "codex" {
        st.codex.interrupt_agent(chat_id, tool_use_id).await
    } else {
        let s = st.runner.claude.lock().unwrap().get(&chat_id).cloned();
        s.map(|s| s.stop_task(tool_use_id)).unwrap_or(false)
    };
    if stopped {
        return Ok("agent");
    }
    let running: Vec<i64> = {
        let c = st.db.conn();
        let mut q = c.prepare("SELECT id FROM runs WHERE chat_id=?1 AND status='running'")?;
        let v = q.query_map([chat_id], |r| r.get(0))?.filter_map(Result::ok).collect();
        v
    };
    for r in running {
        cancel_run(st, r).await?;
    }
    Ok("turn")
}

/// Kill the agent process of a chat (used when settings change or the chat is deleted).
pub fn drop_session(st: &St, chat_id: i64) {
    if let Some(s) = st.runner.claude.lock().unwrap().remove(&chat_id) {
        s.kill();
    }
}

pub fn active_counts(st: &St) -> Value {
    let c = st.db.conn();
    let count = |sql: &str| -> i64 { c.query_row(sql, [], |r| r.get(0)).unwrap_or(0) };
    json!({
        "running_claude": count("SELECT COUNT(*) FROM runs r JOIN chats c ON c.id=r.chat_id WHERE r.status='running' AND c.tool='claude'"),
        "running_codex": count("SELECT COUNT(*) FROM runs r JOIN chats c ON c.id=r.chat_id WHERE r.status='running' AND c.tool='codex'"),
        "queued": count("SELECT COUNT(*) FROM runs WHERE status='queued'"),
        "waiting": count("SELECT COUNT(*) FROM requests WHERE status='pending'"),
    })
}
