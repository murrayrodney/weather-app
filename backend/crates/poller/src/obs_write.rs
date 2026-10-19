//! Observation upserts and dirty-day queue inserts (03 section 7.1), shared by `awn_ingest` and `awn_backfill`.
//!
//! One transaction per station per page: sorted rows, a de-duplicated page, an idempotent upsert that returns only
//! rows that were inserted or really changed, and the dirty days of those rows. Deadlocks and serialization
//! failures are retried a bounded number of times.

use std::collections::BTreeSet;
use std::future::Future;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use famweather_core::time::standard_date;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::mapping::MappedRow;

/// Every AWN-sourced column (04 section 5.6) in the order `push_row` binds them. `rain_inc_in` is never written here.
const COLUMNS: [&str; 23] = [
    "temp_f",
    "feels_like_f",
    "dew_point_f",
    "humidity_pct",
    "wind_speed_mph",
    "wind_gust_mph",
    "max_daily_gust_mph",
    "wind_dir_deg",
    "baro_rel_inhg",
    "baro_abs_inhg",
    "rain_rate_in_hr",
    "rain_event_in",
    "rain_daily_in",
    "rain_weekly_in",
    "rain_monthly_in",
    "rain_yearly_in",
    "rain_total_in",
    "rain_last_at",
    "solar_wm2",
    "uv_index",
    "temp_indoor_f",
    "humidity_indoor_pct",
    "battery_out",
];

/// Largest chunk of one INSERT statement.
const CHUNK: usize = 288;
const RETRY_DELAYS_MS: [u64; 3] = [50, 150, 450];

/// Runs `f`, retrying on SQLSTATE 40P01 (deadlock) and 40001 (serialization failure): three retries after the
/// first attempt, 50/150/450 ms apart.
pub async fn retry_tx<T, F, Fut>(mut f: F) -> Result<T, sqlx::Error>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, sqlx::Error>>,
{
    let mut attempt = 0;
    loop {
        match f().await {
            Err(sqlx::Error::Database(d))
                if matches!(d.code().as_deref(), Some("40P01" | "40001"))
                    && attempt < RETRY_DELAYS_MS.len() =>
            {
                tracing::warn!(
                    attempt = attempt + 1,
                    "retrying transaction after deadlock or serialization failure"
                );
                tokio::time::sleep(Duration::from_millis(RETRY_DELAYS_MS[attempt])).await;
                attempt += 1;
            }
            other => return other,
        }
    }
}

/// `ON CONFLICT` upsert statement text for `n` rows is built with a `QueryBuilder`; this is the tail.
fn conflict_clause(qb: &mut QueryBuilder<'_, Postgres>) {
    qb.push(" ON CONFLICT (station_id, ts_utc) DO UPDATE SET ");
    for (i, c) in COLUMNS.iter().enumerate() {
        if i > 0 {
            qb.push(", ");
        }
        qb.push(format!(
            "{c} = COALESCE(EXCLUDED.{c}, app.observations.{c})"
        ));
    }
    qb.push(", received_at = now() WHERE (");
    qb.push(
        COLUMNS
            .iter()
            .map(|c| format!("app.observations.{c}"))
            .collect::<Vec<_>>()
            .join(", "),
    );
    qb.push(") IS DISTINCT FROM (");
    qb.push(
        COLUMNS
            .iter()
            .map(|c| format!("COALESCE(EXCLUDED.{c}, app.observations.{c})"))
            .collect::<Vec<_>>()
            .join(", "),
    );
    qb.push(") RETURNING ts_utc, (xmax = 0) AS inserted");
}

/// The text of the upsert for a given row count is not interesting; this exposes the clause for a unit test.
pub fn upsert_sql_shape() -> String {
    let mut qb = QueryBuilder::<Postgres>::new("");
    conflict_clause(&mut qb);
    qb.sql().to_string()
}

/// Enqueues `days` for `station` in the caller's transaction, sorted by date. The no-op `DO UPDATE` takes the row
/// lock of an already-queued day (conventions, dirty days): a rollup `DELETE` of that day waits for this transaction
/// and, reading after it, sees the data this transaction wrote, so the day is never lost.
pub async fn enqueue_dirty_days(
    conn: &mut sqlx::PgConnection,
    station: Uuid,
    days: &[NaiveDate],
) -> Result<(), sqlx::Error> {
    if days.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO app.rollup_dirty_days (station_id, local_date) \
         SELECT $1, d FROM unnest($2::date[]) AS d ORDER BY d \
         ON CONFLICT (station_id, local_date) DO UPDATE SET local_date = EXCLUDED.local_date",
    )
    .bind(station)
    .bind(days)
    .execute(conn)
    .await?;
    Ok(())
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PageWrite {
    pub inserted: u32,
    pub updated: u32,
    /// Distinct dirty days enqueued by this page.
    pub dirty_days: u32,
}

/// De-duplicates by timestamp (the last occurrence wins; a duplicate key inside one INSERT is a Postgres error)
/// and sorts by `ts` so concurrent writers lock rows in the same order.
pub fn dedupe_sorted(rows: Vec<MappedRow>) -> Vec<MappedRow> {
    let mut by_ts: std::collections::BTreeMap<DateTime<Utc>, MappedRow> =
        std::collections::BTreeMap::new();
    for r in rows {
        by_ts.insert(r.ts, r);
    }
    by_ts.into_values().collect()
}

/// The standard dates a changed row dirties: its own day and, for rows shortly after a boundary, the previous day
/// (the 4 h look-back covers the console's wall-day window).
pub fn dirty_dates(off_minutes: i32, changed: &[DateTime<Utc>]) -> BTreeSet<NaiveDate> {
    let mut days = BTreeSet::new();
    for ts in changed {
        days.insert(standard_date(off_minutes, *ts));
        days.insert(standard_date(off_minutes, *ts - chrono::Duration::hours(4)));
    }
    days
}

/// Writes one page for one station in one transaction. `rows` must be non-empty and already mapped.
/// With `track_last_observed` the station's `last_observed_at` is advanced (never backwards) in the same transaction.
pub async fn write_page(
    pool: &PgPool,
    station: Uuid,
    off_minutes: i32,
    rows: Vec<MappedRow>,
    track_last_observed: bool,
) -> Result<PageWrite, sqlx::Error> {
    let rows = dedupe_sorted(rows);
    let Some(page_max) = rows.last().map(|r| r.ts) else {
        return Ok(PageWrite::default());
    };
    retry_tx(|| async {
        let mut tx = pool.begin().await?;
        let mut out = PageWrite::default();
        let mut changed: Vec<DateTime<Utc>> = Vec::new();
        for chunk in rows.chunks(CHUNK) {
            let mut qb = QueryBuilder::<Postgres>::new("INSERT INTO app.observations (station_id, ts_utc, ");
            qb.push(COLUMNS.join(", "));
            qb.push(") ");
            qb.push_values(chunk, |mut b, r| {
                b.push_bind(station)
                    .push_bind(r.ts)
                    .push_bind(r.temp_f)
                    .push_bind(r.feels_like_f)
                    .push_bind(r.dew_point_f)
                    .push_bind(r.humidity_pct)
                    .push_bind(r.wind_speed_mph)
                    .push_bind(r.wind_gust_mph)
                    .push_bind(r.max_daily_gust_mph)
                    .push_bind(r.wind_dir_deg)
                    .push_bind(r.baro_rel_inhg)
                    .push_bind(r.baro_abs_inhg)
                    .push_bind(r.rain_rate_in_hr)
                    .push_bind(r.rain_event_in)
                    .push_bind(r.rain_daily_in)
                    .push_bind(r.rain_weekly_in)
                    .push_bind(r.rain_monthly_in)
                    .push_bind(r.rain_yearly_in)
                    .push_bind(r.rain_total_in)
                    .push_bind(r.rain_last_at)
                    .push_bind(r.solar_wm2)
                    .push_bind(r.uv_index)
                    .push_bind(r.temp_indoor_f)
                    .push_bind(r.humidity_indoor_pct)
                    .push_bind(r.battery_out);
            });
            conflict_clause(&mut qb);
            for row in qb.build().fetch_all(&mut *tx).await? {
                let ts: DateTime<Utc> = row.try_get("ts_utc")?;
                let inserted: bool = row.try_get("inserted")?;
                if inserted {
                    out.inserted += 1;
                } else {
                    out.updated += 1;
                }
                changed.push(ts);
            }
        }
        let days: Vec<NaiveDate> = dirty_dates(off_minutes, &changed).into_iter().collect();
        out.dirty_days = days.len() as u32;
        enqueue_dirty_days(&mut tx, station, &days).await?;
        if track_last_observed {
            sqlx::query(
                "UPDATE app.stations SET last_observed_at = GREATEST(COALESCE(last_observed_at, $2), $2) WHERE id = $1",
            )
            .bind(station)
            .bind(page_max)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(out)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn row(secs: i64, temp: f32) -> MappedRow {
        MappedRow {
            ts: Utc.timestamp_opt(secs, 0).unwrap(),
            temp_f: Some(temp),
            ..Default::default()
        }
    }

    #[test]
    fn dedupe_keeps_the_last_and_sorts() {
        let out = dedupe_sorted(vec![row(300, 1.0), row(100, 2.0), row(300, 3.0)]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].ts.timestamp(), 100);
        assert_eq!(out[1].temp_f, Some(3.0));
    }

    #[test]
    fn dirty_dates_include_the_previous_day_within_four_hours_of_midnight() {
        // MST: standard midnight is 07:00Z.
        let t = |h, m| Utc.with_ymd_and_hms(2026, 6, 2, h, m, 0).unwrap();
        let d = |day| NaiveDate::from_ymd_opt(2026, 6, day).unwrap();
        let mid = dirty_dates(-420, &[t(10, 0)]); // 03:00 local: within 4 h of midnight
        assert_eq!(mid.into_iter().collect::<Vec<_>>(), vec![d(1), d(2)]);
        let noon = dirty_dates(-420, &[t(19, 0)]); // 12:00 local
        assert_eq!(noon.into_iter().collect::<Vec<_>>(), vec![d(2)]);
    }

    #[test]
    fn upsert_text_never_writes_rain_inc_and_guards_with_is_distinct_from() {
        let s = upsert_sql_shape();
        assert!(!s.contains("rain_inc_in"));
        assert!(s.contains("IS DISTINCT FROM"));
        assert!(s.contains("received_at = now()"));
        assert!(s.contains("RETURNING ts_utc, (xmax = 0) AS inserted"));
        for c in COLUMNS {
            assert!(
                s.contains(&format!(
                    "{c} = COALESCE(EXCLUDED.{c}, app.observations.{c})"
                )),
                "{c}"
            );
        }
    }
}
