//! Ambient Weather Network client (03 section 5.2): trait, one error enum, live and replay implementations.
//!
//! Neither client touches the database or reserves rate gates; callers reserve (`core::ratelimit::reserve`).

mod live;
mod models;
mod replay;

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

pub use live::LiveAwn;
pub use models::{parse_device, ApiKey, AwnDevice, AwnObservation, Mac, MacError};
pub use replay::{
    ReplayAwn, REPLAY_INVALID_KEY, REPLAY_POLLER_KEY, REPLAY_RATELIMITED_KEY, REPLAY_VALID_KEY,
};

/// Maximum rows per AWN history page.
pub const MAX_HISTORY_LIMIT: u16 = 288;

/// No URL, no source chain, no upstream body: `Display` and `Debug` are safe to log.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AwnError {
    /// HTTP 401 or 403.
    #[error("AWN rejected the API key")]
    Unauthorized,
    /// HTTP 429.
    #[error("AWN rate limit reached")]
    RateLimited { retry_after: Option<Duration> },
    /// 5xx, timeout or connection error.
    #[error("AWN unavailable")]
    Unavailable,
    /// Undecodable or oversize body, or an unexpected status.
    #[error("AWN returned an unusable response")]
    BadResponse,
}

#[async_trait]
pub trait AwnApi: Send + Sync {
    async fn list_devices(&self, key: &ApiKey) -> Result<Vec<AwnDevice>, AwnError>;

    /// Observations of `mac`, newest first, at most `limit` (<= 288) rows, none newer than `end_date`.
    async fn history(
        &self,
        key: &ApiKey,
        mac: &Mac,
        end_date: Option<DateTime<Utc>>,
        limit: u16,
    ) -> Result<Vec<AwnObservation>, AwnError>;
}
