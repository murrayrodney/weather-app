//! `ApiConfig`: `core::config::load(Binary::Api)` plus ONLY the API-specific checks of 02 section 4.1.

use std::time::Duration;

use famweather_core::config::{self, ApiEnv, AppEnv, Binary, Config, ConfigError};
use jsonwebtoken::Algorithm;
use secrecy::SecretString;
use url::Url;

/// Where access-token keys come from.
#[derive(Clone)]
pub enum KeySource {
    /// Local only: HS256 shared secret.
    Hs256(SecretString),
    /// ES256 / RS256 keys from the JWKS document.
    Jwks(Url),
}

pub struct ApiConfig {
    pub env: ApiEnv,
    /// `${SUPABASE_URL}/auth/v1`, trailing slash trimmed.
    pub issuer: String,
    pub allowed_algs: Vec<Algorithm>,
    pub key_source: KeySource,
    /// Overall `/api/v1` request timeout (25 s; tests shorten it).
    pub request_timeout: Duration,
    /// Global in-flight request cap (64).
    pub max_in_flight: usize,
}

impl std::fmt::Debug for ApiConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiConfig")
            .field("env", &self.env)
            .field("issuer", &"[redacted]")
            .field("allowed_algs", &self.allowed_algs)
            .finish_non_exhaustive()
    }
}

fn refused(var: &str, reason: &'static str) -> ConfigError {
    ConfigError::Refused {
        var: var.to_owned(),
        reason,
    }
}

fn invalid(var: &str, reason: &'static str) -> ConfigError {
    ConfigError::Invalid {
        var: var.to_owned(),
        reason,
    }
}

fn local_host(host: &str) -> bool {
    let h = host
        .trim_matches(|c| c == '[' || c == ']')
        .to_ascii_lowercase();
    h == "localhost"
        || h.starts_with("127.")
        || h == "::1"
        || h.ends_with(".localhost")
        || h.ends_with(".test")
        || !h.contains('.')
}

fn check_production_url(var: &str, url: &Url) -> Result<(), ConfigError> {
    if url.scheme() != "https" {
        return Err(refused(var, "must be https in production"));
    }
    match url.host_str() {
        Some(h) if !local_host(h) => Ok(()),
        _ => Err(refused(var, "must be a public host in production")),
    }
}

pub const LOCAL_HS256_SUPABASE_URL: &str = "http://localhost:8000";
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(25);
pub const MAX_IN_FLIGHT: usize = 64;

impl ApiConfig {
    /// Loads and validates the configuration from the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        match config::load(Binary::Api)? {
            Config::Api(env) => Self::from_api_env(env),
            _ => Err(invalid("APP_ENV", "unexpected configuration kind")),
        }
    }

    /// The three API-specific checks (02 4.1) on an environment `core::config` already validated.
    pub fn from_api_env(env: ApiEnv) -> Result<Self, ConfigError> {
        let issuer = format!(
            "{}/auth/v1",
            env.supabase_url.as_str().trim_end_matches('/')
        );
        let key_source = match env.app_env {
            AppEnv::Production => {
                check_production_url("SUPABASE_URL", &env.supabase_url)?;
                let jwks = env
                    .supabase_jwks_url
                    .clone()
                    .ok_or_else(|| ConfigError::MissingEnv("SUPABASE_JWKS_URL".to_owned()))?;
                check_production_url("SUPABASE_JWKS_URL", &jwks)?;
                if env.jwt_hs256_secret.is_some() {
                    return Err(refused("JWT_HS256_SECRET", "not allowed in production"));
                }
                KeySource::Jwks(jwks)
            }
            AppEnv::Local => match (&env.jwt_hs256_secret, &env.supabase_jwks_url) {
                (Some(_), Some(_)) => {
                    return Err(invalid(
                        "SUPABASE_JWKS_URL",
                        "set exactly one of JWT_HS256_SECRET and SUPABASE_JWKS_URL",
                    ))
                }
                (None, None) => {
                    return Err(ConfigError::MissingEnv(
                        "JWT_HS256_SECRET or SUPABASE_JWKS_URL".to_owned(),
                    ))
                }
                (Some(secret), None) => {
                    use secrecy::ExposeSecret;
                    if secret.expose_secret().len() < 32 {
                        return Err(invalid("JWT_HS256_SECRET", "must be at least 32 bytes"));
                    }
                    if env.supabase_url.as_str().trim_end_matches('/') != LOCAL_HS256_SUPABASE_URL {
                        return Err(refused(
                            "SUPABASE_URL",
                            "must be http://localhost:8000 when JWT_HS256_SECRET is used",
                        ));
                    }
                    KeySource::Hs256(secret.clone())
                }
                (None, Some(jwks)) => KeySource::Jwks(jwks.clone()),
            },
        };
        let allowed_algs = match &key_source {
            KeySource::Hs256(_) => vec![Algorithm::HS256],
            KeySource::Jwks(_) => vec![Algorithm::ES256, Algorithm::RS256],
        };
        Ok(Self {
            env,
            issuer,
            allowed_algs,
            key_source,
            request_timeout: REQUEST_TIMEOUT,
            max_in_flight: MAX_IN_FLIGHT,
        })
    }

    pub fn is_production(&self) -> bool {
        self.env.app_env == AppEnv::Production
    }
}

#[cfg(any(test, feature = "test-support"))]
impl ApiConfig {
    /// Test configuration: no environment is read. `key_source` decides HS256 or JWKS; the issuer is given explicitly.
    pub fn for_test(issuer: &str, key_source: KeySource) -> Self {
        use famweather_core::config::AwnMode;
        let allowed_algs = match &key_source {
            KeySource::Hs256(_) => vec![Algorithm::HS256],
            KeySource::Jwks(_) => vec![Algorithm::ES256, Algorithm::RS256],
        };
        let (pk, _) = famweather_core::crypto::generate_keypair();
        let env = ApiEnv {
            app_env: AppEnv::Local,
            database: sqlx::postgres::PgConnectOptions::new(),
            supabase_url: Url::parse("http://localhost:8000").expect("static url"),
            supabase_jwks_url: None,
            jwt_hs256_secret: None,
            awn_mode: AwnMode::Replay,
            awn_application_key: None,
            fixtures_dir: std::path::PathBuf::from("/nonexistent-fixtures"),
            credentials_public_key: pk,
            credentials_key_version: 1,
            bind_addr: "127.0.0.1:0".parse().expect("static address"),
            netlify_proxy_secret: None,
        };
        Self {
            env,
            issuer: issuer.to_owned(),
            allowed_algs,
            key_source,
            request_timeout: REQUEST_TIMEOUT,
            max_in_flight: MAX_IN_FLIGHT,
        }
    }
}
