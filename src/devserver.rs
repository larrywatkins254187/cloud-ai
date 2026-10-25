use crate::state::St;
use axum::Router;

#[derive(Default)]
pub struct DevServers {}

pub fn router() -> Router<St> {
    Router::new()
}

pub async fn stop(_st: &St, _project_id: i64) {}

pub async fn stop_all(_st: &St) {}
