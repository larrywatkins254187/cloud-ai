mod chats;
mod files;
mod gitops;
mod projects;
mod schedules;
mod servers;
mod system;
pub mod usage;
mod users;

use crate::auth;
use crate::state::St;
use axum::routing::{delete, get, post, put};
use axum::Router;

pub fn router() -> Router<St> {
    Router::new()
        // auth
        .route("/api/auth/me", get(auth::me))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/totp", post(auth::verify_totp))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/password", post(auth::change_password))
        .route("/api/setup", post(auth::setup))
        // users (admin)
        .route("/api/users", get(users::list).post(users::create))
        .route("/api/users/{id}", put(users::update).delete(users::remove))
        .route("/api/users/{id}/reset-2fa", post(users::reset_2fa))
        // projects
        .route("/api/projects", get(projects::list).post(projects::create))
        .route("/api/projects/{id}", get(projects::get_one).put(projects::update).delete(projects::remove))
        .route("/api/projects/{id}/users", put(projects::set_users))
        .route("/api/fs/browse", get(projects::browse))
        // chats
        .route("/api/projects/{id}/chats", get(chats::list).post(chats::create))
        .route("/api/chats", get(chats::list_all))
        .route("/api/chats/{id}", get(chats::get_one).put(chats::update).delete(chats::remove))
        .route("/api/chats/{id}/messages", get(chats::messages).post(chats::send))
        .route("/api/chats/{id}/stop", post(chats::stop))
        .route("/api/chats/{id}/agents/stop", post(chats::stop_agent))
        .route("/api/chats/{id}/background", get(chats::background))
        .route("/api/chats/{id}/background/{task_id}/output", get(chats::background_output))
        .route("/api/chats/{id}/background/{task_id}/stop", post(chats::background_stop))
        .route("/api/chats/{id}/merge", post(chats::merge))
        .route("/api/chats/{id}/context", get(chats::context))
        .route("/api/chats/{id}/changes", get(chats::changes))
        .route("/api/chats/{id}/changes/diff", get(chats::change_diff))
        .route("/api/runs/{id}/cancel", post(chats::cancel_run))
        .route("/api/requests/{id}/answer", post(chats::answer))
        .route("/api/requests/pending", get(chats::pending))
        .route("/api/uploads", post(chats::upload))
        .route("/api/uploads/{id}", get(chats::download))
        .route("/api/queue", get(chats::queue))
        // schedules
        .route("/api/schedules", get(schedules::list).post(schedules::create))
        .route("/api/schedules/preview", post(schedules::preview))
        .route("/api/schedules/{id}", put(schedules::update).delete(schedules::remove))
        .route("/api/schedules/{id}/run", post(schedules::run_now))
        // servers + deploys
        .route("/api/servers", get(servers::list).post(servers::create))
        .route("/api/servers/{id}", put(servers::update).delete(servers::remove))
        .route("/api/servers/{id}/test", post(servers::test))
        .route("/api/ssh/public-key", get(servers::public_key))
        .route("/api/projects/{id}/deploys", get(servers::deploys).post(servers::deploy))
        .route("/api/deploys/active", get(servers::active))
        .route("/api/deploys/{id}", get(servers::deploy_one))
        .route("/api/deploys/{id}/rollback", post(servers::rollback))
        // git
        .route("/api/projects/{id}/git/status", get(gitops::status))
        .route("/api/projects/{id}/git/diff", get(gitops::diff))
        .route("/api/projects/{id}/git/log", get(gitops::log))
        .route("/api/projects/{id}/git/branches", get(gitops::branches))
        .route("/api/projects/{id}/git/commit", post(gitops::commit))
        .route("/api/projects/{id}/git/push", post(gitops::push))
        .route("/api/projects/{id}/git/pull", post(gitops::pull))
        .route("/api/projects/{id}/git/checkout", post(gitops::checkout))
        .route("/api/projects/{id}/git/discard", post(gitops::discard))
        // files
        .route("/api/projects/{id}/files", get(files::list).delete(files::remove))
        .route("/api/projects/{id}/files/content", get(files::read).put(files::write))
        .route("/api/projects/{id}/files/mkdir", post(files::mkdir))
        .route("/api/projects/{id}/files/rename", post(files::rename))
        // system
        .route("/api/settings", get(system::get_settings).put(system::put_settings))
        .route("/api/system", get(system::status))
        .route("/api/usage/refresh", post(system::refresh_usage))
        .route("/api/usage/report", get(usage::report))
        .route("/api/events", get(system::events))
        .route("/api/models", get(system::models))
        .route("/api/models/refresh", post(system::refresh_models))
        .route("/api/telegram/test", post(system::telegram_test))
        .route("/api/ws", get(crate::ws::handler))
        .merge(crate::extensions::router())
        .merge(crate::terminal::router())
        .merge(crate::updater::router())
        .merge(crate::devserver::router())
        .merge(crate::browser::router())
        .merge(crate::deploy_assist::router())
        .route("/api/{*rest}", delete(not_found).get(not_found).post(not_found).put(not_found))
}

async fn not_found() -> crate::util::ApiError {
    crate::util::ApiError::not_found()
}
