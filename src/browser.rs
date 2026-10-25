use crate::state::St;
use axum::Router;

#[derive(Default)]
pub struct BrowserState {}

pub fn router() -> Router<St> {
    Router::new()
}

pub async fn stop(_st: &St) {}
