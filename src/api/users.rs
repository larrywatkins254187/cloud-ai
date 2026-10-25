use crate::auth::{validate_password, AdminUser};
use crate::crypto;
use crate::state::St;
use crate::store::{get_user, User};
use crate::util::{now, ApiError, ApiResult};
use axum::extract::{Path, State};
use axum::Json;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};

pub async fn list(State(st): State<St>, AdminUser(_): AdminUser) -> ApiResult<Json<Value>> {
    let c = st.db.conn();
    let mut q = c.prepare(&format!("SELECT {} FROM users ORDER BY username", User::COLS))?;
    let users: Vec<User> = q.query_map([], User::from_row)?.collect::<Result<_, _>>()?;
    let mut out = Vec::new();
    for u in users {
        let mut pq = c.prepare("SELECT project_id FROM project_users WHERE user_id=?1")?;
        let projects: Vec<i64> = pq.query_map([u.id], |r| r.get(0))?.collect::<Result<_, _>>()?;
        let mut v = serde_json::to_value(&u).unwrap();
        v["projects"] = json!(projects);
        out.push(v);
    }
    Ok(Json(Value::Array(out)))
}

#[derive(Deserialize)]
pub struct NewUser {
    username: String,
    password: String,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    projects: Vec<i64>,
}

fn check_role(r: &str) -> ApiResult<()> {
    if r != "admin" && r != "user" {
        return Err(ApiError::bad("role must be admin or user"));
    }
    Ok(())
}

pub async fn create(State(st): State<St>, AdminUser(_): AdminUser, Json(b): Json<NewUser>) -> ApiResult<Json<User>> {
    let role = b.role.unwrap_or_else(|| "user".into());
    check_role(&role)?;
    let name = b.username.trim();
    if name.is_empty() {
        return Err(ApiError::bad("username required"));
    }
    validate_password(&b.password)?;
    let id = {
        let c = st.db.conn();
        c.execute(
            "INSERT INTO users(username,password_hash,role,created_at) VALUES(?1,?2,?3,?4)",
            params![name, crypto::hash_password(&b.password)?, role, now()],
        )
        .map_err(|e| ApiError::bad(format!("could not create user: {e}")))?;
        let id = c.last_insert_rowid();
        for p in &b.projects {
            c.execute("INSERT OR IGNORE INTO project_users(project_id,user_id) VALUES(?1,?2)", params![p, id])?;
        }
        id
    };
    Ok(Json(get_user(&st.db, id)?))
}

#[derive(Deserialize)]
pub struct UpdateUser {
    role: Option<String>,
    password: Option<String>,
    disabled: Option<bool>,
    projects: Option<Vec<i64>>,
}

pub async fn update(State(st): State<St>, AdminUser(me): AdminUser, Path(id): Path<i64>, Json(b): Json<UpdateUser>) -> ApiResult<Json<User>> {
    get_user(&st.db, id)?;
    let c = st.db.conn();
    if let Some(r) = &b.role {
        check_role(r)?;
        if id == me.id && r != "admin" {
            return Err(ApiError::bad("you cannot remove your own admin role"));
        }
        c.execute("UPDATE users SET role=?2 WHERE id=?1", params![id, r])?;
    }
    if let Some(pw) = &b.password {
        if !pw.is_empty() {
            validate_password(pw)?;
            c.execute("UPDATE users SET password_hash=?2 WHERE id=?1", params![id, crypto::hash_password(pw)?])?;
            c.execute("DELETE FROM sessions WHERE user_id=?1", [id])?;
        }
    }
    if let Some(d) = b.disabled {
        if id == me.id && d {
            return Err(ApiError::bad("you cannot disable yourself"));
        }
        c.execute("UPDATE users SET disabled=?2 WHERE id=?1", params![id, d as i64])?;
        if d {
            c.execute("DELETE FROM sessions WHERE user_id=?1", [id])?;
        }
    }
    if let Some(ps) = &b.projects {
        c.execute("DELETE FROM project_users WHERE user_id=?1", [id])?;
        for p in ps {
            c.execute("INSERT OR IGNORE INTO project_users(project_id,user_id) VALUES(?1,?2)", params![p, id])?;
        }
    }
    drop(c);
    Ok(Json(get_user(&st.db, id)?))
}

pub async fn remove(State(st): State<St>, AdminUser(me): AdminUser, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    if id == me.id {
        return Err(ApiError::bad("you cannot delete yourself"));
    }
    st.db.conn().execute("DELETE FROM users WHERE id=?1", [id])?;
    Ok(Json(json!({"ok": true})))
}

pub async fn reset_2fa(State(st): State<St>, AdminUser(_): AdminUser, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let c = st.db.conn();
    c.execute("UPDATE users SET totp_enabled=0, totp_secret=NULL WHERE id=?1", [id])?;
    c.execute("DELETE FROM sessions WHERE user_id=?1", [id])?;
    Ok(Json(json!({"ok": true})))
}
