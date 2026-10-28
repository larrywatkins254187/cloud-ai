use crate::store::{Chat, Deploy, Message, Request, Run};
use serde::Serialize;
use std::sync::{Arc, OnceLock};

/// Everything that happens in the app goes through this bus. The WebSocket hub
/// forwards events to browsers (filtered by project access) and the Telegram
/// bot turns some of them into messages.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    ChatCreated { chat: Chat },
    ChatUpdated { chat: Chat },
    ChatDeleted { chat_id: i64, project_id: i64 },
    Message { message: Message },
    Delta { chat_id: i64, run_id: i64, kind: String, text: String },
    /// How full the chat's context window is (see `agents::claude_context`).
    ChatContext { chat_id: i64, context: serde_json::Value },
    RunQueued { run: Run },
    RunStarted { run: Run },
    RunFinished { run: Run },
    RequestCreated { request: Request },
    RequestResolved { request: Request },
    DeployUpdated { deploy: Deploy },
    DeployLog { deploy_id: i64, project_id: i64, line: String },
    Notice { level: String, text: String },
}

#[derive(Clone, Debug)]
pub struct Envelope {
    /// Project the event belongs to; `None` = visible to admins only.
    pub project_id: Option<i64>,
    pub event: Event,
    /// JSON of the event, built once and shared by every browser connection.
    json: Arc<OnceLock<Arc<str>>>,
}

impl Envelope {
    pub fn project(project_id: i64, event: Event) -> Self {
        Envelope { project_id: Some(project_id), event, json: Default::default() }
    }
    pub fn admin(event: Event) -> Self {
        Envelope { project_id: None, event, json: Default::default() }
    }
    pub fn json(&self) -> Arc<str> {
        self.json.get_or_init(|| serde_json::to_string(&self.event).unwrap_or_default().into()).clone()
    }
}
