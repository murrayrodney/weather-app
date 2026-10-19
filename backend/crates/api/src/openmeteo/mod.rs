//! Open-Meteo access (02 5.6, 5.6a). Nothing is stored. The location is always a [`ViewableStation`]'s own rounded
//! coordinates. Hosts are constants; no API key is ever sent.

use std::future::Future;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use famweather_core::clock::Clock;
use tokio::sync::Semaphore;

use crate::access::ViewableStation;

pub mod cache;
pub mod changes;
mod client;
pub mod percentiles;
mod replay;

pub use cache::Cached;
pub use client::{parse_ensemble, parse_forecast, parse_previous_runs, LiveOpenMeteo};
pub use replay::ReplayOpenMeteo;

pub const FORECAST_HOST: &str = "https://api.open-meteo.com/v1/forecast";
pub const ENSEMBLE_HOST: &str = "https://ensemble-api.open-meteo.com/v1/ensemble";
pub const PREVIOUS_RUNS_HOST: &str = "https://previous-runs-api.open-meteo.com/v1/forecast";

pub const FORECAST_TTL: Duration = Duration::minutes(30);
pub const FORECAST_STALE_MAX: Duration = Duration::hours(6);
pub const CHANGES_TTL: Duration = Duration::hours(2);
pub const CHANGES_STALE_MAX: Duration = Duration::hours(24);
pub const MAX_CONCURRENT_CALLS: usize = 3;
pub const CALLS_PER_MINUTE: u32 = 60;
pub const ENSEMBLE_MEMBERS: usize = 51;

/// First day the Previous Runs archive has day-1 values, and the first day with probability (02 5.6a).
pub const PREVIOUS_RUNS_FIRST_DAY: (i32, u32, u32) = (2024, 1, 25);
pub const PROBABILITY_FIRST_DAY: (i32, u32, u32) = (2024, 10, 18);

/// What a failed upstream call may say: no URL, no body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OmError {
    #[error("open-meteo busy")]
    Busy { retry_after: u64 },
    #[error("open-meteo unavailable")]
    Unavailable,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DailyRaw {
    pub date: NaiveDate,
    pub tmax: Option<f64>,
    pub tmin: Option<f64>,
    pub precip: Option<f64>,
    pub prob_max: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct ForecastRaw {
    pub times: Vec<DateTime<Utc>>,
    pub temp: Vec<Option<f64>>,
    pub precip: Vec<Option<f64>>,
    pub prob: Vec<Option<f64>>,
    pub daily: Vec<DailyRaw>,
}

#[derive(Debug, Clone)]
pub struct EnsembleRaw {
    pub times: Vec<DateTime<Utc>>,
    /// One series per member (control first).
    pub temp: Vec<Vec<Option<f64>>>,
    pub precip: Vec<Vec<Option<f64>>>,
}

/// Index = lead in days (0 = latest run).
#[derive(Debug, Clone)]
pub struct PrevRunsRaw {
    pub times: Vec<DateTime<Utc>>,
    pub temp: Vec<Vec<Option<f64>>>,
    pub precip: Vec<Vec<Option<f64>>>,
    pub prob: Vec<Vec<Option<f64>>>,
}

#[async_trait]
pub trait OpenMeteoApi: Send + Sync {
    async fn forecast(&self, loc: &ViewableStation) -> Result<ForecastRaw, OmError>;
    async fn ensemble(&self, loc: &ViewableStation) -> Result<EnsembleRaw, OmError>;
    /// Hourly previous-runs data for the padded local-date window `from..=to`.
    async fn previous_runs(
        &self,
        loc: &ViewableStation,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<PrevRunsRaw, OmError>;
}

/// In-process token bucket (60 calls a minute). No waiting: an empty bucket is `Busy`.
struct Bucket {
    state: Mutex<(f64, DateTime<Utc>)>,
    per_minute: f64,
    clock: Arc<dyn Clock>,
}

impl Bucket {
    fn new(per_minute: u32, clock: Arc<dyn Clock>) -> Self {
        let now = clock.now();
        Self {
            state: Mutex::new((f64::from(per_minute), now)),
            per_minute: f64::from(per_minute),
            clock,
        }
    }

    fn take(&self) -> Result<(), u64> {
        let now = self.clock.now();
        let mut g = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let elapsed = (now - g.1).num_milliseconds().max(0) as f64 / 1000.0;
        g.0 = (g.0 + elapsed * self.per_minute / 60.0).min(self.per_minute);
        g.1 = now;
        if g.0 >= 1.0 {
            g.0 -= 1.0;
            Ok(())
        } else {
            Err((((1.0 - g.0) * 60.0 / self.per_minute).ceil() as u64).max(1))
        }
    }
}

type Key = (i64, i64);
type ChangesKey = (i64, i64, NaiveDate, NaiveDate);

fn key(loc: &ViewableStation) -> Key {
    (
        (loc.lat() * 100.0).round() as i64,
        (loc.lon() * 100.0).round() as i64,
    )
}

/// Cache, single flight, concurrency cap and call budget in front of an [`OpenMeteoApi`].
pub struct OpenMeteo {
    api: Arc<dyn OpenMeteoApi>,
    forecasts: cache::TtlCache<Key, ForecastRaw>,
    ensembles: cache::TtlCache<Key, EnsembleRaw>,
    changes: cache::TtlCache<ChangesKey, PrevRunsRaw>,
    permits: Semaphore,
    bucket: Bucket,
}

impl OpenMeteo {
    pub fn new(api: Arc<dyn OpenMeteoApi>, clock: Arc<dyn Clock>) -> Self {
        Self::with_limits(api, clock, MAX_CONCURRENT_CALLS, CALLS_PER_MINUTE)
    }

    pub fn with_limits(
        api: Arc<dyn OpenMeteoApi>,
        clock: Arc<dyn Clock>,
        concurrency: usize,
        per_minute: u32,
    ) -> Self {
        Self {
            api,
            forecasts: cache::TtlCache::new(FORECAST_TTL, FORECAST_STALE_MAX, 64, clock.clone()),
            ensembles: cache::TtlCache::new(FORECAST_TTL, FORECAST_STALE_MAX, 64, clock.clone()),
            changes: cache::TtlCache::new(CHANGES_TTL, CHANGES_STALE_MAX, 128, clock.clone()),
            permits: Semaphore::new(concurrency),
            bucket: Bucket::new(per_minute, clock),
        }
    }

    async fn upstream<T>(
        &self,
        fut: impl Future<Output = Result<T, OmError>>,
    ) -> Result<T, OmError> {
        self.bucket
            .take()
            .map_err(|retry_after| OmError::Busy { retry_after })?;
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| OmError::Unavailable)?;
        fut.await
    }

    pub async fn forecast(&self, loc: &ViewableStation) -> Result<Cached<ForecastRaw>, OmError> {
        self.forecasts
            .get(key(loc), || self.upstream(self.api.forecast(loc)))
            .await
    }

    pub async fn ensemble(&self, loc: &ViewableStation) -> Result<Cached<EnsembleRaw>, OmError> {
        self.ensembles
            .get(key(loc), || self.upstream(self.api.ensemble(loc)))
            .await
    }

    pub async fn previous_runs(
        &self,
        loc: &ViewableStation,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Cached<PrevRunsRaw>, OmError> {
        let (a, b) = key(loc);
        self.changes
            .get((a, b, from, to), || {
                self.upstream(self.api.previous_runs(
                    loc,
                    from - Duration::days(1),
                    to + Duration::days(1),
                ))
            })
            .await
    }
}
