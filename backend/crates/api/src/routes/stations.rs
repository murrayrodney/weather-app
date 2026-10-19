use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::access::ViewableStationPath;
use crate::auth::AuthUser;
use crate::error::{ApiError, ApiResult};
use crate::extract::NoQuery;
use crate::repo::stations;
use crate::state::AppState;

pub async fn list(
    State(st): State<AppState>,
    user: AuthUser,
    _: NoQuery,
) -> ApiResult<Json<Value>> {
    let stations = stations::list_visible(&st.db, user.id).await?;
    Ok(Json(json!({ "stations": stations })))
}

pub async fn one(
    State(st): State<AppState>,
    user: AuthUser,
    ViewableStationPath(s): ViewableStationPath,
    _: NoQuery,
) -> ApiResult<Json<stations::StationOut>> {
    let mut v = stations::by_ids(&st.db, user.id, &[s.id()]).await?;
    v.pop().map(Json).ok_or(ApiError::NotFound)
}
