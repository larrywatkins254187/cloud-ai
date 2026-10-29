use crate::auth::{AdminUser, AuthUser};
use crate::deploy;
use crate::state::St;
use crate::store::{self, can_access_project, get_server, Deploy, Server};
use crate::util::{now, ApiError, ApiResult};
use axum::extract::{Path, State};
use axum::Json;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};

pub async fn list(State(st): State<St>, AuthUser(_): AuthUser) -> ApiResult<Json<Vec<Server>>> {
    let c = st.db.conn();
    let mut q = c.prepare(&format!("SELECT {} FROM servers ORDER BY name", Server::COLS))?;
    let v = q.query_map([], Server::from_row)?.collect::<Result<_, _>>()?;
    Ok(Json(v))
}

#[derive(Deserialize)]
pub struct Body {
    name: String,
    host: String,
    #[serde(default = "port22")]
    port: i64,
    username: String,
    /// app_key | key | password
    auth_type: String,
    /// private key (auth_type=key) or password (auth_type=password); empty keeps the old one
    #[serde(default)]
    secret: String,
}

fn port22() -> i64 {
    22
}

fn save_secret(st: &St, id: i64, b: &Body) -> ApiResult<()> {
    if b.secret.trim().is_empty() {
        return Ok(());
    }
    match b.auth_type.as_str() {
        "key" => {
            let path = deploy::custom_key_path(st, id);
            std::fs::create_dir_all(path.parent().unwrap())?;
            let mut key = b.secret.trim().replace("\r\n", "\n");
            key.push('\n');
            std::fs::write(&path, key)?;
            crate::crypto::set_mode_600(&path);
        }
        "password" => {
            st.db.conn().execute("UPDATE servers SET secret_enc=?2 WHERE id=?1", params![id, st.secrets.encrypt(&b.secret)])?;
        }
        _ => {}
    }
    Ok(())
}

fn validate(b: &Body) -> ApiResult<()> {
    if b.name.trim().is_empty() || b.host.trim().is_empty() || b.username.trim().is_empty() {
        return Err(ApiError::bad("name, host and user are required"));
    }
    if !["app_key", "key", "password"].contains(&b.auth_type.as_str()) {
        return Err(ApiError::bad("auth type must be app_key, key or password"));
    }
    Ok(())
}

pub async fn create(State(st): State<St>, AdminUser(_): AdminUser, Json(b): Json<Body>) -> ApiResult<Json<Server>> {
    validate(&b)?;
    if b.auth_type != "app_key" && b.secret.trim().is_empty() {
        return Err(ApiError::bad("private key or password required"));
    }
    let id = {
        let c = st.db.conn();
        c.execute(
            "INSERT INTO servers(name,host,port,username,auth_type,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![b.name.trim(), b.host.trim(), b.port, b.username.trim(), b.auth_type, now()],
        )?;
        c.last_insert_rowid()
    };
    save_secret(&st, id, &b)?;
    deploy::write_ssh_config(&st);
    Ok(Json(get_server(&st.db, id)?))
}

pub async fn update(State(st): State<St>, AdminUser(_): AdminUser, Path(id): Path<i64>, Json(b): Json<Body>) -> ApiResult<Json<Server>> {
    validate(&b)?;
    get_server(&st.db, id)?;
    st.db.conn().execute(
        "UPDATE servers SET name=?2,host=?3,port=?4,username=?5,auth_type=?6 WHERE id=?1",
        params![id, b.name.trim(), b.host.trim(), b.port, b.username.trim(), b.auth_type],
    )?;
    save_secret(&st, id, &b)?;
    deploy::write_ssh_config(&st);
    Ok(Json(get_server(&st.db, id)?))
}

pub async fn remove(State(st): State<St>, AdminUser(_): AdminUser, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    st.db.conn().execute("DELETE FROM servers WHERE id=?1", [id])?;
    let _ = std::fs::remove_file(deploy::custom_key_path(&st, id));
    deploy::write_ssh_config(&st);
    Ok(Json(json!({"ok": true})))
}

pub async fn test(State(st): State<St>, AdminUser(_): AdminUser, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let s = get_server(&st.db, id)?;
    match deploy::test_server(&st, &s).await {
        Ok(out) => Ok(Json(json!({"ok": true, "output": out}))),
        Err(e) => Ok(Json(json!({"ok": false, "error": format!("{e:#}")}))),
    }
}

pub async fn public_key(State(st): State<St>, AdminUser(_): AdminUser) -> ApiResult<Json<Value>> {
    let key = deploy::ensure_app_key(&st)?;
    Ok(Json(json!({"public_key": key})))
}

#[derive(Deserialize)]
pub struct DeployListQuery {
    /// Only deploys older than this id (pagination).
    before: Option<i64>,
    limit: Option<i64>,
}

pub async fn deploys(
    State(st): State<St>,
    AuthUser(u): AuthUser,
    Path(pid): Path<i64>,
    axum::extract::Query(lq): axum::extract::Query<DeployListQuery>,
) -> ApiResult<Json<Value>> {
    if !can_access_project(&st.db, &u, pid) {
        return Err(ApiError::forbidden());
    }
    let limit = lq.limit.unwrap_or(50).clamp(1, 200);
    let c = st.db.conn();
    let mut q = c.prepare(&format!(
        "SELECT {} FROM deploys WHERE project_id=?1 AND id<?2 ORDER BY id DESC LIMIT ?3",
        Deploy::LIST_COLS
    ))?;
    let v: Vec<Deploy> = q.query_map(params![pid, lq.before.unwrap_or(i64::MAX), limit], Deploy::from_row)?.collect::<Result<_, _>>()?;
    Ok(Json(json!(v)))
}

/// Running deploys plus the latest few, across the projects the user can see.
pub async fn active(State(st): State<St>, AuthUser(u): AuthUser) -> ApiResult<Json<Value>> {
    let c = st.db.conn();
    let mut q = c.prepare(
        "SELECT d.id,d.project_id,d.triggered_by,d.kind,d.status,d.commit_sha,d.prev_sha,'' AS log,d.health,d.created_at,d.finished_at,d.tg_thread_id,d.chat_id,p.name \
         FROM deploys d LEFT JOIN projects p ON p.id=d.project_id \
         WHERE d.status='running' OR d.id IN (SELECT id FROM deploys ORDER BY id DESC LIMIT 30) ORDER BY d.id DESC",
    )?;
    let rows: Vec<(Deploy, String)> = q
        .query_map([], |r| Ok((Deploy::from_row(r)?, r.get::<_, Option<String>>(13)?.unwrap_or_default())))?
        .collect::<Result<_, _>>()?;
    drop(q);
    drop(c);
    let out: Vec<Value> = rows
        .into_iter()
        .filter(|(d, _)| can_access_project(&st.db, &u, d.project_id))
        .take(10)
        .map(|(d, name)| {
            let mut v = json!(d);
            v["project_name"] = json!(name);
            v
        })
        .collect();
    Ok(Json(Value::Array(out)))
}

pub async fn deploy_one(State(st): State<St>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Json<Deploy>> {
    let d = store::get_deploy(&st.db, id)?;
    if !can_access_project(&st.db, &u, d.project_id) {
        return Err(ApiError::forbidden());
    }
    Ok(Json(d))
}

pub async fn deploy(State(st): State<St>, AuthUser(u): AuthUser, Path(pid): Path<i64>) -> ApiResult<Json<Value>> {
    if !can_access_project(&st.db, &u, pid) {
        return Err(ApiError::forbidden());
    }
    let id = deploy::start(&st, pid, &u.username, None, None, None).ok_or_else(|| ApiError::bad("a deploy is already running for this project"))?;
    Ok(Json(json!({"id": id})))
}

#[derive(Deserialize)]
pub struct RollbackBody {
    /// Commit to go back to; defaults to the deploy's previous commit.
    sha: Option<String>,
}

pub async fn rollback(State(st): State<St>, AuthUser(u): AuthUser, Path(id): Path<i64>, Json(b): Json<RollbackBody>) -> ApiResult<Json<Value>> {
    let d = store::get_deploy(&st.db, id)?;
    if !can_access_project(&st.db, &u, d.project_id) {
        return Err(ApiError::forbidden());
    }
    let sha = b.sha.or(d.commit_sha).ok_or_else(|| ApiError::bad("no commit recorded for this deploy"))?;
    let nid = deploy::start(&st, d.project_id, &u.username, Some(sha), None, d.chat_id).ok_or_else(|| ApiError::bad("a deploy is already running for this project"))?;
    Ok(Json(json!({"id": nid})))
}
