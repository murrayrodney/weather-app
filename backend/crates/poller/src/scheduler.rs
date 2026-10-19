//! Scheduler, heartbeat file and graceful shutdown (03 sections 3.1, 4.1 and 4.5).

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;

use chrono::{DateTime, Duration as ChronoDuration, DurationRound, Timelike, Utc};
use tokio::task::JoinSet;
use tokio::time::{interval_at, Instant, MissedTickBehavior};

use crate::env::Env;
use crate::job::{JobFlags, RunOutcome};
use crate::jobs::JobName;
use crate::run_named;

/// Heartbeat file touched every 30 s, even when the poller is disabled; `poller healthcheck` reads it.
pub const HEARTBEAT_PATH: &str = "/tmp/poller.heartbeat";
pub const HEARTBEAT_EVERY: Duration = Duration::from_secs(30);
pub const HEARTBEAT_MAX_AGE: Duration = Duration::from_secs(120);
/// Jobs get this long to finish their current unit after SIGTERM/SIGINT (Fly `kill_timeout` is 30 s).
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(25);

/// The exact line 05's smoke test greps for.
pub const DISABLED_MESSAGE: &str = "poller disabled: POLLER_ENABLED=false";

/// Jobs registered by this build, in startup order (a compile-time list: a first deploy could ship ingest alone).
pub const SCHEDULED: &[JobName] = &[
    JobName::AwnIngest,
    JobName::DailyRollup,
    JobName::ReferenceSync,
];

fn implemented(j: JobName) -> bool {
    SCHEDULED.contains(&j)
}

// ---------------------------------------------------------------------------------------------------------------
// Heartbeat

pub fn touch_heartbeat(path: &Path, now: DateTime<Utc>) -> std::io::Result<()> {
    std::fs::write(path, now.to_rfc3339())
}

/// True if `path` was modified within `max_age`. A missing file is unhealthy.
pub fn heartbeat_fresh(path: &Path, max_age: Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age <= max_age)
}

async fn heartbeat_loop(
    path: PathBuf,
    clock: Option<std::sync::Arc<dyn famweather_core::clock::Clock>>,
) {
    let mut iv = tokio::time::interval(HEARTBEAT_EVERY);
    iv.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        iv.tick().await;
        let now = clock.as_ref().map_or_else(Utc::now, |c| c.now());
        if let Err(e) = touch_heartbeat(&path, now) {
            tracing::warn!(error = %e, "could not touch the heartbeat file");
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Boundaries (UTC)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Boundary {
    /// Every `period_min` minutes at second `second` (`awn_ingest`: 5 min at second 45).
    Every { period_min: u32, second: u32 },
    /// Every hour at `minute` (`daily_rollup`: 7; the reference pickup: 20).
    Hourly { minute: u32 },
    /// Every day at `hour:minute` (`reference_sync`: 11:15).
    Daily { hour: u32, minute: u32 },
}

pub const INGEST: Boundary = Boundary::Every {
    period_min: 5,
    second: 45,
};
pub const ROLLUP: Boundary = Boundary::Hourly { minute: 7 };
pub const REFERENCE_DAILY: Boundary = Boundary::Daily {
    hour: 11,
    minute: 15,
};
pub const REFERENCE_PICKUP: Boundary = Boundary::Hourly { minute: 20 };

impl Boundary {
    pub fn period(self) -> Duration {
        match self {
            Boundary::Every { period_min, .. } => Duration::from_secs(u64::from(period_min) * 60),
            Boundary::Hourly { .. } => Duration::from_secs(3600),
            Boundary::Daily { .. } => Duration::from_secs(86_400),
        }
    }

    /// The first boundary strictly after `now`.
    pub fn next_after(self, now: DateTime<Utc>) -> DateTime<Utc> {
        match self {
            Boundary::Every { period_min, second } => {
                let minute = now
                    .duration_trunc(ChronoDuration::minutes(1))
                    .unwrap_or(now);
                (0..=i64::from(period_min) + 1)
                    .map(|k| {
                        minute
                            + ChronoDuration::minutes(k)
                            + ChronoDuration::seconds(i64::from(second))
                    })
                    .find(|t| *t > now && t.minute() % period_min == 0)
                    .expect("a boundary exists within one period")
            }
            Boundary::Hourly { minute } => {
                let hour = now.duration_trunc(ChronoDuration::hours(1)).unwrap_or(now);
                let t = hour + ChronoDuration::minutes(i64::from(minute));
                if t > now {
                    t
                } else {
                    t + ChronoDuration::hours(1)
                }
            }
            Boundary::Daily { hour, minute } => {
                let day = now.duration_trunc(ChronoDuration::days(1)).unwrap_or(now);
                let t = day
                    + ChronoDuration::hours(i64::from(hour))
                    + ChronoDuration::minutes(i64::from(minute));
                if t > now {
                    t
                } else {
                    t + ChronoDuration::days(1)
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Running

async fn run_logged(env: &Env, job: JobName, flags: JobFlags) {
    match run_named(env, job, None, flags).await {
        Ok(RunOutcome::Skipped(_)) | Ok(RunOutcome::Finished { .. }) => {}
        Err(e) => tracing::error!(job = job.as_str(), error = %e, "job could not run"),
    }
}

async fn timer(env: Env, job: JobName, at: Boundary, flags: JobFlags) {
    let first = at.next_after(env.clock.now());
    let delay = (first - env.clock.now()).to_std().unwrap_or_default();
    let mut iv = interval_at(Instant::now() + delay, at.period());
    iv.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = iv.tick() => run_logged(&env, job, flags.clone()).await,
            () = env.cancel.cancelled() => return,
        }
    }
}

/// Startup catch-up (every implemented job once, in order), then the timers.
async fn startup_then_timers(env: Env) {
    for job in SCHEDULED.iter().copied() {
        if env.cancel.is_cancelled() {
            return;
        }
        let flags = JobFlags {
            self_heal: job == JobName::DailyRollup,
            ..JobFlags::default()
        };
        run_logged(&env, job, flags).await;
    }
    let pickup = JobFlags {
        pickup_only: true,
        ..JobFlags::default()
    };
    tokio::join!(
        timer(env.clone(), JobName::AwnIngest, INGEST, JobFlags::default()),
        timer(
            env.clone(),
            JobName::DailyRollup,
            ROLLUP,
            JobFlags::default()
        ),
        timer(
            env.clone(),
            JobName::ReferenceSync,
            REFERENCE_DAILY,
            JobFlags::default()
        ),
        timer(
            env.clone(),
            JobName::ReferenceSync,
            REFERENCE_PICKUP,
            pickup
        ),
    );
}

/// Marks `cancelled` only the `job_runs` rows this process created and that are still `running`.
pub async fn mark_own_runs_cancelled(env: &Env) {
    let ids: Vec<uuid::Uuid> = env
        .run_ids
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    if ids.is_empty() {
        return;
    }
    let res = sqlx::query(
        "UPDATE app.job_runs SET status = 'cancelled', finished_at = now() WHERE id = ANY($1) AND status = 'running'",
    )
    .bind(&ids)
    .execute(&env.pool)
    .await;
    match res {
        Ok(r) if r.rows_affected() > 0 => {
            tracing::warn!(runs = r.rows_affected(), "marked unfinished runs cancelled")
        }
        Ok(_) => {}
        Err(e) => {
            tracing::error!(error = %famweather_core::db::DbError::from(e), "could not mark runs cancelled")
        }
    }
}

/// Startup check (once per process, not per rollup): warns for each station whose stored standard offset differs from
/// its time zone's base offset. The stored value wins. Returns the number of stations warned about.
pub async fn warn_offset_mismatches(env: &Env) -> u32 {
    let rows: Result<Vec<(uuid::Uuid, String, i32)>, _> =
        sqlx::query_as("SELECT id, timezone, std_utc_offset_minutes::int FROM app.stations")
            .fetch_all(&env.pool)
            .await;
    let Ok(rows) = rows else {
        return 0;
    };
    let now = env.clock.now();
    let mut n = 0;
    for (id, tz, stored) in rows {
        if let Some(base) = famweather_core::time::base_offset_minutes(&tz, now) {
            if base != stored {
                n += 1;
                tracing::warn!(station_id = %id, stored, zone_base = base, "stored standard offset differs from the time zone's base offset (stored value wins)");
            }
        }
    }
    n
}

pub type Shutdown = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Resolves on SIGTERM or SIGINT.
pub fn signal_shutdown() -> Shutdown {
    Box::pin(async {
        use tokio::signal::unix::{signal, SignalKind};
        let (Ok(mut term), Ok(mut int)) = (
            signal(SignalKind::terminate()),
            signal(SignalKind::interrupt()),
        ) else {
            std::future::pending::<()>().await;
            return;
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
    })
}

/// Disabled mode: exactly one `info` event, then only the heartbeat. Touches no table, writes no `job_runs`, pings nothing.
pub async fn run_disabled(heartbeat: PathBuf, shutdown: Shutdown) {
    tracing::info!("{}", DISABLED_MESSAGE);
    if let Err(e) = touch_heartbeat(&heartbeat, Utc::now()) {
        tracing::warn!(error = %e, "could not touch the heartbeat file");
    }
    let hb = tokio::spawn(heartbeat_loop(heartbeat, None));
    shutdown.await;
    hb.abort();
}

/// Enabled mode: catch-up, timers, heartbeat; on shutdown cancel, wait `grace`, then abort what is left.
pub async fn run_enabled(env: Env, heartbeat: PathBuf, grace: Duration, shutdown: Shutdown) {
    debug_assert!(SCHEDULED.iter().all(|j| implemented(*j)));
    if let Err(e) = touch_heartbeat(&heartbeat, env.clock.now()) {
        tracing::warn!(error = %e, "could not touch the heartbeat file");
    }
    let hb = tokio::spawn(heartbeat_loop(heartbeat, Some(env.clock.clone())));
    warn_offset_mismatches(&env).await;
    let mut set: JoinSet<()> = JoinSet::new();
    set.spawn(startup_then_timers(env.clone()));
    tokio::select! {
        () = shutdown => tracing::info!("shutdown requested"),
        _ = set.join_next() => tracing::warn!("scheduler task ended unexpectedly"),
    }
    env.cancel.cancel();
    let drained =
        tokio::time::timeout(grace, async { while set.join_next().await.is_some() {} }).await;
    if drained.is_err() {
        tracing::warn!("grace period over: aborting running jobs");
        // Dropping the job futures drops the lock connections, which frees the advisory locks.
        set.abort_all();
        while set.join_next().await.is_some() {}
        mark_own_runs_cancelled(&env).await;
    }
    hb.abort();
    env.pool.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(h: u32, m: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 1, h, m, s).unwrap()
    }

    #[test]
    fn ingest_runs_at_second_45_of_every_fifth_minute() {
        assert_eq!(INGEST.next_after(t(12, 0, 0)), t(12, 0, 45));
        assert_eq!(INGEST.next_after(t(12, 0, 45)), t(12, 5, 45));
        assert_eq!(INGEST.next_after(t(12, 3, 10)), t(12, 5, 45));
        assert_eq!(INGEST.next_after(t(12, 56, 0)), t(13, 0, 45));
        assert_eq!(
            INGEST.next_after(t(23, 58, 0)),
            Utc.with_ymd_and_hms(2026, 6, 2, 0, 0, 45).unwrap()
        );
    }

    #[test]
    fn hourly_and_daily_boundaries() {
        assert_eq!(ROLLUP.next_after(t(12, 6, 59)), t(12, 7, 0));
        assert_eq!(ROLLUP.next_after(t(12, 7, 0)), t(13, 7, 0));
        assert_eq!(REFERENCE_PICKUP.next_after(t(12, 30, 0)), t(13, 20, 0));
        assert_eq!(REFERENCE_DAILY.next_after(t(11, 14, 0)), t(11, 15, 0));
        assert_eq!(
            REFERENCE_DAILY.next_after(t(11, 15, 0)),
            Utc.with_ymd_and_hms(2026, 6, 2, 11, 15, 0).unwrap()
        );
        assert_eq!(INGEST.period(), Duration::from_secs(300));
        assert_eq!(REFERENCE_DAILY.period(), Duration::from_secs(86_400));
    }

    #[test]
    fn only_implemented_jobs_are_scheduled_and_never_the_on_demand_ones() {
        assert!(!SCHEDULED.contains(&JobName::AwnBackfill));
        assert!(!SCHEDULED.contains(&JobName::CredentialReencrypt));
        assert_eq!(SCHEDULED[0], JobName::AwnIngest);
    }

    #[test]
    fn heartbeat_freshness() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("hb");
        assert!(
            !heartbeat_fresh(&p, HEARTBEAT_MAX_AGE),
            "missing file is unhealthy"
        );
        touch_heartbeat(&p, Utc::now()).unwrap();
        assert!(heartbeat_fresh(&p, HEARTBEAT_MAX_AGE));
        let old = std::time::SystemTime::now() - Duration::from_secs(300);
        let f = std::fs::File::options().write(true).open(&p).unwrap();
        f.set_modified(old).unwrap();
        assert!(
            !heartbeat_fresh(&p, HEARTBEAT_MAX_AGE),
            "stale file is unhealthy"
        );
    }

    #[test]
    fn disabled_message_is_the_literal_05_greps_for() {
        assert_eq!(DISABLED_MESSAGE, "poller disabled: POLLER_ENABLED=false");
    }
}
