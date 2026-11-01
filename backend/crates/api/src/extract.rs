//! Extractors that are not station-scoped: strict JSON, strict query allow-lists, the follower id.

use std::collections::HashMap;

use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, FromRequestParts, Path, Request};
use axum::http::request::Parts;
use axum::http::StatusCode;
use serde::de::DeserializeOwned;
use uuid::Uuid;

use crate::error::ApiError;

/// JSON body with the fixed rejection mapping of 02 section 4.8: a length-limit failure is 413, anything else 422.
pub struct ApiJson<T>(pub T);

impl<S, T> FromRequest<S> for ApiJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(v)) => Ok(ApiJson(v)),
            Err(rej) => Err(map_json_rejection(&rej)),
        }
    }
}

fn map_json_rejection(rej: &JsonRejection) -> ApiError {
    if rej.status() == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError::PayloadTooLarge
    } else {
        ApiError::Validation("The request body is not valid.")
    }
}

/// Decoded query parameters restricted to an allow-list. Any other key, or a repeated key, is 422.
#[derive(Debug, Default)]
pub struct Params(HashMap<String, String>);

impl Params {
    pub fn parse(raw: Option<&str>, allowed: &[&str]) -> Result<Self, ApiError> {
        let mut map = HashMap::new();
        for (k, v) in url::form_urlencoded::parse(raw.unwrap_or("").as_bytes()) {
            if !allowed.contains(&k.as_ref()) {
                return Err(ApiError::Validation("Unknown query parameter."));
            }
            if map.insert(k.into_owned(), v.into_owned()).is_some() {
                return Err(ApiError::Validation("Repeated query parameter."));
            }
        }
        Ok(Params(map))
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }
}

/// Routes with no query parameters: any key is 422.
pub struct NoQuery;

impl<S: Send + Sync> FromRequestParts<S> for NoQuery {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Params::parse(parts.uri.query(), &[])?;
        Ok(NoQuery)
    }
}

/// The `{follower_id}` path segment; a malformed UUID is 404.
pub struct FollowerId(pub Uuid);

impl<S: Send + Sync> FromRequestParts<S> for FollowerId {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Path(raw) = Path::<String>::from_request_parts(parts, state)
            .await
            .map_err(|_| ApiError::NotFound)?;
        Uuid::parse_str(&raw)
            .map(FollowerId)
            .map_err(|_| ApiError::NotFound)
    }
}
