//! `awn_backfill` (on demand, 03 section 7.2). Idempotent and resumable through `stations.backfill_floor_ts`.
//! No healthcheck pings. One `job_runs` row per station processed.

use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use famweather_core::awn::{ApiKey, Mac, MAX_HISTORY_LIMIT};
use famweather_core::time::{standard_date, standard_day_bounds};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use crate::awn_call::{awn_call, CallError};
use crate::creds::{load_credentials, stations_of, CredTracker, StationRow};
use crate::job::{run_job, Interrupt, Job, JobContext, JobError, JobFlags, RunOutcome, RunStatus};
use crate::jobs::awn_ingest::{db_text, prepare_rows};
use crate::jobs::daily_rollup::DailyRollup;
use crate::mapping::MapStats;
use crate::obs_write::write_page;

pub const DEFAULT_MAX_PAGES: u32 = 5000;

pub struct AwnBackfill;

#[derive(Debug, Default)]
struct StationDetail {
    pages: u32,
    rows_seen: u32,
    rows_inserted: u32,
    rows_updated: u32,
    rows_rejected: u32,
    earliest_ts: Option<DateTime<Utc>>,
    stopped_because: &'static str,
    bad_values: u32,
    /// At least one AWN call answered: the credential worked (not part of the stored detail).
    reached_awn: bool,
}

impl StationDetail {
    fn json(&self) -> Value {
        json!({
            "pages": self.pages,
            "rows_seen": self.rows_seen,
            "rows_inserted": self.rows_inserted,
            "rows_updated": self.rows_updated,
            "rows_rejected": self.rows_rejected,
            "bad_values": self.bad_values,
            "earliest_ts": self.earliest_ts,
            "stopped_because": self.stopped_because,
        })
    }
}

enum Stop {
    Done,
    Interrupted(Interrupt),
    Failed(String),
    Call(CallError),
}

#[async_trait]
impl Job for AwnBackfill {
    fn name(&self) -> &'static str {
        "awn_backfill"
    }

    fn manages_runs(&self) -> bool {
        true
    }

    async fn run(&self, ctx: &mut JobContext) -> Result<Value, JobError> {
        let pool = ctx.env.pool.clone();
        let mut tracker = CredTracker::default();
        let mut stations_done = 0u32;
        let mut stations_failed = 0u32;
        let mut earliest: BTreeMap<Uuid, (DateTime<Utc>, i32)> = BTreeMap::new();
        let mut interrupted: Option<Interrupt> = None;
        let summary = |done: u32, failed: u32| json!({"stations": done, "failed": failed});
        let fail = |msg: &str, done: u32, failed: u32| JobError::failed(msg, summary(done, failed));

        let creds = load_credentials(&pool, ctx.station)
            .await
            .map_err(|e| fail(&db_text(e), 0, 0))?;
        'creds: for c in creds {
            if let Err(i) = ctx.checkpoint().await {
                interrupted = Some(i);
                break;
            }
            let stations = stations_of(&pool, c.row.id, ctx.station)
                .await
                .map_err(|e| fail(&db_text(e), stations_done, stations_failed))?;
            let Some(keys) = ctx.env.keys.clone() else {
                stations_failed += stations.len() as u32;
                continue;
            };
            let key = match keys.decrypt_row(&c.row) {
                Ok(s) => ApiKey::new(s),
                Err(_) => {
                    tracing::error!(credential_id = %c.row.id, "credential_decrypt_failed");
                    stations_failed += stations.len() as u32;
                    continue;
                }
            };
            for st in &stations {
                let run_id = ctx
                    .begin_run(self.name(), Some(st.id))
                    .await
                    .map_err(|e| fail(&e.to_string(), stations_done, stations_failed))?;
                let mut det = StationDetail::default();
                let outcome =
                    backfill_station(ctx, c.row.id, &key, st, &mut det, &mut earliest).await;
                if det.reached_awn {
                    // last_ok_at, rate_limited -> ok, and the 401 rule sees this credential as a success.
                    tracker
                        .success(&pool, c.row.id)
                        .await
                        .map_err(|e| fail(&db_text(e), stations_done, stations_failed))?;
                }
                let (status, extra): (RunStatus, Option<String>) = match &outcome {
                    Stop::Done => (RunStatus::Success, None),
                    Stop::Interrupted(Interrupt::Cancelled) => (RunStatus::Cancelled, None),
                    Stop::Interrupted(Interrupt::LockLost) => {
                        (RunStatus::Failed, Some("lock_lost".into()))
                    }
                    Stop::Failed(m) => (RunStatus::Failed, Some(m.clone())),
                    Stop::Call(e) => (RunStatus::Failed, Some(e.to_string())),
                };
                let mut detail = det.json();
                if let Some(msg) = extra {
                    detail = crate::job::merge_error(detail, &msg);
                }
                ctx.finish_run(run_id, status, detail)
                    .await
                    .map_err(|e| fail(&e.to_string(), stations_done, stations_failed))?;
                match outcome {
                    Stop::Done => stations_done += 1,
                    Stop::Interrupted(i) => {
                        interrupted = Some(i);
                        break 'creds;
                    }
                    Stop::Failed(_) => stations_failed += 1,
                    Stop::Call(e) => {
                        stations_failed += 1;
                        match e {
                            CallError::Unauthorized => {
                                tracker.unauthorized(c.row.id);
                                break;
                            }
                            CallError::RateLimited => {
                                let _ = tracker.rate_limited(&pool, c.row.id).await;
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        let resolution = tracker
            .resolve(&pool)
            .await
            .map_err(|e| fail(&db_text(e), stations_done, stations_failed))?;
        if let Some(i) = interrupted {
            return Err(JobError::interrupted(
                i,
                summary(stations_done, stations_failed),
            ));
        }

        // Every standard date from each station's earliest written row to today goes into the dirty queue, then the
        // rollup runs (waiting up to 15 minutes for a rollup that is already running).
        enqueue_range(ctx, &earliest)
            .await
            .map_err(|e| fail(&db_text(e), stations_done, stations_failed))?;
        if !earliest.is_empty() {
            run_rollup_after(ctx).await;
        }

        let detail = summary(stations_done, stations_failed);
        if resolution.suspected_application_key_failure {
            return Err(JobError::failed(
                "suspected_application_key_failure",
                detail,
            ));
        }
        if stations_failed > 0 {
            return Err(JobError::failed("units_failed", detail));
        }
        Ok(detail)
    }
}

async fn backfill_station(
    ctx: &mut JobContext,
    cred: Uuid,
    key: &ApiKey,
    st: &StationRow,
    det: &mut StationDetail,
    earliest: &mut BTreeMap<Uuid, (DateTime<Utc>, i32)>,
) -> Stop {
    let pool = ctx.env.pool.clone();
    let env = ctx.env.clone();
    let Ok(mac) = Mac::parse(&st.mac) else {
        det.stopped_because = "bad_mac";
        return Stop::Failed("bad stored MAC".into());
    };
    let off = st.std_utc_offset_minutes;
    let since_ts = ctx.flags.since.map(|d| standard_day_bounds(off, d).0);
    let max_pages = ctx.flags.max_pages.unwrap_or(DEFAULT_MAX_PAGES);
    let resume_floor = if ctx.flags.restart {
        None
    } else {
        st.backfill_floor_ts
    };
    // No cursor means "now": AWN answers with the newest page when no endDate is given.
    let mut end: Option<DateTime<Utc>> = resume_floor.map(|f| f - Duration::milliseconds(1));
    let mut prev_oldest: Option<i64> = None;
    let mut stats = MapStats::default();
    loop {
        if det.pages >= max_pages {
            det.stopped_because = "max_pages";
            return Stop::Done;
        }
        if let Err(i) = ctx.checkpoint().await {
            det.stopped_because = "interrupted";
            return Stop::Interrupted(i);
        }
        let page = awn_call(&env, cred, || {
            let awn = env.awn.clone();
            let (key, mac, end) = (key, &mac, end);
            Box::pin(async move { awn.history(key, mac, end, MAX_HISTORY_LIMIT).await })
        })
        .await;
        let page = match page {
            Ok(p) => {
                det.reached_awn = true;
                p
            }
            Err(CallError::Interrupted) => {
                det.stopped_because = "interrupted";
                return Stop::Interrupted(Interrupt::Cancelled);
            }
            Err(e) => {
                det.stopped_because = "error";
                return Stop::Call(e);
            }
        };
        let oldest_ms = page.iter().map(|o| o.dateutc).min();
        let newest_ms = page.iter().map(|o| o.dateutc).max();
        let (Some(oldest_ms), Some(newest_ms)) = (oldest_ms, newest_ms) else {
            det.stopped_because = "empty_page";
            return Stop::Done;
        };
        det.pages += 1;
        det.rows_seen += page.len() as u32;
        let now = env.clock.now();
        let rows = prepare_rows(&page, now, &mut stats, &mut det.rows_rejected);
        det.bad_values = stats.bad_values;
        if !rows.is_empty() {
            match write_page(&pool, st.id, off, rows, false).await {
                Ok(w) => {
                    det.rows_inserted += w.inserted;
                    det.rows_updated += w.updated;
                }
                Err(e) => {
                    det.stopped_because = "database_error";
                    return Stop::Failed(db_text(e));
                }
            }
        }
        let oldest = Utc
            .timestamp_millis_opt(oldest_ms)
            .single()
            .unwrap_or_default();
        let newest = Utc
            .timestamp_millis_opt(newest_ms)
            .single()
            .unwrap_or_default();
        det.earliest_ts = Some(det.earliest_ts.map_or(oldest, |e| e.min(oldest)));
        earliest
            .entry(st.id)
            .and_modify(|(t, _)| *t = (*t).min(oldest))
            .or_insert((oldest, off));
        // After the page commits: the resume cursor and last_observed_at (the floor only ever moves back).
        if let Err(e) = update_floor(&pool, st.id, oldest, newest).await {
            det.stopped_because = "database_error";
            return Stop::Failed(db_text(e));
        }
        if since_ts.is_some_and(|s| oldest <= s) {
            det.stopped_because = "since_reached";
            return Stop::Done;
        }
        if prev_oldest.is_some_and(|p| oldest_ms >= p) {
            det.stopped_because = "no_progress";
            return Stop::Done;
        }
        prev_oldest = Some(oldest_ms);
        end = Some(oldest - Duration::milliseconds(1));
    }
}

async fn update_floor(
    pool: &PgPool,
    station: Uuid,
    oldest: DateTime<Utc>,
    newest: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE app.stations SET backfill_floor_ts = LEAST(COALESCE(backfill_floor_ts, $2), $2), \
         last_observed_at = GREATEST(COALESCE(last_observed_at, $3), $3) WHERE id = $1",
    )
    .bind(station)
    .bind(oldest)
    .bind(newest)
    .execute(pool)
    .await?;
    Ok(())
}

async fn enqueue_range(
    ctx: &JobContext,
    earliest: &BTreeMap<Uuid, (DateTime<Utc>, i32)>,
) -> Result<(), sqlx::Error> {
    let today_utc = ctx.env.clock.now();
    for (station, (first, off)) in earliest {
        let from: NaiveDate = standard_date(*off, *first);
        let to: NaiveDate = standard_date(*off, today_utc);
        sqlx::query(
            "INSERT INTO app.rollup_dirty_days (station_id, local_date) \
             SELECT $1, d::date FROM generate_series($2::date, $3::date, interval '1 day') AS d ORDER BY d \
             ON CONFLICT (station_id, local_date) DO UPDATE SET local_date = EXCLUDED.local_date",
        )
        .bind(station)
        .bind(from)
        .bind(to)
        .execute(&ctx.env.pool)
        .await?;
    }
    Ok(())
}

/// `run_job_wait(DailyRollup, wait = 15 min)`: poll for the rollup lock every few seconds; if still held when the
/// wait is over, give up with a `warn` (the queue persists, so the scheduled rollup does the same work).
async fn run_rollup_after(ctx: &JobContext) {
    let env = &ctx.env;
    let deadline = tokio::time::Instant::now() + env.rollup_wait.total;
    loop {
        match run_job(env, &DailyRollup, None, JobFlags::default()).await {
            Ok(RunOutcome::Skipped(crate::job::SkipReason::LockHeld)) => {
                if tokio::time::Instant::now() >= deadline {
                    tracing::warn!("daily_rollup still running after the wait; the scheduled run will process the queue");
                    return;
                }
                if !crate::awn_call::sleep_or_cancel(env, env.rollup_wait.poll).await {
                    return;
                }
            }
            Ok(RunOutcome::Finished { status, .. }) => {
                tracing::info!(
                    status = status.as_str(),
                    "post-backfill daily_rollup finished"
                );
                return;
            }
            Ok(RunOutcome::Skipped(_)) => return,
            Err(e) => {
                tracing::warn!(error = %e, "post-backfill daily_rollup could not run");
                return;
            }
        }
    }
}
