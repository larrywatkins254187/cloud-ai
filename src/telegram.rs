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
    let url = format!("https://api.telegram.org/bot{token}/{method}");
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
    let gid = group(st).ok_or_else(|| anyhow!("Telegram group not set"))?;
    let mut body = json!({
        "chat_id": gid,
        "text": truncate(text, 4000),
        "parse_mode": "HTML",
        "disable_web_page_preview": true,
    });
    if let Some(c) = chat {
        if let Some(t) = topic_for(st, c).await {
            body["message_thread_id"] = json!(t);
        }
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
    format!("{tool} · {}{effort} · {} mode", html_escape(model), chat.permission_mode)
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
            if !settings::get_bool(&st.db, "telegram_send_prompts") {
                return Ok(());
            }
            let chat = get_chat(&st.db, run.chat_id)?;
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
                stats.push(format!("${c:.2}"));
            }
            let tok = run.input_tokens.unwrap_or(0) + run.output_tokens.unwrap_or(0);
            if tok > 0 {
                stats.push(format!("{}k tokens", tok / 1000));
            }
            let stats = stats.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
            let (text, kb) = match run.status.as_str() {
                "done" => {
                    let body = if settings::get_bool(&st.db, "telegram_send_results") {
                        format!("\n\n{}", html_escape(&truncate(run.result_text.as_deref().unwrap_or(""), 3200)))
                    } else {
                        String::new()
                    };
                    let kb = project
                        .deployable()
                        .then(|| json!([[{"text": "🚀 Deploy", "callback_data": format!("dp:{}", project.id)}]]));
                    (format!("✅ <b>Done</b> · {stats}{body}{}", link(st, &format!("/chats/{}", chat.id))), kb)
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
            send(st, None, &text, kb).await?;
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
            reply(st, msg, "/deploy — deploy a project\n/status — what is running\n/id — show chat and user ids\n\nApprovals and questions from Claude/Codex arrive here with buttons; reply to a question to type a custom answer.", None).await?;
        }
        _ => {
            // A reply to a question message = typed answer.
            if let Some(rt) = msg["reply_to_message"]["message_id"].as_i64() {
                let target = st.telegram.reply_map.lock().unwrap().get(&rt).cloned();
                if let Some((req_id, qi)) = target {
                    if !text.is_empty() {
                        set_answer(st, req_id, qi, vec![text.clone()], true, &display_name(&msg["from"])).await?;
                    }
                }
            }
        }
    }
    Ok(())
}

async fn on_callback(st: &St, cb: &Value) -> Result<String> {
    let data = cb["data"].as_str().unwrap_or("");
    let by = display_name(&cb["from"]);
    let parts: Vec<&str> = data.split(':').collect();
    let message_id = cb["message"]["message_id"].as_i64().unwrap_or(0);
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
            send(
                st,
                None,
                &format!("Deploy <b>{}</b> now?", html_escape(&p.name)),
                Some(json!([[{"text": "✅ Yes, deploy", "callback_data": format!("dc:{pid}")}, {"text": "Cancel", "callback_data": "dx"}]])),
            )
            .await?;
            Ok("Confirm the deploy".into())
        }
        ["dc", pid] => {
            let pid: i64 = pid.parse()?;
            let id = crate::deploy::start(st, pid, &by, None).ok_or_else(|| anyhow!("A deploy is already running for this project"))?;
            edit(st, message_id, &format!("🚀 Deploy #{id} started by {}", html_escape(&by)), None).await;
            Ok("Deploy started".into())
        }
        ["rb", did] => {
            let did: i64 = did.parse()?;
            let d = store::get_deploy(&st.db, did)?;
            let sha = d.prev_sha.clone().unwrap_or_default();
            send(
                st,
                None,
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
            crate::deploy::start(st, d.project_id, &by, Some(sha)).ok_or_else(|| anyhow!("A deploy is already running for this project"))?;
            edit(st, message_id, &format!("↩️ Rollback started by {}", html_escape(&by)), None).await;
            Ok("Rollback started".into())
        }
        ["dx"] => {
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
