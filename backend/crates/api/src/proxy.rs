//! Optional Netlify signed-proxy check (02 section 4.10). `deploy_context` is deliberately not checked.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;

use crate::error::ApiError;

#[derive(Clone)]
pub struct ProxySecret(pub SecretString);

#[derive(Deserialize)]
struct NfClaims {
    #[allow(dead_code)]
    iss: String,
}

pub async fn verify_signature(
    State(secret): State<ProxySecret>,
    req: Request,
    next: Next,
) -> Response {
    if req.uri().path() == "/api/health" && req.method() == axum::http::Method::GET {
        return next.run(req).await;
    }
    let ok = req
        .headers()
        .get("x-nf-sign")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|tok| {
            let mut v = Validation::new(Algorithm::HS256);
            v.set_issuer(&["netlify"]);
            v.validate_aud = false;
            v.leeway = 5;
            v.set_required_spec_claims(&["exp", "iss"]);
            decode::<NfClaims>(
                tok,
                &DecodingKey::from_secret(secret.0.expose_secret().as_bytes()),
                &v,
            )
            .is_ok()
        });
    if ok {
        next.run(req).await
    } else {
        ApiError::Forbidden.into_response()
    }
}
