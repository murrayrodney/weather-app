//! `awn_ingest` (every 5 min, 03 section 7.1).

use std::collections::{BTreeSet, HashSet};

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use famweather_core::awn::{ApiKey, AwnObservation, Mac, MAX_HISTORY_LIMIT};
use famweather_core::time::standard_date;
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use crate::awn_call::{awn_call, CallError};
use crate::creds::{load_credentials, stations_of, CredTracker, StationRow};
use crate::job::{capped, Interrupt, Job, JobContext, JobError};
use crate::mapping::{map_observation, ts_acceptable, MapStats, MappedRow};
use crate::obs_write::write_page;

/// Pages fetched at most per station and run (about 12 days of 5-minute data).
pub const MAX_PAGES: usize = 12;

pub struct AwnIngest;

#[derive(Debug, Default, Serialize)]
pub struct Gap {
    pub station_id: Uuid,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

#[derive(Debug, Default, Serialize)]
pub struct IngestDetail {
    pub credentials: u32,
    pub stations: u32,
    pub empty_stations: u32,
    pub rows_seen: u32,
    pub rows_inserted: u32,
    pub rows_updated: u32,
    pub rows_rejected: u32,
    pub unknown_devices: Vec<String>,
    pub missing_devices: u32,
    pub unknown_fields: Vec<String>,
    pub bad_values: u32,
    pub unauthorized: u32,
    pub rate_limited: u32,
    pub decrypt_failed: u32,
    pub failed_units: u32,
    pub marked_invalid: u32,
    pub gaps: Vec<Gap>,
    pub gap_beyond_catchup: bool,
}

impl IngestDetail {
    fn snapshot(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({}))
    }
}

/// How one station's turn ended when it did not complete.
enum StationFail {
    Call(CallError),
    Db(String),
}

pub(crate) fn db_text(e: sqlx::Error) -> String {
    famweather_core::db::DbError::from(e).to_string()
}

/// Rows per request: enough to cover `now - last` at 5-minute spacing, with a margin.
pub fn history_limit(now: DateTime<Utc>, last: DateTime<Utc>) -> u16 {
    let minutes = (now - last).num_minutes().max(0) as f64;
    let n = (minutes / 5.0).ceil() as i64 + 2;
    n.clamp(12, i64::from(MAX_HISTORY_LIMIT)) as u16
}

/// Maps, validates and sorts a raw page. Returns the rows (deduplicated later at write time).
pub(crate) fn prepare_rows(
    raw: &[AwnObservation],
    now: DateTime<Utc>,
    stats: &mut MapStats,
    rejected: &mut u32,
) -> Vec<MappedRow> {
    let mut out = Vec::with_capacity(raw.len());
    for o in raw {
        match map_observation(o, stats) {
            Some(r) if ts_acceptable(r.ts, now) => out.push(r),
            _ => *rejected += 1,
        }
    }
    out
}

struct FetchResult {
    /// Newest page first, as fetched.
    pages: Vec<Vec<AwnObservation>>,
    /// Why paging stopped early, if it did: the error or an interruption.
    stopped: Option<Result<Interrupt, CallError>>,
}

fn oldest_of(page: &[AwnObservation]) -> Option<i64> {
    page.iter().map(|o| o.dateutc).min()
}

async fn fetch_pages(
    ctx: &mut JobContext,
    cred: Uuid,
    key: &ApiKey,
    mac: &Mac,
    last: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> FetchResult {
    let env = ctx.env.clone();
    let mut pages: Vec<Vec<AwnObservation>> = Vec::new();
    let limit = last.map_or(MAX_HISTORY_LIMIT, |t| history_limit(now, t));
    let mut end: Option<DateTime<Utc>> = None;
    let max_pages = if last.is_some() { MAX_PAGES } else { 1 };
    while pages.len() < max_pages {
        if let Err(i) = ctx.checkpoint().await {
            return FetchResult {
                pages,
                stopped: Some(Ok(i)),
            };
        }
        let page = awn_call(&env, cred, || {
            Box::pin({
                let awn = env.awn.clone();
                let end = end;
                async move { awn.history(key, mac, end, limit).await }
            })
        })
        .await;
        let page = match page {
            Ok(p) => p,
            Err(CallError::Interrupted) => {
                return FetchResult {
                    pages,
                    stopped: Some(Ok(Interrupt::Cancelled)),
                }
            }
            Err(e) => {
                return FetchResult {
                    pages,
                    stopped: Some(Err(e)),
                }
            }
        };
        let oldest = oldest_of(&page);
        let empty = page.is_empty();
        pages.push(page);
        let (Some(t), Some(oldest)) = (last, oldest) else {
            // empty station (single page) or an empty page
            break;
        };
        if empty || oldest <= t.timestamp_millis() {
            break;
        }
        end = Utc.timestamp_millis_opt(oldest - 1).single();
    }
    FetchResult {
        pages,
        stopped: None,
    }
}

/// Ingests one station. `Ok(None)` is a clean finish, `Ok(Some(i))` an interruption after the writes done so far.
async fn ingest_station(
    ctx: &mut JobContext,
    cred: Uuid,
    key: &ApiKey,
    st: &StationRow,
    d: &mut IngestDetail,
    stats: &mut MapStats,
) -> Result<Option<Interrupt>, StationFail> {
    let mac = Mac::parse(&st.mac).map_err(|_| StationFail::Db("bad stored MAC".into()))?;
    let pool: PgPool = ctx.env.pool.clone();
    let now = ctx.env.clock.now();
    let last: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT max(ts_utc) FROM app.observations WHERE station_id = $1")
            .bind(st.id)
            .fetch_one(&pool)
            .await
            .map_err(|e| StationFail::Db(db_text(e)))?;
    if last.is_none() {
        d.empty_stations += 1;
    }
    let fetched = fetch_pages(ctx, cred, key, &mac, last, now).await;

    // Write phase: oldest page first, so max(ts_utc) only advances over a contiguous prefix.
    let oldest_fetched = fetched.pages.iter().filter_map(|p| oldest_of(p)).min();
    let mut write_err = None;
    for page in fetched.pages.iter().rev() {
        d.rows_seen += page.len() as u32;
        let rows = prepare_rows(page, now, stats, &mut d.rows_rejected);
        if rows.is_empty() {
            continue;
        }
        match write_page(&pool, st.id, st.std_utc_offset_minutes, rows, true).await {
            Ok(w) => {
                d.rows_inserted += w.inserted;
                d.rows_updated += w.updated;
            }
            Err(e) => {
                write_err = Some(StationFail::Db(db_text(e)));
                break;
            }
        }
    }
    if let (Some(t), Some(oldest)) = (last, oldest_fetched) {
        let oldest = Utc.timestamp_millis_opt(oldest).single().unwrap_or(t);
        if oldest > t {
            d.gap_beyond_catchup = true;
            if d.gaps.len() < crate::job::MAX_LIST_ITEMS {
                d.gaps.push(Gap {
                    station_id: st.id,
                    from: t,
                    to: oldest,
                });
            }
            tracing::warn!(
                station_id = %st.id,
                "gap beyond ingest catch-up; run: poller run-job awn_backfill --station {} --restart --since {}",
                st.id,
                standard_date(st.std_utc_offset_minutes, t)
            );
        }
    }
    if let Some(e) = write_err {
        return Err(e);
    }
    match fetched.stopped {
        None => Ok(None),
        Some(Ok(i)) => Ok(Some(i)),
        Some(Err(e)) => Err(StationFail::Call(e)),
    }
}

#[async_trait]
impl Job for AwnIngest {
    fn name(&self) -> &'static str {
        "awn_ingest"
    }

    fn ping_suffix(&self) -> Option<&'static str> {
        Some("AWN_INGEST")
    }

    async fn run(&self, ctx: &mut JobContext) -> Result<Value, JobError> {
        let mut d = IngestDetail::default();
        let mut stats = MapStats::default();
        let mut tracker = CredTracker::default();
        let pool = ctx.env.pool.clone();
        let fail = |d: &IngestDetail, msg: &str| JobError::failed(msg, d.snapshot());

        let creds = load_credentials(&pool, ctx.station)
            .await
            .map_err(|e| fail(&d, &db_text(e)))?;
        d.credentials = creds.len() as u32;
        let mut unknown: BTreeSet<String> = BTreeSet::new();
        let mut interrupted: Option<Interrupt> = None;

        'creds: for c in creds {
            if let Err(i) = ctx.checkpoint().await {
                interrupted = Some(i);
                break;
            }
            let stations = stations_of(&pool, c.row.id, ctx.station)
                .await
                .map_err(|e| fail(&d, &db_text(e)))?;
            d.stations += stations.len() as u32;
            let Some(keys) = ctx.env.keys.clone() else {
                d.decrypt_failed += 1;
                d.failed_units += stations.len() as u32;
                tracing::error!(credential_id = %c.row.id, "credential_decrypt_failed");
                continue;
            };
            let secret = match keys.decrypt_row(&c.row) {
                Ok(s) => s,
                Err(_) => {
                    // No status change: the key set may simply be older than the data (job fails, never invalid).
                    tracing::error!(credential_id = %c.row.id, "credential_decrypt_failed");
                    d.decrypt_failed += 1;
                    d.failed_units += stations.len() as u32;
                    continue;
                }
            };
            let key = ApiKey::new(secret);
            let env = ctx.env.clone();
            let devices = awn_call(&env, c.row.id, || {
                let awn = env.awn.clone();
                let key = &key;
                Box::pin(async move { awn.list_devices(key).await })
            })
            .await;
            let devices = match devices {
                Ok(v) => v,
                Err(e) => {
                    d.failed_units += stations.len() as u32;
                    match handle_call_error(&mut tracker, &mut d, &pool, c.row.id, &e).await {
                        Some(i) => {
                            interrupted = Some(i);
                            break 'creds;
                        }
                        None => continue,
                    }
                }
            };
            tracker
                .success(&pool, c.row.id)
                .await
                .map_err(|e| fail(&d, &db_text(e)))?;

            // Devices at AWN that have no stations row at all (Q5): last 4 of the MAC only.
            let device_macs: Vec<String> = devices.iter().map(|x| x.mac.to_string()).collect();
            let known: HashSet<String> =
                sqlx::query_scalar("SELECT mac FROM app.stations WHERE mac = ANY($1)")
                    .bind(&device_macs)
                    .fetch_all(&pool)
                    .await
                    .map_err(|e| fail(&d, &db_text(e)))?
                    .into_iter()
                    .collect();
            for dev in &devices {
                if !known.contains(dev.mac.as_str()) {
                    unknown.insert(dev.mac.last4());
                }
            }

            let mut remaining = stations.len() as u32;
            for st in &stations {
                remaining -= 1;
                if let Err(i) = ctx.checkpoint().await {
                    interrupted = Some(i);
                    break 'creds;
                }
                if !devices.iter().any(|x| x.mac.as_str() == st.mac) {
                    d.missing_devices += 1;
                    tracing::warn!(station_id = %st.id, "station MAC missing from the AWN device list");
                    continue;
                }
                match ingest_station(ctx, c.row.id, &key, st, &mut d, &mut stats).await {
                    Ok(None) => {}
                    Ok(Some(i)) => {
                        interrupted = Some(i);
                        break 'creds;
                    }
                    Err(StationFail::Db(msg)) => {
                        tracing::warn!(station_id = %st.id, error = %msg, "station ingest failed");
                        d.failed_units += 1;
                    }
                    Err(StationFail::Call(e)) => {
                        tracing::warn!(station_id = %st.id, credential_id = %c.row.id, error = %e, "station ingest failed");
                        d.failed_units += 1;
                        let stop_credential =
                            matches!(e, CallError::Unauthorized | CallError::RateLimited);
                        if let Some(i) =
                            handle_call_error(&mut tracker, &mut d, &pool, c.row.id, &e).await
                        {
                            interrupted = Some(i);
                            break 'creds;
                        }
                        if stop_credential {
                            // Stop using this credential for the rest of the run.
                            d.failed_units += remaining;
                            break;
                        }
                    }
                }
            }
        }

        d.unknown_devices = capped(&unknown.into_iter().collect::<Vec<_>>());
        d.unknown_fields = stats.unknown_fields.iter().cloned().collect();
        d.bad_values = stats.bad_values;
        if !d.unknown_devices.is_empty() {
            tracing::warn!(devices = ?d.unknown_devices, "AWN devices without a station row (not created)");
        }
        d.rate_limited = tracker.rate_limited;

        let resolution = tracker
            .resolve(&pool)
            .await
            .map_err(|e| fail(&d, &db_text(e)))?;
        d.marked_invalid = resolution.marked_invalid;
        if let Some(i) = interrupted {
            return Err(JobError::interrupted(i, d.snapshot()));
        }
        if resolution.suspected_application_key_failure {
            return Err(fail(&d, "suspected_application_key_failure"));
        }
        if d.failed_units > 0 {
            return Err(fail(&d, "units_failed"));
        }
        Ok(d.snapshot())
    }

    /// `/fail` only when every station failed in the run or the last three completed runs each failed;
    /// a partial failure still pings success so one flaky device does not page.
    async fn ping_fail_on_failure(&self, pool: &PgPool, detail: &Value) -> bool {
        let stations = detail["stations"].as_u64().unwrap_or(0);
        let failed = detail["failed_units"].as_u64().unwrap_or(0);
        let suspected = detail["error"].as_str() == Some("suspected_application_key_failure");
        if suspected || (stations > 0 && failed >= stations) {
            return true;
        }
        let recent: Result<Vec<String>, _> = sqlx::query_scalar(
            "SELECT status FROM app.job_runs WHERE job = 'awn_ingest' AND status IN ('success','failed') \
             ORDER BY finished_at DESC, started_at DESC LIMIT 3",
        )
        .fetch_all(pool)
        .await;
        match recent {
            Ok(v) => v.len() == 3 && v.iter().all(|s| s == "failed"),
            Err(_) => true,
        }
    }
}

/// Records a failed call in the tracker; returns an interruption when shutdown was requested.
pub(crate) async fn handle_call_error(
    tracker: &mut CredTracker,
    d: &mut IngestDetail,
    pool: &PgPool,
    cred: Uuid,
    e: &CallError,
) -> Option<Interrupt> {
    match e {
        CallError::Unauthorized => {
            d.unauthorized += 1;
            tracker.unauthorized(cred);
        }
        CallError::RateLimited => {
            if let Err(err) = tracker.rate_limited(pool, cred).await {
                tracing::warn!(error = %db_text(err), "could not record rate_limited");
            }
        }
        CallError::Interrupted => return Some(Interrupt::Cancelled),
        CallError::Busy(b) => {
            tracing::warn!(error = %b, "gate busy; unit counts as a transient failure");
        }
        CallError::Unavailable | CallError::BadResponse => {}
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn limit_covers_the_gap_with_a_margin_and_is_clamped() {
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();
        assert_eq!(
            history_limit(now, now - Duration::minutes(5)),
            12,
            "floor of 12"
        );
        assert_eq!(history_limit(now, now - Duration::hours(3)), 38);
        assert_eq!(
            history_limit(now, now - Duration::days(5)),
            288,
            "ceiling of 288"
        );
        assert_eq!(
            history_limit(now, now + Duration::hours(1)),
            12,
            "future last is clamped up"
        );
    }
}
