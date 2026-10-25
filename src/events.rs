use crate::store::{Chat, Deploy, Message, Request, Run};
use serde::Serialize;
use serde_json::Value;

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
    RunQueued { run: Run },
    RunStarted { run: Run },
    RunFinished { run: Run },
    RequestCreated { request: Request },
    RequestResolved { request: Request },
    DeployUpdated { deploy: Deploy },
    DeployLog { deploy_id: i64, project_id: i64, line: String },
    Notice { level: String, text: String },
    Status { data: Value },
}

#[derive(Clone, Debug)]
pub struct Envelope {
    /// Project the event belongs to; `None` = visible to admins only.
    pub project_id: Option<i64>,
    /// Visible to every logged-in user regardless of project.
    pub public: bool,
    pub event: Event,
}

impl Envelope {
    pub fn project(project_id: i64, event: Event) -> Self {
        Envelope { project_id: Some(project_id), public: false, event }
    }
    pub fn admin(event: Event) -> Self {
        Envelope { project_id: None, public: false, event }
    }
    pub fn public(event: Event) -> Self {
        Envelope { project_id: None, public: true, event }
    }
}
