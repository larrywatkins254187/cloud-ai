//! A simple file browser/editor confined to a project folder.
use crate::auth::AuthUser;
use crate::state::St;
use crate::store::{can_access_project, get_project};
use crate::util::{ApiError, ApiResult};
use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Component, PathBuf};

#[derive(Deserialize, Default)]
pub struct Q {
    #[serde(default)]
    path: String,
}

/// Resolve `rel` inside the project, refusing anything that escapes it.
fn resolve(st: &St, u: &crate::store::User, pid: i64, rel: &str) -> ApiResult<(PathBuf, PathBuf)> {
    if !can_access_project(&st.db, u, pid) {
        return Err(ApiError::forbidden());
    }
    let root = PathBuf::from(get_project(&st.db, pid)?.path);
    let rel = rel.trim_start_matches('/');
    let mut p = root.clone();
    for comp in std::path::Path::new(rel).components() {
        match comp {
            Component::Normal(c) => p.push(c),
            Component::CurDir => {}
            _ => return Err(ApiError::bad("invalid path")),
        }
    }
    // Follow symlinks only if they stay inside the project.
    if let Ok(real) = p.canonicalize() {
        let real_root = root.canonicalize().unwrap_or(root.clone());
        if !real.starts_with(&real_root) {
            return Err(ApiError::bad("path is outside the project"));
        }
    }
    Ok((root, p))
}

pub async fn list(State(st): State<St>, AuthUser(u): AuthUser, Path(pid): Path<i64>, Query(q): Query<Q>) -> ApiResult<Json<Value>> {
    let (root, p) = resolve(&st, &u, pid, &q.path)?;
    let mut items = Vec::new();
    for e in std::fs::read_dir(&p)? {
        let Ok(e) = e else { continue };
        let meta = e.metadata().ok();
        let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        let rel = e.path().strip_prefix(&root).map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
        items.push(json!({
            "name": e.file_name().to_string_lossy(),
            "path": rel,
            "dir": is_dir,
            "size": meta.as_ref().map(|m| m.len()).unwrap_or(0),
        }));
    }
    items.sort_by(|a, b| {
        b["dir"].as_bool().cmp(&a["dir"].as_bool()).then(
            a["name"].as_str().unwrap_or("").to_lowercase().cmp(&b["name"].as_str().unwrap_or("").to_lowercase()),
        )
    });
    Ok(Json(json!({"path": q.path, "items": items})))
}

pub async fn read(State(st): State<St>, AuthUser(u): AuthUser, Path(pid): Path<i64>, Query(q): Query<Q>) -> ApiResult<Json<Value>> {
    let (_, p) = resolve(&st, &u, pid, &q.path)?;
    let meta = std::fs::metadata(&p)?;
    if meta.len() > 2 * 1024 * 1024 {
        return Ok(Json(json!({"path": q.path, "binary": true, "too_large": true, "size": meta.len()})));
    }
    let bytes = std::fs::read(&p)?;
    match String::from_utf8(bytes) {
        Ok(text) => Ok(Json(json!({"path": q.path, "content": text, "size": meta.len()}))),
        Err(_) => Ok(Json(json!({"path": q.path, "binary": true, "size": meta.len()}))),
    }
}

#[derive(Deserialize)]
pub struct WriteBody {
    path: String,
    content: String,
}

pub async fn write(State(st): State<St>, AuthUser(u): AuthUser, Path(pid): Path<i64>, Json(b): Json<WriteBody>) -> ApiResult<Json<Value>> {
    let (_, p) = resolve(&st, &u, pid, &b.path)?;
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&p, b.content)?;
    Ok(Json(json!({"ok": true})))
}

pub async fn mkdir(State(st): State<St>, AuthUser(u): AuthUser, Path(pid): Path<i64>, Json(b): Json<Q>) -> ApiResult<Json<Value>> {
    let (_, p) = resolve(&st, &u, pid, &b.path)?;
    std::fs::create_dir_all(p)?;
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
pub struct RenameBody {
    from: String,
    to: String,
}

pub async fn rename(State(st): State<St>, AuthUser(u): AuthUser, Path(pid): Path<i64>, Json(b): Json<RenameBody>) -> ApiResult<Json<Value>> {
    let (_, from) = resolve(&st, &u, pid, &b.from)?;
    let (_, to) = resolve(&st, &u, pid, &b.to)?;
    if to.exists() {
        return Err(ApiError::bad("target already exists"));
    }
    std::fs::rename(from, to)?;
    Ok(Json(json!({"ok": true})))
}

pub async fn remove(State(st): State<St>, AuthUser(u): AuthUser, Path(pid): Path<i64>, Query(q): Query<Q>) -> ApiResult<Json<Value>> {
    if q.path.trim_matches('/').is_empty() {
        return Err(ApiError::bad("refusing to delete the project root"));
    }
    let (_, p) = resolve(&st, &u, pid, &q.path)?;
    if p.is_dir() {
        std::fs::remove_dir_all(p)?;
    } else {
        std::fs::remove_file(p)?;
    }
    Ok(Json(json!({"ok": true})))
}
