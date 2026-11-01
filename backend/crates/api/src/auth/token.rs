//! Strict access-token verification (02 section 4.3, S1, D2). Nothing but the user and session ids escapes.

use std::sync::Arc;

use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use uuid::Uuid;

use super::jwks::JwksCache;

pub const MAX_TOKEN_BYTES: usize = 8192;
pub const AUDIENCE: &str = "authenticated";
pub const LEEWAY_SECS: u64 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifiedClaims {
    pub user_id: Uuid,
    pub session_id: Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    /// 401 `unauthorized`. The reason is for a debug log only and never contains the token.
    Invalid(&'static str),
    /// 503 `auth_unavailable`: the key set cannot be used right now.
    Unavailable,
}

pub enum KeyMode {
    Hs256(DecodingKey),
    Jwks(Arc<JwksCache>),
}

/// Holds key material, so its `Debug` is written by hand (grep test, 7.4).
pub struct TokenVerifier {
    issuer: String,
    algs: Vec<Algorithm>,
    mode: KeyMode,
}

impl std::fmt::Debug for TokenVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TokenVerifier([redacted])")
    }
}

#[derive(Deserialize)]
struct Claims {
    sub: String,
    role: Option<String>,
    session_id: Option<String>,
    is_anonymous: Option<bool>,
}

impl TokenVerifier {
    pub fn hs256(issuer: String, secret: &SecretString) -> Self {
        Self {
            issuer,
            algs: vec![Algorithm::HS256],
            mode: KeyMode::Hs256(DecodingKey::from_secret(secret.expose_secret().as_bytes())),
        }
    }

    pub fn jwks(issuer: String, cache: Arc<JwksCache>) -> Self {
        Self {
            issuer,
            algs: vec![Algorithm::ES256, Algorithm::RS256],
            mode: KeyMode::Jwks(cache),
        }
    }

    /// `header` is the full `Authorization` header value.
    pub async fn verify(&self, header: &str) -> Result<VerifiedClaims, AuthError> {
        use AuthError::Invalid;
        // 1. `Bearer <token>`, scheme case-insensitive, token <= 8 KiB, exactly two dots.
        let (scheme, token) = header
            .split_once(' ')
            .ok_or(Invalid("not a bearer header"))?;
        if !scheme.eq_ignore_ascii_case("bearer") {
            return Err(Invalid("not a bearer header"));
        }
        let token = token.trim();
        if token.is_empty() || token.len() > MAX_TOKEN_BYTES {
            return Err(Invalid("token size"));
        }
        if token.bytes().filter(|b| *b == b'.').count() != 2 {
            return Err(Invalid("token shape"));
        }
        // 2. Header. jku, jwk, x5u, x5c and crit are parsed by the library but never used.
        let header = decode_header(token).map_err(|_| Invalid("header"))?;
        // 3. Algorithm allow-list, before any key lookup.
        let alg = header.alg;
        if !self.algs.contains(&alg) {
            return Err(Invalid("algorithm not allowed"));
        }
        // 4. Key selection.
        let key = match &self.mode {
            KeyMode::Hs256(k) => k.clone(),
            KeyMode::Jwks(cache) => {
                let kid = header.kid.as_deref().ok_or(Invalid("missing kid"))?;
                let jwk = cache
                    .get(kid)
                    .await
                    .map_err(|_| AuthError::Unavailable)?
                    .ok_or(Invalid("unknown kid"))?;
                if jwk.kty_alg != alg {
                    return Err(Invalid("key type does not match algorithm"));
                }
                if jwk.jwk_alg.is_some_and(|a| a != alg) {
                    return Err(Invalid("jwk alg mismatch"));
                }
                if !jwk.use_sig {
                    return Err(Invalid("jwk use is not sig"));
                }
                jwk.key
            }
        };
        // 5. Claims.
        let mut v = Validation::new(alg);
        v.algorithms = vec![alg];
        v.set_issuer(&[self.issuer.as_str()]);
        v.set_audience(&[AUDIENCE]);
        v.validate_exp = true;
        v.validate_nbf = true;
        v.leeway = LEEWAY_SECS;
        v.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        let data = decode::<Claims>(token, &key, &v).map_err(|_| Invalid("signature or claims"))?;
        let c = data.claims;
        // 6. Post-checks.
        if c.role.as_deref() != Some("authenticated") {
            return Err(Invalid("role"));
        }
        let user_id = Uuid::parse_str(&c.sub).map_err(|_| Invalid("sub"))?;
        let session_id = c
            .session_id
            .as_deref()
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or(Invalid("session_id"))?;
        if c.is_anonymous == Some(true) {
            return Err(Invalid("anonymous user"));
        }
        Ok(VerifiedClaims {
            user_id,
            session_id,
        })
    }
}
