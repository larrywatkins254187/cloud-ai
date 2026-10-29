//! Telegram bot: one forum topic per chat in a single group, reports of chats,
//! prompts and results, approvals/questions with inline buttons (answers sync
//! with the web UI both ways) and deploy/rollback buttons.
use crate::events::Event;
use crate::settings;
use crate::state::St;
use crate::store::{self, get_chat, get_project, Chat, Request, Run};
use crate::util::{html_escape, truncate};
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;

#[derive(Default)]
pub struct TelegramState {
    /// request id → messages sent for it (message id, text)
    req_msgs: Mutex<HashMap<i64, Vec<(i64, String)>>>,
    /// request id → (question id → picked answers)
    partial: Mutex<HashMap<i64, HashMap<String, Vec<String>>>>,
    /// request id → question ids finished (multi-select submitted)
    done_q: Mutex<HashMap<i64, HashSet<String>>>,
    /// telegram message id → (request id, question index), for typed replies
    reply_map: Mutex<HashMap<i64, (i64, usize)>>,
    topic_lock: tokio::sync::Mutex<()>,
    /// deploys whose "started" message was already sent
    deploy_started: Mutex<HashSet<i64>>,
    /// `/new` picker message id → first prompt typed after the command
    pending_new: Mutex<HashMap<i64, String>>,
}

fn token(st: &St) -> Option<String> {
    let t = settings::get(&st.db, "telegram_token");
    (!t.trim().is_empty()).then(|| t.trim().to_string())
}

fn group(st: &St) -> Option<String> {
    let g = settings::get(&st.db, "telegram_chat_id");
    (!g.trim().is_empty()).then(|| g.trim().to_string())
}

pub async fn call(st: &St, method: &str, body: Value) -> Result<Value> {
    let token = token(st).ok_or_else(|| anyhow!("Telegram bot token not set"))?;
    // CLOUD_AI_TELEGRAM_API points the bot at another Bot API server (tests, self-hosted API).
    let base = std::env::var("CLOUD_AI_TELEGRAM_API").unwrap_or_else(|_| "https://api.telegram.org".into());
    let url = format!("{}/bot{token}/{method}", base.trim_end_matches('/'));
    let resp = st
        .http
        .post(&url)
        .timeout(Duration::from_secs(if method == "getUpdates" { 60 } else { 20 }))
        .json(&body)
        .send()
        .await?;
    let v: Value = resp.json().await?;
    if v["ok"] != true {
        bail!("telegram {method}: {}", v["description"].as_str().unwrap_or("error"));
    }
    Ok(v["result"].clone())
}

async fn topic_for(st: &St, chat: &Chat) -> Option<i64> {
    let _g = st.telegram.topic_lock.lock().await;
    let fresh = get_chat(&st.db, chat.id).ok()?;
    if let Some(t) = fresh.tg_topic_id {
        return (t > 0).then_some(t);
    }
    let gid = group(st)?;
    let project = get_project(&st.db, chat.project_id).ok()?;
    let name = truncate(&format!("{} · {} #{}", project.name, chat.title, chat.id), 120);
    let icon = if chat.tool == "codex" { 0x6FB9F0 } else { 0xFF93B2 };
    let topic = match call(st, "createForumTopic", json!({"chat_id": gid, "name": name, "icon_color": icon})).await {
        Ok(v) => v["message_thread_id"].as_i64().unwrap_or(0),
        Err(e) => {
            tracing::info!("telegram topics unavailable ({e}); posting to the group");
            0
        }
    };
    let _ = st.db.conn().execute("UPDATE chats SET tg_topic_id=?2 WHERE id=?1", rusqlite::params![chat.id, topic]);
    (topic > 0).then_some(topic)
}

/// Send an HTML message to the group (into the chat's topic when given).
pub async fn send(st: &St, chat: Option<&Chat>, text: &str, keyboard: Option<Value>) -> Result<i64> {
    let thread = match chat {
        Some(c) => topic_for(st, c).await,
        None => None,
    };
    send_to(st, thread, text, keyboard).await
}

/// Send an HTML message to the group, into forum topic `thread` (None = General).
async fn send_to(st: &St, thread: Option<i64>, text: &str, keyboard: Option<Value>) -> Result<i64> {
    let gid = group(st).ok_or_else(|| anyhow!("Telegram group not set"))?;
    let mut body = json!({
        "chat_id": gid,
        "text": truncate(text, 4000),
        "parse_mode": "HTML",
        "disable_web_page_preview": true,
    });
    if let Some(t) = thread.filter(|t| *t > 0) {
        body["message_thread_id"] = json!(t);
    }
    if let Some(k) = keyboard {
        body["reply_markup"] = json!({"inline_keyboard": k});
    }
    let r = call(st, "sendMessage", body).await?;
    Ok(r["message_id"].as_i64().unwrap_or(0))
}

async fn edit(st: &St, message_id: i64, text: &str, keyboard: Option<Value>) {
    let Some(gid) = group(st) else { return };
    let mut body = json!({
        "chat_id": gid, "message_id": message_id, "text": truncate(text, 4000),
        "parse_mode": "HTML", "disable_web_page_preview": true,
    });
    body["reply_markup"] = json!({"inline_keyboard": keyboard.unwrap_or(json!([]))});
    if let Err(e) = call(st, "editMessageText", body).await {
        tracing::debug!("telegram edit: {e}");
    }
}

fn link(st: &St, path: &str) -> String {
    let base = settings::get(&st.db, "public_url");
    if base.is_empty() {
        String::new()
    } else {
        format!("\n<a href=\"{}{}\">Open in panel</a>", base.trim_end_matches('/'), path)
    }
}

fn header(st: &St, chat: &Chat) -> String {
    let project = get_project(&st.db, chat.project_id).map(|p| p.name).unwrap_or_default();
    format!("<b>{}</b> · {}", html_escape(&project), html_escape(&chat.title))
}

fn tool_line(chat: &Chat) -> String {
    let tool = if chat.tool == "codex" { "Codex" } else { "Claude Code" };
    let model = if chat.model.is_empty() { "default model" } else { &chat.model };
    let effort = if chat.effort.is_empty() { String::new() } else { format!(" · {}", chat.effort) };
    let mode = match chat.permission_mode.as_str() {
        "edits" => "auto-edit",
        "ask" => "ask before changes",
        "plan" => "plan only",
        _ => "full access",
    };
    format!("{tool} · {}{effort} · {mode}", html_escape(model))
}

fn fmt_duration(run: &Run) -> String {
    let (Some(a), Some(b)) = (&run.started_at, &run.finished_at) else { return String::new() };
    let (Ok(a), Ok(b)) = (chrono::DateTime::parse_from_rfc3339(a), chrono::DateTime::parse_from_rfc3339(b)) else {
        return String::new();
    };
    let s = (b - a).num_seconds().max(0);
    if s >= 60 { format!("{}m {}s", s / 60, s % 60) } else { format!("{s}s") }
}

pub fn spawn(st: St) {
    let s = st.clone();
    tokio::spawn(async move { event_loop(s).await });
    tokio::spawn(async move { update_loop(st).await });
}

async fn event_loop(st: St) {
    let mut rx = st.bus.subscribe();
    loop {
        let env = match rx.recv().await {
            Ok(e) => e,
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => return,
        };
        // Streaming deltas and logs are most of the traffic and never reach Telegram.
        if matches!(env.event, Event::Delta { .. } | Event::DeployLog { .. } | Event::Message { .. } | Event::ChatUpdated { .. } | Event::RunQueued { .. }) {
            continue;
        }
        if token(&st).is_none() || group(&st).is_none() {
            continue;
        }
        if let Err(e) = on_event(&st, env.event).await {
            tracing::debug!("telegram: {e:#}");
        }
    }
}

async fn on_event(st: &St, ev: Event) -> Result<()> {
    match ev {
        Event::ChatCreated { chat } => {
            let by = store::username(&st.db, chat.user_id);
            let src = if chat.schedule_id.is_some() { " (schedule)" } else { "" };
            let text = format!(
                "🆕 New chat by <b>{}</b>{src}\n{}\n{}{}",
                html_escape(&by),
                header(st, &chat),
                tool_line(&chat),
                link(st, &format!("/chats/{}", chat.id))
            );
            send(st, Some(&chat), &text, None).await?;
        }
        Event::RunStarted { run } => {
            let chat = get_chat(&st.db, run.chat_id)?;
            if run.source == "telegram" {
                // The prompt is already in the topic; just show that the agent is working.
                spawn_typing(st.clone(), run.id, chat.tg_topic_id.filter(|t| *t > 0));
                return Ok(());
            }
            if !settings::get_bool(&st.db, "telegram_send_prompts") {
                return Ok(());
            }
            let by = store::username(&st.db, run.user_id);
            let icon = if run.source == "schedule" { "⏰" } else { "▶️" };
            let att = run.attachments.as_array().map(|a| a.len()).unwrap_or(0);
            let att = if att > 0 { format!("\n📎 {att} attachment(s)") } else { String::new() };
            let text = format!(
                "{icon} <b>{}</b> → {}\n\n{}{att}",
                html_escape(&by),
                tool_line(&chat),
                html_escape(&truncate(&run.prompt, 3000))
            );
            send(st, Some(&chat), &text, None).await?;
        }
        Event::RunFinished { run } => {
            if run.status == "queued" || run.status == "running" {
                return Ok(());
            }
            let chat = get_chat(&st.db, run.chat_id)?;
            let project = get_project(&st.db, chat.project_id)?;
            let mut stats = vec![fmt_duration(&run)];
            if let Some(c) = run.cost_usd.filter(|c| *c > 0.0) {
                stats.push(if c < 0.01 { "<$0.01".to_string() } else { format!("${c:.2}") });
            }
            let tok = run.input_tokens.unwrap_or(0) + run.output_tokens.unwrap_or(0);
            if tok > 0 {
                stats.push(format!("{}k tokens", tok / 1000));
            }
            let stats = stats.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
            let (text, kb) = match run.status.as_str() {
                "done" => {
                    let kb = project
                        .deployable()
                        .then(|| json!([[{"text": "🚀 Deploy", "callback_data": format!("dp:{}", project.id)}]]));
                    let footer = format!("✅ <b>Done</b> · {stats}{}", link(st, &format!("/chats/{}", chat.id)));
                    let answer = run.result_text.as_deref().unwrap_or("").trim();
                    // Prompts sent from Telegram always get their answer back there.
                    if !answer.is_empty() && (run.source == "telegram" || settings::get_bool(&st.db, "telegram_send_results")) {
                        let chunks = answer_chunks(answer);
                        let last = chunks.len() - 1;
                        for (i, c) in chunks.into_iter().enumerate() {
                            if i == last {
                                send(st, Some(&chat), &format!("{c}\n\n{footer}"), kb.clone()).await?;
                            } else {
                                send(st, Some(&chat), &c, None).await?;
                            }
                        }
                        return Ok(());
                    }
                    (footer, kb)
                }
                "cancelled" => (format!("⏹ <b>Stopped</b> · {stats}"), None),
                _ => (
                    format!("❌ <b>Failed</b> · {stats}\n\n{}", html_escape(&truncate(run.error.as_deref().unwrap_or(""), 2500))),
                    None,
                ),
            };
            send(st, Some(&chat), &text, kb).await?;
        }
        Event::RequestCreated { request } => send_request(st, &request).await?,
        Event::RequestResolved { request } => resolve_request(st, &request).await,
        Event::DeployUpdated { deploy } => {
            let project = get_project(&st.db, deploy.project_id)?;
            let what = if deploy.kind == "rollback" { "Rollback" } else { "Deploy" };
            let sha = deploy.commit_sha.as_deref().map(|s| format!(" <code>{}</code>", &s[..s.len().min(8)])).unwrap_or_default();
            if deploy.status == "running" && !st.telegram.deploy_started.lock().unwrap().insert(deploy.id) {
                return Ok(());
            }
            let text = match deploy.status.as_str() {
                "running" => format!("🚀 {what} of <b>{}</b> started by {}{sha}", html_escape(&project.name), html_escape(&deploy.triggered_by)),
                "success" => format!(
                    "✅ {what} of <b>{}</b> succeeded{sha}{}",
                    html_escape(&project.name),
                    deploy.health.as_deref().map(|h| format!("\nHealth: {}", html_escape(h))).unwrap_or_default()
                ),
                _ => format!(
                    "❌ {what} of <b>{}</b> failed{sha}\n<pre>{}</pre>",
                    html_escape(&project.name),
                    html_escape(&crate::util::tail(&deploy.log, 1500))
                ),
            };
            let kb = (deploy.status == "success" && deploy.prev_sha.is_some())
                .then(|| json!([[{"text": "↩️ Rollback to previous", "callback_data": format!("rb:{}", deploy.id)}]]));
            send_to(st, deploy.tg_thread_id, &text, kb).await?;
        }
        Event::Notice { level, text } => {
            let icon = match level.as_str() {
                "error" => "🛑",
                "warn" => "⚠️",
                _ => "ℹ️",
            };
            send(st, None, &format!("{icon} {}", html_escape(&text)), None).await?;
        }
        _ => {}
    }
    Ok(())
}

fn approval_text(st: &St, chat: &Chat, r: &Request) -> String {
    let d = &r.data;
    let reason = d["reason"].as_str().filter(|s| !s.is_empty()).map(|s| format!("\n<i>{}</i>", html_escape(s))).unwrap_or_default();
    format!(
        "🔐 <b>Approval needed</b>\n{}\nTool: <b>{}</b>{reason}\n<pre>{}</pre>",
        header(st, chat),
        html_escape(d["tool"].as_str().unwrap_or("")),
        html_escape(&truncate(d["summary"].as_str().unwrap_or(""), 2500))
    )
}

fn question_text(st: &St, chat: &Chat, q: &Value, picked: Option<&Vec<String>>) -> String {
    let mut s = format!(
        "❓ <b>{}</b>\n{}\n\n{}\n",
        html_escape(q["header"].as_str().unwrap_or("Question")),
        header(st, chat),
        html_escape(q["question"].as_str().unwrap_or(""))
    );
    for o in q["options"].as_array().cloned().unwrap_or_default() {
        let desc = o["description"].as_str().unwrap_or("");
        s.push_str(&format!(
            "\n• <b>{}</b>{}",
            html_escape(o["label"].as_str().unwrap_or("")),
            if desc.is_empty() { String::new() } else { format!(" — {}", html_escape(desc)) }
        ));
    }
    if q["allow_other"] == true {
        s.push_str("\n\n<i>Or reply to this message with your own answer.</i>");
    }
    if let Some(p) = picked.filter(|p| !p.is_empty()) {
        s.push_str(&format!("\n\nSelected: {}", html_escape(&p.join(", "))));
    }
    s
}

fn question_keyboard(req_id: i64, qi: usize, q: &Value, picked: &[String]) -> Value {
    let multi = q["multi"] == true;
    let mut rows: Vec<Value> = q["options"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(oi, o)| {
            let label = o["label"].as_str().unwrap_or("").to_string();
            let mark = if multi && picked.contains(&label) { "✅ " } else { "" };
            json!([{"text": truncate(&format!("{mark}{label}"), 60), "callback_data": format!("qo:{req_id}:{qi}:{oi}")}])
        })
        .collect();
    if multi {
        rows.push(json!([{"text": "Submit ✔", "callback_data": format!("qd:{req_id}:{qi}")}]));
    }
    rows.push(json!([{"text": "✖ Decline", "callback_data": format!("ap:{req_id}:0")}]));
    Value::Array(rows)
}

async fn send_request(st: &St, r: &Request) -> Result<()> {
    let chat = get_chat(&st.db, r.chat_id)?;
    let mut sent = Vec::new();
    if r.kind == "approval" {
        let text = approval_text(st, &chat, r);
        let kb = json!([[
            {"text": "✅ Allow", "callback_data": format!("ap:{}:1", r.id)},
            {"text": "❌ Deny", "callback_data": format!("ap:{}:0", r.id)}
        ]]);
        let id = send(st, Some(&chat), &text, Some(kb)).await?;
        sent.push((id, text));
    } else {
        for (qi, q) in r.data["questions"].as_array().cloned().unwrap_or_default().iter().enumerate() {
            let text = question_text(st, &chat, q, None);
            let id = send(st, Some(&chat), &text, Some(question_keyboard(r.id, qi, q, &[]))).await?;
            st.telegram.reply_map.lock().unwrap().insert(id, (r.id, qi));
            sent.push((id, text));
        }
    }
    st.telegram.req_msgs.lock().unwrap().insert(r.id, sent);
    Ok(())
}

fn answer_summary(r: &Request) -> String {
    let a = r.answer.clone().unwrap_or(Value::Null);
    if r.kind == "approval" {
        return if a["allow"] == true { "Allowed".into() } else { "Denied".into() };
    }
    if a["allow"] == false {
        return "Declined".into();
    }
    let mut parts = Vec::new();
    for q in r.data["questions"].as_array().cloned().unwrap_or_default() {
        let id = q["id"].as_str().unwrap_or("");
        let picked: Vec<String> = a["answers"][id]
            .as_array()
            .map(|x| x.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        parts.push(picked.join(", "));
    }
    parts.join(" | ")
}

async fn resolve_request(st: &St, r: &Request) {
    let msgs = st.telegram.req_msgs.lock().unwrap().remove(&r.id).unwrap_or_default();
    st.telegram.partial.lock().unwrap().remove(&r.id);
    st.telegram.done_q.lock().unwrap().remove(&r.id);
    st.telegram.reply_map.lock().unwrap().retain(|_, (rid, _)| *rid != r.id);
    let status = if r.status == "expired" {
        "⌛ <b>Expired</b>".to_string()
    } else {
        format!(
            "{} <b>{}</b> by {}",
            if r.answer.as_ref().map(|a| a["allow"] == false).unwrap_or(false) { "🚫" } else { "✅" },
            html_escape(&answer_summary(r)),
            html_escape(r.answered_by.as_deref().unwrap_or("?"))
        )
    };
    for (id, text) in msgs {
        edit(st, id, &format!("{text}\n\n{status}"), None).await;
    }
}

fn allowed_user(st: &St, user_id: i64) -> bool {
    let list = settings::get(&st.db, "telegram_allowed_users");
    let ids: Vec<&str> = list.split(|c: char| c == ',' || c.is_whitespace()).filter(|s| !s.is_empty()).collect();
    ids.is_empty() || ids.contains(&user_id.to_string().as_str())
}

fn display_name(from: &Value) -> String {
    let name = from["username"]
        .as_str()
        .map(|u| format!("@{u}"))
        .or_else(|| from["first_name"].as_str().map(String::from))
        .unwrap_or_else(|| "someone".into());
    format!("{name} (Telegram)")
}

async fn update_loop(st: St) {
    let mut offset: i64 = 0;
    loop {
        if token(&st).is_none() {
            tokio::time::sleep(Duration::from_secs(10)).await;
            continue;
        }
        let res = call(&st, "getUpdates", json!({"offset": offset, "timeout": 45, "allowed_updates": ["message", "callback_query"]})).await;
        match res {
            Ok(Value::Array(updates)) => {
                for u in updates {
                    offset = offset.max(u["update_id"].as_i64().unwrap_or(0) + 1);
                    if let Err(e) = on_update(&st, &u).await {
                        tracing::debug!("telegram update: {e:#}");
                    }
                }
            }
            Ok(_) => {}
            Err(e) => {
                tracing::debug!("telegram getUpdates: {e:#}");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

fn same_group(st: &St, chat_id: &Value) -> bool {
    group(st).map(|g| g == chat_id.to_string().trim_matches('"')).unwrap_or(false)
}

/// The forum topic a message belongs to (None for the General topic).
fn topic_of(msg: &Value) -> Option<i64> {
    (msg["is_topic_message"] == true).then(|| msg["message_thread_id"].as_i64()).flatten()
}

async fn reply(st: &St, msg: &Value, text: &str, keyboard: Option<Value>) -> Result<i64> {
    let mut body = json!({"chat_id": msg["chat"]["id"], "text": text, "parse_mode": "HTML"});
    if let Some(t) = msg["message_thread_id"].as_i64() {
        body["message_thread_id"] = json!(t);
    }
    if let Some(k) = keyboard {
        body["reply_markup"] = json!({"inline_keyboard": k});
    }
    Ok(call(st, "sendMessage", body).await?["message_id"].as_i64().unwrap_or(0))
}

async fn on_update(st: &St, u: &Value) -> Result<()> {
    if let Some(cb) = u.get("callback_query") {
        let from = &cb["from"];
        let msg = &cb["message"];
        let ok = same_group(st, &msg["chat"]["id"]) && allowed_user(st, from["id"].as_i64().unwrap_or(0));
        let text = if ok {
            match on_callback(st, cb).await {
                Ok(t) => t,
                Err(e) => format!("{e}"),
            }
        } else {
            "Not allowed".to_string()
        };
        let _ = call(st, "answerCallbackQuery", json!({"callback_query_id": cb["id"], "text": truncate(&text, 190)})).await;
        return Ok(());
    }
    let Some(msg) = u.get("message") else { return Ok(()) };
    let text = msg["text"].as_str().unwrap_or("").trim().to_string();
    let cmd = text.split_whitespace().next().unwrap_or("").split('@').next().unwrap_or("").to_string();
    if cmd == "/id" {
        reply(
            st,
            msg,
            &format!("chat id: <code>{}</code>\nyour user id: <code>{}</code>", msg["chat"]["id"], msg["from"]["id"]),
            None,
        )
        .await?;
        return Ok(());
    }
    if !same_group(st, &msg["chat"]["id"]) || !allowed_user(st, msg["from"]["id"].as_i64().unwrap_or(0)) {
        return Ok(());
    }
    match cmd.as_str() {
        "/deploy" => {
            let projects: Vec<(i64, String)> = {
                let c = st.db.conn();
                let mut q = c.prepare(&format!("SELECT {} FROM projects ORDER BY name", store::Project::COLS))?;
                let v = q.query_map([], store::Project::from_row)?.filter_map(Result::ok).filter(|p| p.deployable()).map(|p| (p.id, p.name)).collect();
                v
            };
            if projects.is_empty() {
                reply(st, msg, "No project has a deploy configured.", None).await?;
            } else {
                let kb: Vec<Value> = projects
                    .iter()
                    .map(|(id, name)| json!([{"text": name, "callback_data": format!("dp:{id}")}]))
                    .collect();
                reply(st, msg, "🚀 Which project do you want to deploy?", Some(Value::Array(kb))).await?;
            }
        }
        "/status" => {
            let c = crate::runner::active_counts(st);
            let m = crate::util::meminfo();
            reply(
                st,
                msg,
                &format!(
                    "Running: Claude {} · Codex {}\nQueued: {} · Waiting for answer: {}\nRAM free: {} / {} MB",
                    c["running_claude"], c["running_codex"], c["queued"], c["waiting"], m.available_mb, m.total_mb
                ),
                None,
            )
            .await?;
        }
        "/help" | "/start" => {
            reply(st, msg, "/new [prompt] — start a new chat (pick project and tool)\n/stop — stop the running prompt in this topic\n/deploy — deploy a project\n/status — what is running\n/id — show chat and user ids\n\nWrite in a chat's topic to send the next prompt to that chat. Approvals and questions arrive with buttons; reply to a question to type a custom answer.", None).await?;
        }
        "/new" => {
            let prompt = text.splitn(2, char::is_whitespace).nth(1).unwrap_or("").trim().to_string();
            new_chat_picker(st, msg, prompt).await?;
        }
        "/stop" => {
            let Some(chat) = topic_of(msg).and_then(|t| chat_for_topic(st, t)) else {
                reply(st, msg, "Use /stop inside a chat's topic.", None).await?;
                return Ok(());
            };
            let runs: Vec<i64> = {
                let c = st.db.conn();
                let mut q = c.prepare("SELECT id FROM runs WHERE chat_id=?1 AND status='running'")?;
                let v = q.query_map([chat.id], |r| r.get(0))?.filter_map(Result::ok).collect();
                v
            };
            if runs.is_empty() {
                reply(st, msg, "Nothing is running in this chat.", None).await?;
            }
            for r in runs {
                crate::runner::cancel_run(st, r).await?;
            }
        }
        _ => {
            // A reply to a question message = typed answer.
            if let Some(rt) = msg["reply_to_message"]["message_id"].as_i64() {
                let target = st.telegram.reply_map.lock().unwrap().get(&rt).cloned();
                if let Some((req_id, qi)) = target {
                    if !text.is_empty() {
                        set_answer(st, req_id, qi, vec![text.clone()], true, &display_name(&msg["from"])).await?;
                    }
                    return Ok(());
                }
            }
            // Other bots' commands, or service messages (topic created, member joined…).
            if cmd.starts_with('/') || msg["from"]["is_bot"] == true {
                return Ok(());
            }
            prompt_from_message(st, msg).await?;
        }
    }
    Ok(())
}

/// The chat whose forum topic this is.
fn chat_for_topic(st: &St, topic: i64) -> Option<Chat> {
    let id: i64 = st
        .db
        .conn()
        .query_row("SELECT id FROM chats WHERE tg_topic_id=?1 ORDER BY id DESC LIMIT 1", [topic], |r| r.get(0))
        .ok()?;
    get_chat(&st.db, id).ok()
}

/// A plain message in a chat's topic becomes that chat's next prompt.
async fn prompt_from_message(st: &St, msg: &Value) -> Result<()> {
    let text = msg["text"].as_str().or(msg["caption"].as_str()).unwrap_or("").trim().to_string();
    let has_file = msg.get("photo").is_some() || msg.get("document").is_some();
    if text.is_empty() && !has_file {
        return Ok(());
    }
    let Some(chat) = topic_of(msg).and_then(|t| chat_for_topic(st, t)) else {
        reply(st, msg, "This topic isn't linked to a chat. Send /new to start one.", None).await?;
        return Ok(());
    };
    let mut atts = Vec::new();
    if has_file {
        match download_attachment(st, msg).await {
            Ok(a) => atts.push(a),
            Err(e) => {
                reply(st, msg, &format!("Couldn't download the file: {}", html_escape(&format!("{e:#}"))), None).await?;
                return Ok(());
            }
        }
    }
    let busy = chat.status == "running" || chat.status == "queued" || chat.status == "waiting";
    let author = display_name(&msg["from"]);
    crate::runner::enqueue(
        st,
        crate::runner::NewRun {
            chat_id: chat.id,
            user_id: None,
            source: "telegram",
            schedule_id: None,
            prompt: &text,
            attachments: Value::Array(atts),
            chain: vec![],
            author: Some(author),
        },
    )?;
    let _ = st.db.conn().execute("UPDATE chats SET updated_at=?2 WHERE id=?1", rusqlite::params![chat.id, crate::util::now()]);
    let _ = call(
        st,
        "setMessageReaction",
        json!({"chat_id": msg["chat"]["id"], "message_id": msg["message_id"], "reaction": [{"type": "emoji", "emoji": "👀"}]}),
    )
    .await;
    if busy {
        reply(st, msg, "⏳ Queued — it runs after the current prompt.", None).await?;
    }
    Ok(())
}

/// Save a photo (largest size) or document from a message as an upload.
async fn download_attachment(st: &St, msg: &Value) -> Result<Value> {
    let (file_id, filename, mime) = if let Some(sizes) = msg["photo"].as_array() {
        let best = sizes.last().ok_or_else(|| anyhow!("empty photo"))?;
        (best["file_id"].as_str().unwrap_or("").to_string(), "photo.jpg".to_string(), "image/jpeg".to_string())
    } else {
        let d = &msg["document"];
        let name = d["file_name"].as_str().unwrap_or("file").to_string();
        let mime = d["mime_type"].as_str().map(String::from).unwrap_or_else(|| mime_guess::from_path(&name).first_or_octet_stream().to_string());
        (d["file_id"].as_str().unwrap_or("").to_string(), name, mime)
    };
    let f = call(st, "getFile", json!({"file_id": file_id})).await?;
    let path = f["file_path"].as_str().ok_or_else(|| anyhow!("file too big for the Bot API (20 MB max)"))?;
    let token = token(st).ok_or_else(|| anyhow!("no token"))?;
    let base = std::env::var("CLOUD_AI_TELEGRAM_API").unwrap_or_else(|_| "https://api.telegram.org".into());
    let url = format!("{}/file/bot{token}/{path}", base.trim_end_matches('/'));
    let data = st.http.get(&url).timeout(Duration::from_secs(120)).send().await?.error_for_status()?.bytes().await?;
    let dir = st.cfg.uploads_dir().join(chrono::Utc::now().format("%Y-%m").to_string());
    std::fs::create_dir_all(&dir)?;
    let safe: String = filename.chars().map(|c| if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' }).collect();
    let dest = dir.join(format!("{}-{}", uuid::Uuid::new_v4().simple(), safe));
    std::fs::write(&dest, &data)?;
    let dest = dest.to_string_lossy().to_string();
    let id = {
        let c = st.db.conn();
        c.execute(
            "INSERT INTO attachments(user_id,filename,path,mime,size,created_at) VALUES(NULL,?1,?2,?3,?4,?5)",
            rusqlite::params![filename, dest, mime, data.len() as i64, crate::util::now()],
        )?;
        c.last_insert_rowid()
    };
    Ok(json!({"id": id, "filename": filename, "path": dest, "mime": mime, "size": data.len() as i64}))
}

/// `/new`: pick a project, then a tool; the prompt typed after the command is sent once the chat exists.
async fn new_chat_picker(st: &St, msg: &Value, prompt: String) -> Result<()> {
    let projects: Vec<(i64, String)> = {
        let c = st.db.conn();
        let mut q = c.prepare("SELECT id, name FROM projects ORDER BY name")?;
        let v = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.filter_map(Result::ok).collect();
        v
    };
    if projects.is_empty() {
        reply(st, msg, "There are no projects yet. Add one in the panel first.", None).await?;
        return Ok(());
    }
    let (text, kb) = if projects.len() == 1 {
        tool_picker(projects[0].0, &projects[0].1)
    } else {
        let mut rows: Vec<Value> = projects.iter().map(|(id, name)| json!([{"text": truncate(name, 60), "callback_data": format!("np:{id}")}])).collect();
        rows.push(json!([{"text": "Cancel", "callback_data": "dx"}]));
        ("🆕 New chat — which project?".to_string(), Value::Array(rows))
    };
    let id = reply(st, msg, &text, Some(kb)).await?;
    st.telegram.pending_new.lock().unwrap().insert(id, prompt);
    Ok(())
}

fn tool_picker(pid: i64, name: &str) -> (String, Value) {
    (
        format!("🆕 New chat in <b>{}</b> — which agent?", html_escape(name)),
        json!([
            [{"text": "Claude Code", "callback_data": format!("nt:{pid}:claude")}, {"text": "Codex", "callback_data": format!("nt:{pid}:codex")}],
            [{"text": "Cancel", "callback_data": "dx"}]
        ]),
    )
}

async fn create_chat_from_picker(st: &St, message_id: i64, pid: i64, tool: &str, by: &str) -> Result<String> {
    let prompt = st.telegram.pending_new.lock().unwrap().remove(&message_id).unwrap_or_default();
    let title = if prompt.is_empty() { "Telegram chat".to_string() } else { crate::chats::title_from(&prompt) };
    let chat = crate::chats::create_chat(
        st,
        crate::chats::NewChat {
            project_id: pid,
            user_id: None,
            title,
            tool: tool.to_string(),
            model: String::new(),
            effort: String::new(),
            permission_mode: settings::get(&st.db, "default_permission_mode"),
            worktree: false,
            auto_deploy: false,
            schedule_id: None,
            read_only: false,
        },
    )?;
    if !prompt.is_empty() {
        crate::runner::enqueue(
            st,
            crate::runner::NewRun {
                chat_id: chat.id,
                user_id: None,
                source: "telegram",
                schedule_id: None,
                prompt: &prompt,
                attachments: json!([]),
                chain: vec![],
                author: Some(by.to_string()),
            },
        )?;
    }
    let next = if prompt.is_empty() { "Write your first message in its topic." } else { "The prompt was sent; follow it in its topic." };
    edit(st, message_id, &format!("🆕 Chat <b>{}</b> created. {next}", html_escape(&chat.title)), None).await;
    Ok("Chat created".into())
}

/// Keep Telegram's "typing…" indicator on while a Telegram prompt runs.
fn spawn_typing(st: St, run_id: i64, thread: Option<i64>) {
    tokio::spawn(async move {
        let Some(gid) = group(&st) else { return };
        for _ in 0..400 {
            let running = store::get_run(&st.db, run_id).map(|r| r.status == "running").unwrap_or(false);
            if !running {
                return;
            }
            let mut body = json!({"chat_id": gid, "action": "typing"});
            if let Some(t) = thread {
                body["message_thread_id"] = json!(t);
            }
            let _ = call(&st, "sendChatAction", body).await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}

/// Split an answer into Telegram-sized HTML messages (Markdown code, bold and
/// fences converted). Long answers are capped; the panel has the full text.
fn answer_chunks(text: &str) -> Vec<String> {
    const RAW_MAX: usize = 3000; // leaves room for escaping and the footer
    const MAX_CHUNKS: usize = 8;
    let mut raw: Vec<String> = vec![String::new()];
    for line in text.lines() {
        let mut line = line.to_string();
        // Hard-wrap single lines that are longer than a chunk.
        while line.len() > RAW_MAX {
            let mut cut = RAW_MAX;
            while !line.is_char_boundary(cut) {
                cut -= 1;
            }
            let rest = line.split_off(cut);
            raw.push(line);
            raw.push(String::new());
            line = rest;
        }
        let cur = raw.last_mut().unwrap();
        if !cur.is_empty() && cur.len() + line.len() + 1 > RAW_MAX {
            raw.push(line);
        } else {
            if !cur.is_empty() {
                cur.push('\n');
            }
            cur.push_str(&line);
        }
    }
    raw.retain(|c| !c.trim().is_empty());
    let truncated = raw.len() > MAX_CHUNKS;
    raw.truncate(MAX_CHUNKS);
    let mut in_fence = false;
    let mut out: Vec<String> = raw.iter().map(|c| md_to_html(c, &mut in_fence)).collect();
    if out.is_empty() {
        out.push(String::new());
    }
    if truncated {
        out.last_mut().unwrap().push_str("\n\n<i>… answer shortened; the full text is in the panel.</i>");
    }
    out
}

/// Minimal Markdown → Telegram HTML. `in_fence` carries an open ``` block across chunks.
fn md_to_html(chunk: &str, in_fence: &mut bool) -> String {
    let mut out = String::new();
    if *in_fence {
        out.push_str("<pre>");
    }
    for (i, line) in chunk.lines().enumerate() {
        if line.trim_start().starts_with("```") {
            if *in_fence {
                out.push_str("</pre>");
            } else {
                if i > 0 {
                    out.push('\n');
                }
                out.push_str("<pre>");
            }
            *in_fence = !*in_fence;
            continue;
        }
        if i > 0 && !out.ends_with("<pre>") {
            out.push('\n');
        }
        if *in_fence {
            out.push_str(&html_escape(line));
        } else {
            out.push_str(&inline_md(line));
        }
    }
    if *in_fence {
        out.push_str("</pre>");
    }
    out
}

fn inline_md(line: &str) -> String {
    // Headings → bold.
    let t = line.trim_start();
    if t.starts_with('#') {
        let h = t.trim_start_matches('#').trim();
        if !h.is_empty() && t.chars().nth(t.len() - t.trim_start_matches('#').len()) == Some(' ') {
            return format!("<b>{}</b>", html_escape(h));
        }
    }
    let mut out = String::new();
    let mut rest = line;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('`') {
            if let Some(end) = after.find('`') {
                out.push_str(&format!("<code>{}</code>", html_escape(&after[..end])));
                rest = &after[end + 1..];
                continue;
            }
        }
        if let Some(after) = rest.strip_prefix("**") {
            if let Some(end) = after.find("**").filter(|e| *e > 0) {
                out.push_str(&format!("<b>{}</b>", html_escape(&after[..end])));
                rest = &after[end + 2..];
                continue;
            }
        }
        let ch = rest.chars().next().unwrap();
        out.push_str(&html_escape(&ch.to_string()));
        rest = &rest[ch.len_utf8()..];
    }
    out
}

async fn on_callback(st: &St, cb: &Value) -> Result<String> {
    let data = cb["data"].as_str().unwrap_or("");
    let by = display_name(&cb["from"]);
    let parts: Vec<&str> = data.split(':').collect();
    let message_id = cb["message"]["message_id"].as_i64().unwrap_or(0);
    // Forum topic the button was pressed in, so follow-ups stay in that topic.
    let thread = topic_of(&cb["message"]);
    match parts.as_slice() {
        ["ap", req, allow] => {
            let req: i64 = req.parse()?;
            let allow = *allow == "1";
            crate::runner::answer(st, req, json!({"allow": allow, "message": if allow { "" } else { "Denied from Telegram" }}), &by)?;
            Ok(if allow { "Allowed".into() } else { "Denied".into() })
        }
        ["qo", req, qi, oi] => {
            let (req, qi, oi): (i64, usize, usize) = (req.parse()?, qi.parse()?, oi.parse()?);
            let r = store::get_request(&st.db, req)?;
            let q = r.data["questions"][qi].clone();
            let label = q["options"][oi]["label"].as_str().unwrap_or("").to_string();
            if q["multi"] == true {
                let picked = {
                    let mut p = st.telegram.partial.lock().unwrap();
                    let entry = p.entry(req).or_default().entry(q["id"].as_str().unwrap_or("").to_string()).or_default();
                    if let Some(i) = entry.iter().position(|x| *x == label) {
                        entry.remove(i);
                    } else {
                        entry.push(label.clone());
                    }
                    entry.clone()
                };
                let chat = get_chat(&st.db, r.chat_id)?;
                edit(st, message_id, &question_text(st, &chat, &q, Some(&picked)), Some(question_keyboard(req, qi, &q, &picked))).await;
                Ok(format!("Selected: {}", picked.join(", ")))
            } else {
                set_answer(st, req, qi, vec![label.clone()], true, &by).await?;
                Ok(format!("Answered: {label}"))
            }
        }
        ["qd", req, qi] => {
            let (req, qi): (i64, usize) = (req.parse()?, qi.parse()?);
            let r = store::get_request(&st.db, req)?;
            let qid = r.data["questions"][qi]["id"].as_str().unwrap_or("").to_string();
            let picked = st.telegram.partial.lock().unwrap().get(&req).and_then(|m| m.get(&qid).cloned()).unwrap_or_default();
            set_answer(st, req, qi, picked, true, &by).await?;
            Ok("Submitted".into())
        }
        ["dp", pid] => {
            let pid: i64 = pid.parse()?;
            let p = get_project(&st.db, pid)?;
            send_to(
                st,
                thread,
                &format!("Deploy <b>{}</b> now?", html_escape(&p.name)),
                Some(json!([[{"text": "✅ Yes, deploy", "callback_data": format!("dc:{pid}")}, {"text": "Cancel", "callback_data": "dx"}]])),
            )
            .await?;
            Ok("Confirm the deploy".into())
        }
        ["dc", pid] => {
            let pid: i64 = pid.parse()?;
            // Asked from a chat's topic: the deploy ships that chat's changes.
            let chat = thread.and_then(|t| chat_for_topic(st, t)).filter(|c| c.project_id == pid).map(|c| c.id);
            let id = crate::deploy::start(st, pid, &by, None, thread, chat).ok_or_else(|| anyhow!("A deploy is already running for this project"))?;
            edit(st, message_id, &format!("🚀 Deploy #{id} started by {}", html_escape(&by)), None).await;
            Ok("Deploy started".into())
        }
        ["rb", did] => {
            let did: i64 = did.parse()?;
            let d = store::get_deploy(&st.db, did)?;
            let sha = d.prev_sha.clone().unwrap_or_default();
            send_to(
                st,
                thread,
                &format!("Roll back to <code>{}</code>?", html_escape(&sha[..sha.len().min(8)])),
                Some(json!([[{"text": "↩️ Yes, roll back", "callback_data": format!("rc:{did}")}, {"text": "Cancel", "callback_data": "dx"}]])),
            )
            .await?;
            Ok("Confirm the rollback".into())
        }
        ["rc", did] => {
            let did: i64 = did.parse()?;
            let d = store::get_deploy(&st.db, did)?;
            let sha = d.prev_sha.ok_or_else(|| anyhow!("no previous version known"))?;
            crate::deploy::start(st, d.project_id, &by, Some(sha), thread, d.chat_id).ok_or_else(|| anyhow!("A deploy is already running for this project"))?;
            edit(st, message_id, &format!("↩️ Rollback started by {}", html_escape(&by)), None).await;
            Ok("Rollback started".into())
        }
        ["np", pid] => {
            let pid: i64 = pid.parse()?;
            let p = get_project(&st.db, pid)?;
            let (text, kb) = tool_picker(pid, &p.name);
            edit(st, message_id, &text, Some(kb)).await;
            Ok(p.name)
        }
        ["nt", pid, tool] => create_chat_from_picker(st, message_id, pid.parse()?, tool, &by).await,
        ["dx"] => {
            st.telegram.pending_new.lock().unwrap().remove(&message_id);
            edit(st, message_id, "Cancelled", None).await;
            Ok("Cancelled".into())
        }
        _ => Ok("Unknown action".into()),
    }
}

/// Record the answer to one question; when every question is answered the
/// request is resolved (which also updates the web UI).
async fn set_answer(st: &St, req_id: i64, qi: usize, answer: Vec<String>, done: bool, by: &str) -> Result<()> {
    let r = store::get_request(&st.db, req_id)?;
    if r.status != "pending" {
        bail!("Already answered");
    }
    let qs = r.data["questions"].as_array().cloned().unwrap_or_default();
    let qid = qs.get(qi).and_then(|q| q["id"].as_str()).unwrap_or("").to_string();
    let complete = {
        let mut p = st.telegram.partial.lock().unwrap();
        let entry = p.entry(req_id).or_default();
        entry.insert(qid.clone(), answer);
        let mut d = st.telegram.done_q.lock().unwrap();
        let dset = d.entry(req_id).or_default();
        if done {
            dset.insert(qid);
        }
        qs.iter().all(|q| dset.contains(q["id"].as_str().unwrap_or("")))
    };
    if complete {
        let answers = st.telegram.partial.lock().unwrap().get(&req_id).cloned().unwrap_or_default();
        crate::runner::answer(st, req_id, json!({"allow": true, "answers": answers}), by)?;
    }
    Ok(())
}

pub async fn test(st: &St) -> Result<Value> {
    let me = call(st, "getMe", json!({})).await?;
    if group(st).is_some() {
        send(st, None, "✅ cloud-ai is connected to this group.", None).await?;
    }
    Ok(me)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_of_message() {
        assert_eq!(topic_of(&json!({"message_thread_id": 42, "is_topic_message": true})), Some(42));
        // General topic, or a plain reply thread in a non-forum group.
        assert_eq!(topic_of(&json!({"message_id": 1})), None);
        assert_eq!(topic_of(&json!({"message_thread_id": 7})), None);
    }

    #[test]
    fn markdown_to_telegram_html() {
        let mut f = false;
        assert_eq!(md_to_html("Use `a<b>` and **bold**", &mut f), "Use <code>a&lt;b&gt;</code> and <b>bold</b>");
        assert_eq!(md_to_html("## Title\ntext", &mut f), "<b>Title</b>\ntext");
        assert_eq!(md_to_html("x\n```rust\nlet a = 1 < 2;\n```\ny", &mut f), "x\n<pre>let a = 1 &lt; 2;</pre>\ny");
        assert!(!f);
        // A fence left open at a chunk boundary is closed and reopened.
        assert_eq!(md_to_html("```\nfn a()", &mut f), "<pre>fn a()</pre>");
        assert!(f);
        assert_eq!(md_to_html("}\n```\ndone", &mut f), "<pre>}</pre>\ndone");
        assert!(!f);
        assert_eq!(md_to_html("a ** b `c", &mut f), "a ** b `c");
    }

    #[test]
    fn answers_are_split_and_capped() {
        let long = "line\n".repeat(5000);
        let chunks = answer_chunks(&long);
        assert_eq!(chunks.len(), 8);
        assert!(chunks.iter().all(|c| c.len() < 4000));
        assert!(chunks.last().unwrap().contains("shortened"));
        assert_eq!(answer_chunks("hi"), vec!["hi".to_string()]);
        let one_line = "x".repeat(7000);
        assert_eq!(answer_chunks(&one_line).len(), 3);
    }
}
