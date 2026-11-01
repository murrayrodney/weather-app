use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde_json::{json, Value};

use crate::state::AppState;

/// Liveness plus a bounded database ping (02 5.1). No version, host or configuration data.
pub async fn health(State(st): State<AppState>) -> (StatusCode, Json<Value>) {
    let ping = tokio::time::timeout(
        Duration::from_secs(2),
        sqlx::query("SELECT 1").execute(&st.db),
    )
    .await;
    match ping {
        Ok(Ok(_)) => (StatusCode::OK, Json(json!({ "status": "ok" }))),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "status": "unavailable" })),
        ),
    }
}
