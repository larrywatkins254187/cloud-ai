use crate::auth::AuthUser;
use crate::git;
use crate::state::St;
use crate::store::{can_access_project, get_chat, get_project, User};
use crate::util::{ApiError, ApiResult};
use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Deserialize, Default)]
pub struct Q {
    /// Work in a chat's worktree instead of the project folder.
    chat: Option<i64>,
    path: Option<String>,
}

fn dir(st: &St, u: &User, pid: i64, chat: Option<i64>) -> ApiResult<PathBuf> {
    if !can_access_project(&st.db, u, pid) {
        return Err(ApiError::forbidden());
    }
    if let Some(cid) = chat {
        let c = get_chat(&st.db, cid)?;
        if c.project_id != pid {
            return Err(ApiError::bad("chat belongs to another project"));
        }
        return Ok(PathBuf::from(c.cwd));
    }
    Ok(PathBuf::from(get_project(&st.db, pid)?.path))
}

pub async fn status(State(st): State<St>, AuthUser(u): AuthUser, Path(pid): Path<i64>, Query(q): Query<Q>) -> ApiResult<Json<Value>> {
    let d = dir(&st, &u, pid, q.chat)?;
    Ok(Json(git::status(&d).await?))
}

pub async fn diff(State(st): State<St>, AuthUser(u): AuthUser, Path(pid): Path<i64>, Query(q): Query<Q>) -> ApiResult<Json<Value>> {
    let d = dir(&st, &u, pid, q.chat)?;
    let text = git::diff(&d, q.path.as_deref()).await?;
    Ok(Json(json!({"diff": crate::util::truncate(&text, 2_000_000)})))
}

pub async fn log(State(st): State<St>, AuthUser(u): AuthUser, Path(pid): Path<i64>, Query(q): Query<Q>) -> ApiResult<Json<Value>> {
    let d = dir(&st, &u, pid, q.chat)?;
    Ok(Json(git::log(&d, 60).await?))
}

pub async fn branches(State(st): State<St>, AuthUser(u): AuthUser, Path(pid): Path<i64>, Query(q): Query<Q>) -> ApiResult<Json<Value>> {
    let d = dir(&st, &u, pid, q.chat)?;
    Ok(Json(git::branches(&d).await?))
}

#[derive(Deserialize)]
pub struct CommitBody {
    message: String,
    chat: Option<i64>,
}

pub async fn commit(State(st): State<St>, AuthUser(u): AuthUser, Path(pid): Path<i64>, Json(b): Json<CommitBody>) -> ApiResult<Json<Value>> {
    let d = dir(&st, &u, pid, b.chat)?;
    if b.message.trim().is_empty() {
        return Err(ApiError::bad("commit message required"));
    }
    let done = git::commit_all(&d, b.message.trim()).await.map_err(|e| ApiError::bad(e.to_string()))?;
    Ok(Json(json!({"ok": true, "committed": done})))
}

#[derive(Deserialize, Default)]
pub struct ChatBody {
    chat: Option<i64>,
}

async fn simple(d: PathBuf, args: &[&str]) -> ApiResult<Json<Value>> {
    let o = git::run_timeout(&d, args, Duration::from_secs(300)).await?;
    if !o.ok {
        return Err(ApiError::bad(format!("{}{}", o.stdout, o.stderr).trim().to_string()));
    }
    Ok(Json(json!({"ok": true, "output": format!("{}{}", o.stdout, o.stderr)})))
}

pub async fn push(State(st): State<St>, AuthUser(u): AuthUser, Path(pid): Path<i64>, Json(b): Json<ChatBody>) -> ApiResult<Json<Value>> {
    let d = dir(&st, &u, pid, b.chat)?;
    let branch = git::current_branch(&d).await.unwrap_or_else(|| "HEAD".into());
    simple(d, &["push", "-u", "origin", &branch]).await
}

pub async fn pull(State(st): State<St>, AuthUser(u): AuthUser, Path(pid): Path<i64>, Json(b): Json<ChatBody>) -> ApiResult<Json<Value>> {
    let d = dir(&st, &u, pid, b.chat)?;
    simple(d, &["pull", "--ff-only"]).await
}

#[derive(Deserialize)]
pub struct CheckoutBody {
    branch: String,
    #[serde(default)]
    create: bool,
}

pub async fn checkout(State(st): State<St>, AuthUser(u): AuthUser, Path(pid): Path<i64>, Json(b): Json<CheckoutBody>) -> ApiResult<Json<Value>> {
    let d = dir(&st, &u, pid, None)?;
    if b.create {
        simple(d, &["checkout", "-b", b.branch.trim()]).await
    } else {
        let name = b.branch.trim().trim_start_matches("origin/").to_string();
        simple(d, &["checkout", &name]).await
    }
}

#[derive(Deserialize)]
pub struct DiscardBody {
    path: String,
    chat: Option<i64>,
}

pub async fn discard(State(st): State<St>, AuthUser(u): AuthUser, Path(pid): Path<i64>, Json(b): Json<DiscardBody>) -> ApiResult<Json<Value>> {
    let d = dir(&st, &u, pid, b.chat)?;
    let tracked = git::run(&d, &["ls-files", "--error-unmatch", &b.path]).await?.ok;
    if tracked {
        simple(d, &["checkout", "HEAD", "--", &b.path]).await
    } else {
        simple(d, &["clean", "-f", "--", &b.path]).await
    }
}
