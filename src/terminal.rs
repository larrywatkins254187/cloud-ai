use crate::state::St;
use axum::Router;

#[derive(Default)]
pub struct TerminalState {}

pub fn router() -> Router<St> {
    Router::new()
}

pub fn stop_all(_st: &St) {}
