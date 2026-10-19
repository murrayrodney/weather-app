use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, FollowerId, NoQuery};
use crate::repo::follows::{self, Decision};
use crate::state::AppState;

pub async fn list(
    State(st): State<AppState>,
    user: AuthUser,
    _: NoQuery,
) -> ApiResult<Json<follows::FollowLists>> {
    Ok(Json(follows::list(&st.db, user.id).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestBody {
    followee_id: String,
}

pub async fn request(
    State(st): State<AppState>,
    user: AuthUser,
    ApiJson(body): ApiJson<RequestBody>,
) -> ApiResult<(StatusCode, Json<follows::RequestOut>)> {
    // A malformed id is the same 404 as an unknown or inactive user.
    let target = Uuid::parse_str(&body.followee_id).map_err(|_| ApiError::NotFound)?;
    let (created, out) = follows::request(&st.db, user.id, target).await?;
    let code = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((code, Json(out)))
}

async fn decide(
    st: AppState,
    user: AuthUser,
    follower: Uuid,
    d: Decision,
) -> ApiResult<Json<follows::DecisionOut>> {
    Ok(Json(follows::decide(&st.db, user.id, follower, d).await?))
}

pub async fn approve(
    State(st): State<AppState>,
    user: AuthUser,
    FollowerId(f): FollowerId,
) -> ApiResult<Json<follows::DecisionOut>> {
    decide(st, user, f, Decision::Approve).await
}

pub async fn decline(
    State(st): State<AppState>,
    user: AuthUser,
    FollowerId(f): FollowerId,
) -> ApiResult<Json<follows::DecisionOut>> {
    decide(st, user, f, Decision::Decline).await
}

pub async fn revoke(
    State(st): State<AppState>,
    user: AuthUser,
    FollowerId(f): FollowerId,
) -> ApiResult<Json<follows::DecisionOut>> {
    decide(st, user, f, Decision::Revoke).await
}
