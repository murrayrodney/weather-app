//! In-memory per-user limiters (02 section 4.9). One keyed `governor` limiter per quota, key = verified user id.
//! The API is a single machine (P3), so a restart giving one fresh burst is acceptable for these low-cost routes.

use std::num::NonZeroU32;
use std::time::Duration;

use governor::clock::{Clock, DefaultClock};
use governor::{Quota, RateLimiter};
use uuid::Uuid;

/// `Err(seconds)` carries the whole seconds (rounded up, at least 1) until the next request would pass.
pub trait KeyedLimit: Send + Sync {
    fn check(&self, key: Uuid) -> Result<(), u64>;
    /// Drops idle keys (every 10 minutes).
    fn evict(&self);
}

struct Governed<C: Clock + Send + Sync> {
    limiter: RateLimiter<
        Uuid,
        governor::state::keyed::DashMapStateStore<Uuid>,
        C,
        governor::middleware::NoOpMiddleware<C::Instant>,
    >,
    clock: C,
}

impl<C> KeyedLimit for Governed<C>
where
    C: Clock + Send + Sync + 'static,
    C::Instant: Send + Sync,
{
    fn check(&self, key: Uuid) -> Result<(), u64> {
        self.limiter.check_key(&key).map_err(|not_until| {
            let wait = not_until.wait_time_from(self.clock.now());
            (wait.as_secs_f64().ceil() as u64).max(1)
        })
    }

    fn evict(&self) {
        self.limiter.retain_recent();
        self.limiter.shrink_to_fit();
    }
}

fn quota(per_minute: u32, burst: u32) -> Quota {
    Quota::per_minute(NonZeroU32::new(per_minute).expect("non-zero"))
        .allow_burst(NonZeroU32::new(burst).expect("non-zero"))
}

fn build<C>(q: Quota, clock: C) -> Box<dyn KeyedLimit>
where
    C: Clock + Clone + Send + Sync + 'static,
    C::Instant: Send + Sync,
{
    Box::new(Governed {
        limiter: RateLimiter::dashmap_with_clock(q, clock.clone()),
        clock,
    })
}

pub struct UserLimiters {
    /// 120 per minute, burst 40: every `/api/v1` request, inside the guard.
    pub general: Box<dyn KeyedLimit>,
    /// 30 per minute, burst 10: `GET /users/search`.
    pub search: Box<dyn KeyedLimit>,
    /// 6 per minute, burst 3: baseline candidates and PUT.
    pub baseline: Box<dyn KeyedLimit>,
    /// 20 per minute, burst 10: `GET .../forecast` and `.../forecast/changes`. Every cache miss spends the process-wide
    /// Open-Meteo budget (60 calls a minute), so one user must not be able to drain it. (Not named in 02; chosen here.)
    pub forecast: Box<dyn KeyedLimit>,
}

impl UserLimiters {
    pub fn new() -> Self {
        Self::with_clock(DefaultClock::default())
    }

    pub fn with_clock<C>(clock: C) -> Self
    where
        C: Clock + Clone + Send + Sync + 'static,
        C::Instant: Send + Sync,
    {
        Self {
            general: build(quota(120, 40), clock.clone()),
            search: build(quota(30, 10), clock.clone()),
            baseline: build(quota(6, 3), clock.clone()),
            forecast: build(quota(20, 10), clock),
        }
    }

    /// Test-only: a different quota for the baseline limiter (tests that make many baseline calls).
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_baseline_quota<C>(clock: C, per_minute: u32, burst: u32) -> Self
    where
        C: Clock + Clone + Send + Sync + 'static,
        C::Instant: Send + Sync,
    {
        let mut l = Self::with_clock(clock.clone());
        l.baseline = build(quota(per_minute, burst), clock);
        l
    }

    /// Test-only: a different quota for the forecast limiter.
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_forecast_quota<C>(clock: C, per_minute: u32, burst: u32) -> Self
    where
        C: Clock + Clone + Send + Sync + 'static,
        C::Instant: Send + Sync,
    {
        let mut l = Self::with_clock(clock.clone());
        l.forecast = build(quota(per_minute, burst), clock);
        l
    }

    /// Test-only: a different quota for the search limiter.
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_search_quota<C>(clock: C, per_minute: u32, burst: u32) -> Self
    where
        C: Clock + Clone + Send + Sync + 'static,
        C::Instant: Send + Sync,
    {
        let mut l = Self::with_clock(clock.clone());
        l.search = build(quota(per_minute, burst), clock);
        l
    }

    pub fn evict_idle(&self) {
        self.general.evict();
        self.search.evict();
        self.baseline.evict();
        self.forecast.evict();
    }
}

impl Default for UserLimiters {
    fn default() -> Self {
        Self::new()
    }
}

/// Runs `evict_idle` every 10 minutes for the life of the process.
pub fn spawn_eviction(limiters: std::sync::Arc<UserLimiters>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(600));
        tick.tick().await;
        loop {
            tick.tick().await;
            limiters.evict_idle();
        }
    });
}
