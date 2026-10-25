use crate::auth::AuthUser;
use crate::chats::{validate_mode, validate_tool};
use crate::scheduler;
use crate::settings;
use crate::state::St;
use crate::store::{self, can_access_project, get_schedule, Schedule, User};
use crate::util::{now, ApiError, ApiResult};
use axum::extract::{Path, State};
use axum::Json;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize, Clone)]
pub struct Body {
    project_id: i64,
    name: String,
    #[serde(default = "yes")]
    enabled: bool,
    kind: String,
    run_at: Option<String>,
    time_of_day: Option<String>,
    weekdays: Option<String>,
    cron: Option<String>,
    tool: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    effort: String,
    #[serde(default = "bypass")]
    permission_mode: String,
    #[serde(default = "new_mode")]
    mode: String,
    chat_id: Option<i64>,
    prompts: Vec<String>,
    #[serde(default)]
    use_worktree: bool,
    #[serde(default)]
    auto_deploy: bool,
}

fn yes() -> bool {
    true
}
fn bypass() -> String {
    "bypass".into()
}
fn new_mode() -> String {
    "new".into()
}

fn to_schedule(b: &Body) -> Schedule {
    Schedule {
        id: 0,
        project_id: b.project_id,
        user_id: None,
        name: b.name.clone(),
        enabled: b.enabled,
        kind: b.kind.clone(),
        run_at: b.run_at.clone(),
        time_of_day: b.time_of_day.clone(),
        weekdays: b.weekdays.clone(),
        cron: b.cron.clone(),
        tool: b.tool.clone(),
        model: b.model.clone(),
        effort: b.effort.clone(),
        permission_mode: b.permission_mode.clone(),
        mode: b.mode.clone(),
        chat_id: b.chat_id,
        prompts: json!(b.prompts),
        use_worktree: b.use_worktree,
        auto_deploy: b.auto_deploy,
        last_run_at: None,
        next_run_at: None,
        created_at: String::new(),
    }
}

fn validate(st: &St, u: &User, b: &Body) -> ApiResult<()> {
    if !can_access_project(&st.db, u, b.project_id) {
        return Err(ApiError::forbidden());
    }
    validate_tool(&b.tool).map_err(|e| ApiError::bad(e.to_string()))?;
    validate_mode(&b.permission_mode).map_err(|e| ApiError::bad(e.to_string()))?;
    if b.name.trim().is_empty() {
        return Err(ApiError::bad("name required"));
    }
    if b.prompts.iter().all(|p| p.trim().is_empty()) {
        return Err(ApiError::bad("at least one prompt is required"));
    }
    if b.mode != "new" && b.mode != "continue" {
        return Err(ApiError::bad("mode must be new or continue"));
    }
    if let Some(cid) = b.chat_id {
        let chat = store::get_chat(&st.db, cid).map_err(|_| ApiError::bad("chat not found"))?;
        if chat.project_id != b.project_id {
            return Err(ApiError::bad("chat belongs to another project"));
        }
    }
    let s = to_schedule(b);
    scheduler::next_after(&s, settings::tz(&st.db), chrono::Utc::now()).map_err(|e| ApiError::bad(e.to_string()))?;
    Ok(())
}

fn with_names(st: &St, s: &Schedule) -> Value {
    let mut v = serde_json::to_value(s).unwrap();
    v["project"] = json!(store::get_project(&st.db, s.project_id).map(|p| p.name).unwrap_or_default());
    v["user"] = json!(store::username(&st.db, s.user_id));
    let tz = settings::tz(&st.db);
    if let Some(n) = &s.next_run_at {
        if let Ok(t) = chrono::DateTime::parse_from_rfc3339(n) {
            v["next_run_local"] = json!(t.with_timezone(&tz).format("%a %Y-%m-%d %H:%M").to_string());
        }
    }
    v
}

pub async fn list(State(st): State<St>, AuthUser(u): AuthUser) -> ApiResult<Json<Value>> {
    let allowed: Vec<i64> = store::list_projects(&st.db, &u)?.iter().map(|p| p.id).collect();
    let items: Vec<Schedule> = {
        let c = st.db.conn();
        let mut q = c.prepare(&format!("SELECT {} FROM schedules ORDER BY name COLLATE NOCASE", Schedule::COLS))?;
        let v = q.query_map([], Schedule::from_row)?.collect::<Result<Vec<_>, _>>()?;
        v
    };
    let out: Vec<Value> = items.iter().filter(|s| allowed.contains(&s.project_id)).map(|s| with_names(&st, s)).collect();
    Ok(Json(json!({"schedules": out, "timezone": settings::get(&st.db, "timezone")})))
}

pub async fn create(State(st): State<St>, AuthUser(u): AuthUser, Json(b): Json<Body>) -> ApiResult<Json<Value>> {
    validate(&st, &u, &b)?;
    let id = {
        let c = st.db.conn();
        c.execute(
            "INSERT INTO schedules(project_id,user_id,name,enabled,kind,run_at,time_of_day,weekdays,cron,tool,model,effort,permission_mode,mode,chat_id,prompts,use_worktree,auto_deploy,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
            params![
                b.project_id, u.id, b.name.trim(), b.enabled as i64, b.kind, b.run_at, b.time_of_day, b.weekdays, b.cron,
                b.tool, b.model, b.effort, b.permission_mode, b.mode, b.chat_id, json!(b.prompts).to_string(),
                b.use_worktree as i64, b.auto_deploy as i64, now()
            ],
        )?;
        c.last_insert_rowid()
    };
    scheduler::recompute(&st, id)?;
    Ok(Json(with_names(&st, &get_schedule(&st.db, id)?)))
}

pub async fn update(State(st): State<St>, AuthUser(u): AuthUser, Path(id): Path<i64>, Json(b): Json<Body>) -> ApiResult<Json<Value>> {
    let old = get_schedule(&st.db, id)?;
    if !can_access_project(&st.db, &u, old.project_id) {
        return Err(ApiError::forbidden());
    }
    validate(&st, &u, &b)?;
    st.db.conn().execute(
        "UPDATE schedules SET project_id=?2,name=?3,enabled=?4,kind=?5,run_at=?6,time_of_day=?7,weekdays=?8,cron=?9,tool=?10,model=?11,effort=?12,
         permission_mode=?13,mode=?14,chat_id=?15,prompts=?16,use_worktree=?17,auto_deploy=?18 WHERE id=?1",
        params![
            id, b.project_id, b.name.trim(), b.enabled as i64, b.kind, b.run_at, b.time_of_day, b.weekdays, b.cron, b.tool,
            b.model, b.effort, b.permission_mode, b.mode, b.chat_id, json!(b.prompts).to_string(), b.use_worktree as i64,
            b.auto_deploy as i64
        ],
    )?;
    scheduler::recompute(&st, id)?;
    Ok(Json(with_names(&st, &get_schedule(&st.db, id)?)))
}

pub async fn remove(State(st): State<St>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let s = get_schedule(&st.db, id)?;
    if !can_access_project(&st.db, &u, s.project_id) {
        return Err(ApiError::forbidden());
    }
    st.db.conn().execute("DELETE FROM schedules WHERE id=?1", [id])?;
    Ok(Json(json!({"ok": true})))
}

pub async fn run_now(State(st): State<St>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let s = get_schedule(&st.db, id)?;
    if !can_access_project(&st.db, &u, s.project_id) {
        return Err(ApiError::forbidden());
    }
    let chat_id = scheduler::fire(&st, id).map_err(|e| ApiError::bad(format!("{e:#}")))?;
    Ok(Json(json!({"chat_id": chat_id})))
}

#[derive(Deserialize)]
pub struct PreviewBody {
    kind: String,
    run_at: Option<String>,
    time_of_day: Option<String>,
    weekdays: Option<String>,
    cron: Option<String>,
}

pub async fn preview(State(st): State<St>, AuthUser(_): AuthUser, Json(b): Json<PreviewBody>) -> ApiResult<Json<Value>> {
    let body = Body {
        project_id: 0,
        name: String::new(),
        enabled: true,
        kind: b.kind,
        run_at: b.run_at,
        time_of_day: b.time_of_day,
        weekdays: b.weekdays,
        cron: b.cron,
        tool: "claude".into(),
        model: String::new(),
        effort: String::new(),
        permission_mode: bypass(),
        mode: new_mode(),
        chat_id: None,
        prompts: vec![],
        use_worktree: false,
        auto_deploy: false,
    };
    match scheduler::preview(&to_schedule(&body), settings::tz(&st.db), 5) {
        Ok(times) => Ok(Json(json!({"ok": true, "next": times}))),
        Err(e) => Ok(Json(json!({"ok": false, "error": e.to_string()}))),
    }
}
