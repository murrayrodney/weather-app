//! JWKS cache (02 section 4.3.1): TTL 600 s with lazy refresh, stale keys up to 3600 s, one forced refetch per 60 s
//! for an unknown `kid`, outages are `Unavailable` (503), never `Invalid` (401).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use famweather_core::clock::Clock;
use jsonwebtoken::{Algorithm, DecodingKey};
use serde_json::Value;
use tokio::sync::Mutex;

pub const TTL: Duration = Duration::seconds(600);
pub const STALE_CAP: Duration = Duration::seconds(3600);
pub const UNKNOWN_KID_COOLDOWN: Duration = Duration::seconds(60);
/// After a failed lazy (TTL) refresh, wait this long before trying again.
pub const LAZY_RETRY: Duration = Duration::seconds(10);
pub const MAX_BODY_BYTES: usize = 64 * 1024;
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// A fetch failed (network, status, oversize body). Carries no URL or cause text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("jwks fetch failed")]
pub struct FetchError;

#[async_trait]
pub trait JwksFetcher: Send + Sync {
    /// The raw JWKS document.
    async fn fetch(&self) -> Result<Vec<u8>, FetchError>;
}

/// Production fetcher: https only (outside tests), 3 s timeout, no redirects, 64 KiB cap, no URL in errors.
pub struct HttpJwksFetcher {
    client: reqwest::Client,
    url: url::Url,
}

impl HttpJwksFetcher {
    pub fn new(url: url::Url, https_only: bool) -> Result<Self, FetchError> {
        let client = reqwest::Client::builder()
            .timeout(FETCH_TIMEOUT)
            .connect_timeout(FETCH_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .https_only(https_only)
            .user_agent(concat!("family-weather/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| FetchError)?;
        Ok(Self { client, url })
    }
}

#[async_trait]
impl JwksFetcher for HttpJwksFetcher {
    async fn fetch(&self) -> Result<Vec<u8>, FetchError> {
        let mut resp = self
            .client
            .get(self.url.clone())
            .send()
            .await
            .map_err(|e| {
                let kind = famweather_core::redact::http_error(e);
                tracing::warn!(error = %kind, "jwks fetch failed");
                FetchError
            })?;
        if !resp.status().is_success() {
            tracing::warn!(status = resp.status().as_u16(), "jwks fetch failed");
            return Err(FetchError);
        }
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| {
            let kind = famweather_core::redact::http_error(e);
            tracing::warn!(error = %kind, "jwks fetch failed");
            FetchError
        })? {
            if body.len() + chunk.len() > MAX_BODY_BYTES {
                tracing::warn!("jwks response too large");
                return Err(FetchError);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

/// One usable public key.
#[derive(Clone)]
pub struct JwksKey {
    /// The algorithm the key type implies: EC P-256 -> ES256, RSA -> RS256.
    pub kty_alg: Algorithm,
    /// The JWK's own `alg`, when present.
    pub jwk_alg: Option<Algorithm>,
    /// `use` is absent or `sig`.
    pub use_sig: bool,
    pub key: DecodingKey,
}

fn parse_alg(s: &str) -> Option<Algorithm> {
    match s {
        "ES256" => Some(Algorithm::ES256),
        "RS256" => Some(Algorithm::RS256),
        _ => None,
    }
}

/// Parses a JWKS document into usable keys. Only `kty` EC (crv P-256) and RSA are accepted; the rest is skipped.
pub fn parse_jwks(raw: &[u8]) -> Result<HashMap<String, JwksKey>, FetchError> {
    let v: Value = serde_json::from_slice(raw).map_err(|_| FetchError)?;
    let keys = v.get("keys").and_then(Value::as_array).ok_or(FetchError)?;
    let mut out = HashMap::new();
    for k in keys {
        let s = |name: &str| k.get(name).and_then(Value::as_str);
        let Some(kid) = s("kid") else { continue };
        let jwk_alg = match s("alg") {
            None => None,
            Some(a) => match parse_alg(a) {
                Some(a) => Some(a),
                None => continue,
            },
        };
        let use_sig = s("use").is_none_or(|u| u == "sig");
        let parsed = match s("kty") {
            Some("EC") if s("crv") == Some("P-256") => match (s("x"), s("y")) {
                (Some(x), Some(y)) => DecodingKey::from_ec_components(x, y)
                    .ok()
                    .map(|key| (Algorithm::ES256, key)),
                _ => None,
            },
            Some("RSA") => match (s("n"), s("e")) {
                (Some(n), Some(e)) => DecodingKey::from_rsa_components(n, e)
                    .ok()
                    .map(|key| (Algorithm::RS256, key)),
                _ => None,
            },
            _ => None,
        };
        if let Some((kty_alg, key)) = parsed {
            out.insert(
                kid.to_owned(),
                JwksKey {
                    kty_alg,
                    jwk_alg,
                    use_sig,
                    key,
                },
            );
        }
    }
    Ok(out)
}

#[derive(Default)]
struct Inner {
    keys: Arc<HashMap<String, JwksKey>>,
    fetched_at: Option<DateTime<Utc>>,
}

#[derive(Default)]
struct RefreshState {
    last_forced_at: Option<DateTime<Utc>>,
    last_forced_ok: bool,
    last_lazy_failed_at: Option<DateTime<Utc>>,
}

/// The JWKS cannot be used right now (never fetched, or older than the stale cap and the refetch failed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unavailable;

pub struct JwksCache {
    fetcher: Arc<dyn JwksFetcher>,
    clock: Arc<dyn Clock>,
    inner: RwLock<Inner>,
    refresh: Mutex<RefreshState>,
}

impl JwksCache {
    pub fn new(fetcher: Arc<dyn JwksFetcher>, clock: Arc<dyn Clock>) -> Self {
        Self {
            fetcher,
            clock,
            inner: RwLock::new(Inner::default()),
            refresh: Mutex::new(RefreshState::default()),
        }
    }

    /// Startup fetch: a failure or zero usable keys is an error (the caller exits the process).
    pub async fn load_initial(&self) -> Result<(), FetchError> {
        self.fetch_and_store().await
    }

    async fn fetch_and_store(&self) -> Result<(), FetchError> {
        let raw = self.fetcher.fetch().await?;
        let keys = parse_jwks(&raw)?;
        if keys.is_empty() {
            tracing::warn!("jwks document has no usable keys");
            return Err(FetchError);
        }
        let mut g = self.inner.write().unwrap_or_else(|p| p.into_inner());
        g.keys = Arc::new(keys);
        g.fetched_at = Some(self.clock.now());
        Ok(())
    }

    fn age(&self) -> Option<Duration> {
        let g = self.inner.read().unwrap_or_else(|p| p.into_inner());
        g.fetched_at.map(|t| self.clock.now() - t)
    }

    fn lookup(&self, kid: &str) -> Option<JwksKey> {
        let g = self.inner.read().unwrap_or_else(|p| p.into_inner());
        g.keys.get(kid).cloned()
    }

    async fn ensure_usable(&self) -> Result<(), Unavailable> {
        let Some(age) = self.age() else {
            return Err(Unavailable);
        };
        if age <= TTL {
            return Ok(());
        }
        // Lazy refresh: the first caller refreshes, the others use the stale keys.
        match self.refresh.try_lock() {
            Ok(mut st) => {
                if self.age().is_some_and(|a| a > TTL) {
                    let retry_ok = st
                        .last_lazy_failed_at
                        .is_none_or(|t| self.clock.now() - t >= LAZY_RETRY);
                    if retry_ok {
                        match self.fetch_and_store().await {
                            Ok(()) => st.last_lazy_failed_at = None,
                            Err(_) => st.last_lazy_failed_at = Some(self.clock.now()),
                        }
                    }
                }
            }
            Err(_) if age > STALE_CAP => {
                // Someone else is refreshing and our keys are too old to use: wait for that fetch.
                let _g = self.refresh.lock().await;
            }
            Err(_) => {}
        }
        match self.age() {
            Some(a) if a <= STALE_CAP => Ok(()),
            _ => Err(Unavailable),
        }
    }

    /// The key for `kid`. `Ok(None)` means the key is not in the set after (at most) one refetch.
    pub async fn get(&self, kid: &str) -> Result<Option<JwksKey>, Unavailable> {
        self.ensure_usable().await?;
        if let Some(k) = self.lookup(kid) {
            return Ok(Some(k));
        }
        let mut st = self.refresh.lock().await;
        if let Some(k) = self.lookup(kid) {
            return Ok(Some(k));
        }
        let now = self.clock.now();
        if let Some(t) = st.last_forced_at {
            if now - t < UNKNOWN_KID_COOLDOWN {
                return if st.last_forced_ok {
                    Ok(None)
                } else {
                    Err(Unavailable)
                };
            }
        }
        st.last_forced_at = Some(now);
        match self.fetch_and_store().await {
            Ok(()) => {
                st.last_forced_ok = true;
                Ok(self.lookup(kid))
            }
            Err(_) => {
                st.last_forced_ok = false;
                Err(Unavailable)
            }
        }
    }
}
