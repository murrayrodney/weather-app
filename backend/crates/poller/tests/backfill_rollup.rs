//! `awn_backfill` and `daily_rollup` replay integration tests (03 section 12.2 and 12.3).

mod common;

use std::time::Duration;

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use common::{capture_logs, slot, taken, Builder, CountingAwn, World};
use famweather_core::ratelimit::try_job_lock;
use famweather_core::testdb::role_hook;
use poller_lib::job::{JobFlags, RunOutcome, RunStatus, SkipReason};
use poller_lib::mapping::MappedRow;
use poller_lib::obs_write::write_page;
use poller_lib::{run_named, JobName};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn run(w: &World, job: JobName, station: Option<Uuid>, flags: JobFlags) -> RunOutcome {
    run_named(&w.env, job, station, flags).await.unwrap()
}

fn status(o: &RunOutcome) -> RunStatus {
    match o {
        RunOutcome::Finished { status, .. } => *status,
        other => panic!("not finished: {other:?}"),
    }
}

fn detail(o: &RunOutcome) -> Value {
    match o {
        RunOutcome::Finished { detail, .. } => detail.clone(),
        other => panic!("not finished: {other:?}"),
    }
}

async fn backfill_details(w: &World, station: Uuid) -> Vec<Value> {
    sqlx::query_scalar("SELECT detail FROM app.job_runs WHERE job = 'awn_backfill' AND subject_id = $1 ORDER BY started_at, id")
        .bind(station)
        .fetch_all(&w.db)
        .await
        .unwrap()
}

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

#[sqlx::test(migrations = false)]
async fn backfill_is_resumable_idempotent_and_writes_one_job_run_per_station_and_no_pings(
    db: PgPool,
) {
    // Pings are configured for the backfill suffix: it must never use them.
    let pings = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&pings)
        .await;
    let c = slot::<CountingAwn>();
    let w = Builder::new(db)
        .awn_counting(&c)
        .ping("AWN_BACKFILL", format!("{}/bf", pings.uri()))
        .build()
        .await;
    let seeded = w.seeded().await;

    // Two pages per station, then stop.
    let out = run(
        &w,
        JobName::AwnBackfill,
        None,
        JobFlags {
            max_pages: Some(2),
            ..JobFlags::default()
        },
    )
    .await;
    assert_eq!(status(&out), RunStatus::Success, "{out:?}");
    assert_eq!(
        w.count("SELECT count(*) FROM app.job_runs WHERE job = 'awn_backfill'")
            .await,
        3,
        "one row per station"
    );
    for (_, _, st) in &seeded {
        let dets = backfill_details(&w, *st).await;
        assert_eq!(dets.len(), 1);
        assert_eq!(dets[0]["pages"], 2);
        assert_eq!(dets[0]["rows_inserted"], 576);
        assert_eq!(dets[0]["stopped_because"], "max_pages");
        let floor: Option<DateTime<Utc>> =
            sqlx::query_scalar("SELECT backfill_floor_ts FROM app.stations WHERE id = $1")
                .bind(st)
                .fetch_one(&w.db)
                .await
                .unwrap();
        let min: DateTime<Utc> =
            sqlx::query_scalar("SELECT min(ts_utc) FROM app.observations WHERE station_id = $1")
                .bind(st)
                .fetch_one(&w.db)
                .await
                .unwrap();
        assert_eq!(floor, Some(min), "the floor is the oldest row written");
    }

    // A rerun resumes at the floor and ends on an empty page.
    let out = run(&w, JobName::AwnBackfill, None, JobFlags::default()).await;
    assert_eq!(status(&out), RunStatus::Success);
    for (_, _, st) in &seeded {
        let dets = backfill_details(&w, *st).await;
        assert_eq!(dets.len(), 2);
        assert_eq!(dets[1]["pages"], 60, "{}", dets[1]);
        assert_eq!(dets[1]["stopped_because"], "empty_page");
        assert_eq!(
            w.count(&format!(
                "SELECT count(*) FROM app.observations WHERE station_id = '{st}'"
            ))
            .await,
            62 * 288
        );
    }
    // Another run finishes immediately.
    run(&w, JobName::AwnBackfill, None, JobFlags::default()).await;
    for (_, _, st) in &seeded {
        let dets = backfill_details(&w, *st).await;
        assert_eq!(dets[2]["pages"], 0);
        assert_eq!(dets[2]["stopped_because"], "empty_page");
    }
    // --restart walks the whole history again and changes nothing.
    run(
        &w,
        JobName::AwnBackfill,
        None,
        JobFlags {
            restart: true,
            ..JobFlags::default()
        },
    )
    .await;
    for (_, _, st) in &seeded {
        let dets = backfill_details(&w, *st).await;
        assert_eq!(dets[3]["pages"], 62);
        assert_eq!(dets[3]["rows_inserted"], 0);
        assert_eq!(dets[3]["rows_updated"], 0);
    }
    assert!(
        pings.received_requests().await.unwrap().is_empty(),
        "awn_backfill sends no pings"
    );
    assert!(taken(&c).history_count() > 100);
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn since_stops_paging_and_restart_since_fills_an_ingest_gap(db: PgPool) {
    let c = slot::<CountingAwn>();
    let w = Builder::new(db).awn_counting(&c).build().await;
    let seeded = w.seeded().await;
    let st = seeded[0].2;

    // Ingest left a hole: one old row, then 12 pages (about 12 days) of newer data.
    let last = Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap();
    sqlx::query("INSERT INTO app.observations (station_id, ts_utc, temp_f) VALUES ($1, $2, 50)")
        .bind(st)
        .bind(last)
        .execute(&w.db)
        .await
        .unwrap();
    sqlx::query("UPDATE app.stations SET last_observed_at = $2 WHERE id = $1")
        .bind(st)
        .bind(last)
        .execute(&w.db)
        .await
        .unwrap();
    let ing = run(&w, JobName::AwnIngest, Some(st), JobFlags::default()).await;
    assert_eq!(detail(&ing)["gap_beyond_catchup"], true);
    let hole: i64 = w
        .count(&format!(
            "SELECT count(*) FROM app.observations WHERE station_id = '{st}'"
        ))
        .await;
    assert_eq!(
        hole,
        1 + 12 * 288,
        "ingest fetched 12 pages and left a hole"
    );

    // The remedy from the log line: --restart --since <gap start date>.
    let out = run(
        &w,
        JobName::AwnBackfill,
        Some(st),
        JobFlags {
            restart: true,
            since: Some(d(2026, 10, 4)),
            ..JobFlags::default()
        },
    )
    .await;
    assert_eq!(status(&out), RunStatus::Success, "{out:?}");
    let dets = backfill_details(&w, st).await;
    assert_eq!(dets[0]["stopped_because"], "since_reached");
    // Every 5-minute reading from the gap start to the newest row exists now: no hole.
    let n: i64 = w.count(&format!("SELECT count(*) FROM app.observations WHERE station_id = '{st}' AND ts_utc >= '2026-10-04T12:00:00Z'")).await;
    assert_eq!(n, 13 * 288 + 1);
    let max_gap: i64 = w
        .count(&format!(
            "SELECT coalesce(max(extract(epoch from gap))::bigint, 0) FROM (SELECT ts_utc - lag(ts_utc) OVER (ORDER BY ts_utc) AS gap \
             FROM app.observations WHERE station_id = '{st}' AND ts_utc >= '2026-10-04T12:00:00Z') g"
        ))
        .await;
    assert_eq!(max_gap, 300);
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn the_post_backfill_rollup_waits_for_a_held_lock_then_runs_and_gives_up_after_the_wait(
    db: PgPool,
) {
    let w = Builder::new(db).build().await;
    let seeded = w.seeded().await;
    let st = seeded[0].2;
    let hook = role_hook("app_poller").unwrap();

    // Held for 200 ms, shorter than the (test) wait of 600 ms.
    let held = try_job_lock(&w.env.db_opts, "daily_rollup", Some(hook.clone()))
        .await
        .unwrap()
        .unwrap();
    let releaser = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        held.release().await.unwrap();
    });
    let out = run(
        &w,
        JobName::AwnBackfill,
        Some(st),
        JobFlags {
            max_pages: Some(1),
            ..JobFlags::default()
        },
    )
    .await;
    releaser.await.unwrap();
    assert_eq!(status(&out), RunStatus::Success);
    assert!(
        w.count("SELECT count(*) FROM app.daily_rollups").await >= 1,
        "the rollup ran after the lock was released"
    );
    assert_eq!(
        w.count("SELECT count(*) FROM app.rollup_dirty_days").await,
        0,
        "queue consumed"
    );

    // Held beyond the wait: the backfill still succeeds; the queue persists for the scheduled rollup.
    sqlx::query("DELETE FROM app.daily_rollups")
        .execute(&w.db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM app.rollup_dirty_days")
        .execute(&w.db)
        .await
        .unwrap();
    let held = try_job_lock(&w.env.db_opts, "daily_rollup", Some(hook))
        .await
        .unwrap()
        .unwrap();
    let (_g, logs) = capture_logs();
    let out = run(
        &w,
        JobName::AwnBackfill,
        Some(st),
        JobFlags {
            restart: true,
            max_pages: Some(1),
            ..JobFlags::default()
        },
    )
    .await;
    assert_eq!(status(&out), RunStatus::Success);
    assert_eq!(w.count("SELECT count(*) FROM app.daily_rollups").await, 0);
    assert!(
        w.count("SELECT count(*) FROM app.rollup_dirty_days").await >= 1,
        "the queue persists"
    );
    assert!(
        logs.text()
            .contains("daily_rollup still running after the wait"),
        "{}",
        logs.text()
    );
    held.release().await.unwrap();
    w.close().await;
}

// ---------------------------------------------------------------------------------------------------------------
// daily_rollup

#[sqlx::test(migrations = false)]
async fn rollups_after_backfill_cover_current_data_with_resets_glitches_and_caps(db: PgPool) {
    let w = Builder::new(db).build().await;
    let seeded = w.seeded().await;
    let out = run(&w, JobName::AwnBackfill, None, JobFlags::default()).await;
    assert_eq!(status(&out), RunStatus::Success, "{out:?}");

    for (_, _, st) in &seeded {
        let (n, newest, oldest): (i64, NaiveDate, NaiveDate) =
            sqlx::query_as("SELECT count(*), max(local_date), min(local_date) FROM app.daily_rollups WHERE station_id = $1").bind(st).fetch_one(&w.db).await.unwrap();
        assert!(
            (62..=64).contains(&n),
            "62 days of 5-minute data spread over {n} local days"
        );
        assert_eq!(
            newest,
            d(2026, 10, 17),
            "current-year data (replay shifts the newest reading to now)"
        );
        assert_eq!(oldest.format("%Y").to_string(), "2026");
        // The day that includes now is incomplete; a fully covered earlier day is complete with 288 readings.
        let today_complete: bool = sqlx::query_scalar("SELECT is_complete FROM app.daily_rollups WHERE station_id = $1 AND local_date = '2026-10-17'").bind(st).fetch_one(&w.db).await.unwrap();
        assert!(!today_complete);
        let (cnt, complete): (i16, bool) = sqlx::query_as("SELECT obs_count, is_complete FROM app.daily_rollups WHERE station_id = $1 AND local_date = '2026-10-01'").bind(st).fetch_one(&w.db).await.unwrap();
        assert_eq!((cnt, complete), (288, true));
        // The base series carries one reset, one glitch and one capped jump (scaled copies keep them).
        let (resets, capped): (i64, i64) = sqlx::query_as("SELECT sum(rain_resets), sum(rain_capped) FROM app.daily_rollups WHERE station_id = $1").bind(st).fetch_one(&w.db).await.unwrap();
        assert_eq!((resets, capped), (1, 1), "station {st}");
        let rain: Option<f64> = sqlx::query_scalar(
            "SELECT sum(rain_in)::float8 FROM app.daily_rollups WHERE station_id = $1",
        )
        .bind(st)
        .fetch_one(&w.db)
        .await
        .unwrap();
        assert!(rain.unwrap() > 0.5, "rain accumulates: {rain:?}");
    }
    let rollup: Value = sqlx::query_scalar("SELECT detail FROM app.job_runs WHERE job = 'daily_rollup' ORDER BY started_at DESC LIMIT 1").fetch_one(&w.db).await.unwrap();
    assert_eq!(rollup["rain_glitches"], 3, "{rollup}");
    assert_eq!(rollup["resets"], 3);
    assert_eq!(rollup["capped"], 3);
    w.close().await;
}

async fn rollup_hash(w: &World) -> String {
    sqlx::query_scalar(
        "SELECT coalesce(md5(string_agg(station_id::text || local_date::text || xmin::text || coalesce(rain_in::text,'') || \
         coalesce(tmax_f::text,'') || obs_count::text, ',' ORDER BY station_id, local_date)), '') FROM app.daily_rollups",
    )
    .fetch_one(&w.db)
    .await
    .unwrap()
}

#[sqlx::test(migrations = false)]
async fn an_empty_history_processes_everything_in_date_order_and_a_second_run_changes_nothing(
    db: PgPool,
) {
    let w = Builder::new(db).build().await;
    let seeded = w.seeded().await;
    run(
        &w,
        JobName::AwnBackfill,
        Some(seeded[0].2),
        JobFlags {
            max_pages: Some(10),
            ..JobFlags::default()
        },
    )
    .await;
    // Wipe the rollups and the queue: only the self-heal can find the days now.
    sqlx::query("DELETE FROM app.daily_rollups")
        .execute(&w.db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM app.rollup_dirty_days")
        .execute(&w.db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM app.job_runs")
        .execute(&w.db)
        .await
        .unwrap();

    let out = run(
        &w,
        JobName::DailyRollup,
        None,
        JobFlags {
            self_heal: true,
            ..JobFlags::default()
        },
    )
    .await;
    assert_eq!(status(&out), RunStatus::Success);
    let dt = detail(&out);
    assert_eq!(dt["self_healed"], true);
    let n = w.count("SELECT count(*) FROM app.daily_rollups").await;
    assert!(n >= 10, "10 pages of 288 rows is 10 days: {n}");
    // Date order: transaction start times never decrease with the date.
    let times: Vec<(NaiveDate, DateTime<Utc>)> =
        sqlx::query_as("SELECT local_date, computed_at FROM app.daily_rollups ORDER BY local_date")
            .fetch_all(&w.db)
            .await
            .unwrap();
    assert!(times.windows(2).all(|p| p[0].1 <= p[1].1));

    let before = rollup_hash(&w).await;
    let again = run(&w, JobName::DailyRollup, None, JobFlags::default()).await;
    assert_eq!(detail(&again)["days_changed"], 0, "{}", detail(&again));
    assert_eq!(
        before,
        rollup_hash(&w).await,
        "identical tables and unchanged xmin"
    );
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn a_dirty_day_reprocesses_its_whole_month_but_never_rewrites_neighbouring_rows(db: PgPool) {
    let w = Builder::new(db).build().await;
    let seeded = w.seeded().await;
    let st = seeded[0].2;
    run(&w, JobName::AwnBackfill, Some(st), JobFlags::default()).await;
    // September 2026 is fully inside the 62 days of data. Standard (MST) month bounds:
    let (sep_start, oct_start) = ("2026-09-01T07:00:00Z", "2026-10-01T07:00:00Z");
    sqlx::query("UPDATE app.observations SET rain_inc_in = NULL WHERE station_id = $1 AND ts_utc >= $2::timestamptz AND ts_utc < $3::timestamptz")
        .bind(st).bind(sep_start).bind(oct_start).execute(&w.db).await.unwrap();
    // Sentinels on the predecessor row and the first look-ahead rows: they are read, never rewritten.
    sqlx::query("UPDATE app.observations SET rain_inc_in = 0.123 WHERE station_id = $1 AND ts_utc IN ( \
        (SELECT max(ts_utc) FROM app.observations WHERE station_id = $1 AND ts_utc < $2::timestamptz), \
        (SELECT min(ts_utc) FROM app.observations WHERE station_id = $1 AND ts_utc >= $3::timestamptz))")
        .bind(st).bind(sep_start).bind(oct_start).execute(&w.db).await.unwrap();
    // One dirty day only.
    sqlx::query(
        "INSERT INTO app.rollup_dirty_days (station_id, local_date) VALUES ($1, '2026-09-15')",
    )
    .bind(st)
    .execute(&w.db)
    .await
    .unwrap();
    let out = run(&w, JobName::DailyRollup, Some(st), JobFlags::default()).await;
    assert_eq!(status(&out), RunStatus::Success);
    assert_eq!(
        detail(&out)["days_processed"],
        30,
        "every day of September: {}",
        detail(&out)
    );
    let nulls: i64 = w.count(&format!("SELECT count(*) FROM app.observations WHERE station_id = '{st}' AND ts_utc >= '{sep_start}' AND ts_utc < '{oct_start}' AND rain_inc_in IS NULL")).await;
    assert_eq!(nulls, 0, "the whole month was rewritten from one dirty day");
    let sentinels: i64 = w.count(&format!("SELECT count(*) FROM app.observations WHERE station_id = '{st}' AND rain_inc_in = 0.123")).await;
    assert_eq!(
        sentinels, 2,
        "predecessor and look-ahead rows were not rewritten"
    );
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn the_row_already_queued_interleaving_never_loses_a_day(db: PgPool) {
    let w = Builder::new(db).build().await;
    let owner = w.add_owner("Rodney").await;
    let cred = w.add_credential(owner, common::POLLER_KEY).await;
    let st = w.add_station(owner, cred, "AA:BB:CC:00:00:01").await;
    sqlx::query(
        "INSERT INTO app.rollup_dirty_days (station_id, local_date) VALUES ($1, '2026-06-02')",
    )
    .bind(st)
    .execute(&w.db)
    .await
    .unwrap();

    // The rollup transaction: its first statement deletes the queued day and holds the row lock.
    let mut rollup_tx = w.env.pool.begin().await.unwrap();
    sqlx::query(
        "DELETE FROM app.rollup_dirty_days WHERE station_id = $1 AND local_date = '2026-06-02'",
    )
    .bind(st)
    .execute(&mut *rollup_tx)
    .await
    .unwrap();

    // An ingest commit for the same day arrives meanwhile: its enqueue waits for the rollup's commit...
    let pool = w.env.pool.clone();
    let ingest = tokio::spawn(async move {
        let row = MappedRow {
            ts: Utc.with_ymd_and_hms(2026, 6, 2, 19, 0, 0).unwrap(),
            temp_f: Some(60.0),
            ..Default::default()
        };
        write_page(&pool, st, -420, vec![row], true).await.unwrap()
    });
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        !ingest.is_finished(),
        "the enqueue blocks on the row lock until the rollup commits"
    );
    rollup_tx.commit().await.unwrap();
    // ...and then re-inserts the day, so the rollup's DELETE did not lose it.
    let w1 = tokio::time::timeout(Duration::from_secs(10), ingest)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(w1.inserted, 1);
    let days: Vec<NaiveDate> =
        sqlx::query_scalar("SELECT local_date FROM app.rollup_dirty_days WHERE station_id = $1")
            .bind(st)
            .fetch_all(&w.db)
            .await
            .unwrap();
    assert_eq!(days, [d(2026, 6, 2)]);
    w.close().await;
}

async fn put(w: &World, st: Uuid, rows: Vec<MappedRow>) {
    write_page(&w.env.pool, st, -420, rows, false)
        .await
        .unwrap();
}

#[sqlx::test(migrations = false)]
async fn rain_from_counters_with_a_reset_a_glitch_and_a_capped_jump(db: PgPool) {
    let w = Builder::new(db)
        .clock_at(Utc.with_ymd_and_hms(2026, 6, 10, 12, 0, 0).unwrap())
        .build()
        .await;
    let owner = w.add_owner("Rodney").await;
    let cred = w.add_credential(owner, common::POLLER_KEY).await;
    let st = w.add_station(owner, cred, "AA:BB:CC:00:00:01").await;
    // 17 readings, 5 minutes apart from 08:00Z on 2026-06-02 (the standard day starts at 07:00Z).
    let totals = [
        10.00, 10.01, 10.01, 10.02, 10.02, 9.00, 9.00, 10.03, 10.03, 10.04, 0.00, 0.02, 0.02, 0.03,
        3.03, 3.03, 3.04,
    ];
    let start = Utc.with_ymd_and_hms(2026, 6, 2, 8, 0, 0).unwrap();
    let rows: Vec<MappedRow> = totals
        .iter()
        .enumerate()
        .map(|(i, t)| MappedRow {
            ts: start + chrono::Duration::minutes(5 * i as i64),
            temp_f: Some(60.0),
            rain_total_in: Some(*t as f32),
            ..Default::default()
        })
        .collect();
    put(&w, st, rows).await;
    let out = run(&w, JobName::DailyRollup, None, JobFlags::default()).await;
    assert_eq!(status(&out), RunStatus::Success);
    assert_eq!(detail(&out)["rain_glitches"], 1);
    let r: (Option<f64>, i16, i16, i16, bool, Option<f32>, String) = sqlx::query_as(
        "SELECT rain_in::float8, rain_resets, rain_capped, obs_count, is_complete, tmax_f, rain_reconcile_status FROM app.daily_rollups WHERE station_id = $1 AND local_date = '2026-06-02'",
    )
    .bind(st)
    .fetch_one(&w.db)
    .await
    .unwrap();
    assert_eq!(r.0, Some(0.08));
    assert_eq!((r.1, r.2, r.3), (1, 1, 17));
    assert!(!r.4, "17 readings are far below 90% coverage");
    assert_eq!(r.5, None, "below 50% coverage there are no temperatures");
    assert_eq!(r.6, "unchecked");
    let incs: Vec<Option<f64>> = sqlx::query_scalar(
        "SELECT rain_inc_in::float8 FROM app.observations WHERE station_id = $1 ORDER BY ts_utc",
    )
    .bind(st)
    .fetch_all(&w.db)
    .await
    .unwrap();
    let want = [
        None,
        Some(0.01),
        Some(0.0),
        Some(0.01),
        Some(0.0),
        Some(0.0),
        Some(0.0),
        Some(0.01),
        Some(0.0),
        Some(0.01),
        Some(0.0),
        Some(0.02),
        Some(0.0),
        Some(0.01),
        Some(0.0),
        Some(0.0),
        Some(0.01),
    ];
    assert_eq!(incs, want);
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn no_counter_means_null_rain_and_standard_time_days_ignore_dst(db: PgPool) {
    let w = Builder::new(db)
        .clock_at(Utc.with_ymd_and_hms(2026, 11, 5, 12, 0, 0).unwrap())
        .build()
        .await;
    let owner = w.add_owner("Rodney").await;
    let cred = w.add_credential(owner, common::POLLER_KEY).await;
    let st = w.add_station(owner, cred, "AA:BB:CC:00:00:01").await;
    // The DST end (2026-11-01): the standard day is still exactly [07:00Z, 07:00Z + 24 h): 288 readings, no counters.
    sqlx::query("INSERT INTO app.observations (station_id, ts_utc, temp_f) SELECT $1, g, 50 FROM generate_series('2026-11-01T07:00:00Z'::timestamptz, '2026-11-02T06:55:00Z', interval '5 minutes') g")
        .bind(st).execute(&w.db).await.unwrap();
    sqlx::query(
        "INSERT INTO app.rollup_dirty_days (station_id, local_date) VALUES ($1, '2026-11-01')",
    )
    .bind(st)
    .execute(&w.db)
    .await
    .unwrap();
    run(&w, JobName::DailyRollup, None, JobFlags::default()).await;
    let r: Vec<(NaiveDate, i16, bool, Option<f64>)> = sqlx::query_as("SELECT local_date, obs_count, is_complete, rain_in::float8 FROM app.daily_rollups WHERE station_id = $1 ORDER BY 1").bind(st).fetch_all(&w.db).await.unwrap();
    assert_eq!(r, [(d(2026, 11, 1), 288, true, None)]);
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn housekeeping_deletes_old_job_runs_once_a_day_after_0300_utc(db: PgPool) {
    let w = Builder::new(db)
        .clock_at(Utc.with_ymd_and_hms(2026, 10, 17, 4, 7, 0).unwrap())
        .build()
        .await;
    sqlx::query("INSERT INTO app.job_runs (job, status, started_at, finished_at) VALUES ('awn_ingest','success', now() - interval '100 days', now() - interval '100 days'), ('awn_ingest','success', now() - interval '10 days', now() - interval '10 days')")
        .execute(&w.db).await.unwrap();
    let first = run(&w, JobName::DailyRollup, None, JobFlags::default()).await;
    assert_eq!(detail(&first)["job_runs_deleted"], 1, "{}", detail(&first));
    assert_eq!(detail(&first)["housekeeping_done_on"], "2026-10-17");
    sqlx::query("INSERT INTO app.job_runs (job, status, started_at, finished_at) VALUES ('awn_ingest','success', now() - interval '120 days', now() - interval '120 days')").execute(&w.db).await.unwrap();
    let second = run(&w, JobName::DailyRollup, None, JobFlags::default()).await;
    assert_eq!(detail(&second)["job_runs_deleted"], 0, "already done today");
    assert!(detail(&second)["housekeeping_done_on"].is_null());
    // Before 03:00 UTC nothing is done even on a new day.
    w.clock
        .set(Utc.with_ymd_and_hms(2026, 10, 18, 2, 7, 0).unwrap());
    let early = run(&w, JobName::DailyRollup, None, JobFlags::default()).await;
    assert!(detail(&early)["housekeeping_done_on"].is_null());
    assert_eq!(
        w.count("SELECT count(*) FROM app.job_runs WHERE started_at < now() - interval '90 days'")
            .await,
        1
    );
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn a_second_rollup_cannot_run_while_the_first_holds_the_lock(db: PgPool) {
    let w = Builder::new(db).build().await;
    let held = try_job_lock(
        &w.env.db_opts,
        "daily_rollup",
        Some(role_hook("app_poller").unwrap()),
    )
    .await
    .unwrap()
    .unwrap();
    let out = run(&w, JobName::DailyRollup, None, JobFlags::default()).await;
    assert!(matches!(out, RunOutcome::Skipped(SkipReason::LockHeld)));
    held.release().await.unwrap();
    w.close().await;
}
