use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::Json;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::error::{ApiError, ApiResult};
use crate::extract::Params;
use crate::repo::follows;
use crate::state::AppState;

/// The `search` limiter, checked before any database work (it sits before the handler body).
pub struct SearchLimit;

impl FromRequestParts<AppState> for SearchLimit {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, st).await?;
        st.limiters
            .search
            .check(user.id)
            .map_err(|retry_after| ApiError::RateLimited { retry_after })?;
        Ok(SearchLimit)
    }
}

pub async fn search(
    State(st): State<AppState>,
    user: AuthUser,
    _: SearchLimit,
    req: Request,
) -> ApiResult<Json<Value>> {
    let params = Params::parse(req.uri().query(), &["q"])?;
    let q = params.get("q").unwrap_or("").trim();
    let len = q.chars().count();
    if len < 2 {
        return Err(ApiError::Validation("Type at least 2 characters."));
    }
    if len > 40 || q.chars().any(char::is_control) {
        return Err(ApiError::Validation("That search is not valid."));
    }
    let users = follows::search(&st.db, user.id, q).await?;
    Ok(Json(json!({ "users": users })))
}
