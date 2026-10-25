use crate::config::Config;
use crate::crypto::Secrets;
use crate::db::Db;
use crate::events::{Envelope, Event};
use std::sync::Arc;
use tokio::sync::broadcast;

pub type St = Arc<AppState>;

pub struct AppState {
    pub cfg: Config,
    pub db: Db,
    pub secrets: Secrets,
    pub bus: broadcast::Sender<Envelope>,
    /// Shared HTTP client (Telegram, GitHub, npm registry, health checks).
    pub http: reqwest::Client,
    pub runner: crate::runner::RunnerState,
    pub codex: crate::agents::codex::CodexManager,
    pub telegram: crate::telegram::TelegramState,
    pub login_limiter: crate::auth::LoginLimiter,
    pub sysmon: crate::sysmon::SysMon,
    pub deploys: crate::deploy::DeployState,
    pub terminals: crate::terminal::TerminalState,
    pub devservers: crate::devserver::DevServers,
    pub browser: crate::browser::BrowserState,
    pub updater: crate::updater::UpdaterState,
}

impl AppState {
    pub fn emit(&self, env: Envelope) {
        let _ = self.bus.send(env);
    }
    pub fn emit_project(&self, project_id: i64, event: Event) {
        self.emit(Envelope::project(project_id, event));
    }
    pub fn notice(&self, level: &str, text: impl Into<String>) {
        let text = text.into();
        self.db.log_event(level, &text);
        self.emit(Envelope::admin(Event::Notice { level: level.into(), text }));
    }
}
