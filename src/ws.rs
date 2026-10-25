//! Live updates for the browser: every bus event the user may see is pushed
//! over one WebSocket.
use crate::auth::AuthUser;
use crate::state::St;
use crate::store::{can_access_project, User};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::broadcast::error::RecvError;

pub async fn handler(State(st): State<St>, AuthUser(u): AuthUser, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| run(st, u, socket))
}

async fn run(st: St, user: User, mut socket: WebSocket) {
    let mut rx = st.bus.subscribe();
    // Cache project access checks for a short while.
    let mut access: HashMap<i64, (bool, Instant)> = HashMap::new();
    let mut ping = tokio::time::interval(Duration::from_secs(25));
    loop {
        tokio::select! {
            ev = rx.recv() => {
                let env = match ev {
                    Ok(e) => e,
                    Err(RecvError::Lagged(_)) => {
                        let _ = socket.send(Message::Text("{\"type\":\"resync\"}".into())).await;
                        continue;
                    }
                    Err(RecvError::Closed) => break,
                };
                let visible = if env.public || user.is_admin() {
                    true
                } else if let Some(pid) = env.project_id {
                    let entry = access.get(&pid).copied();
                    match entry {
                        Some((ok, t)) if t.elapsed() < Duration::from_secs(30) => ok,
                        _ => {
                            let ok = can_access_project(&st.db, &user, pid);
                            access.insert(pid, (ok, Instant::now()));
                            ok
                        }
                    }
                } else {
                    false
                };
                if !visible {
                    continue;
                }
                let Ok(text) = serde_json::to_string(&env.event) else { continue };
                if socket.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
            _ = ping.tick() => {
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
        }
    }
}
