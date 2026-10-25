use crate::agents::Attachment;
use crate::auth::AuthUser;
use crate::chats::{create_chat, delete_chat, merge_worktree, remove_worktree, validate_mode, NewChat};
use crate::runner::{self, enqueue, NewRun};
use crate::settings;
use crate::state::St;
use crate::store::{self, can_access_project, get_chat, Chat, Message, Request, Run, User};
use crate::util::{now, ApiError, ApiResult};
use axum::body::Body;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::header;
use axum::response::Response;
use axum::Json;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};

fn chat_for(st: &St, u: &User, id: i64) -> ApiResult<Chat> {
    let chat = get_chat(&st.db, id)?;
    if !can_access_project(&st.db, u, chat.project_id) {
        return Err(ApiError::forbidden());
    }
    Ok(chat)
}

fn chat_json(st: &St, c: &Chat) -> Value {
    let mut v = serde_json::to_value(c).unwrap();
    v["user"] = json!(store::username(&st.db, c.user_id));
    v
}

#[derive(Deserialize)]
pub struct ListQ {
    archived: Option<bool>,
}

pub async fn list(State(st): State<St>, AuthUser(u): AuthUser, Path(pid): Path<i64>, Query(q): Query<ListQ>) -> ApiResult<Json<Value>> {
    if !can_access_project(&st.db, &u, pid) {
        return Err(ApiError::forbidden());
    }
    let chats: Vec<Chat> = {
        let c = st.db.conn();
        let mut s = c.prepare(&format!(
            "SELECT {} FROM chats WHERE project_id=?1 AND archived=?2 ORDER BY updated_at DESC LIMIT 300",
            Chat::COLS
        ))?;
        let v = s.query_map(params![pid, q.archived.unwrap_or(false) as i64], Chat::from_row)?.collect::<Result<_, _>>()?;
        v
    };
    Ok(Json(Value::Array(chats.iter().map(|c| chat_json(&st, c)).collect())))
}

pub async fn list_all(State(st): State<St>, AuthUser(u): AuthUser) -> ApiResult<Json<Value>> {
    let projects: Vec<i64> = store::list_projects(&st.db, &u)?.iter().map(|p| p.id).collect();
    if projects.is_empty() {
        return Ok(Json(json!([])));
    }
    let ids = projects.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let chats: Vec<Chat> = {
        let c = st.db.conn();
        let mut s = c.prepare(&format!(
            "SELECT {} FROM chats WHERE project_id IN ({ids}) AND archived=0 ORDER BY updated_at DESC LIMIT 60",
            Chat::COLS
        ))?;
        let v = s.query_map([], Chat::from_row)?.collect::<Result<_, _>>()?;
        v
    };
    Ok(Json(Value::Array(chats.iter().map(|c| chat_json(&st, c)).collect())))
}

#[derive(Deserialize)]
pub struct CreateBody {
    title: Option<String>,
    tool: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    effort: String,
    permission_mode: Option<String>,
    #[serde(default)]
    worktree: bool,
    #[serde(default)]
    auto_deploy: bool,
    prompt: Option<String>,
    #[serde(default)]
    attachments: Vec<i64>,
}

fn load_attachments(st: &St, u: &User, ids: &[i64]) -> ApiResult<Value> {
    let mut out = Vec::new();
    for id in ids {
        let a: (i64, Option<i64>, String, String, String, i64) = st.db.conn().query_row(
            "SELECT id,user_id,filename,path,mime,size FROM attachments WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )?;
        if a.1 != Some(u.id) && !u.is_admin() {
            return Err(ApiError::forbidden());
        }
        out.push(serde_json::to_value(Attachment { id: a.0, filename: a.2, path: a.3, mime: a.4, size: a.5 }).unwrap());
    }
    Ok(Value::Array(out))
}

fn title_from(prompt: &str) -> String {
    let first = prompt.lines().find(|l| !l.trim().is_empty()).unwrap_or("New chat").trim();
    let mut t: String = first.chars().take(60).collect();
    if first.chars().count() > 60 {
        t.push('…');
    }
    t
}

pub async fn create(State(st): State<St>, AuthUser(u): AuthUser, Path(pid): Path<i64>, Json(b): Json<CreateBody>) -> ApiResult<Json<Value>> {
    if !can_access_project(&st.db, &u, pid) {
        return Err(ApiError::forbidden());
    }
    let prompt = b.prompt.clone().unwrap_or_default();
    let title = b.title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| title_from(&prompt));
    let chat = create_chat(
        &st,
        NewChat {
            project_id: pid,
            user_id: Some(u.id),
            title,
            tool: b.tool.clone(),
            model: b.model.clone(),
            effort: b.effort.clone(),
            permission_mode: b.permission_mode.clone().unwrap_or_else(|| settings::get(&st.db, "default_permission_mode")),
            worktree: b.worktree,
            auto_deploy: b.auto_deploy,
            schedule_id: None,
        },
    )
    .map_err(|e| ApiError::bad(format!("{e:#}")))?;
    if !prompt.trim().is_empty() || !b.attachments.is_empty() {
        let atts = load_attachments(&st, &u, &b.attachments)?;
        enqueue(
            &st,
            NewRun { chat_id: chat.id, user_id: Some(u.id), source: "web", schedule_id: None, prompt: &prompt, attachments: atts, chain: vec![] },
        )?;
    }
    Ok(Json(chat_json(&st, &get_chat(&st.db, chat.id)?)))
}

pub async fn get_one(State(st): State<St>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let chat = chat_for(&st, &u, id)?;
    let project = store::get_project(&st.db, chat.project_id)?;
    let (runs, requests) = {
        let c = st.db.conn();
        let mut q = c.prepare(&format!(
            "SELECT {} FROM runs WHERE chat_id=?1 AND status IN ('queued','running') ORDER BY id",
            Run::COLS
        ))?;
        let runs: Vec<Run> = q.query_map([id], Run::from_row)?.collect::<Result<_, _>>()?;
        let mut q = c.prepare(&format!("SELECT {} FROM requests WHERE chat_id=?1 AND status='pending' ORDER BY id", Request::COLS))?;
        let requests: Vec<Request> = q.query_map([id], Request::from_row)?.collect::<Result<_, _>>()?;
        (runs, requests)
    };
    let mut v = chat_json(&st, &chat);
    v["project"] = json!({"id": project.id, "name": project.name, "path": project.path, "deployable": project.deployable()});
    v["active_runs"] = json!(runs);
    v["pending_requests"] = json!(requests);
    Ok(Json(v))
}

#[derive(Deserialize)]
pub struct UpdateBody {
    title: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    permission_mode: Option<String>,
    auto_deploy: Option<bool>,
    archived: Option<bool>,
}

pub async fn update(State(st): State<St>, AuthUser(u): AuthUser, Path(id): Path<i64>, Json(b): Json<UpdateBody>) -> ApiResult<Json<Value>> {
    let chat = chat_for(&st, &u, id)?;
    if let Some(m) = &b.permission_mode {
        validate_mode(m).map_err(|e| ApiError::bad(e.to_string()))?;
    }
    {
        let c = st.db.conn();
        if let Some(v) = &b.title {
            c.execute("UPDATE chats SET title=?2 WHERE id=?1", params![id, v.trim()])?;
        }
        if let Some(v) = &b.model {
            c.execute("UPDATE chats SET model=?2 WHERE id=?1", params![id, v])?;
        }
        if let Some(v) = &b.effort {
            c.execute("UPDATE chats SET effort=?2 WHERE id=?1", params![id, v])?;
        }
        if let Some(v) = &b.permission_mode {
            c.execute("UPDATE chats SET permission_mode=?2 WHERE id=?1", params![id, v])?;
        }
        if let Some(v) = b.auto_deploy {
            c.execute("UPDATE chats SET auto_deploy=?2 WHERE id=?1", params![id, v as i64])?;
        }
        if let Some(v) = b.archived {
            c.execute("UPDATE chats SET archived=?2 WHERE id=?1", params![id, v as i64])?;
        }
        c.execute("UPDATE chats SET updated_at=?2 WHERE id=?1", params![id, now()])?;
    }
    if (b.model.is_some() || b.effort.is_some() || b.permission_mode.is_some()) && chat.status != "running" && chat.status != "waiting" {
        // The next prompt restarts the CLI (resuming the same session) with the new settings.
        runner::drop_session(&st, id);
    }
    runner::emit_chat(&st, id);
    Ok(Json(chat_json(&st, &get_chat(&st.db, id)?)))
}

pub async fn remove(State(st): State<St>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let chat = chat_for(&st, &u, id)?;
    delete_chat(&st, &chat).await?;
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
pub struct MsgQ {
    before: Option<i64>,
    limit: Option<i64>,
}

pub async fn messages(State(st): State<St>, AuthUser(u): AuthUser, Path(id): Path<i64>, Query(q): Query<MsgQ>) -> ApiResult<Json<Value>> {
    chat_for(&st, &u, id)?;
    let limit = q.limit.unwrap_or(400).clamp(1, 2000);
    let before = q.before.unwrap_or(i64::MAX);
    let c = st.db.conn();
    let mut s = c.prepare(&format!(
        "SELECT {} FROM (SELECT * FROM messages WHERE chat_id=?1 AND id<?2 ORDER BY id DESC LIMIT ?3) ORDER BY id",
        Message::COLS
    ))?;
    let msgs: Vec<Message> = s.query_map(params![id, before, limit], Message::from_row)?.collect::<Result<_, _>>()?;
    let mut s = c.prepare(&format!("SELECT {} FROM requests WHERE chat_id=?1 ORDER BY id DESC LIMIT 100", Request::COLS))?;
    let reqs: Vec<Request> = s.query_map([id], Request::from_row)?.collect::<Result<_, _>>()?;
    Ok(Json(json!({"messages": msgs, "requests": reqs, "has_more": msgs.len() as i64 == limit})))
}

#[derive(Deserialize)]
pub struct SendBody {
    prompt: String,
    #[serde(default)]
    attachments: Vec<i64>,
}

pub async fn send(State(st): State<St>, AuthUser(u): AuthUser, Path(id): Path<i64>, Json(b): Json<SendBody>) -> ApiResult<Json<Run>> {
    chat_for(&st, &u, id)?;
    if b.prompt.trim().is_empty() && b.attachments.is_empty() {
        return Err(ApiError::bad("empty prompt"));
    }
    let atts = load_attachments(&st, &u, &b.attachments)?;
    let run = enqueue(
        &st,
        NewRun { chat_id: id, user_id: Some(u.id), source: "web", schedule_id: None, prompt: &b.prompt, attachments: atts, chain: vec![] },
    )?;
    st.db.conn().execute("UPDATE chats SET updated_at=?2 WHERE id=?1", params![id, now()])?;
    Ok(Json(run))
}

pub async fn stop(State(st): State<St>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    chat_for(&st, &u, id)?;
    let runs: Vec<i64> = {
        let c = st.db.conn();
        let mut q = c.prepare("SELECT id FROM runs WHERE chat_id=?1 AND status IN ('queued','running') ORDER BY id DESC")?;
        let v = q.query_map([id], |r| r.get(0))?.collect::<Result<_, _>>()?;
        v
    };
    for r in runs {
        runner::cancel_run(&st, r).await?;
    }
    Ok(Json(json!({"ok": true})))
}

pub async fn cancel_run(State(st): State<St>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let run = store::get_run(&st.db, id)?;
    chat_for(&st, &u, run.chat_id)?;
    runner::cancel_run(&st, id).await?;
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
pub struct MergeBody {
    #[serde(default)]
    remove: bool,
}

pub async fn merge(State(st): State<St>, AuthUser(u): AuthUser, Path(id): Path<i64>, Json(b): Json<MergeBody>) -> ApiResult<Json<Value>> {
    let chat = chat_for(&st, &u, id)?;
    let out = merge_worktree(&st, &chat).await.map_err(|e| ApiError::bad(format!("{e:#}")))?;
    if b.remove {
        remove_worktree(&st, &chat, true).await.map_err(|e| ApiError::bad(format!("{e:#}")))?;
        let project = store::get_project(&st.db, chat.project_id)?;
        st.db.conn().execute(
            "UPDATE chats SET cwd=?2, worktree_branch=NULL, archived=1 WHERE id=?1",
            params![id, project.path],
        )?;
        runner::drop_session(&st, id);
        runner::emit_chat(&st, id);
    }
    Ok(Json(json!({"ok": true, "output": out})))
}

pub async fn answer(State(st): State<St>, AuthUser(u): AuthUser, Path(id): Path<i64>, Json(b): Json<Value>) -> ApiResult<Json<Request>> {
    let req = store::get_request(&st.db, id)?;
    chat_for(&st, &u, req.chat_id)?;
    let r = runner::answer(&st, id, b, &u.username).map_err(|e| ApiError::bad(e.to_string()))?;
    Ok(Json(r))
}

pub async fn upload(State(st): State<St>, AuthUser(u): AuthUser, mut mp: Multipart) -> ApiResult<Json<Value>> {
    let dir = st.cfg.uploads_dir().join(chrono::Utc::now().format("%Y-%m").to_string());
    std::fs::create_dir_all(&dir)?;
    let mut out = Vec::new();
    while let Some(field) = mp.next_field().await.map_err(|e| ApiError::bad(e.to_string()))? {
        let filename = field.file_name().unwrap_or("file").to_string();
        let safe: String = filename
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
            .collect();
        let mime = field
            .content_type()
            .map(String::from)
            .unwrap_or_else(|| mime_guess::from_path(&filename).first_or_octet_stream().to_string());
        let data = field.bytes().await.map_err(|e| ApiError::bad(e.to_string()))?;
        let path = dir.join(format!("{}-{}", uuid::Uuid::new_v4().simple(), safe));
        std::fs::write(&path, &data)?;
        let id = {
            let c = st.db.conn();
            c.execute(
                "INSERT INTO attachments(user_id,filename,path,mime,size,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
                params![u.id, filename, path.to_string_lossy(), mime, data.len() as i64, now()],
            )?;
            c.last_insert_rowid()
        };
        out.push(json!(Attachment { id, filename, path: path.to_string_lossy().to_string(), mime, size: data.len() as i64 }));
    }
    Ok(Json(Value::Array(out)))
}

pub async fn download(State(st): State<St>, AuthUser(_u): AuthUser, Path(id): Path<i64>) -> ApiResult<Response> {
    let (path, mime, name): (String, String, String) = st.db.conn().query_row(
        "SELECT path,mime,filename FROM attachments WHERE id=?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let data = tokio::fs::read(&path).await?;
    Ok(Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_DISPOSITION, format!("inline; filename=\"{}\"", name.replace('"', "")))
        .header(header::CACHE_CONTROL, "private, max-age=86400")
        .body(Body::from(data))
        .unwrap())
}

pub async fn queue(State(st): State<St>, AuthUser(u): AuthUser) -> ApiResult<Json<Value>> {
    let projects: Vec<i64> = store::list_projects(&st.db, &u)?.iter().map(|p| p.id).collect();
    let ids = projects.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    if ids.is_empty() {
        return Ok(Json(json!({"runs": [], "counts": runner::active_counts(&st)})));
    }
    let c = st.db.conn();
    let mut q = c.prepare(&format!(
        "SELECT r.id, r.chat_id, r.status, r.source, substr(r.prompt,1,200), r.created_at, r.started_at, c.title, c.tool, p.name, p.id
         FROM runs r JOIN chats c ON c.id=r.chat_id JOIN projects p ON p.id=c.project_id
         WHERE r.status IN ('queued','running') AND p.id IN ({ids}) ORDER BY r.id"
    ))?;
    let rows: Vec<Value> = q
        .query_map([], |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?, "chat_id": r.get::<_, i64>(1)?, "status": r.get::<_, String>(2)?,
                "source": r.get::<_, String>(3)?, "prompt": r.get::<_, String>(4)?, "created_at": r.get::<_, String>(5)?,
                "started_at": r.get::<_, Option<String>>(6)?, "chat_title": r.get::<_, String>(7)?, "tool": r.get::<_, String>(8)?,
                "project": r.get::<_, String>(9)?, "project_id": r.get::<_, i64>(10)?,
            }))
        })?
        .collect::<Result<_, _>>()?;
    drop(q);
    drop(c);
    Ok(Json(json!({"runs": rows, "counts": runner::active_counts(&st)})))
}

/// Approvals and questions waiting for an answer, across the user's projects.
pub async fn pending(State(st): State<St>, AuthUser(u): AuthUser) -> ApiResult<Json<Value>> {
    let projects: Vec<i64> = store::list_projects(&st.db, &u)?.iter().map(|p| p.id).collect();
    if projects.is_empty() {
        return Ok(Json(json!([])));
    }
    let ids = projects.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let c = st.db.conn();
    let mut q = c.prepare(&format!(
        "SELECT r.id, r.chat_id, r.kind, r.data, r.created_at, c.title, p.name FROM requests r
         JOIN chats c ON c.id=r.chat_id JOIN projects p ON p.id=c.project_id
         WHERE r.status='pending' AND p.id IN ({ids}) ORDER BY r.id"
    ))?;
    let rows: Vec<Value> = q
        .query_map([], |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?, "chat_id": r.get::<_, i64>(1)?, "kind": r.get::<_, String>(2)?,
                "data": serde_json::from_str::<Value>(&r.get::<_, String>(3)?).unwrap_or(Value::Null),
                "created_at": r.get::<_, String>(4)?, "chat_title": r.get::<_, String>(5)?, "project": r.get::<_, String>(6)?,
            }))
        })?
        .collect::<Result<_, _>>()?;
    Ok(Json(Value::Array(rows)))
}
