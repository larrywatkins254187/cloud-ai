use crate::auth::{AdminUser, AuthUser};
use crate::state::St;
use crate::store::{self, can_access_project, get_project, list_projects, Project};
use crate::util::{now, ApiError, ApiResult};
use axum::extract::{Path, Query, State};
use axum::Json;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

pub async fn list(State(st): State<St>, AuthUser(u): AuthUser) -> ApiResult<Json<Value>> {
    let projects = list_projects(&st.db, &u)?;
    let c = st.db.conn();
    let mut out = Vec::new();
    for p in projects {
        let mut v = serde_json::to_value(&p).unwrap();
        let counts: (i64, i64) = c.query_row(
            "SELECT COUNT(*), COALESCE(SUM(status IN ('running','waiting','queued')),0) FROM chats WHERE project_id=?1 AND archived=0",
            [p.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        v["chat_count"] = json!(counts.0);
        v["active_count"] = json!(counts.1);
        v["deployable"] = json!(p.deployable());
        if u.is_admin() {
            let mut q = c.prepare("SELECT user_id FROM project_users WHERE project_id=?1")?;
            let users: Vec<i64> = q.query_map([p.id], |r| r.get(0))?.collect::<Result<_, _>>()?;
            v["users"] = json!(users);
        }
        out.push(v);
    }
    Ok(Json(Value::Array(out)))
}

pub async fn get_one(State(st): State<St>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    if !can_access_project(&st.db, &u, id) {
        return Err(ApiError::forbidden());
    }
    let p = get_project(&st.db, id)?;
    let mut v = serde_json::to_value(&p).unwrap();
    v["deployable"] = json!(p.deployable());
    v["is_git"] = json!(crate::git::is_repo(std::path::Path::new(&p.path)).await);
    v["exists"] = json!(std::path::Path::new(&p.path).is_dir());
    Ok(Json(v))
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct ProjectBody {
    name: Option<String>,
    path: Option<String>,
    description: Option<String>,
    dev_command: Option<String>,
    dev_port: Option<i64>,
    prod_url: Option<String>,
    deploy_target: Option<String>,
    server_id: Option<i64>,
    deploy_method: Option<String>,
    remote_path: Option<String>,
    deploy_command: Option<String>,
    rsync_excludes: Option<String>,
    auto_commit: Option<bool>,
    health_url: Option<String>,
    auto_rollback: Option<bool>,
    users: Option<Vec<i64>>,
    // create only
    source: Option<String>,
    git_url: Option<String>,
    git_init: Option<bool>,
    import_server_id: Option<i64>,
    import_remote_path: Option<String>,
}

fn validate(b: &ProjectBody) -> ApiResult<()> {
    if let Some(t) = &b.deploy_target {
        if !["none", "local", "remote"].contains(&t.as_str()) {
            return Err(ApiError::bad("deploy target must be none, local or remote"));
        }
    }
    if let Some(m) = &b.deploy_method {
        if !["git", "rsync"].contains(&m.as_str()) {
            return Err(ApiError::bad("deploy method must be git or rsync"));
        }
    }
    Ok(())
}

pub async fn create(State(st): State<St>, AdminUser(_): AdminUser, Json(b): Json<ProjectBody>) -> ApiResult<Json<Project>> {
    validate(&b)?;
    let path_s = b.path.clone().unwrap_or_default().trim().trim_end_matches('/').to_string();
    if !path_s.starts_with('/') {
        return Err(ApiError::bad("path must be absolute, e.g. /var/www/my-app"));
    }
    let path = PathBuf::from(&path_s);
    let source = b.source.clone().unwrap_or_else(|| "existing".into());
    match source.as_str() {
        "existing" => {
            if !path.is_dir() {
                return Err(ApiError::bad("folder does not exist"));
            }
        }
        "new" => {
            if path.exists() && std::fs::read_dir(&path).map(|mut d| d.next().is_some()).unwrap_or(true) {
                return Err(ApiError::bad("folder already exists and is not empty"));
            }
            std::fs::create_dir_all(&path)?;
            if b.git_init.unwrap_or(true) {
                crate::git::must(&path, &["init"]).await?;
            }
        }
        "clone" => {
            let url = b.git_url.clone().unwrap_or_default();
            if url.trim().is_empty() {
                return Err(ApiError::bad("git URL required"));
            }
            if path.exists() && std::fs::read_dir(&path).map(|mut d| d.next().is_some()).unwrap_or(true) {
                return Err(ApiError::bad("target folder already exists and is not empty"));
            }
            let parent = path.parent().ok_or_else(|| ApiError::bad("bad path"))?;
            std::fs::create_dir_all(parent)?;
            let o = crate::git::run_timeout(parent, &["clone", url.trim(), &path_s], Duration::from_secs(900)).await?;
            if !o.ok {
                return Err(ApiError::bad(format!("git clone failed: {}", o.stderr.trim())));
            }
        }
        "remote" => {
            let sid = b.import_server_id.ok_or_else(|| ApiError::bad("server required"))?;
            let rp = b.import_remote_path.clone().unwrap_or_default();
            if rp.trim().is_empty() {
                return Err(ApiError::bad("remote path required"));
            }
            let server = store::get_server(&st.db, sid)?;
            std::fs::create_dir_all(&path)?;
            crate::deploy::rsync_from(&st, &server, rp.trim(), &path).await?;
        }
        _ => return Err(ApiError::bad("unknown source")),
    }
    let name = b
        .name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "project".into()));
    let id = {
        let c = st.db.conn();
        c.execute(
            "INSERT INTO projects(name,path,created_at) VALUES(?1,?2,?3)",
            params![name.trim(), path_s, now()],
        )
        .map_err(|e| ApiError::bad(format!("could not add project: {e}")))?;
        c.last_insert_rowid()
    };
    // Importing from a server usually means deploying back to it.
    let mut b = b;
    if source == "remote" && b.deploy_target.is_none() {
        b.deploy_target = Some("remote".into());
        b.server_id = b.import_server_id;
        b.remote_path = b.import_remote_path.clone();
        b.deploy_method = Some("rsync".into());
    }
    apply(&st, id, &b)?;
    Ok(Json(get_project(&st.db, id)?))
}

fn apply(st: &St, id: i64, b: &ProjectBody) -> ApiResult<()> {
    let c = st.db.conn();
    macro_rules! set {
        ($field:ident, $col:literal) => {
            if let Some(v) = &b.$field {
                c.execute(concat!("UPDATE projects SET ", $col, "=?2 WHERE id=?1"), params![id, v])?;
            }
        };
    }
    set!(name, "name");
    set!(description, "description");
    set!(dev_command, "dev_command");
    set!(prod_url, "prod_url");
    set!(deploy_target, "deploy_target");
    set!(deploy_method, "deploy_method");
    set!(remote_path, "remote_path");
    set!(deploy_command, "deploy_command");
    set!(rsync_excludes, "rsync_excludes");
    set!(health_url, "health_url");
    if b.dev_port.is_some() || b.dev_command.is_some() {
        c.execute("UPDATE projects SET dev_port=?2 WHERE id=?1", params![id, b.dev_port])?;
    }
    if b.server_id.is_some() || b.deploy_target.is_some() {
        c.execute("UPDATE projects SET server_id=?2 WHERE id=?1", params![id, b.server_id])?;
    }
    if let Some(v) = b.auto_commit {
        c.execute("UPDATE projects SET auto_commit=?2 WHERE id=?1", params![id, v as i64])?;
    }
    if let Some(v) = b.auto_rollback {
        c.execute("UPDATE projects SET auto_rollback=?2 WHERE id=?1", params![id, v as i64])?;
    }
    if let Some(users) = &b.users {
        c.execute("DELETE FROM project_users WHERE project_id=?1", [id])?;
        for u in users {
            c.execute("INSERT OR IGNORE INTO project_users(project_id,user_id) VALUES(?1,?2)", params![id, u])?;
        }
    }
    Ok(())
}

pub async fn update(State(st): State<St>, AdminUser(_): AdminUser, Path(id): Path<i64>, Json(b): Json<ProjectBody>) -> ApiResult<Json<Project>> {
    validate(&b)?;
    get_project(&st.db, id)?;
    if let Some(p) = &b.path {
        let p = p.trim().trim_end_matches('/');
        if !std::path::Path::new(p).is_dir() {
            return Err(ApiError::bad("folder does not exist"));
        }
        st.db.conn().execute("UPDATE projects SET path=?2 WHERE id=?1", params![id, p])?;
    }
    apply(&st, id, &b)?;
    Ok(Json(get_project(&st.db, id)?))
}

pub async fn set_users(State(st): State<St>, AdminUser(_): AdminUser, Path(id): Path<i64>, Json(users): Json<Vec<i64>>) -> ApiResult<Json<Value>> {
    let b = ProjectBody { users: Some(users), ..Default::default() };
    apply(&st, id, &b)?;
    Ok(Json(json!({"ok": true})))
}

/// Removes the project from the panel. Files on disk are never deleted.
pub async fn remove(State(st): State<St>, AdminUser(_): AdminUser, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let chats: Vec<i64> = {
        let c = st.db.conn();
        let mut q = c.prepare("SELECT id FROM chats WHERE project_id=?1")?;
        let v = q.query_map([id], |r| r.get(0))?.collect::<Result<_, _>>()?;
        v
    };
    for cid in chats {
        if let Ok(chat) = store::get_chat(&st.db, cid) {
            crate::chats::delete_chat(&st, &chat).await?;
        }
    }
    crate::devserver::stop(&st, id).await;
    st.db.conn().execute("DELETE FROM projects WHERE id=?1", [id])?;
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
pub struct BrowseQ {
    path: Option<String>,
}

/// Folder picker for admins.
pub async fn browse(AdminUser(_): AdminUser, Query(q): Query<BrowseQ>) -> ApiResult<Json<Value>> {
    let path = PathBuf::from(q.path.filter(|p| !p.is_empty()).unwrap_or_else(|| "/".into()));
    let mut dirs = Vec::new();
    for e in std::fs::read_dir(&path)? {
        let Ok(e) = e else { continue };
        if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            let name = e.file_name().to_string_lossy().to_string();
            dirs.push(json!({"name": name, "path": e.path().to_string_lossy(), "git": e.path().join(".git").exists()}));
        }
    }
    dirs.sort_by(|a, b| a["name"].as_str().unwrap_or("").to_lowercase().cmp(&b["name"].as_str().unwrap_or("").to_lowercase()));
    Ok(Json(json!({
        "path": path.to_string_lossy(),
        "parent": path.parent().map(|p| p.to_string_lossy().to_string()),
        "dirs": dirs,
    })))
}
