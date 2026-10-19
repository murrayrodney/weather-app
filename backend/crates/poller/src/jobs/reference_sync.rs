//! `reference_sync` (daily 11:15 UTC, plus the hourly pickup of never-synced stations, 03 section 7.5).

use async_trait::async_trait;
use chrono::{Duration, NaiveDate};
use famweather_core::acis::{AcisDay, AcisError};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, QueryBuilder, Row};

use crate::awn_call::sleep_or_cancel;
use crate::job::{capped, Interrupt, Job, JobContext, JobError};
use crate::obs_write::retry_tx;

/// How far back a first load reaches.
pub const REFERENCE_START_YEAR: i32 = 1991;
/// Days of possible revisions that every refresh re-reads.
pub const REFRESH_DAYS: i64 = 14;
const WRITE_CHUNK: usize = 1000;
const ACIS_RETRIES: usize = 3;

pub struct ReferenceSync;

#[derive(Debug, Default, Serialize)]
struct Detail {
    stations: u32,
    full_loads: u32,
    refreshes: u32,
    rows_seen: u32,
    rows_written: u32,
    empty_results: u32,
    unparsed: u32,
    failed_units: u32,
    failed: Vec<String>,
}

/// `sdate` for one station (03 section 7.5): a never-synced station loads from `REFERENCE_START_YEAR`;
/// otherwise `min(last_synced_date + 1 day, yesterday - 13 days)`, so a long-stale station catches up fully and a
/// recent one still re-reads 14 days.
pub fn start_date(last_synced: Option<NaiveDate>, yesterday: NaiveDate) -> NaiveDate {
    match last_synced {
        None => NaiveDate::from_ymd_opt(REFERENCE_START_YEAR, 1, 1).expect("valid date"),
        Some(l) => (l + Duration::days(1)).min(yesterday - Duration::days(REFRESH_DAYS - 1)),
    }
}

async fn candidates(
    pool: &PgPool,
    pickup_only: bool,
) -> Result<Vec<(String, Option<NaiveDate>)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, last_synced_date FROM app.reference_stations \
         WHERE (NOT $1 OR last_synced_date IS NULL) ORDER BY id",
    )
    .bind(pickup_only)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|r| Ok((r.try_get("id")?, r.try_get("last_synced_date")?)))
        .collect()
}

async fn write_days(pool: &PgPool, sid: &str, days: &[AcisDay]) -> Result<u32, sqlx::Error> {
    let mut written = 0u32;
    let mut sorted: Vec<&AcisDay> = days.iter().collect();
    sorted.sort_by_key(|d| d.date);
    sorted.dedup_by_key(|d| d.date);
    for chunk in sorted.chunks(WRITE_CHUNK) {
        written += retry_tx(|| async {
            let mut qb = QueryBuilder::<Postgres>::new(
                "INSERT INTO app.reference_daily (reference_station_id, date, tmax_f, tmin_f, precip_in, precip_trace, quality_flags) ",
            );
            qb.push_values(chunk, |mut b, d| {
                b.push_bind(sid)
                    .push_bind(d.date)
                    .push_bind(d.tmax_f.map(|x| x as f32))
                    .push_bind(d.tmin_f.map(|x| x as f32))
                    .push_bind(d.precip_in.map(|x| x as f32))
                    .push_bind(d.precip_trace)
                    .push_bind(d.quality_flags.clone());
            });
            qb.push(
                " ON CONFLICT (reference_station_id, date) DO UPDATE SET tmax_f = EXCLUDED.tmax_f, \
                 tmin_f = EXCLUDED.tmin_f, precip_in = EXCLUDED.precip_in, precip_trace = EXCLUDED.precip_trace, \
                 quality_flags = EXCLUDED.quality_flags, fetched_at = now() \
                 WHERE (app.reference_daily.tmax_f, app.reference_daily.tmin_f, app.reference_daily.precip_in, \
                        app.reference_daily.precip_trace, app.reference_daily.quality_flags) \
                       IS DISTINCT FROM (EXCLUDED.tmax_f, EXCLUDED.tmin_f, EXCLUDED.precip_in, \
                        EXCLUDED.precip_trace, EXCLUDED.quality_flags)",
            );
            let n = qb.build().execute(pool).await?.rows_affected();
            Ok(n as u32)
        })
        .await?;
    }
    Ok(written)
}

#[async_trait]
impl Job for ReferenceSync {
    fn name(&self) -> &'static str {
        "reference_sync"
    }

    fn ping_suffix(&self) -> Option<&'static str> {
        Some("REFERENCE_SYNC")
    }

    /// The hourly pickup writes no `job_runs` row and sends no ping when there is nothing to load.
    async fn should_run(&self, ctx: &JobContext) -> bool {
        if !ctx.flags.pickup_only {
            return true;
        }
        matches!(candidates(&ctx.env.pool, true).await, Ok(v) if !v.is_empty())
    }

    async fn run(&self, ctx: &mut JobContext) -> Result<Value, JobError> {
        let mut d = Detail::default();
        let snap = |d: &Detail| serde_json::to_value(d).unwrap_or_else(|_| json!({}));
        let pool = ctx.env.pool.clone();
        let todo = candidates(&pool, ctx.flags.pickup_only)
            .await
            .map_err(|e| JobError::failed(crate::jobs::awn_ingest::db_text(e), snap(&d)))?;
        let yesterday = (ctx.env.clock.now() - Duration::days(1)).date_naive();
        let mut first = true;
        for (sid, last_synced) in todo {
            if let Err(i) = ctx.checkpoint().await {
                return Err(JobError::interrupted(i, snap(&d)));
            }
            if !first && !sleep_or_cancel(&ctx.env, ctx.env.acis_spacing).await {
                return Err(JobError::interrupted(Interrupt::Cancelled, snap(&d)));
            }
            first = false;
            d.stations += 1;
            let sdate = start_date(last_synced, yesterday);
            if last_synced.is_none() {
                d.full_loads += 1;
            } else {
                d.refreshes += 1;
            }
            let mut attempt = 0usize;
            let result = loop {
                match ctx.env.acis.station_data(&sid, sdate, yesterday).await {
                    Err(AcisError::Unavailable) if attempt < ACIS_RETRIES => {
                        attempt += 1;
                        let wait = ctx
                            .env
                            .backoff
                            .unavailable
                            .get(attempt - 1)
                            .copied()
                            .unwrap_or_default();
                        if !sleep_or_cancel(&ctx.env, wait).await {
                            return Err(JobError::interrupted(Interrupt::Cancelled, snap(&d)));
                        }
                    }
                    other => break other,
                }
            };
            let days = match result {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(sid = %sid, error = %e, "ACIS request failed");
                    d.failed_units += 1;
                    d.failed.push(sid);
                    continue;
                }
            };
            d.rows_seen += days.len() as u32;
            d.unparsed += days.iter().map(|x| u32::from(x.unparsed)).sum::<u32>();
            if days.iter().any(|x| x.unparsed > 0) {
                tracing::warn!(sid = %sid, "unparseable ACIS elements stored as NULL");
            }
            if days.is_empty() {
                // A refresh that returns no rows leaves last_synced_date unchanged (and replay's empty result too).
                d.empty_results += 1;
                continue;
            }
            match write_days(&pool, &sid, &days).await {
                Ok(n) => d.rows_written += n,
                Err(e) => {
                    tracing::warn!(sid = %sid, error = %crate::jobs::awn_ingest::db_text(e), "reference write failed");
                    d.failed_units += 1;
                    d.failed.push(sid);
                    continue;
                }
            }
            let last = days.iter().map(|x| x.date).max().expect("non-empty");
            sqlx::query("UPDATE app.reference_stations SET last_synced_date = $2 WHERE id = $1")
                .bind(&sid)
                .bind(last)
                .execute(&pool)
                .await
                .map_err(|e| JobError::failed(crate::jobs::awn_ingest::db_text(e), snap(&d)))?;
        }
        d.failed = capped(&d.failed);
        if d.failed_units > 0 {
            return Err(JobError::failed("units_failed", snap(&d)));
        }
        Ok(snap(&d))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn start_date_rules() {
        let yesterday = d("2026-10-16");
        assert_eq!(start_date(None, yesterday), d("1991-01-01"));
        // recent: last synced yesterday -> re-read 14 days (yesterday - 13 days)
        assert_eq!(
            start_date(Some(d("2026-10-16")), yesterday),
            d("2026-10-03")
        );
        // stale: last synced long ago -> catch up from the day after
        assert_eq!(
            start_date(Some(d("2026-08-01")), yesterday),
            d("2026-08-02")
        );
    }
}
