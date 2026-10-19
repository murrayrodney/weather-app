//! Error type and envelope (conventions "HTTP API", 02 section 4.11). Messages are fixed static text and never echo input.

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use famweather_core::db::DbError;
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    Unauthorized,
    AccountInactive,
    Forbidden,
    NotFound,
    MethodNotAllowed,
    RateLimited { retry_after: u64 },
    Validation(&'static str),
    InvalidKey,
    UnknownDevice,
    NotACandidate,
    TimezoneRequired,
    DeviceAlreadyRegistered,
    Conflict,
    RequestTimeout,
    PayloadTooLarge,
    Overloaded,
    AuthUnavailable,
    UpstreamBusy { retry_after: u64 },
    UpstreamUnavailable,
    Internal,
}

impl ApiError {
    pub fn validation(msg: &'static str) -> Self {
        ApiError::Validation(msg)
    }

    pub fn status(&self) -> StatusCode {
        match self {
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::AccountInactive | ApiError::Forbidden => StatusCode::FORBIDDEN,
            ApiError::NotFound => StatusCode::NOT_FOUND,
            ApiError::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            ApiError::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            ApiError::Validation(_)
            | ApiError::InvalidKey
            | ApiError::UnknownDevice
            | ApiError::NotACandidate
            | ApiError::TimezoneRequired => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::DeviceAlreadyRegistered | ApiError::Conflict => StatusCode::CONFLICT,
            ApiError::RequestTimeout => StatusCode::REQUEST_TIMEOUT,
            ApiError::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            ApiError::Overloaded | ApiError::AuthUnavailable | ApiError::UpstreamBusy { .. } => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            ApiError::UpstreamUnavailable => StatusCode::BAD_GATEWAY,
            ApiError::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            ApiError::Unauthorized => "unauthorized",
            ApiError::AccountInactive => "account_inactive",
            ApiError::Forbidden => "forbidden",
            ApiError::NotFound => "not_found",
            ApiError::MethodNotAllowed => "method_not_allowed",
            ApiError::RateLimited { .. } => "rate_limited",
            ApiError::Validation(_) => "validation_failed",
            ApiError::InvalidKey => "invalid_key",
            ApiError::UnknownDevice => "unknown_device",
            ApiError::NotACandidate => "not_a_candidate",
            ApiError::TimezoneRequired => "timezone_required",
            ApiError::DeviceAlreadyRegistered => "device_already_registered",
            ApiError::Conflict => "conflict",
            ApiError::RequestTimeout => "request_timeout",
            ApiError::PayloadTooLarge => "payload_too_large",
            ApiError::Overloaded => "overloaded",
            ApiError::AuthUnavailable => "auth_unavailable",
            ApiError::UpstreamBusy { .. } => "upstream_busy",
            ApiError::UpstreamUnavailable => "upstream_unavailable",
            ApiError::Internal => "internal",
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            ApiError::Unauthorized => "Sign in again.",
            ApiError::AccountInactive => "This account is not allowed to use the app.",
            ApiError::Forbidden => "Not allowed.",
            ApiError::NotFound => "Not found.",
            ApiError::MethodNotAllowed => "That method is not allowed here.",
            ApiError::RateLimited { .. } => "Too many requests. Try again shortly.",
            ApiError::Validation(m) => m,
            ApiError::InvalidKey => "That Ambient Weather key was not accepted.",
            ApiError::UnknownDevice => {
                "One of the stations is not on that Ambient Weather account."
            }
            ApiError::NotACandidate => "That station is not one of the suggested baselines.",
            ApiError::TimezoneRequired => "A time zone is required for this station.",
            ApiError::DeviceAlreadyRegistered => "That station is already registered.",
            ApiError::Conflict => "The request conflicts with the current state. Try again.",
            ApiError::RequestTimeout => "The request took too long.",
            ApiError::PayloadTooLarge => "The request is too large.",
            ApiError::Overloaded => "The service is busy. Try again shortly.",
            ApiError::AuthUnavailable => "Sign-in checks are unavailable. Try again shortly.",
            ApiError::UpstreamBusy { .. } => "An upstream service is busy. Try again shortly.",
            ApiError::UpstreamUnavailable => "An upstream service is unavailable.",
            ApiError::Internal => "Something went wrong.",
        }
    }

    fn retry_after(&self) -> Option<u64> {
        match self {
            ApiError::RateLimited { retry_after } | ApiError::UpstreamBusy { retry_after } => {
                Some((*retry_after).max(1))
            }
            ApiError::Overloaded => Some(1),
            _ => None,
        }
    }
}

pub fn envelope(code: &str, message: &str) -> serde_json::Value {
    json!({ "error": { "code": code, "message": message } })
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut resp = (
            self.status(),
            axum::Json(envelope(self.code(), self.message())),
        )
            .into_response();
        if let Some(secs) = self.retry_after() {
            if let Ok(v) = HeaderValue::from_str(&secs.to_string()) {
                resp.headers_mut().insert(header::RETRY_AFTER, v);
            }
        }
        resp
    }
}

/// Database failures reduce to a safe cause (no URL, no driver text) in the log and a generic 500.
impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        let cause = DbError::from(e);
        tracing::error!(cause = %cause, "database error");
        ApiError::Internal
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

/// Replaces empty-bodied 408/413/503 responses produced by layers with the error envelope (02 section 4.2).
pub async fn envelope_fixup(req: Request, next: Next) -> Response {
    let resp = next.run(req).await;
    let json = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if json {
        return resp;
    }
    let err = match resp.status() {
        StatusCode::REQUEST_TIMEOUT => ApiError::RequestTimeout,
        StatusCode::PAYLOAD_TOO_LARGE => ApiError::PayloadTooLarge,
        // axum answers a known path with the wrong method with an empty 405 (and an `Allow` header, which is kept).
        StatusCode::METHOD_NOT_ALLOWED => ApiError::MethodNotAllowed,
        StatusCode::SERVICE_UNAVAILABLE => ApiError::Overloaded,
        _ => return resp,
    };
    // Keep headers a layer set (for example Retry-After); the body and content type are replaced.
    let (mut parts, _) = resp.into_parts();
    let fresh = err.into_response();
    let (fresh_parts, body) = fresh.into_parts();
    parts.headers.remove(header::CONTENT_LENGTH);
    for (k, v) in fresh_parts.headers.iter() {
        parts.headers.insert(k.clone(), v.clone());
    }
    Response::from_parts(parts, Body::new(body))
}

/// 404 fallback with the envelope.
pub async fn not_found_json() -> ApiError {
    ApiError::NotFound
}

/// `HandleErrorLayer` handler for `LoadShed`: saturation fails fast with 503 `overloaded`.
pub async fn overloaded(_err: tower::BoxError) -> ApiError {
    ApiError::Overloaded
}
