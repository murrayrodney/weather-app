//! `reference_sync` (with the hourly pickup) and `credential_reencrypt` integration tests (03 section 12.2).

mod common;

use chrono::{NaiveDate, TimeZone, Utc};
use common::{slot, taken, Builder, SpyAcis, World};
use famweather_core::acis::AcisError;
use famweather_core::crypto::{encrypt_credential, generate_keypair};
use famweather_core::ratelimit::try_job_lock;
use famweather_core::testdb::role_hook;
use poller_lib::job::{JobFlags, RunOutcome, RunStatus, SkipReason};
use poller_lib::jobs::credential_reencrypt::{reencrypt, swap};
use poller_lib::keys::{CredentialRow, KeyRing};
use poller_lib::{run_named, JobName};
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;
use sqlx::PgPool;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use uuid::Uuid;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

async fn run(w: &World, job: JobName, flags: JobFlags) -> RunOutcome {
    run_named(&w.env, job, None, flags).await.unwrap()
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

async fn demo_references(w: &World) {
    for id in ["USW00099001", "USW00099002", "USW00099003"] {
        w.add_reference(id).await;
    }
}

#[sqlx::test(migrations = false)]
async fn first_load_reads_the_whole_record_marks_trace_and_a_refresh_with_no_rows_changes_nothing(
    db: PgPool,
) {
    let s = slot::<SpyAcis>();
    let w = Builder::new(db).acis_spy(&s).build().await;
    demo_references(&w).await;

    let out = run(&w, JobName::ReferenceSync, JobFlags::default()).await;
    assert_eq!(status(&out), RunStatus::Success, "{out:?}");
    let dt = detail(&out);
    assert_eq!(
        (dt["stations"].as_u64(), dt["full_loads"].as_u64()),
        (Some(3), Some(3))
    );
    for id in ["USW00099001", "USW00099002", "USW00099003"] {
        let years: i64 = w.count(&format!("SELECT count(DISTINCT extract(year FROM date)) FROM app.reference_daily WHERE reference_station_id = '{id}'")).await;
        assert!(years >= 12, "{id}: {years} distinct years");
        let last: NaiveDate =
            sqlx::query_scalar("SELECT last_synced_date FROM app.reference_stations WHERE id = $1")
                .bind(id)
                .fetch_one(&w.db)
                .await
                .unwrap();
        assert_eq!(last, d(2026, 9, 30), "the last date returned");
    }
    // Trace: precip NULL and precip_trace true (never 0 here; the API treats trace as 0 in sums).
    assert!(
        w.count("SELECT count(*) FROM app.reference_daily WHERE precip_trace")
            .await
            > 0
    );
    assert_eq!(
        w.count(
            "SELECT count(*) FROM app.reference_daily WHERE precip_trace AND precip_in IS NOT NULL"
        )
        .await,
        0
    );
    assert!(
        w.count("SELECT count(*) FROM app.reference_daily WHERE quality_flags LIKE '%p:T%'")
            .await
            > 0
    );
    assert!(
        w.count("SELECT count(*) FROM app.reference_daily WHERE quality_flags LIKE '%p:A%'")
            .await
            > 0,
        "accumulated precipitation flagged"
    );
    let first = taken(&s).calls.lock().unwrap().first().cloned().unwrap();
    assert_eq!(first.1, d(1991, 1, 1), "full load from 1991");
    assert_eq!(first.2, d(2026, 10, 16), "through yesterday (UTC)");

    // Second run: sdate = min(last + 1, yesterday - 13) = 2026-10-01; the fixture ends 09-30, so no rows come back:
    // last_synced_date stays and nothing is rewritten.
    let before: i64 = w.count("SELECT count(*) FROM app.reference_daily").await;
    let out = run(&w, JobName::ReferenceSync, JobFlags::default()).await;
    assert_eq!(status(&out), RunStatus::Success);
    assert_eq!(detail(&out)["empty_results"], 3);
    assert_eq!(
        taken(&s).calls.lock().unwrap().last().unwrap().1,
        d(2026, 10, 1)
    );
    assert_eq!(
        w.count("SELECT count(*) FROM app.reference_daily").await,
        before
    );
    let last: NaiveDate = sqlx::query_scalar(
        "SELECT last_synced_date FROM app.reference_stations WHERE id = 'USW00099001'",
    )
    .fetch_one(&w.db)
    .await
    .unwrap();
    assert_eq!(last, d(2026, 9, 30));
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn a_recent_station_rereads_14_days_a_stale_one_catches_up_and_a_revision_is_picked_up(
    db: PgPool,
) {
    let s = slot::<SpyAcis>();
    // "Now" is 2026-10-01, so yesterday is the last fixture day, 2026-09-30.
    let w = Builder::new(db)
        .acis_spy(&s)
        .clock_at(Utc.with_ymd_and_hms(2026, 10, 1, 13, 0, 0).unwrap())
        .build()
        .await;
    w.add_reference("USW00099001").await;
    run(&w, JobName::ReferenceSync, JobFlags::default()).await;
    let spy = taken(&s);

    // Recent: last_synced = 09-30 = yesterday -> sdate = yesterday - 13 days = 09-17; 14 rows come back, nothing changes.
    let out = run(&w, JobName::ReferenceSync, JobFlags::default()).await;
    assert_eq!(spy.calls.lock().unwrap().last().unwrap().1, d(2026, 9, 17));
    assert_eq!(detail(&out)["rows_seen"], 14);
    assert_eq!(
        detail(&out)["rows_written"],
        0,
        "unchanged rows are not rewritten"
    );

    // Revised value in the re-read window is stored.
    let tmax_before: f32 = sqlx::query_scalar("SELECT tmax_f FROM app.reference_daily WHERE reference_station_id = 'USW00099001' AND date = '2026-09-25'").fetch_one(&w.db).await.unwrap();
    *spy.tmax_bump.lock().unwrap() = 5.0;
    let out = run(&w, JobName::ReferenceSync, JobFlags::default()).await;
    assert!(
        detail(&out)["rows_written"].as_u64().unwrap() >= 13,
        "{}",
        detail(&out)
    );
    let tmax_after: f32 = sqlx::query_scalar("SELECT tmax_f FROM app.reference_daily WHERE reference_station_id = 'USW00099001' AND date = '2026-09-25'").fetch_one(&w.db).await.unwrap();
    assert_eq!(tmax_after, tmax_before + 5.0);
    *spy.tmax_bump.lock().unwrap() = 0.0;

    // Stale: last synced long ago -> catch up from the day after.
    sqlx::query("UPDATE app.reference_stations SET last_synced_date = '2026-08-01'")
        .execute(&w.db)
        .await
        .unwrap();
    run(&w, JobName::ReferenceSync, JobFlags::default()).await;
    assert_eq!(spy.calls.lock().unwrap().last().unwrap().1, d(2026, 8, 2));
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn the_hourly_pickup_loads_only_never_synced_stations_and_is_silent_when_idle(db: PgPool) {
    let pings = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&pings)
        .await;
    let w = Builder::new(db)
        .ping("REFERENCE_SYNC", format!("{}/ref", pings.uri()))
        .build()
        .await;
    demo_references(&w).await;
    run(&w, JobName::ReferenceSync, JobFlags::default()).await;
    let runs = w
        .count("SELECT count(*) FROM app.job_runs WHERE job = 'reference_sync'")
        .await;
    let pings_before = pings.received_requests().await.unwrap().len();

    // Nothing to load: no run row, no ping.
    let pickup = JobFlags {
        pickup_only: true,
        ..JobFlags::default()
    };
    let out = run(&w, JobName::ReferenceSync, pickup.clone()).await;
    assert!(
        matches!(out, RunOutcome::Skipped(SkipReason::NothingToDo)),
        "{out:?}"
    );
    assert_eq!(
        w.count("SELECT count(*) FROM app.job_runs WHERE job = 'reference_sync'")
            .await,
        runs
    );
    assert_eq!(pings.received_requests().await.unwrap().len(), pings_before);

    // A baseline an owner just chose (never synced) is fully loaded by the pickup.
    sqlx::query("INSERT INTO app.reference_stations (id, name, lat, lon, timezone, std_utc_offset_minutes) VALUES ('USW00099004','New Baseline',40.4,-105.0,'America/Denver',-420)")
        .execute(&w.db).await.unwrap();
    // The pickup never overlaps the daily run: while the lock is held it is skipped.
    let held = try_job_lock(
        &w.env.db_opts,
        "reference_sync",
        Some(role_hook("app_poller").unwrap()),
    )
    .await
    .unwrap()
    .unwrap();
    let out = run(&w, JobName::ReferenceSync, pickup.clone()).await;
    assert!(matches!(out, RunOutcome::Skipped(SkipReason::LockHeld)));
    held.release().await.unwrap();
    // Its fixture is missing (USW00099004): it replays empty, succeeds and stays NULL-synced.
    let out = run(&w, JobName::ReferenceSync, pickup).await;
    assert_eq!(status(&out), RunStatus::Success);
    assert_eq!(detail(&out)["stations"], 1, "only the never-synced station");
    assert!(
        pings.received_requests().await.unwrap().len() > pings_before,
        "a real pickup run pings"
    );
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn a_pickup_with_a_fixture_loads_the_new_station_within_the_hour(db: PgPool) {
    let w = Builder::new(db).build().await;
    demo_references(&w).await;
    sqlx::query("UPDATE app.reference_stations SET last_synced_date = '2026-09-30' WHERE id <> 'USW00099003'").execute(&w.db).await.unwrap();
    let out = run(
        &w,
        JobName::ReferenceSync,
        JobFlags {
            pickup_only: true,
            ..JobFlags::default()
        },
    )
    .await;
    assert_eq!(status(&out), RunStatus::Success);
    assert_eq!(detail(&out)["full_loads"], 1);
    assert!(
        w.count(
            "SELECT count(*) FROM app.reference_daily WHERE reference_station_id = 'USW00099003'"
        )
        .await
            > 4000
    );
    assert_eq!(
        w.count(
            "SELECT count(*) FROM app.reference_daily WHERE reference_station_id <> 'USW00099003'"
        )
        .await,
        0
    );
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn a_station_without_a_fixture_replays_empty_and_succeeds_without_retries(db: PgPool) {
    let s = slot::<SpyAcis>();
    let w = Builder::new(db).acis_spy(&s).build().await;
    w.add_reference("USW00094035").await;
    let out = run(&w, JobName::ReferenceSync, JobFlags::default()).await;
    assert_eq!(status(&out), RunStatus::Success, "{out:?}");
    assert_eq!(detail(&out)["empty_results"], 1);
    assert_eq!(
        taken(&s).attempts.load(Ordering::SeqCst),
        1,
        "no retry storm"
    );
    let last: Option<NaiveDate> = sqlx::query_scalar(
        "SELECT last_synced_date FROM app.reference_stations WHERE id = 'USW00094035'",
    )
    .fetch_one(&w.db)
    .await
    .unwrap();
    assert_eq!(last, None);
    assert_eq!(w.count("SELECT count(*) FROM app.reference_daily").await, 0);
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn acis_5xx_is_retried_three_times_then_that_station_fails_and_the_others_continue(
    db: PgPool,
) {
    let s = slot::<SpyAcis>();
    let w = Builder::new(db).acis_spy(&s).build().await;
    demo_references(&w).await;
    *taken(&s).fail.lock().unwrap() = Some(AcisError::Unavailable);
    let out = run(&w, JobName::ReferenceSync, JobFlags::default()).await;
    assert_eq!(status(&out), RunStatus::Failed);
    assert_eq!(detail(&out)["failed_units"], 3, "every station was tried");
    assert_eq!(
        taken(&s).attempts.load(Ordering::SeqCst),
        3 * 4,
        "1 attempt + 3 retries per station"
    );
    // A bad (4xx / undecodable) answer is not retried.
    taken(&s).attempts.store(0, Ordering::SeqCst);
    *taken(&s).fail.lock().unwrap() = Some(AcisError::BadResponse);
    run(&w, JobName::ReferenceSync, JobFlags::default()).await;
    assert_eq!(taken(&s).attempts.load(Ordering::SeqCst), 3);
    w.close().await;
}

// ---------------------------------------------------------------------------------------------------------------
// credential_reencrypt

struct Rotation {
    w: World,
    pk1: famweather_core::crypto::CredentialPublicKey,
    pk3: famweather_core::crypto::CredentialPublicKey,
}

async fn insert_cred(
    w: &World,
    pk: &famweather_core::crypto::CredentialPublicKey,
    version: i16,
    status: &str,
    plain: &str,
) -> Uuid {
    let owner = w.add_owner(&format!("Owner {}", Uuid::new_v4())).await;
    let id = Uuid::new_v4();
    let enc = encrypt_credential(pk, version, id, owner, &SecretString::from(plain)).unwrap();
    sqlx::query("INSERT INTO app.credentials (id, owner_id, api_key_ciphertext, encapsulated_key, key_version, status) VALUES ($1,$2,$3,$4,$5,$6)")
        .bind(id).bind(owner).bind(enc.ciphertext).bind(enc.encapsulated_key).bind(version).bind(status)
        .execute(&w.db).await.unwrap();
    id
}

async fn rotation(db: PgPool) -> (Rotation, Vec<(Uuid, &'static str)>, Uuid) {
    let mut w = Builder::new(db).build().await;
    let (pk1, sk1) = generate_keypair();
    let (_pk2, sk2) = generate_keypair();
    let (pk3, _sk3) = generate_keypair();
    // current = version 2, previous = version 1
    w.keys = Arc::new(KeyRing::new((2, sk2.clone()), Some((1, sk1))).unwrap());
    w.env.keys = Some(w.keys.clone());
    let pk2 = famweather_core::crypto::public_key_from_private(&sk2);
    let a = insert_cred(&w, &pk1, 1, "ok", "key-for-owner-a-0001").await;
    let b = insert_cred(&w, &pk2, 2, "ok", "key-for-owner-b-0002").await;
    let replaced = insert_cred(&w, &pk1, 1, "replaced", "key-for-replaced-0003").await;
    let above = insert_cred(&w, &pk3, 3, "ok", "key-for-future-0004").await;
    (
        Rotation { w, pk1, pk3 },
        vec![
            (a, "key-for-owner-a-0001"),
            (b, "key-for-owner-b-0002"),
            (replaced, "key-for-replaced-0003"),
        ],
        above,
    )
}

async fn cred_row(w: &World, id: Uuid) -> CredentialRow {
    let r: (Uuid, Vec<u8>, Vec<u8>, i16) = sqlx::query_as("SELECT owner_id, api_key_ciphertext, encapsulated_key, key_version FROM app.credentials WHERE id = $1").bind(id).fetch_one(&w.db).await.unwrap();
    CredentialRow {
        id,
        owner_id: r.0,
        ciphertext: r.1,
        encapsulated_key: r.2,
        key_version: r.3,
    }
}

#[sqlx::test(migrations = false)]
async fn rotation_rewrites_every_old_row_including_replaced_and_only_reports_newer_ones(
    db: PgPool,
) {
    let (rot, rows, above) = rotation(db).await;
    let w = &rot.w;
    let _ = (&rot.pk1, &rot.pk3);
    let above_before = cred_row(w, above).await;

    let out = run(w, JobName::CredentialReencrypt, JobFlags::default()).await;
    assert_eq!(status(&out), RunStatus::Success, "{out:?}");
    let dt = detail(&out);
    assert_eq!(
        (
            dt["reencrypted"].as_u64(),
            dt["skipped_cas"].as_u64(),
            dt["failed"].as_u64()
        ),
        (Some(2), Some(0), Some(0)),
        "{dt}"
    );
    assert_eq!(dt["above_current"], 1);
    assert_eq!(dt["by_version"]["2"], 3, "{dt}");
    assert_eq!(dt["by_version"]["3"], 1);
    assert_eq!(dt["by_version"].get("1"), None);

    for (id, plain) in &rows {
        let row = cred_row(w, *id).await;
        assert_eq!(row.key_version, 2);
        assert_eq!(
            w.keys.decrypt_row(&row).unwrap().expose_secret(),
            *plain,
            "decrypts to the original"
        );
    }
    let above_after = cred_row(w, above).await;
    assert_eq!(
        (above_after.key_version, above_after.ciphertext),
        (3, above_before.ciphertext),
        "never touched"
    );

    // audit rows: only for the two rewritten rows, no secrets
    assert_eq!(w.count("SELECT count(*) FROM app.audit_log WHERE action = 'credential.reencrypted' AND subject_type = 'credential'").await, 2);
    let detail_json: Value = sqlx::query_scalar(
        "SELECT detail FROM app.audit_log WHERE action = 'credential.reencrypted' LIMIT 1",
    )
    .fetch_one(&w.db)
    .await
    .unwrap();
    assert_eq!(
        detail_json,
        serde_json::json!({"from_version": 1, "to_version": 2})
    );

    // A rerun changes nothing.
    let snapshot: Vec<(Uuid, Vec<u8>)> =
        sqlx::query_as("SELECT id, api_key_ciphertext FROM app.credentials ORDER BY id")
            .fetch_all(&w.db)
            .await
            .unwrap();
    let again = run(w, JobName::CredentialReencrypt, JobFlags::default()).await;
    assert_eq!(detail(&again)["reencrypted"], 0);
    let after: Vec<(Uuid, Vec<u8>)> =
        sqlx::query_as("SELECT id, api_key_ciphertext FROM app.credentials ORDER BY id")
            .fetch_all(&w.db)
            .await
            .unwrap();
    assert_eq!(snapshot, after);
    assert_eq!(
        w.count("SELECT count(*) FROM app.audit_log WHERE action = 'credential.reencrypted'")
            .await,
        2
    );
}

#[sqlx::test(migrations = false)]
async fn a_row_altered_between_read_and_update_fails_the_three_part_cas_and_is_skipped(db: PgPool) {
    let (rot, rows, _above) = rotation(db).await;
    let w = &rot.w;
    let (id, _) = rows[0];
    let row = cred_row(w, id).await;
    let plan = reencrypt(&w.keys, &row).unwrap();
    // Someone changes the ciphertext after we read it.
    let mut changed = row.ciphertext.clone();
    changed[0] ^= 0xff;
    sqlx::query("UPDATE app.credentials SET api_key_ciphertext = $2 WHERE id = $1")
        .bind(id)
        .bind(&changed)
        .execute(&w.db)
        .await
        .unwrap();
    let swapped = swap(&w.env.pool, id, &plan).await.unwrap();
    assert!(!swapped, "zero rows updated: skipped");
    let now_row = cred_row(w, id).await;
    assert_eq!(
        (now_row.key_version, now_row.ciphertext),
        (1, changed),
        "left as the other writer made it"
    );
    assert_eq!(
        w.count("SELECT count(*) FROM app.audit_log WHERE action = 'credential.reencrypted'")
            .await,
        0
    );
}

#[sqlx::test(migrations = false)]
async fn a_row_that_cannot_be_decrypted_fails_the_job_but_others_are_rewritten(db: PgPool) {
    let (rot, rows, _above) = rotation(db).await;
    let w = &rot.w;
    // Corrupt one version-1 row's ciphertext (still within the column's size check).
    let (bad, _) = rows[0];
    sqlx::query("UPDATE app.credentials SET api_key_ciphertext = decode(repeat('ab', 40), 'hex') WHERE id = $1").bind(bad).execute(&w.db).await.unwrap();
    let out = run(w, JobName::CredentialReencrypt, JobFlags::default()).await;
    assert_eq!(status(&out), RunStatus::Failed);
    let dt = detail(&out);
    assert_eq!(
        (dt["failed"].as_u64(), dt["reencrypted"].as_u64()),
        (Some(1), Some(1)),
        "{dt}"
    );
    assert_eq!(
        cred_row(w, bad).await.key_version,
        1,
        "the unreadable row is untouched"
    );
    let (replaced, plain) = rows[2];
    assert_eq!(
        w.keys
            .decrypt_row(&cred_row(w, replaced).await)
            .unwrap()
            .expose_secret(),
        plain
    );
}
