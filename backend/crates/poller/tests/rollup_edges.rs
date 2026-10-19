//! Regression tests from the step 3 review: resets across day and month boundaries, the ingest-first interleaving,
//! SQL/Rust day agreement, the once-per-process offset warning, backfill credential bookkeeping, bounded run ids.

mod common;

use std::time::Duration;

use chrono::{DateTime, Duration as Dur, NaiveDate, TimeZone, Utc};
use common::{capture_logs, Builder, World, POLLER_KEY};
use poller_lib::job::{JobFlags, RunOutcome, RunStatus};
use poller_lib::mapping::MappedRow;
use poller_lib::obs_write::{enqueue_dirty_days, write_page};
use poller_lib::scheduler::warn_offset_mismatches;
use poller_lib::{run_named, JobName};
use sqlx::PgPool;
use uuid::Uuid;

async fn run(w: &World, job: JobName, station: Option<Uuid>) -> RunOutcome {
    run_named(&w.env, job, station, JobFlags::default())
        .await
        .unwrap()
}

fn status(o: &RunOutcome) -> RunStatus {
    match o {
        RunOutcome::Finished { status, .. } => *status,
        other => panic!("not finished: {other:?}"),
    }
}

async fn station(w: &World) -> Uuid {
    let owner = w.add_owner("Rodney").await;
    let cred = w.add_credential(owner, POLLER_KEY).await;
    w.add_station(owner, cred, "AA:BB:CC:00:00:01").await
}

fn reading(ts: DateTime<Utc>, total: f32) -> MappedRow {
    MappedRow {
        ts,
        temp_f: Some(60.0),
        rain_total_in: Some(total),
        ..Default::default()
    }
}

/// A counter reset to 0.00 at the reading `reset_k` steps (5 minutes) after `boundary`, then 0.04 in of rain from
/// `k = 3` on. Returns nothing; asserts the two days' rain and resets.
async fn straddle(db: PgPool, boundary: DateTime<Utc>, reset_k: i64) {
    let w = Builder::new(db)
        .clock_at(boundary + Dur::days(10))
        .build()
        .await;
    let st = station(&w).await;
    let rows: Vec<MappedRow> = (-6..=6)
        .map(|k| {
            let total = if k < reset_k {
                10.00
            } else if k < 3 {
                0.00
            } else {
                0.04
            };
            reading(boundary + Dur::minutes(5 * k), total)
        })
        .collect();
    write_page(&w.env.pool, st, -420, rows, false)
        .await
        .unwrap();
    let out = run(&w, JobName::DailyRollup, None).await;
    assert_eq!(status(&out), RunStatus::Success, "{out:?}");
    let days: Vec<(NaiveDate, Option<f64>, i16)> = sqlx::query_as(
        "SELECT local_date, rain_in::float8, rain_resets FROM app.daily_rollups WHERE station_id = $1 ORDER BY 1",
    )
    .bind(st)
    .fetch_all(&w.db)
    .await
    .unwrap();
    assert_eq!(days.len(), 2, "{days:?}");
    // The drop 10.00 -> 0.00 is never rain; the reset happens in the day that holds the 0.00 reading.
    let (before, after) = if reset_k < 0 { (1, 0) } else { (0, 1) };
    assert_eq!((days[0].1, days[0].2), (Some(0.0), before), "{days:?}");
    assert_eq!((days[1].1, days[1].2), (Some(0.04), after), "{days:?}");
    let incs: Vec<Option<f64>> = sqlx::query_scalar(
        "SELECT rain_inc_in::float8 FROM app.observations WHERE station_id = $1 ORDER BY ts_utc",
    )
    .bind(st)
    .fetch_all(&w.db)
    .await
    .unwrap();
    assert_eq!(
        incs.iter().flatten().filter(|i| **i > 0.0).count(),
        1,
        "{incs:?}"
    );
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn a_reset_on_the_first_reading_of_a_month_counts_from_zero(db: PgPool) {
    // 2026-06-01 00:00 local standard time (MST) is 07:00Z: the boundary between May and June.
    straddle(db, Utc.with_ymd_and_hms(2026, 6, 1, 7, 0, 0).unwrap(), 0).await;
}

#[sqlx::test(migrations = false)]
async fn a_reset_on_the_last_reading_of_a_month_counts_from_zero(db: PgPool) {
    straddle(db, Utc.with_ymd_and_hms(2026, 6, 1, 7, 0, 0).unwrap(), -1).await;
}

#[sqlx::test(migrations = false)]
async fn a_reset_on_the_first_reading_of_a_day_counts_from_zero(db: PgPool) {
    straddle(db, Utc.with_ymd_and_hms(2026, 6, 10, 7, 0, 0).unwrap(), 0).await;
}

#[sqlx::test(migrations = false)]
async fn a_reset_on_the_last_reading_of_a_day_counts_from_zero(db: PgPool) {
    straddle(db, Utc.with_ymd_and_hms(2026, 6, 10, 7, 0, 0).unwrap(), -1).await;
}

#[sqlx::test(migrations = false)]
async fn each_month_processed_alone_uses_the_one_predecessor_row_across_the_boundary(db: PgPool) {
    // Process each month on its own (only that month queued): the first June difference needs the last May row.
    let boundary = Utc.with_ymd_and_hms(2026, 6, 1, 7, 0, 0).unwrap();
    let w = Builder::new(db)
        .clock_at(boundary + Dur::days(10))
        .build()
        .await;
    let st = station(&w).await;
    let rows: Vec<MappedRow> = (-8..=8)
        .map(|k| {
            // plain counter 10.00 in May; reset to 0.00 on the first June reading; real rain 0.04 at k = 5
            let total = match k {
                ..=-1 => 10.00,
                5.. => 0.04,
                _ => 0.00,
            };
            reading(boundary + Dur::minutes(5 * k), total)
        })
        .collect();
    write_page(&w.env.pool, st, -420, rows, false)
        .await
        .unwrap();
    sqlx::query("DELETE FROM app.rollup_dirty_days")
        .execute(&w.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO app.rollup_dirty_days VALUES ($1, '2026-05-31')")
        .bind(st)
        .execute(&w.db)
        .await
        .unwrap();
    run(&w, JobName::DailyRollup, None).await;
    let may: Vec<Option<f64>> = sqlx::query_scalar(
        "SELECT rain_inc_in::float8 FROM app.observations WHERE station_id = $1 AND ts_utc < $2 ORDER BY ts_utc",
    )
    .bind(st)
    .bind(boundary)
    .fetch_all(&w.db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO app.rollup_dirty_days VALUES ($1, '2026-06-01')")
        .bind(st)
        .execute(&w.db)
        .await
        .unwrap();
    run(&w, JobName::DailyRollup, None).await;
    let june: Vec<Option<f64>> = sqlx::query_scalar(
        "SELECT rain_inc_in::float8 FROM app.observations WHERE station_id = $1 AND ts_utc >= $2 ORDER BY ts_utc",
    )
    .bind(st)
    .bind(boundary)
    .fetch_all(&w.db)
    .await
    .unwrap();
    // The May-side rows were written by the May run, the June-side by the June run (which read the last May row
    // as its predecessor, so the reset on the first June reading is seen): one consistent series, exactly 0.04 in.
    let june_resets: i16 = sqlx::query_scalar(
        "SELECT rain_resets FROM app.daily_rollups WHERE station_id = $1 AND local_date = '2026-06-01'",
    )
    .bind(st)
    .fetch_one(&w.db)
    .await
    .unwrap();
    assert_eq!(june_resets, 1);
    let all: Vec<f64> = may
        .iter()
        .chain(june.iter())
        .skip(1)
        .map(|i| i.unwrap())
        .collect();
    assert!((all.iter().sum::<f64>() - 0.04).abs() < 1e-9, "{all:?}");
    assert_eq!(all.iter().filter(|x| **x > 0.0).count(), 1, "{all:?}");
    // And a re-run of May after June changes nothing on the May rows.
    sqlx::query("INSERT INTO app.rollup_dirty_days VALUES ($1, '2026-05-31')")
        .bind(st)
        .execute(&w.db)
        .await
        .unwrap();
    run(&w, JobName::DailyRollup, None).await;
    let may_again: Vec<Option<f64>> = sqlx::query_scalar(
        "SELECT rain_inc_in::float8 FROM app.observations WHERE station_id = $1 AND ts_utc < $2 ORDER BY ts_utc",
    )
    .bind(st)
    .bind(boundary)
    .fetch_all(&w.db)
    .await
    .unwrap();
    assert_eq!(may, may_again);
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn ingest_enqueues_first_then_the_rollup_waits_and_sees_the_new_rows(db: PgPool) {
    let w = Builder::new(db)
        .clock_at(Utc.with_ymd_and_hms(2026, 6, 10, 12, 0, 0).unwrap())
        .build()
        .await;
    let st = station(&w).await;
    let day = NaiveDate::from_ymd_opt(2026, 6, 2).unwrap();
    // The day is already queued (an earlier ingest).
    let t1 = Utc.with_ymd_and_hms(2026, 6, 2, 19, 0, 0).unwrap();
    write_page(&w.env.pool, st, -420, vec![reading(t1, 1.0)], false)
        .await
        .unwrap();

    // A second ingest transaction: new reading, then the enqueue (row lock on the queued day), still uncommitted.
    let mut ingest = w.env.pool.begin().await.unwrap();
    sqlx::query("INSERT INTO app.observations (station_id, ts_utc, temp_f) VALUES ($1, $2, 61)")
        .bind(st)
        .bind(t1 + Dur::minutes(5))
        .execute(&mut *ingest)
        .await
        .unwrap();
    enqueue_dirty_days(&mut ingest, st, &[day]).await.unwrap();

    // The rollup starts meanwhile. Its DELETE of the queued day must wait for the ingest commit.
    let env = w.env.clone();
    let rollup = tokio::spawn(async move {
        run_named(&env, JobName::DailyRollup, None, JobFlags::default())
            .await
            .unwrap()
    });
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(
        !rollup.is_finished(),
        "without the row lock of ON CONFLICT DO UPDATE the rollup would already have deleted the day and finished"
    );
    ingest.commit().await.unwrap();
    let out = tokio::time::timeout(Duration::from_secs(15), rollup)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status(&out), RunStatus::Success);
    // It deleted the queue row only after the commit, so its computation includes the second reading.
    let obs: i16 = sqlx::query_scalar(
        "SELECT obs_count FROM app.daily_rollups WHERE station_id = $1 AND local_date = $2",
    )
    .bind(st)
    .bind(day)
    .fetch_one(&w.db)
    .await
    .unwrap();
    assert_eq!(obs, 2);
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn standard_date_in_rust_equals_sql_lst_date(db: PgPool) {
    let w = Builder::new(db).build().await;
    let offsets = [-720, -420, -360, -90, 0, 345, 600, 840];
    // Every 17 minutes across a year that contains both DST changes of the northern hemisphere, plus the edges.
    let start = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let mut instants: Vec<DateTime<Utc>> = (0..(365 * 24 * 60 / 17))
        .map(|i| start + Dur::minutes(17 * i))
        .collect();
    for (h, m, s) in [(6, 59, 59), (7, 0, 0), (7, 0, 1), (23, 59, 59), (0, 0, 0)] {
        instants.push(Utc.with_ymd_and_hms(2026, 11, 1, h, m, s).unwrap());
        instants.push(Utc.with_ymd_and_hms(2026, 3, 8, h, m, s).unwrap());
    }
    for off in offsets {
        let sql: Vec<NaiveDate> =
            sqlx::query_scalar("SELECT app.lst_date(t, $2) FROM unnest($1::timestamptz[]) AS t")
                .bind(&instants)
                .bind(off)
                .fetch_all(&w.db)
                .await
                .unwrap();
        let rust: Vec<NaiveDate> = instants
            .iter()
            .map(|t| famweather_core::time::standard_date(off, *t))
            .collect();
        assert_eq!(sql, rust, "offset {off}");
    }
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn a_stored_offset_that_differs_from_the_zone_warns_once_at_startup_not_per_rollup(
    db: PgPool,
) {
    let w = Builder::new(db)
        .clock_at(Utc.with_ymd_and_hms(2026, 6, 10, 12, 0, 0).unwrap())
        .build()
        .await;
    let st = station(&w).await;
    // Denver's standard offset is -420; store -360 (the DST value) by mistake.
    sqlx::query("UPDATE app.stations SET std_utc_offset_minutes = -360 WHERE id = $1")
        .bind(st)
        .execute(&w.db)
        .await
        .unwrap();
    let (_g, logs) = capture_logs();
    assert_eq!(warn_offset_mismatches(&w.env).await, 1);
    let needle = "stored standard offset differs from the time zone's base offset";
    assert_eq!(logs.text().matches(needle).count(), 1);
    // Rollups (hourly in production) do not repeat it, and the stored value still wins.
    let t = Utc.with_ymd_and_hms(2026, 6, 2, 19, 0, 0).unwrap();
    write_page(&w.env.pool, st, -360, vec![reading(t, 1.0)], false)
        .await
        .unwrap();
    for _ in 0..3 {
        run(&w, JobName::DailyRollup, None).await;
    }
    assert_eq!(logs.text().matches(needle).count(), 1);
    // A matching station warns about nothing.
    sqlx::query("UPDATE app.stations SET std_utc_offset_minutes = -420 WHERE id = $1")
        .bind(st)
        .execute(&w.db)
        .await
        .unwrap();
    assert_eq!(warn_offset_mismatches(&w.env).await, 0);
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn backfill_records_credential_success_and_feeds_the_401_rule(db: PgPool) {
    let w = Builder::new(db).build().await;
    // A: rate_limited, working key. B: a bogus key (401). C: working key.
    let (a, b, c) = (Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3));
    for (id, key, mac, status) in [
        (a, POLLER_KEY, "AA:BB:CC:00:00:01", "rate_limited"),
        (b, "bogus-key", "AA:BB:CC:00:00:07", "ok"),
        (c, POLLER_KEY, "AA:BB:CC:00:00:02", "ok"),
    ] {
        let owner = w.add_owner(&format!("Owner {id}")).await;
        w.add_credential_with(id, owner, key, 1, status).await;
        w.add_station(owner, id, mac).await;
    }
    let out = run_named(
        &w.env,
        JobName::AwnBackfill,
        None,
        JobFlags {
            max_pages: Some(1),
            ..JobFlags::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(status(&out), RunStatus::Failed, "B's station failed");
    let st = |id: Uuid| {
        let db = w.db.clone();
        async move {
            sqlx::query_as::<_, (String, Option<DateTime<Utc>>)>(
                "SELECT status, last_ok_at FROM app.credentials WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&db)
            .await
            .unwrap()
        }
    };
    let (sa, oka) = st(a).await;
    assert_eq!(sa, "ok", "rate_limited -> ok after a success");
    assert!(oka.is_some(), "last_ok_at set");
    assert!(st(c).await.1.is_some());
    // Other credentials succeeded, so the 401 credential is blamed and marked invalid, with one audit row.
    assert_eq!(st(b).await.0, "invalid");
    assert_eq!(
        w.count("SELECT count(*) FROM app.audit_log WHERE action = 'credential.invalid'")
            .await,
        1
    );
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn finished_runs_leave_the_run_id_list(db: PgPool) {
    let w = Builder::new(db).build().await;
    station(&w).await;
    for _ in 0..3 {
        run(&w, JobName::AwnIngest, None).await;
    }
    run(&w, JobName::AwnBackfill, None).await; // writes per-station rows itself
    assert!(
        w.env.run_ids.lock().unwrap().is_empty(),
        "bounded by runs in flight: {:?}",
        w.env.run_ids.lock().unwrap()
    );
    w.close().await;
}
