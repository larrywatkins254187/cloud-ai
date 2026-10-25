use crate::state::St;
use axum::Router;

#[derive(Default)]
pub struct UpdaterState {}

pub fn router() -> Router<St> {
    Router::new()
}

pub fn spawn(_st: St) {}
