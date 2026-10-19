//! `awn_ingest` replay integration tests (03 section 12.2).

mod common;

use chrono::{Duration, TimeZone, Utc};
use common::{capture_logs, slot, synth_root, taken, Builder, CountingAwn, World, POLLER_KEY};
use poller_lib::job::{JobFlags, RunOutcome, RunStatus};
use poller_lib::synth::StationSpec;
use poller_lib::{run_named, JobName};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

async fn ingest(w: &World, station: Option<Uuid>) -> RunOutcome {
    run_named(&w.env, JobName::AwnIngest, station, JobFlags::default())
        .await
        .unwrap()
}

fn detail_of(o: &RunOutcome) -> Value {
    match o {
        RunOutcome::Finished { detail, .. } => detail.clone(),
        other => panic!("run did not finish: {other:?}"),
    }
}

fn status_of(o: &RunOutcome) -> RunStatus {
    match o {
        RunOutcome::Finished { status, .. } => *status,
        other => panic!("run did not finish: {other:?}"),
    }
}

async fn table_fingerprint(w: &World) -> String {
    sqlx::query_scalar(
        "SELECT coalesce(md5(string_agg(station_id::text || ts_utc::text || xmin::text || coalesce(temp_f::text, ''), ',' \
         ORDER BY station_id, ts_utc)), '') FROM app.observations",
    )
    .fetch_one(&w.db)
    .await
    .unwrap()
}

#[sqlx::test(migrations = false)]
async fn three_seeded_stations_are_ingested_from_replay(db: PgPool) {
    let c = slot::<CountingAwn>();
    let w = Builder::new(db).awn_counting(&c).build().await;
    let seeded = w.seeded().await;
    let (_g, logs) = capture_logs();
    let out = ingest(&w, None).await;
    assert_eq!(status_of(&out), RunStatus::Success, "{out:?}");
    let d = detail_of(&out);
    assert_eq!(d["credentials"], 3);
    assert_eq!(d["stations"], 3);
    assert_eq!(d["empty_stations"], 3);
    assert_eq!(d["rows_inserted"], 3 * 288);
    assert_eq!(d["failed_units"], 0);

    // Timestamps are shifted so the newest reading is the 5-minute floor of "now".
    let newest = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
    for (_, cred, st) in &seeded {
        let (n, max): (i64, chrono::DateTime<Utc>) = sqlx::query_as(
            "SELECT count(*), max(ts_utc) FROM app.observations WHERE station_id = $1",
        )
        .bind(st)
        .fetch_one(&w.db)
        .await
        .unwrap();
        assert_eq!((n, max), (288, newest));
        let last: chrono::DateTime<Utc> =
            sqlx::query_scalar("SELECT last_observed_at FROM app.stations WHERE id = $1")
                .bind(st)
                .fetch_one(&w.db)
                .await
                .unwrap();
        assert_eq!(last, newest);
        let ok: Option<chrono::DateTime<Utc>> =
            sqlx::query_scalar("SELECT last_ok_at FROM app.credentials WHERE id = $1")
                .bind(cred)
                .fetch_one(&w.db)
                .await
                .unwrap();
        assert!(ok.is_some(), "last_ok_at is set");
    }
    assert_eq!(
        w.count(
            "SELECT count(*) FROM app.job_runs WHERE job = 'awn_ingest' AND status = 'success'"
        )
        .await,
        1
    );
    // Exactly one page per empty station, and no gap warning.
    let counting = taken(&c);
    assert_eq!(counting.history_count(), 3);
    assert_eq!(counting.devices_count(), 3);
    assert!(!logs.text().contains("gap beyond ingest catch-up"));
    // Dirty days were enqueued by the same transactions.
    assert!(w.count("SELECT count(*) FROM app.rollup_dirty_days").await >= 3);
    // Gates: the poller reserves its own app gate and the per-key gates, never the API's.
    assert_eq!(
        w.count("SELECT count(*) FROM app.rate_gates WHERE name = 'awn:app:poller'")
            .await,
        1
    );
    assert_eq!(
        w.count("SELECT count(*) FROM app.rate_gates WHERE name LIKE 'awn:key:%'")
            .await,
        3
    );
    assert_eq!(
        w.count("SELECT count(*) FROM app.rate_gates WHERE name = 'awn:app:api'")
            .await,
        0
    );
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn a_second_run_on_identical_input_changes_nothing(db: PgPool) {
    let w = Builder::new(db).build().await;
    w.seeded().await;
    assert_eq!(status_of(&ingest(&w, None).await), RunStatus::Success);
    sqlx::query("DELETE FROM app.rollup_dirty_days")
        .execute(&w.db)
        .await
        .unwrap();
    let before = table_fingerprint(&w).await;
    let out = ingest(&w, None).await;
    let d = detail_of(&out);
    assert_eq!(status_of(&out), RunStatus::Success);
    assert_eq!(d["rows_inserted"], 0);
    assert_eq!(d["rows_updated"], 0, "{d}");
    assert_eq!(
        before,
        table_fingerprint(&w).await,
        "identical rows and unchanged xmin"
    );
    assert_eq!(
        w.count("SELECT count(*) FROM app.rollup_dirty_days").await,
        0,
        "nothing re-enqueued"
    );
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn unknown_missing_disabled_invalid_and_replaced_are_handled(db: PgPool) {
    let c = slot::<CountingAwn>();
    let w = Builder::new(db).awn_counting(&c).build().await;
    let seeded = w.seeded().await;
    // Unknown device: :03 has no stations row at all (its device still shows up in the key's device list).
    sqlx::query("DELETE FROM app.stations WHERE id = $1")
        .bind(seeded[2].2)
        .execute(&w.db)
        .await
        .unwrap();
    // Missing device: a station whose MAC the key does not list.
    w.add_station(seeded[0].0, seeded[0].1, "AA:BB:CC:00:00:09")
        .await;
    // Disabled station.
    sqlx::query("UPDATE app.stations SET is_enabled = false WHERE id = $1")
        .bind(seeded[1].2)
        .execute(&w.db)
        .await
        .unwrap();
    // Stations on an invalid and on a replaced credential.
    let o4 = w.add_owner("Invalid Owner").await;
    let c4 = Uuid::new_v4();
    w.add_credential_with(c4, o4, POLLER_KEY, 1, "invalid")
        .await;
    w.add_station(o4, c4, "AA:BB:CC:00:00:04").await;
    let o5 = w.add_owner("Replaced Owner").await;
    let c5 = Uuid::new_v4();
    w.add_credential_with(c5, o5, POLLER_KEY, 1, "replaced")
        .await;
    w.add_station(o5, c5, "AA:BB:CC:00:00:05").await;

    let out = ingest(&w, None).await;
    let d = detail_of(&out);
    assert_eq!(d["unknown_devices"], serde_json::json!(["0003"]), "{d}");
    assert_eq!(d["missing_devices"], 1);
    assert_eq!(
        d["credentials"], 1,
        "only the live credential with an enabled station is used (Alex's has none enabled)"
    );
    assert_eq!(
        d["rows_inserted"], 288,
        "only Rodney's station :01 was ingested"
    );
    assert_eq!(w.count("SELECT count(*) FROM app.observations").await, 288);
    assert_eq!(taken(&c).history_count(), 1);
    // The skipped credentials are untouched.
    assert_eq!(w.count("SELECT count(*) FROM app.credentials WHERE status IN ('invalid','replaced') AND last_ok_at IS NOT NULL").await, 0);
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn catch_up_fills_a_three_hour_hole_with_one_page_and_last_observed_never_moves_backwards(
    db: PgPool,
) {
    let c = slot::<CountingAwn>();
    let w = Builder::new(db).awn_counting(&c).build().await;
    let seeded = w.seeded().await;
    ingest(&w, None).await;
    let st = seeded[0].2;
    let cutoff = Utc.with_ymd_and_hms(2026, 10, 17, 9, 0, 0).unwrap();
    sqlx::query("DELETE FROM app.observations WHERE station_id = $1 AND ts_utc > $2")
        .bind(st)
        .bind(cutoff)
        .execute(&w.db)
        .await
        .unwrap();
    let calls_before = taken(&c).history_count();
    let out = ingest(&w, Some(st)).await;
    assert_eq!(status_of(&out), RunStatus::Success);
    let d = detail_of(&out);
    assert_eq!(d["rows_inserted"], 36, "09:05 .. 12:00 at 5 minutes: {d}");
    assert_eq!(d["gaps"], serde_json::json!([]));
    assert_eq!(
        taken(&c).history_count() - calls_before,
        1,
        "oldest <= last ends paging after the first page"
    );
    let (_, _, limit) = taken(&c)
        .history_calls
        .lock()
        .unwrap()
        .last()
        .cloned()
        .unwrap();
    assert_eq!(limit, 39, "ceil(183.5 min / 5) + 2: now is 12:03:30");

    let future = Utc.with_ymd_and_hms(2026, 10, 17, 13, 0, 0).unwrap();
    sqlx::query("UPDATE app.stations SET last_observed_at = $2 WHERE id = $1")
        .bind(st)
        .bind(future)
        .execute(&w.db)
        .await
        .unwrap();
    ingest(&w, Some(st)).await;
    let last: chrono::DateTime<Utc> =
        sqlx::query_scalar("SELECT last_observed_at FROM app.stations WHERE id = $1")
            .bind(st)
            .fetch_one(&w.db)
            .await
            .unwrap();
    assert_eq!(last, future, "GREATEST: never backwards");
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn a_thirteen_day_gap_stops_at_twelve_pages_records_the_gap_and_logs_the_remedy(db: PgPool) {
    let c = slot::<CountingAwn>();
    let w = Builder::new(db).awn_counting(&c).build().await;
    let seeded = w.seeded().await;
    let st = seeded[0].2;
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
    let (_g, logs) = capture_logs();
    let out = ingest(&w, Some(st)).await;
    assert_eq!(
        status_of(&out),
        RunStatus::Success,
        "a gap is flagged, it is not a failed unit: {out:?}"
    );
    let d = detail_of(&out);
    assert_eq!(taken(&c).history_count(), 12, "stops at 12 pages");
    assert_eq!(d["gap_beyond_catchup"], true);
    let gaps = d["gaps"].as_array().unwrap();
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0]["station_id"], st.to_string());
    assert_eq!(gaps[0]["from"], "2026-10-04T12:00:00Z");
    assert_eq!(d["rows_inserted"], 12 * 288);
    let text = logs.text();
    let remedy = format!("awn_backfill --station {st} --restart --since 2026-10-04");
    assert!(
        text.contains("gap beyond ingest catch-up; run: poller run-job"),
        "{text}"
    );
    assert!(text.contains(&remedy), "{text}");
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn thirty_minute_data_gives_complete_days_with_48_readings(db: PgPool) {
    let newest = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
    let root = synth_root(&[("AA:BB:CC:00:00:01", StationSpec::simple(newest, 10, 30, 11))]);
    let w = Builder::new(db)
        .fixtures(root.path().to_owned())
        .build()
        .await;
    let (owner, cred, st) = {
        let owner = w.add_owner("Rodney").await;
        let cred = w.add_credential(owner, POLLER_KEY).await;
        let st = w.add_station(owner, cred, "AA:BB:CC:00:00:01").await;
        (owner, cred, st)
    };
    let _ = (owner, cred);
    assert_eq!(status_of(&ingest(&w, None).await), RunStatus::Success);
    let out = run_named(
        &w.env,
        JobName::DailyRollup,
        None,
        JobFlags {
            self_heal: true,
            ..JobFlags::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(status_of(&out), RunStatus::Success, "{out:?}");
    let complete: Vec<(i16, bool)> = sqlx::query_as("SELECT obs_count, is_complete FROM app.daily_rollups WHERE station_id = $1 AND is_complete ORDER BY local_date")
        .bind(st)
        .fetch_all(&w.db)
        .await
        .unwrap();
    assert!(complete.len() >= 4, "{complete:?}");
    assert!(complete.iter().all(|(n, c)| *n == 48 && *c));
    let incomplete: i64 = w
        .count("SELECT count(*) FROM app.daily_rollups WHERE NOT is_complete")
        .await;
    assert!(
        incomplete >= 1,
        "the day that includes now (and the partial first day) are incomplete"
    );
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn bad_values_are_counted_stored_as_null_and_never_fail_the_batch(db: PgPool) {
    let newest = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
    let root = synth_root(&[("AA:BB:CC:00:00:01", StationSpec::simple(newest, 2, 5, 12))]);
    // Corrupt three rows of the first page on disk.
    let page = root
        .path()
        .join("awn/stations/AA:BB:CC:00:00:01/history/page-0001.json");
    let mut rows: Vec<Value> = serde_json::from_slice(&std::fs::read(&page).unwrap()).unwrap();
    rows[0]["humidity"] = serde_json::json!(101);
    rows[1]["tempf"] = serde_json::json!("hot");
    rows[2]["uv"] = serde_json::json!(-2);
    std::fs::write(&page, serde_json::to_vec(&rows).unwrap()).unwrap();
    let w = Builder::new(db)
        .fixtures(root.path().to_owned())
        .build()
        .await;
    let owner = w.add_owner("Rodney").await;
    let cred = w.add_credential(owner, POLLER_KEY).await;
    let st = w.add_station(owner, cred, "AA:BB:CC:00:00:01").await;
    let out = ingest(&w, None).await;
    assert_eq!(status_of(&out), RunStatus::Success);
    let d = detail_of(&out);
    assert_eq!(d["bad_values"], 3, "{d}");
    let newest_row: (Option<i16>,) = sqlx::query_as("SELECT humidity_pct FROM app.observations WHERE station_id = $1 ORDER BY ts_utc DESC LIMIT 1").bind(st).fetch_one(&w.db).await.unwrap();
    assert_eq!(newest_row.0, None);
    assert_eq!(w.count("SELECT count(*) FROM app.observations").await, 288);
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn a_late_row_enqueues_its_day_and_the_previous_day_only_near_a_boundary(db: PgPool) {
    let w = Builder::new(db).build().await;
    let owner = w.add_owner("Rodney").await;
    let cred = w.add_credential(owner, POLLER_KEY).await;
    let st = w.add_station(owner, cred, "AA:BB:CC:00:00:01").await;
    let row = |h: u32, m: u32| poller_lib::mapping::MappedRow {
        ts: Utc.with_ymd_and_hms(2026, 6, 2, h, m, 0).unwrap(),
        temp_f: Some(60.0),
        ..Default::default()
    };
    // 19:00Z = 12:00 local standard time: only its own day.
    poller_lib::obs_write::write_page(&w.env.pool, st, -420, vec![row(19, 0)], true)
        .await
        .unwrap();
    let days: Vec<chrono::NaiveDate> = sqlx::query_scalar(
        "SELECT local_date FROM app.rollup_dirty_days WHERE station_id = $1 ORDER BY 1",
    )
    .bind(st)
    .fetch_all(&w.db)
    .await
    .unwrap();
    assert_eq!(days, [chrono::NaiveDate::from_ymd_opt(2026, 6, 2).unwrap()]);
    // 08:00Z = 01:00 local: within 4 h of the boundary, so the previous day is re-dirtied too.
    sqlx::query("DELETE FROM app.rollup_dirty_days")
        .execute(&w.db)
        .await
        .unwrap();
    poller_lib::obs_write::write_page(&w.env.pool, st, -420, vec![row(8, 0)], true)
        .await
        .unwrap();
    let days: Vec<chrono::NaiveDate> = sqlx::query_scalar(
        "SELECT local_date FROM app.rollup_dirty_days WHERE station_id = $1 ORDER BY 1",
    )
    .bind(st)
    .fetch_all(&w.db)
    .await
    .unwrap();
    assert_eq!(
        days,
        [
            chrono::NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(),
            chrono::NaiveDate::from_ymd_opt(2026, 6, 2).unwrap()
        ]
    );
    // Re-writing identical rows changes nothing and enqueues nothing.
    sqlx::query("DELETE FROM app.rollup_dirty_days")
        .execute(&w.db)
        .await
        .unwrap();
    let again =
        poller_lib::obs_write::write_page(&w.env.pool, st, -420, vec![row(8, 0), row(19, 0)], true)
            .await
            .unwrap();
    assert_eq!((again.inserted, again.updated, again.dirty_days), (0, 0, 0));
    assert_eq!(
        w.count("SELECT count(*) FROM app.rollup_dirty_days").await,
        0
    );
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn duplicate_timestamps_in_one_page_do_not_break_the_upsert(db: PgPool) {
    let w = Builder::new(db).build().await;
    let owner = w.add_owner("Rodney").await;
    let cred = w.add_credential(owner, POLLER_KEY).await;
    let st = w.add_station(owner, cred, "AA:BB:CC:00:00:01").await;
    let t = Utc.with_ymd_and_hms(2026, 6, 2, 19, 0, 0).unwrap();
    let mk = |temp: f32| poller_lib::mapping::MappedRow {
        ts: t,
        temp_f: Some(temp),
        ..Default::default()
    };
    let w1 = poller_lib::obs_write::write_page(
        &w.env.pool,
        st,
        -420,
        vec![mk(50.0), mk(51.0), mk(52.0)],
        false,
    )
    .await
    .unwrap();
    assert_eq!(w1.inserted, 1);
    let temp: f32 = sqlx::query_scalar("SELECT temp_f FROM app.observations WHERE station_id = $1")
        .bind(st)
        .fetch_one(&w.db)
        .await
        .unwrap();
    assert_eq!(temp, 52.0, "the last duplicate wins");
    // A later reading with a NULL does not erase a stored value (COALESCE), a different value updates the row.
    let null_row = poller_lib::mapping::MappedRow {
        ts: t,
        temp_f: None,
        ..Default::default()
    };
    let w2 = poller_lib::obs_write::write_page(&w.env.pool, st, -420, vec![null_row], false)
        .await
        .unwrap();
    assert_eq!((w2.inserted, w2.updated), (0, 0));
    let w3 = poller_lib::obs_write::write_page(&w.env.pool, st, -420, vec![mk(55.0)], false)
        .await
        .unwrap();
    assert_eq!((w3.inserted, w3.updated), (0, 1));
    let _ = Duration::zero();
    w.close().await;
}
