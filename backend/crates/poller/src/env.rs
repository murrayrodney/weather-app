//! Everything a job needs from the outside world, bundled so tests can swap the clock, the AWN/ACIS clients, the
//! gate spacing and the backoff (constructor arguments under `test-support`, never environment variables).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use famweather_core::acis::AcisApi;
use famweather_core::awn::AwnApi;
use famweather_core::clock::Clock;
use famweather_core::config::AwnMode;
use famweather_core::db::ConnectHook;
use famweather_core::ratelimit::LockTimeouts;
use secrecy::SecretString;
use sqlx::postgres::PgConnectOptions;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::keys::KeyRing;

/// Gate spacing in `AWN_MODE=replay`. Replay never sends a request to AWN, so AWN's real limits do not apply there;
/// the gates still run (same statement, same sleeps) at this spacing so a local backfill takes seconds, not minutes.
pub const REPLAY_GATE_SPACING: Duration = Duration::from_millis(20);

/// Gate spacing and the longest a reservation may wait (03 section 5.3).
#[derive(Debug, Clone, Copy)]
pub struct GateConfig {
    pub app_spacing: Duration,
    pub key_spacing: Duration,
    pub max_wait: Duration,
}

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            app_spacing: Duration::from_millis(500),
            key_spacing: Duration::from_millis(1000),
            max_wait: Duration::from_secs(120),
        }
    }
}

impl GateConfig {
    /// The gates for `mode`: AWN's real spacing in live mode (not configurable), `REPLAY_GATE_SPACING` in replay.
    pub fn for_awn_mode(mode: AwnMode) -> Self {
        match mode {
            AwnMode::Live => Self::default(),
            AwnMode::Replay => Self {
                app_spacing: REPLAY_GATE_SPACING,
                key_spacing: REPLAY_GATE_SPACING,
                max_wait: Self::default().max_wait,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_uses_the_short_spacing_and_live_uses_the_real_one() {
        let replay = GateConfig::for_awn_mode(AwnMode::Replay);
        assert_eq!(replay.app_spacing, Duration::from_millis(20));
        assert_eq!(replay.key_spacing, Duration::from_millis(20));
        assert_eq!(replay.max_wait, Duration::from_secs(120));

        let live = GateConfig::for_awn_mode(AwnMode::Live);
        assert_eq!(live.app_spacing, Duration::from_millis(500));
        assert_eq!(live.key_spacing, Duration::from_millis(1000));
        assert_eq!(live.max_wait, Duration::from_secs(120));
    }
}

/// Waits between retries (03 section 5.3, items 2 and 3).
#[derive(Debug, Clone)]
pub struct Backoff {
    /// 429 without `Retry-After`: wait before attempt 2, 3, 4, 5 (five attempts in all). +-20% jitter.
    pub rate_limited: Vec<Duration>,
    /// 5xx / timeout / undecodable body: wait before each of the 3 retries.
    pub unavailable: Vec<Duration>,
    pub jitter: bool,
    /// A `Retry-After` above this is clamped.
    pub max_retry_after: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            rate_limited: [2, 4, 8, 16].map(Duration::from_secs).to_vec(),
            unavailable: [1, 3, 9].map(Duration::from_secs).to_vec(),
            jitter: true,
            max_retry_after: Duration::from_secs(120),
        }
    }
}

/// Post-backfill rollup wait: poll `try_job_lock` every `poll` for up to `total`.
#[derive(Debug, Clone, Copy)]
pub struct RollupWait {
    pub poll: Duration,
    pub total: Duration,
}

impl Default for RollupWait {
    fn default() -> Self {
        Self {
            poll: Duration::from_secs(5),
            total: Duration::from_secs(15 * 60),
        }
    }
}

#[derive(Clone)]
pub struct Env {
    pub pool: PgPool,
    /// Same role and TLS as the pool: used for the detached job-lock connections.
    pub db_opts: PgConnectOptions,
    /// Tests pass `SET ROLE app_poller` here.
    pub lock_hook: Option<ConnectHook>,
    pub lock_timeouts: LockTimeouts,
    pub clock: Arc<dyn Clock>,
    pub awn: Arc<dyn AwnApi>,
    pub acis: Arc<dyn AcisApi>,
    pub keys: Option<Arc<KeyRing>>,
    /// Healthcheck URLs by env suffix (`AWN_INGEST`, `DAILY_ROLLUP`, `REFERENCE_SYNC`); empty locally.
    pub pings: HashMap<String, SecretString>,
    // Timing is crate-private: production always has the constants of 03; only `cfg(test-support)` constructors
    // below can change it (03 section 3.2: no env override, no public knob).
    pub(crate) gates: GateConfig,
    pub(crate) backoff: Backoff,
    /// Pause between ACIS requests of `reference_sync`.
    pub(crate) acis_spacing: Duration,
    pub(crate) rollup_wait: RollupWait,
    pub cancel: CancellationToken,
    /// Ids of every `job_runs` row this process created (shutdown marks only these `cancelled`).
    pub run_ids: Arc<Mutex<Vec<Uuid>>>,
}

impl Env {
    /// Production defaults: real gate spacing, backoff and lock timeouts.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pool: PgPool,
        db_opts: PgConnectOptions,
        clock: Arc<dyn Clock>,
        awn: Arc<dyn AwnApi>,
        acis: Arc<dyn AcisApi>,
        keys: Option<Arc<KeyRing>>,
        pings: HashMap<String, SecretString>,
        cancel: CancellationToken,
    ) -> Env {
        Env {
            pool,
            db_opts,
            lock_hook: None,
            lock_timeouts: LockTimeouts::default(),
            clock,
            awn,
            acis,
            keys,
            pings,
            gates: GateConfig::default(),
            backoff: Backoff::default(),
            acis_spacing: Duration::from_secs(1),
            rollup_wait: RollupWait::default(),
            cancel,
            run_ids: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Test-only overrides.
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_fast_timing(mut self) -> Env {
        self.gates = GateConfig {
            app_spacing: Duration::from_millis(2),
            key_spacing: Duration::from_millis(2),
            max_wait: Duration::from_secs(30),
        };
        self.backoff = Backoff {
            rate_limited: vec![Duration::from_millis(5); 4],
            unavailable: vec![Duration::from_millis(5); 3],
            jitter: false,
            max_retry_after: Duration::from_millis(50),
        };
        self.acis_spacing = Duration::from_millis(1);
        self.rollup_wait = RollupWait {
            poll: Duration::from_millis(20),
            total: Duration::from_millis(600),
        };
        self
    }

    /// Sets the gate spacing for the configured `AWN_MODE` (see `GateConfig::for_awn_mode`). Production calls this once
    /// at startup; `Env::new` alone keeps the live spacing.
    pub fn with_awn_mode(mut self, mode: AwnMode) -> Env {
        self.gates = GateConfig::for_awn_mode(mode);
        self
    }

    /// Test-only: explicit gate spacing.
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_gates(mut self, gates: GateConfig) -> Env {
        self.gates = gates;
        self
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn with_lock_hook(mut self, hook: ConnectHook) -> Env {
        self.lock_hook = Some(hook);
        self
    }
}
