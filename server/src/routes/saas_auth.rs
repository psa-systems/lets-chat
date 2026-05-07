use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Router;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/webhooks/maintenance", post(maintenance_webhook))
}

async fn maintenance_webhook() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}
