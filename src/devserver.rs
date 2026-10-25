//! Per-project dev servers (e.g. `npm run dev -- -p 4001`) started and stopped
//! from the panel. Each runs in its own process group so stopping it also stops
//! the children (npm → next, vite, …).
use crate::auth::AuthUser;
use crate::state::St;
use crate::store::{can_access_project, get_project};
use crate::util::{now, ApiError, ApiResult};
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

const LOG_LINES: usize = 500;

pub struct DevServer {
    pid: u32,
    command: String,
    port: Option<i64>,
    started_at: String,
    log: Mutex<VecDeque<String>>,
    exit: Mutex<Option<String>>,
}

#[derive(Default)]
pub struct DevServers {
    map: Mutex<HashMap<i64, Arc<DevServer>>>,
}

pub fn router() -> Router<St> {
    Router::new()
        .route("/api/projects/{id}/dev", get(status))
        .route("/api/projects/{id}/dev/start", post(start))
        .route("/api/projects/{id}/dev/stop", post(stop_api))
}

fn push(d: &DevServer, line: String) {
    let mut l = d.log.lock().unwrap();
    l.push_back(line);
    while l.len() > LOG_LINES {
        l.pop_front();
    }
}

fn info(d: Option<&Arc<DevServer>>) -> Value {
    match d {
        Some(d) => {
            let exit = d.exit.lock().unwrap().clone();
            json!({
                "running": exit.is_none(),
                "pid": d.pid,
                "command": d.command,
                "port": d.port,
                "started_at": d.started_at,
                "exit": exit,
                "log": d.log.lock().unwrap().iter().cloned().collect::<Vec<_>>().join("\n"),
            })
        }
        None => json!({"running": false}),
    }
}

async fn status(State(st): State<St>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    if !can_access_project(&st.db, &u, id) {
        return Err(ApiError::forbidden());
    }
    let d = st.devservers.map.lock().unwrap().get(&id).cloned();
    Ok(Json(info(d.as_ref())))
}

async fn start(State(st): State<St>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    if !can_access_project(&st.db, &u, id) {
        return Err(ApiError::forbidden());
    }
    let p = get_project(&st.db, id)?;
    if p.dev_command.trim().is_empty() {
        return Err(ApiError::bad("no dev command configured for this project (Settings tab)"));
    }
    if let Some(d) = st.devservers.map.lock().unwrap().get(&id) {
        if d.exit.lock().unwrap().is_none() {
            return Ok(Json(info(Some(d))));
        }
    }
    if let Some(port) = p.dev_port {
        if std::net::TcpListener::bind(("127.0.0.1", port as u16)).is_err() {
            return Err(ApiError::bad(format!(
                "port {port} is already in use — the dev server may already be running outside the panel"
            )));
        }
    }
    let mut cmd = Command::new("bash");
    cmd.arg("-lc")
        .arg(&p.dev_command)
        .current_dir(&p.path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    if let Some(port) = p.dev_port {
        cmd.env("PORT", port.to_string());
    }
    let mut child = cmd.spawn().map_err(|e| ApiError::bad(format!("could not start: {e}")))?;
    let d = Arc::new(DevServer {
        pid: child.id().unwrap_or(0),
        command: p.dev_command.clone(),
        port: p.dev_port,
        started_at: now(),
        log: Mutex::new(VecDeque::new()),
        exit: Mutex::new(None),
    });
    for stream in [child.stdout.take().map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>), child.stderr.take().map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>)].into_iter().flatten() {
        let d = d.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stream).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                push(&d, strip_ansi(&l));
            }
        });
    }
    {
        let d = d.clone();
        tokio::spawn(async move {
            let status = child.wait().await;
            let msg = match status {
                Ok(s) => format!("exited with {}", s.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into())),
                Err(e) => e.to_string(),
            };
            push(&d, format!("[{msg}]"));
            *d.exit.lock().unwrap() = Some(msg);
        });
    }
    st.devservers.map.lock().unwrap().insert(id, d.clone());
    Ok(Json(info(Some(&d))))
}

fn kill_group(pid: u32, sig: i32) {
    if pid > 0 {
        unsafe {
            libc::kill(-(pid as i32), sig);
        }
    }
}

pub async fn stop(st: &St, project_id: i64) {
    let d = st.devservers.map.lock().unwrap().get(&project_id).cloned();
    if let Some(d) = d {
        if d.exit.lock().unwrap().is_some() {
            return;
        }
        kill_group(d.pid, libc::SIGTERM);
        for _ in 0..50 {
            if d.exit.lock().unwrap().is_some() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        kill_group(d.pid, libc::SIGKILL);
    }
}

async fn stop_api(State(st): State<St>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    if !can_access_project(&st.db, &u, id) {
        return Err(ApiError::forbidden());
    }
    stop(&st, id).await;
    let d = st.devservers.map.lock().unwrap().get(&id).cloned();
    Ok(Json(info(d.as_ref())))
}

pub async fn stop_all(st: &St) {
    let ids: Vec<i64> = st.devservers.map.lock().unwrap().keys().copied().collect();
    for id in ids {
        stop(st, id).await;
    }
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c2 in chars.by_ref() {
                    if c2.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

pub fn is_running(st: &St, project_id: i64) -> bool {
    st.devservers.map.lock().unwrap().get(&project_id).map(|d| d.exit.lock().unwrap().is_none()).unwrap_or(false)
}
