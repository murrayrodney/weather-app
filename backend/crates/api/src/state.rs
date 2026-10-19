//! Shared application state (02 section 4.2).

use std::sync::Arc;
use std::time::Duration;

use famweather_core::acis::AcisApi;
use famweather_core::awn::AwnApi;
use famweather_core::clock::Clock;
use famweather_core::crypto::CredentialPublicKey;
use sqlx::PgPool;

use crate::auth::token::TokenVerifier;
use crate::baseline::BaselineCache;
use crate::config::ApiConfig;
use crate::limits::UserLimiters;
use crate::openmeteo::OpenMeteo;

/// Rate-gate spacings and waits (conventions "AWN keys and rate limits"). Production values by default;
/// tests may shorten the spacings.
#[derive(Debug, Clone, Copy)]
pub struct GateConfig {
    pub awn_app_spacing: Duration,
    pub awn_key_spacing: Duration,
    pub acis_spacing: Duration,
    pub max_wait: Duration,
}

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            awn_app_spacing: Duration::from_millis(1000),
            awn_key_spacing: Duration::from_millis(1000),
            acis_spacing: Duration::from_millis(1000),
            max_wait: Duration::from_secs(3),
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub cfg: Arc<ApiConfig>,
    pub jwt: Arc<TokenVerifier>,
    pub awn: Arc<dyn AwnApi>,
    pub limiters: Arc<UserLimiters>,
    pub acis: Arc<dyn AcisApi>,
    pub baseline_cache: Arc<BaselineCache>,
    pub cred_pubkey: CredentialPublicKey,
    pub clock: Arc<dyn Clock>,
    pub openmeteo: Arc<OpenMeteo>,
    pub gates: GateConfig,
}
