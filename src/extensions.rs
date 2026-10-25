use crate::state::St;
use axum::Router;

pub fn router() -> Router<St> {
    Router::new()
}
