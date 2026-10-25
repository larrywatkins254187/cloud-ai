//! Web terminal: PTY sessions that survive browser reconnects (useful on
//! phones), streamed over a WebSocket to xterm.js. Admin only.
use crate::auth::AdminUser;
use crate::state::St;
use crate::util::{ApiError, ApiResult};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::routing::{delete, get};
use axum::{Json, Router};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

const SCROLLBACK: usize = 256 * 1024;

pub struct Term {
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub created_at: String,
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    buffer: Mutex<Vec<u8>>,
    out: broadcast::Sender<Vec<u8>>,
    alive: Mutex<bool>,
}

#[derive(Default)]
pub struct TerminalState {
    terms: Mutex<HashMap<String, Arc<Term>>>,
}

pub fn router() -> Router<St> {
    Router::new()
        .route("/api/terminals", get(list).post(create))
        .route("/api/terminals/{id}", delete(remove))
        .route("/api/terminals/{id}/ws", get(attach))
}

#[derive(Deserialize, Default)]
pub struct NewTerm {
    cwd: Option<String>,
    /// Command to run instead of an interactive shell (e.g. `claude auth login`).
    command: Option<String>,
    title: Option<String>,
}

/// Start a terminal session. Used by the API and by the CLI login helpers.
pub fn spawn_term(st: &St, b: NewTerm) -> anyhow::Result<Arc<Term>> {
    let pty = native_pty_system();
    let pair = pty.openpty(PtySize { rows: 30, cols: 100, pixel_width: 0, pixel_height: 0 })?;
    let cwd = b.cwd.filter(|c| std::path::Path::new(c).is_dir()).unwrap_or_else(|| {
        std::env::var("HOME").unwrap_or_else(|_| "/root".into())
    });
    let mut cmd = match &b.command {
        Some(c) if !c.trim().is_empty() => {
            let mut cb = CommandBuilder::new("bash");
            // Keep the shell open after the command so its output stays readable.
            cb.args(["-lc", &format!("{c}; echo; echo '[process finished — press Enter to close]'; read")]);
            cb
        }
        _ => {
            let mut cb = CommandBuilder::new("bash");
            cb.arg("-l");
            cb
        }
    };
    cmd.cwd(&cwd);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("IS_SANDBOX", "1");
    let child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader()?;
    let writer = pair.master.take_writer()?;
    let (tx, _) = broadcast::channel(256);
    let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
    let term = Arc::new(Term {
        id: id.clone(),
        title: b.title.unwrap_or_else(|| b.command.clone().unwrap_or_else(|| "shell".into())),
        cwd,
        created_at: crate::util::now(),
        master: Mutex::new(pair.master),
        writer: Mutex::new(writer),
        child: Mutex::new(child),
        buffer: Mutex::new(Vec::new()),
        out: tx,
        alive: Mutex::new(true),
    });
    {
        let t = term.clone();
        let st = st.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let chunk = buf[..n].to_vec();
                        {
                            let mut b = t.buffer.lock().unwrap();
                            b.extend_from_slice(&chunk);
                            if b.len() > SCROLLBACK {
                                let cut = b.len() - SCROLLBACK;
                                b.drain(..cut);
                            }
                        }
                        let _ = t.out.send(chunk);
                    }
                }
            }
            *t.alive.lock().unwrap() = false;
            let _ = t.out.send(b"\r\n\x1b[90m[session ended]\x1b[0m\r\n".to_vec());
            st.terminals.terms.lock().unwrap().remove(&t.id);
        });
    }
    st.terminals.terms.lock().unwrap().insert(id, term.clone());
    Ok(term)
}

fn info(t: &Term) -> Value {
    json!({"id": t.id, "title": t.title, "cwd": t.cwd, "created_at": t.created_at, "alive": *t.alive.lock().unwrap()})
}

async fn list(State(st): State<St>, AdminUser(_): AdminUser) -> Json<Value> {
    let terms = st.terminals.terms.lock().unwrap();
    let mut v: Vec<Value> = terms.values().map(|t| info(t)).collect();
    v.sort_by(|a, b| a["created_at"].as_str().cmp(&b["created_at"].as_str()));
    Json(Value::Array(v))
}

async fn create(State(st): State<St>, AdminUser(_): AdminUser, Json(b): Json<NewTerm>) -> ApiResult<Json<Value>> {
    let t = spawn_term(&st, b)?;
    Ok(Json(info(&t)))
}

async fn remove(State(st): State<St>, AdminUser(_): AdminUser, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let t = st.terminals.terms.lock().unwrap().remove(&id).ok_or_else(ApiError::not_found)?;
    let _ = t.child.lock().unwrap().kill();
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
pub struct AttachQ {
    cols: Option<u16>,
    rows: Option<u16>,
}

async fn attach(
    State(st): State<St>,
    AdminUser(_): AdminUser,
    Path(id): Path<String>,
    Query(q): Query<AttachQ>,
    ws: WebSocketUpgrade,
) -> ApiResult<Response> {
    let t = st.terminals.terms.lock().unwrap().get(&id).cloned().ok_or_else(ApiError::not_found)?;
    if let (Some(cols), Some(rows)) = (q.cols, q.rows) {
        let _ = t.master.lock().unwrap().resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
    }
    Ok(ws.on_upgrade(move |socket| pump(t, socket)))
}

async fn pump(t: Arc<Term>, mut socket: WebSocket) {
    let mut rx = t.out.subscribe();
    let backlog = t.buffer.lock().unwrap().clone();
    if !backlog.is_empty() && socket.send(Message::Binary(backlog.into())).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            out = rx.recv() => match out {
                Ok(chunk) => {
                    if socket.send(Message::Binary(chunk.into())).await.is_err() { break; }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Text(text))) => {
                    // Control messages are JSON; everything else is keyboard input.
                    if text.starts_with("{\"type\"") {
                        if let Ok(v) = serde_json::from_str::<Value>(&text) {
                            if v["type"] == "resize" {
                                let cols = v["cols"].as_u64().unwrap_or(80) as u16;
                                let rows = v["rows"].as_u64().unwrap_or(24) as u16;
                                let _ = t.master.lock().unwrap().resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
                                continue;
                            }
                        }
                    }
                    let _ = t.writer.lock().unwrap().write_all(text.as_bytes());
                }
                Some(Ok(Message::Binary(b))) => {
                    let _ = t.writer.lock().unwrap().write_all(&b);
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            }
        }
    }
}

pub fn stop_all(st: &St) {
    for (_, t) in st.terminals.terms.lock().unwrap().drain() {
        let _ = t.child.lock().unwrap().kill();
    }
}
