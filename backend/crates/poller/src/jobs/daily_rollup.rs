//! `daily_rollup` (hourly, and after backfill, 03 section 7.3).
//!
//! Consumes `app.rollup_dirty_days`, expands each dirty day to its whole month (a counter-choice change re-dirties
//! the month), and per station-day, in one transaction, deletes the queue row first (the row lock makes a concurrent
//! ingest wait and re-enqueue the day, so nothing is lost) and then computes `rain_inc_in` and the rollup row.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration as StdDuration;

use async_trait::async_trait;
use chrono::{DateTime, Datelike, Duration, NaiveDate, Timelike, Utc};
use famweather_core::rain::consts::RESET_NEAR_ZERO_IN;
use famweather_core::rain::{
    self, choose_counter, increments_seeded, Counter, Counters, Inc, RainRow,
};
use famweather_core::time::standard_day_bounds;
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::{PgConnection, PgPool, Row};
use uuid::Uuid;

use crate::job::{Job, JobContext, JobError};
use crate::jobs::awn_ingest::db_text;

pub const TEMP_MIN_F: f32 = -60.0;
pub const TEMP_MAX_F: f32 = 130.0;
/// Coverage below this blanks `tmax_f`/`tmin_f`; at or above `COVERAGE_COMPLETE` the day is complete.
pub const COVERAGE_MIN: f64 = 0.5;
pub const COVERAGE_COMPLETE: f64 = 0.9;
const JOB_RUNS_KEEP_DAYS: i64 = 90;
const SELF_HEAL_DAYS: i64 = 60;
const RETRY_DELAYS_MS: [u64; 3] = [50, 150, 450];

pub struct DailyRollup;

#[derive(Debug, Default, Serialize)]
pub struct RollupDetail {
    pub stations: u32,
    pub days_processed: u32,
    pub days_changed: u32,
    pub resets: u32,
    pub mismatches: u32,
    pub temp_outliers: u32,
    pub self_healed: bool,
    pub housekeeping_done_on: Option<String>,
    pub job_runs_deleted: u64,
}

// ---------------------------------------------------------------------------------------------------------------
// Pure computation

/// One stored reading as the rollup needs it.
#[derive(Debug, Clone)]
pub struct SeriesRow {
    pub ts: DateTime<Utc>,
    pub temp_f: Option<f32>,
    pub counters: Counters,
}

/// Everything computed for one standard day.
#[derive(Debug, Clone, PartialEq)]
pub struct DayResult {
    pub obs_count: u16,
    pub tmax_f: Option<f32>,
    pub tmin_f: Option<f32>,
    pub is_complete: bool,
    pub coverage: f64,
    pub interval_min: i64,
    pub rain_in: Option<f64>,
    pub resets: u32,
    pub temp_outliers: u32,
}

/// 5 min if the median gap between the day's consecutive readings is at most 7.5 min, else 30 min.
pub fn observed_interval_minutes(ts: &[DateTime<Utc>]) -> i64 {
    if ts.len() < 2 {
        return 30;
    }
    let mut gaps: Vec<i64> = ts.windows(2).map(|w| (w[1] - w[0]).num_seconds()).collect();
    gaps.sort_unstable();
    let median_s = if gaps.len() % 2 == 1 {
        gaps[gaps.len() / 2] as f64
    } else {
        (gaps[gaps.len() / 2 - 1] + gaps[gaps.len() / 2]) as f64 / 2.0
    };
    if median_s <= 7.5 * 60.0 {
        5
    } else {
        30
    }
}

/// Temperatures and completeness of one day from its readings (`day` is time-ordered and inside the day).
fn temperature_part(
    day: &[&SeriesRow],
    contains_now: bool,
) -> (u16, Option<f32>, Option<f32>, bool, f64, i64, u32) {
    let ts: Vec<DateTime<Utc>> = day.iter().map(|r| r.ts).collect();
    let interval = observed_interval_minutes(&ts);
    let with_temp: Vec<f32> = day.iter().filter_map(|r| r.temp_f).collect();
    let obs_count = with_temp.len() as u16;
    let covered_min = i64::from(obs_count) * interval;
    let coverage = (covered_min as f64 / (24.0 * 60.0)).min(1.0);
    let mut outliers = 0u32;
    let valid: Vec<f32> = with_temp
        .into_iter()
        .filter(|t| {
            let ok = (TEMP_MIN_F..=TEMP_MAX_F).contains(t);
            if !ok {
                outliers += 1;
            }
            ok
        })
        .collect();
    let (mut tmax, mut tmin) = (None, None);
    if coverage >= COVERAGE_MIN {
        tmax = valid.iter().copied().reduce(f32::max);
        tmin = valid.iter().copied().reduce(f32::min);
    }
    let complete = coverage >= COVERAGE_COMPLETE && !contains_now;
    (
        obs_count, tmax, tmin, complete, coverage, interval, outliers,
    )
}

/// Computes one day from a month series (`incs` aligned with `series`; `None` when the month has no usable counter).
pub fn compute_day(
    series: &[SeriesRow],
    incs: Option<&[Inc]>,
    day_start: DateTime<Utc>,
    day_end: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Option<DayResult> {
    let lo = series.partition_point(|r| r.ts < day_start);
    let hi = series.partition_point(|r| r.ts < day_end);
    if lo == hi {
        return None;
    }
    let day: Vec<&SeriesRow> = series[lo..hi].iter().collect();
    let contains_now = day_start <= now && now < day_end;
    let (obs_count, tmax_f, tmin_f, is_complete, coverage, interval_min, temp_outliers) =
        temperature_part(&day, contains_now);
    let (mut rain_in, mut resets) = (None, 0);
    if let Some(incs) = incs {
        let slice = &incs[lo..hi];
        let sum: f64 = slice.iter().filter_map(|i| i.inc).sum();
        if slice.iter().any(|i| i.inc.is_some()) {
            rain_in = Some(rain::round3(sum));
        }
        resets = slice.iter().filter(|i| i.reset).count() as u32;
    }
    Some(DayResult {
        obs_count,
        tmax_f,
        tmin_f,
        is_complete,
        coverage,
        interval_min,
        rain_in,
        resets,
        temp_outliers,
    })
}

// ---------------------------------------------------------------------------------------------------------------
// Month series cache

struct MonthSeries {
    /// The one predecessor row (if any), then the month's rows. The predecessor supplies the previous reading for the
    /// first increment and, with `seed_peak`, the rain peak carried into the month.
    rows: Vec<SeriesRow>,
    /// Increments over `rows` with the counter chosen from the month's own rows (`None`: no usable counter).
    incs: Option<Vec<Inc>>,
    /// Fingerprint (row count, newest `received_at`) over `[from, to]` taken before loading the rows.
    fingerprint: (i64, Option<DateTime<Utc>>),
    span: (DateTime<Utc>, Option<DateTime<Utc>>),
}

struct MonthKey {
    station: Uuid,
    first: NaiveDate,
}

fn month_bounds(off: i32, first: NaiveDate) -> (DateTime<Utc>, DateTime<Utc>) {
    let next = if first.month() == 12 {
        NaiveDate::from_ymd_opt(first.year() + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1)
    }
    .expect("valid month");
    (
        standard_day_bounds(off, first).0,
        standard_day_bounds(off, next).0,
    )
}

async fn fingerprint(
    conn: &mut PgConnection,
    station: Uuid,
    span: (DateTime<Utc>, Option<DateTime<Utc>>),
) -> Result<(i64, Option<DateTime<Utc>>), sqlx::Error> {
    let r = sqlx::query(
        "SELECT count(*)::bigint AS n, max(received_at) AS newest FROM app.observations \
         WHERE station_id = $1 AND ts_utc >= $2 AND ($3::timestamptz IS NULL OR ts_utc <= $3)",
    )
    .bind(station)
    .bind(span.0)
    .bind(span.1)
    .fetch_one(conn)
    .await?;
    Ok((r.try_get("n")?, r.try_get("newest")?))
}

fn series_row(r: &sqlx::postgres::PgRow) -> Result<SeriesRow, sqlx::Error> {
    Ok(SeriesRow {
        ts: r.try_get("ts_utc")?,
        temp_f: r.try_get("temp_f")?,
        counters: Counters {
            total: r.try_get("total")?,
            yearly: r.try_get("yearly")?,
            daily: r.try_get("daily")?,
        },
    })
}

/// How far before the predecessor the peak seed looks when no near-zero reading (a reset or a fresh counter) is
/// found. Bounds the scan; the counter is non-decreasing apart from ignored dips, so its peak is almost always in
/// the most recent readings.
const SEED_MAX_LOOKBACK_DAYS: i64 = 366;

/// The high-water mark of `counter` before a window: the maximum value from the last reading below
/// `RESET_NEAR_ZERO_IN` (the last reset, or the counter's start; the peak restarts there) up to and including the
/// predecessor reading at `pred_ts`, looking back at most [`SEED_MAX_LOOKBACK_DAYS`]. One bounded index range scan
/// per month load. `None` when there is no such reading with a value.
async fn seed_peak(
    conn: &mut PgConnection,
    station: Uuid,
    counter: Counter,
    pred_ts: DateTime<Utc>,
) -> Result<Option<f64>, sqlx::Error> {
    // Column names come from the closed `Counter` enum, never from input.
    let col = match counter {
        Counter::Total => "rain_total_in",
        Counter::Yearly => "rain_yearly_in",
        Counter::Daily => "rain_daily_in",
    };
    let sql = format!(
        "SELECT max({col})::float8 FROM app.observations WHERE station_id = $1 AND ts_utc <= $2 \
           AND ts_utc >= GREATEST($2 - make_interval(days => $4), COALESCE( \
             (SELECT max(ts_utc) FROM app.observations WHERE station_id = $1 AND ts_utc <= $2 \
                AND ts_utc >= $2 - make_interval(days => $4) AND {col} < $3::float8), '-infinity'::timestamptz))"
    );
    sqlx::query_scalar(&sql)
        .bind(station)
        .bind(pred_ts)
        .bind(RESET_NEAR_ZERO_IN)
        .bind(SEED_MAX_LOOKBACK_DAYS as i32)
        .fetch_one(&mut *conn)
        .await
}

const SERIES_COLS: &str ="ts_utc, temp_f, rain_total_in::float8 AS total, rain_yearly_in::float8 AS yearly, rain_daily_in::float8 AS daily";

async fn load_month(
    conn: &mut PgConnection,
    key: &MonthKey,
    off: i32,
) -> Result<MonthSeries, sqlx::Error> {
    let (start, end) = month_bounds(off, key.first);
    // One predecessor row: the first difference of the month needs the previous reading.
    let pred_rows = sqlx::query(&format!(
        "SELECT {SERIES_COLS} FROM app.observations WHERE station_id = $1 AND ts_utc < $2 ORDER BY ts_utc DESC LIMIT 1"
    ))
    .bind(key.station)
    .bind(start)
    .fetch_all(&mut *conn)
    .await?;
    let pred: Vec<SeriesRow> = pred_rows.iter().map(series_row).collect::<Result<_, _>>()?;
    let lo = pred.first().map_or(start, |p| p.ts);
    // The month's result depends only on the predecessor and the month's own rows (the bound at `end` is inclusive,
    // which only ever over-detects a change).
    let span = (lo, Some(end));
    let fp = fingerprint(&mut *conn, key.station, span).await?;
    let month_rows = sqlx::query(&format!(
        "SELECT {SERIES_COLS} FROM app.observations WHERE station_id = $1 AND ts_utc >= $2 AND ts_utc < $3 ORDER BY ts_utc"
    ))
    .bind(key.station)
    .bind(start)
    .bind(end)
    .fetch_all(&mut *conn)
    .await?;
    let month: Vec<SeriesRow> = month_rows
        .iter()
        .map(series_row)
        .collect::<Result<_, _>>()?;

    let counters: Vec<Counters> = month.iter().map(|r| r.counters).collect();
    let counter = choose_counter(&counters);
    // The peak before the month: the high-water mark of the chosen counter up to and including the predecessor.
    let seed = match (counter, pred.first()) {
        (Some(c), Some(p)) => seed_peak(&mut *conn, key.station, c, p.ts).await?,
        _ => None,
    };
    let mut rows = Vec::with_capacity(month.len() + 1);
    rows.extend(pred);
    rows.extend(month);
    let incs = counter.map(|c| {
        let rr: Vec<RainRow> = rows
            .iter()
            .map(|r| RainRow {
                ts: r.ts,
                counter: r.counters.get(c),
            })
            .collect();
        increments_seeded(seed, &rr)
    });
    Ok(MonthSeries {
        rows,
        incs,
        fingerprint: fp,
        span,
    })
}

// ---------------------------------------------------------------------------------------------------------------
// Persistence

fn retryable(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if matches!(d.code().as_deref(), Some("40P01" | "40001")))
}

struct DayOutcome {
    result: Option<DayResult>,
    changed: bool,
}

async fn process_day_once(
    pool: &PgPool,
    cache: &mut Option<(MonthKey, MonthSeries)>,
    station: Uuid,
    off: i32,
    date: NaiveDate,
    now: DateTime<Utc>,
) -> Result<DayOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;
    // 1. Take the queue row's lock (and remove it). An ingest transaction enqueueing the same day waits for our
    //    commit and then re-inserts it, so a change committed after our read is never lost.
    sqlx::query("DELETE FROM app.rollup_dirty_days WHERE station_id = $1 AND local_date = $2")
        .bind(station)
        .bind(date)
        .execute(&mut *tx)
        .await?;
    let first = date.with_day(1).expect("first of month");
    let reuse = match cache.as_ref() {
        Some((k, m)) if k.station == station && k.first == first => {
            fingerprint(&mut tx, station, m.span).await? == m.fingerprint
        }
        _ => false,
    };
    if !reuse {
        let key = MonthKey { station, first };
        let m = load_month(&mut tx, &key, off).await?;
        *cache = Some((key, m));
    }
    let (_, month) = cache.as_ref().expect("cache filled");
    let (day_start, day_end) = standard_day_bounds(off, date);
    let Some(res) = compute_day(&month.rows, month.incs.as_deref(), day_start, day_end, now) else {
        tx.commit().await?;
        return Ok(DayOutcome {
            result: None,
            changed: false,
        });
    };
    // 2. rain_inc_in for the rows of this day only (the predecessor row is read, never rewritten).
    let lo = month.rows.partition_point(|r| r.ts < day_start);
    let hi = month.rows.partition_point(|r| r.ts < day_end);
    let ts: Vec<DateTime<Utc>> = month.rows[lo..hi].iter().map(|r| r.ts).collect();
    let inc: Vec<Option<f64>> = match &month.incs {
        Some(v) => v[lo..hi].iter().map(|i| i.inc).collect(),
        None => vec![None; hi - lo],
    };
    sqlx::query(
        "UPDATE app.observations o SET rain_inc_in = round(v.inc::numeric, 3) \
         FROM unnest($2::timestamptz[], $3::float8[]) AS v(ts, inc) \
         WHERE o.station_id = $1 AND o.ts_utc = v.ts AND o.rain_inc_in IS DISTINCT FROM round(v.inc::numeric, 3)",
    )
    .bind(station)
    .bind(&ts)
    .bind(&inc)
    .execute(&mut *tx)
    .await?;
    // 3. the rollup row (reconciliation is not implemented: status stays 'unchecked', the AWN columns NULL).
    let changed = sqlx::query(
        "INSERT INTO app.daily_rollups (station_id, local_date, rain_in, tmax_f, tmin_f, obs_count, is_complete, \
           rain_resets, rain_reconcile_status, computed_at) \
         VALUES ($1, $2, round($3::float8::numeric, 3), $4, $5, $6, $7, $8, 'unchecked', now()) \
         ON CONFLICT (station_id, local_date) DO UPDATE SET rain_in = EXCLUDED.rain_in, tmax_f = EXCLUDED.tmax_f, \
           tmin_f = EXCLUDED.tmin_f, obs_count = EXCLUDED.obs_count, is_complete = EXCLUDED.is_complete, \
           rain_resets = EXCLUDED.rain_resets, \
           awn_daily_rain_in = NULL, rain_reconcile_status = 'unchecked', rain_reconcile_diff_in = NULL, computed_at = now() \
         WHERE (app.daily_rollups.rain_in, app.daily_rollups.tmax_f, app.daily_rollups.tmin_f, app.daily_rollups.obs_count, \
                app.daily_rollups.is_complete, app.daily_rollups.rain_resets, \
                app.daily_rollups.awn_daily_rain_in, app.daily_rollups.rain_reconcile_status, app.daily_rollups.rain_reconcile_diff_in) \
               IS DISTINCT FROM (EXCLUDED.rain_in, EXCLUDED.tmax_f, EXCLUDED.tmin_f, EXCLUDED.obs_count, EXCLUDED.is_complete, \
                EXCLUDED.rain_resets, NULL::numeric, 'unchecked', NULL::numeric)",
    )
    .bind(station)
    .bind(date)
    .bind(res.rain_in)
    .bind(res.tmax_f)
    .bind(res.tmin_f)
    .bind(res.obs_count as i16)
    .bind(res.is_complete)
    .bind(res.resets as i16)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        > 0;
    tx.commit().await?;
    Ok(DayOutcome {
        result: Some(res),
        changed,
    })
}

async fn process_day(
    pool: &PgPool,
    cache: &mut Option<(MonthKey, MonthSeries)>,
    station: Uuid,
    off: i32,
    date: NaiveDate,
    now: DateTime<Utc>,
) -> Result<DayOutcome, sqlx::Error> {
    let mut attempt = 0;
    loop {
        match process_day_once(pool, cache, station, off, date, now).await {
            Err(e) if retryable(&e) && attempt < RETRY_DELAYS_MS.len() => {
                tracing::warn!(
                    attempt = attempt + 1,
                    "retrying rollup day after deadlock or serialization failure"
                );
                *cache = None;
                tokio::time::sleep(StdDuration::from_millis(RETRY_DELAYS_MS[attempt])).await;
                attempt += 1;
            }
            other => return other,
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Queue

async fn enqueue_range(
    pool: &PgPool,
    station: Option<Uuid>,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query(
        "INSERT INTO app.rollup_dirty_days (station_id, local_date) \
         SELECT s.id, d::date FROM app.stations s CROSS JOIN generate_series($1::date, $2::date, interval '1 day') AS d \
         WHERE ($3::uuid IS NULL OR s.id = $3) ORDER BY s.id, d \
         ON CONFLICT (station_id, local_date) DO UPDATE SET local_date = EXCLUDED.local_date",
    )
    .bind(from)
    .bind(to)
    .bind(station)
    .execute(pool)
    .await?
    .rows_affected())
}

/// Self-heal: days with observations but no rollup, and unfinished rollups of the last 60 days whose day is over.
async fn self_heal(
    pool: &PgPool,
    station: Option<Uuid>,
    now: DateTime<Utc>,
) -> Result<u64, sqlx::Error> {
    let a = sqlx::query(
        "INSERT INTO app.rollup_dirty_days (station_id, local_date) \
         SELECT DISTINCT o.station_id, app.lst_date(o.ts_utc, s.std_utc_offset_minutes) AS d \
         FROM app.observations o JOIN app.stations s ON s.id = o.station_id \
         LEFT JOIN app.daily_rollups r ON r.station_id = o.station_id \
              AND r.local_date = app.lst_date(o.ts_utc, s.std_utc_offset_minutes) \
         WHERE r.station_id IS NULL AND ($1::uuid IS NULL OR s.id = $1) \
         ORDER BY 1, 2 \
         ON CONFLICT (station_id, local_date) DO UPDATE SET local_date = EXCLUDED.local_date",
    )
    .bind(station)
    .execute(pool)
    .await?
    .rows_affected();
    let b = sqlx::query(
        "INSERT INTO app.rollup_dirty_days (station_id, local_date) \
         SELECT r.station_id, r.local_date FROM app.daily_rollups r JOIN app.stations s ON s.id = r.station_id \
         WHERE NOT r.is_complete AND r.local_date < app.lst_date($2, s.std_utc_offset_minutes) \
           AND r.local_date >= app.lst_date($2, s.std_utc_offset_minutes) - $3::int \
           AND ($1::uuid IS NULL OR s.id = $1) \
         ORDER BY 1, 2 \
         ON CONFLICT (station_id, local_date) DO UPDATE SET local_date = EXCLUDED.local_date",
    )
    .bind(station)
    .bind(now)
    .bind(SELF_HEAL_DAYS as i32)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(a + b)
}

struct DirtyRow {
    station: Uuid,
    off: i32,
    date: NaiveDate,
}

async fn read_queue(pool: &PgPool, station: Option<Uuid>) -> Result<Vec<DirtyRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT d.station_id, d.local_date, s.std_utc_offset_minutes::int AS off \
         FROM app.rollup_dirty_days d JOIN app.stations s ON s.id = d.station_id \
         WHERE ($1::uuid IS NULL OR d.station_id = $1) ORDER BY d.station_id, d.local_date",
    )
    .bind(station)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|r| {
            Ok(DirtyRow {
                station: r.try_get("station_id")?,
                off: r.try_get("off")?,
                date: r.try_get("local_date")?,
            })
        })
        .collect()
}

/// Standard dates with observations inside the month.
async fn days_with_observations(
    pool: &PgPool,
    station: Uuid,
    off: i32,
    first: NaiveDate,
) -> Result<Vec<NaiveDate>, sqlx::Error> {
    let (start, end) = month_bounds(off, first);
    sqlx::query_scalar(
        "SELECT DISTINCT app.lst_date(ts_utc, $4::int) AS d FROM app.observations \
         WHERE station_id = $1 AND ts_utc >= $2 AND ts_utc < $3 ORDER BY d",
    )
    .bind(station)
    .bind(start)
    .bind(end)
    .bind(off)
    .fetch_all(pool)
    .await
}

// ---------------------------------------------------------------------------------------------------------------
// Job

#[async_trait]
impl Job for DailyRollup {
    fn name(&self) -> &'static str {
        "daily_rollup"
    }

    fn ping_suffix(&self) -> Option<&'static str> {
        Some("DAILY_ROLLUP")
    }

    async fn run(&self, ctx: &mut JobContext) -> Result<Value, JobError> {
        let mut d = RollupDetail::default();
        let snap = |d: &RollupDetail| serde_json::to_value(d).unwrap_or_else(|_| json!({}));
        let fail = |d: &RollupDetail, e: sqlx::Error| JobError::failed(db_text(e), snap(d));
        let pool = ctx.env.pool.clone();
        let now = ctx.env.clock.now();
        let forced = ctx.flags.from.zip(ctx.flags.to);

        // Daily housekeeping: the first run after 03:00 UTC that has not done it today.
        let today = now.date_naive().to_string();
        let mut housekeeping = false;
        if forced.is_none() && now.hour() >= 3 {
            let last: Option<String> = sqlx::query_scalar(
                "SELECT max(detail->>'housekeeping_done_on') FROM app.job_runs WHERE job = 'daily_rollup' AND status = 'success'",
            )
            .fetch_one(&pool)
            .await
            .map_err(|e| fail(&d, e))?;
            housekeeping = last.as_deref().is_none_or(|l| l < today.as_str());
        }

        if let Some((from, to)) = forced {
            enqueue_range(&pool, ctx.station, from, to)
                .await
                .map_err(|e| fail(&d, e))?;
        }
        if ctx.flags.self_heal || housekeeping {
            self_heal(&pool, ctx.station, now)
                .await
                .map_err(|e| fail(&d, e))?;
            d.self_healed = true;
        }

        let queue = read_queue(&pool, ctx.station)
            .await
            .map_err(|e| fail(&d, e))?;
        // station -> (offset, month -> dirty days)
        let mut plan: BTreeMap<Uuid, (i32, BTreeMap<NaiveDate, BTreeSet<NaiveDate>>)> =
            BTreeMap::new();
        for q in queue {
            let first = q.date.with_day(1).expect("first of month");
            plan.entry(q.station)
                .or_insert_with(|| (q.off, BTreeMap::new()))
                .1
                .entry(first)
                .or_default()
                .insert(q.date);
        }
        d.stations = plan.len() as u32;

        for (station, (off, months)) in plan {
            let mut cache: Option<(MonthKey, MonthSeries)> = None;
            for (first, dirty) in months {
                // Any dirty day schedules every day of its month that has observations.
                let mut days: BTreeSet<NaiveDate> =
                    days_with_observations(&pool, station, off, first)
                        .await
                        .map_err(|e| fail(&d, e))?
                        .into_iter()
                        .collect();
                days.extend(dirty);
                for date in days {
                    if let Err(i) = ctx.checkpoint().await {
                        return Err(JobError::interrupted(i, snap(&d)));
                    }
                    let out = process_day(&pool, &mut cache, station, off, date, now)
                        .await
                        .map_err(|e| fail(&d, e))?;
                    if let Some(r) = out.result {
                        d.days_processed += 1;
                        d.days_changed += u32::from(out.changed);
                        d.resets += r.resets;
                        d.temp_outliers += r.temp_outliers;
                    }
                }
            }
        }

        if housekeeping && forced.is_none() {
            let cutoff = now - Duration::days(JOB_RUNS_KEEP_DAYS);
            d.job_runs_deleted = sqlx::query("DELETE FROM app.job_runs WHERE started_at < $1")
                .bind(cutoff)
                .execute(&pool)
                .await
                .map_err(|e| fail(&d, e))?
                .rows_affected();
            d.housekeeping_done_on = Some(today);
        }
        Ok(snap(&d))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use famweather_core::rain::increments;

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 2, 7, 0, 0).unwrap() // MST midnight
    }

    fn readings(step_min: i64, n: i64, temp: impl Fn(i64) -> Option<f32>) -> Vec<SeriesRow> {
        (0..n)
            .map(|i| SeriesRow {
                ts: t0() + Duration::minutes(step_min * i),
                temp_f: temp(i),
                counters: Counters::default(),
            })
            .collect()
    }

    fn day(rows: &[SeriesRow], now_offset_days: i64) -> DayResult {
        let (s, e) = (t0(), t0() + Duration::hours(24));
        compute_day(rows, None, s, e, t0() + Duration::days(now_offset_days)).unwrap()
    }

    #[test]
    fn full_days_are_complete_at_both_intervals() {
        let five = day(&readings(5, 288, |i| Some(50.0 + (i % 10) as f32)), 2);
        assert!(five.is_complete && five.obs_count == 288 && five.interval_min == 5);
        assert_eq!((five.tmax_f, five.tmin_f), (Some(59.0), Some(50.0)));
        let thirty = day(&readings(30, 48, |_| Some(60.0)), 2);
        assert!(thirty.is_complete && thirty.obs_count == 48 && thirty.interval_min == 30);
    }

    #[test]
    fn coverage_thresholds_are_interval_aware() {
        // 30 min data, 20 readings: 0.42 -> no temperatures, incomplete
        let low = day(&readings(30, 20, |_| Some(60.0)), 2);
        assert!(!low.is_complete && low.tmax_f.is_none() && low.tmin_f.is_none());
        // 40 readings: 0.83 -> computed but incomplete
        let mid = day(&readings(30, 40, |_| Some(60.0)), 2);
        assert!(!mid.is_complete && mid.tmax_f == Some(60.0));
        // 5 min data, 130 readings: 0.45 -> none; 150 readings: 0.52 -> values
        assert!(day(&readings(5, 130, |_| Some(60.0)), 2).tmax_f.is_none());
        assert!(day(&readings(5, 150, |_| Some(60.0)), 2).tmax_f.is_some());
    }

    #[test]
    fn the_day_containing_now_is_never_complete() {
        let r = day(&readings(5, 288, |_| Some(60.0)), 0);
        assert!(!r.is_complete && r.obs_count == 288);
    }

    #[test]
    fn outliers_are_ignored_and_counted() {
        let rows = readings(5, 288, |i| {
            Some(match i {
                0 => 200.0,
                1 => -80.0,
                _ => 55.0,
            })
        });
        let r = day(&rows, 2);
        assert_eq!(
            (r.tmax_f, r.tmin_f, r.temp_outliers),
            (Some(55.0), Some(55.0), 2)
        );
        assert_eq!(
            r.obs_count, 288,
            "obs_count counts every reading with a temperature"
        );
    }

    #[test]
    fn readings_without_temperature_do_not_count() {
        let r = day(&readings(5, 288, |i| (i % 2 == 0).then_some(60.0)), 2);
        assert_eq!(r.obs_count, 144);
        assert!(!r.is_complete);
    }

    #[test]
    fn median_gap_picks_the_interval() {
        let ts = |step: i64, n: i64| -> Vec<DateTime<Utc>> {
            (0..n).map(|i| t0() + Duration::minutes(step * i)).collect()
        };
        assert_eq!(observed_interval_minutes(&ts(5, 10)), 5);
        assert_eq!(observed_interval_minutes(&ts(30, 10)), 30);
        assert_eq!(observed_interval_minutes(&ts(5, 1)), 30);
        // a few long gaps do not change the median
        let mut v = ts(5, 100);
        v.push(v[99] + Duration::hours(3));
        assert_eq!(observed_interval_minutes(&v), 5);
    }

    #[test]
    fn rain_sums_only_the_days_rows_and_counts_resets() {
        let mut rows = readings(5, 288, |_| Some(60.0));
        // counter: 1.00 for the first 100 readings, then 1.10
        for (i, r) in rows.iter_mut().enumerate() {
            r.counters.total = Some(if i < 100 { 1.0 } else { 1.1 });
        }
        let rr: Vec<RainRow> = rows
            .iter()
            .map(|r| RainRow {
                ts: r.ts,
                counter: r.counters.total,
            })
            .collect();
        let incs = increments(&rr);
        let (s, e) = (t0(), t0() + Duration::hours(24));
        let r = compute_day(&rows, Some(&incs), s, e, t0() + Duration::days(3)).unwrap();
        assert_eq!(r.rain_in, Some(0.1));
        assert_eq!(r.resets, 0);
        // no usable counter: NULL, not 0
        let none = compute_day(&rows, None, s, e, t0() + Duration::days(3)).unwrap();
        assert_eq!(none.rain_in, None);
    }

    #[test]
    fn month_bounds_follow_the_standard_offset() {
        let (s, e) = month_bounds(-420, NaiveDate::from_ymd_opt(2026, 3, 1).unwrap());
        assert_eq!(s, Utc.with_ymd_and_hms(2026, 3, 1, 7, 0, 0).unwrap());
        assert_eq!(e, Utc.with_ymd_and_hms(2026, 4, 1, 7, 0, 0).unwrap());
        let (_, e) = month_bounds(-420, NaiveDate::from_ymd_opt(2026, 12, 1).unwrap());
        assert_eq!(e, Utc.with_ymd_and_hms(2027, 1, 1, 7, 0, 0).unwrap());
    }
}
