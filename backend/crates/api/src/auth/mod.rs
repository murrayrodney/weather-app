//! Authentication: token verification, per-user limiter, user and session check, all in ONE middleware so their
//! order cannot be inverted (02 section 4.4).

pub mod jwks;
pub mod token;

use axum::extract::{FromRequestParts, Request, State};
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use famweather_core::db::DbError;
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;
use token::AuthError;

/// The verified, active caller. Built only by [`guard`].
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: Uuid,
    pub display_name: String,
    pub is_admin: bool,
    pub session_id: Uuid,
}

impl<S: Send + Sync> FromRequestParts<S> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        // Defense in depth: a route that somehow skipped the guard has no user and is refused.
        parts
            .extensions
            .get::<AuthUser>()
            .cloned()
            .ok_or(ApiError::Unauthorized)
    }
}

/// Whether the token's session is still live (`app_auth.session_active`). Kept in one function so removing it, should
/// proposals/03 option (c) ever be needed, is a one-function change.
pub const USER_SESSION_SQL: &str = "SELECT u.display_name, u.is_admin, u.is_active, \
     app_auth.session_active($2, $1) AS session_ok FROM app.users u WHERE u.id = $1";

pub async fn load_user(
    db: &sqlx::PgPool,
    user_id: Uuid,
    session_id: Uuid,
) -> Result<AuthUser, ApiError> {
    use sqlx::Row;
    let row = sqlx::query(USER_SESSION_SQL)
        .bind(user_id)
        .bind(session_id)
        .fetch_optional(db)
        .await
        .map_err(|e| {
            tracing::error!(cause = %DbError::from(e), "user check failed");
            ApiError::AuthUnavailable
        })?;
    let Some(row) = row else {
        return Err(ApiError::AccountInactive);
    };
    let decode = |e: sqlx::Error| {
        tracing::error!(cause = %DbError::from(e), "user check failed");
        ApiError::AuthUnavailable
    };
    let is_active: bool = row.try_get("is_active").map_err(decode)?;
    if !is_active {
        return Err(ApiError::AccountInactive);
    }
    let session_ok: bool = row.try_get("session_ok").map_err(decode)?;
    if !session_ok {
        return Err(ApiError::Unauthorized);
    }
    Ok(AuthUser {
        id: user_id,
        display_name: row.try_get("display_name").map_err(decode)?,
        is_admin: row.try_get("is_admin").map_err(decode)?,
        session_id,
    })
}

/// Startup probe (4.4 step 6): the function must exist, be executable by `app_api` and return false for random ids.
pub async fn startup_probe(db: &sqlx::PgPool) -> Result<(), String> {
    let ok: bool =
        sqlx::query_scalar("SELECT app_auth.session_active(gen_random_uuid(), gen_random_uuid())")
            .fetch_one(db)
            .await
            .map_err(|e| format!("session check probe failed ({})", DbError::from(e)))?;
    if ok {
        return Err("session check probe returned true for random ids".to_owned());
    }
    Ok(())
}

pub async fn guard(State(st): State<AppState>, mut req: Request, next: Next) -> Response {
    // Only the header value crosses an await point: the request body is not `Sync`.
    let header = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    match authenticate(&st, header).await {
        Ok(user) => {
            tracing::Span::current().record("user_id", tracing::field::display(user.id));
            req.extensions_mut().insert(user);
            next.run(req).await
        }
        Err(e) => e.into_response(),
    }
}

async fn authenticate(st: &AppState, header: Option<String>) -> Result<AuthUser, ApiError> {
    let header = header.ok_or(ApiError::Unauthorized)?;
    // 1. Signature and claims. No database access.
    let claims = match st.jwt.verify(&header).await {
        Ok(c) => c,
        Err(AuthError::Invalid(reason)) => {
            tracing::debug!(reason, "token rejected");
            return Err(ApiError::Unauthorized);
        }
        Err(AuthError::Unavailable) => return Err(ApiError::AuthUnavailable),
    };
    // 2. Per-user limiter, BEFORE any database work.
    st.limiters
        .general
        .check(claims.user_id)
        .map_err(|retry_after| ApiError::RateLimited { retry_after })?;
    // 3. User and session, one query, never cached.
    load_user(&st.db, claims.user_id, claims.session_id).await
}
