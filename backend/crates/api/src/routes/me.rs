use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::extract::NoQuery;
use crate::state::AppState;

pub async fn me(State(st): State<AppState>, user: AuthUser, _: NoQuery) -> ApiResult<Json<Value>> {
    let status: Option<String> = sqlx::query_scalar(
        "SELECT status FROM app.credentials WHERE owner_id = $1 AND status <> 'replaced' ORDER BY created_at DESC LIMIT 1",
    )
    .bind(user.id)
    .fetch_optional(&st.db)
    .await?;
    Ok(Json(json!({
        "id": user.id,
        "display_name": user.display_name,
        "is_admin": user.is_admin,
        "credential": status.map(|s| json!({ "status": s })),
    })))
}
