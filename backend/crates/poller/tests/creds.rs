//! Credential rules, backoff and redaction (03 sections 5.3 and 12.1 to 12.3), driven through `awn_ingest`.

mod common;

use std::sync::atomic::Ordering;
use std::time::Duration;

use chrono::{DateTime, Utc};
use common::{capture_logs, slot, taken, Builder, CountingAwn, FlakyAwn, World, POLLER_KEY};
use famweather_core::awn::{AwnError, LiveAwn};
use poller_lib::creds::{mark_invalid, set_status, touch_last_ok};
use poller_lib::job::{JobFlags, RunOutcome, RunStatus};
use poller_lib::{run_named, JobName};
use secrecy::SecretString;
use serde_json::Value;
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn ingest(w: &World) -> RunOutcome {
    run_named(&w.env, JobName::AwnIngest, None, JobFlags::default())
        .await
        .unwrap()
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

async fn cred_status(w: &World, id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM app.credentials WHERE id = $1")
        .bind(id)
        .fetch_one(&w.db)
        .await
        .unwrap()
}

/// Two credentials with controlled ids (so the processing order is known): A holds `key_a`, B holds `key_b`.
async fn two_credentials(w: &World, a_first: bool, key_a: &str, key_b: &str) -> (Uuid, Uuid) {
    let (lo, hi) = (Uuid::from_u128(1), Uuid::from_u128(2));
    let (a, b) = if a_first { (lo, hi) } else { (hi, lo) };
    let (oa, ob) = (w.add_owner("Owner A").await, w.add_owner("Owner B").await);
    w.add_credential_with(a, oa, key_a, 1, "ok").await;
    w.add_credential_with(b, ob, key_b, 1, "ok").await;
    w.add_station(oa, a, "AA:BB:CC:00:00:01").await;
    w.add_station(ob, b, "AA:BB:CC:00:00:02").await;
    (a, b)
}

async fn invalid_only_if_another_succeeded(db: PgPool, a_first: bool) {
    let w = Builder::new(db).build().await;
    let (a, b) = two_credentials(&w, a_first, "bogus-key-for-a", POLLER_KEY).await;
    let out = ingest(&w).await;
    assert_eq!(status(&out), RunStatus::Failed, "A's station failed");
    let dt = detail(&out);
    assert_eq!(dt["marked_invalid"], 1, "a_first={a_first}: {dt}");
    assert_eq!(cred_status(&w, a).await, "invalid");
    assert_eq!(cred_status(&w, b).await, "ok");
    assert_eq!(
        w.count("SELECT count(*) FROM app.audit_log WHERE action = 'credential.invalid'")
            .await,
        1
    );
    let subject: Uuid = sqlx::query_scalar(
        "SELECT subject_id FROM app.audit_log WHERE action = 'credential.invalid'",
    )
    .fetch_one(&w.db)
    .await
    .unwrap();
    assert_eq!(subject, a);
    // Invalid credentials are not selected again.
    let again = ingest(&w).await;
    assert_eq!(detail(&again)["credentials"], 1);
    assert_eq!(
        w.count("SELECT count(*) FROM app.audit_log WHERE action = 'credential.invalid'")
            .await,
        1,
        "no second audit row"
    );
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn a_401_credential_is_invalid_only_if_another_succeeded_when_it_is_processed_first(
    db: PgPool,
) {
    invalid_only_if_another_succeeded(db, true).await;
}

#[sqlx::test(migrations = false)]
async fn a_401_credential_is_invalid_only_if_another_succeeded_when_it_is_processed_last(
    db: PgPool,
) {
    invalid_only_if_another_succeeded(db, false).await;
}

#[sqlx::test(migrations = false)]
async fn when_every_credential_fails_with_401_none_is_marked_and_the_run_names_the_suspect(
    db: PgPool,
) {
    let w = Builder::new(db).build().await;
    let (a, b) = two_credentials(&w, true, "bogus-key-for-a", "bogus-key-for-b").await;
    let out = ingest(&w).await;
    assert_eq!(status(&out), RunStatus::Failed);
    assert_eq!(detail(&out)["error"], "suspected_application_key_failure");
    assert_eq!(
        (cred_status(&w, a).await, cred_status(&w, b).await),
        ("ok".into(), "ok".into())
    );
    assert_eq!(w.count("SELECT count(*) FROM app.audit_log").await, 0);
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn a_lone_401_credential_is_never_marked_invalid(db: PgPool) {
    let w = Builder::new(db).build().await;
    let owner = w.add_owner("Lone").await;
    let cred = w.add_credential(owner, "bogus-key-for-lone").await;
    w.add_station(owner, cred, "AA:BB:CC:00:00:01").await;
    let out = ingest(&w).await;
    assert_eq!(status(&out), RunStatus::Failed);
    assert_eq!(detail(&out)["error"], "suspected_application_key_failure");
    assert_eq!(cred_status(&w, cred).await, "ok");
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn credential_status_writes_are_guarded_minimal_and_audit_only_real_changes(db: PgPool) {
    let w = Builder::new(db).build().await;
    let pool = &w.env.pool;
    let owner = w.add_owner("Guarded").await;
    let live = Uuid::new_v4();
    w.add_credential_with(live, owner, POLLER_KEY, 1, "ok")
        .await;
    let owner2 = w.add_owner("Other").await;
    let replaced = Uuid::new_v4();
    w.add_credential_with(replaced, owner2, POLLER_KEY, 1, "replaced")
        .await;
    let owner3 = w.add_owner("Third").await;
    let invalid = Uuid::new_v4();
    w.add_credential_with(invalid, owner3, POLLER_KEY, 1, "invalid")
        .await;

    let changed_at = |id: Uuid| {
        let db = w.db.clone();
        async move {
            sqlx::query_scalar::<_, DateTime<Utc>>(
                "SELECT status_changed_at FROM app.credentials WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&db)
            .await
            .unwrap()
        }
    };
    // Unchanged value: no row changes, status_changed_at stays.
    let t0 = changed_at(live).await;
    assert!(!set_status(pool, live, "ok").await.unwrap());
    assert_eq!(changed_at(live).await, t0);
    // A real change moves it.
    assert!(set_status(pool, live, "rate_limited").await.unwrap());
    assert!(changed_at(live).await >= t0);
    assert!(set_status(pool, live, "ok").await.unwrap());
    // Replaced and invalid rows are never touched, never audited.
    for id in [replaced, invalid] {
        let before = cred_status(&w, id).await;
        assert!(!set_status(pool, id, "ok").await.unwrap());
        assert!(!set_status(pool, id, "rate_limited").await.unwrap());
        assert!(!mark_invalid(pool, id).await.unwrap());
        touch_last_ok(pool, id).await.unwrap();
        assert_eq!(cred_status(&w, id).await, before);
    }
    assert_eq!(w.count("SELECT count(*) FROM app.credentials WHERE status IN ('replaced','invalid') AND last_ok_at IS NOT NULL").await, 0);
    assert_eq!(w.count("SELECT count(*) FROM app.audit_log").await, 0);
    // last_ok_at has its own statement and does not move status_changed_at.
    let t1 = changed_at(live).await;
    touch_last_ok(pool, live).await.unwrap();
    assert_eq!(changed_at(live).await, t1);
    assert!(mark_invalid(pool, live).await.unwrap());
    assert!(
        !mark_invalid(pool, live).await.unwrap(),
        "already invalid: no second change, no second audit row"
    );
    assert_eq!(
        w.count("SELECT count(*) FROM app.audit_log WHERE action = 'credential.invalid'")
            .await,
        1
    );
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn two_429s_then_200_succeeds_and_the_status_stays_ok(db: PgPool) {
    let f = Arc::new(std::sync::Mutex::new(None::<Arc<FlakyAwn>>));
    let f2 = f.clone();
    let w = Builder::new(db)
        .awn(move |inner| {
            let flaky = FlakyAwn::devices(inner, AwnError::RateLimited { retry_after: None }, 2);
            *f2.lock().unwrap() = Some(flaky.clone());
            flaky
        })
        .build()
        .await;
    let owner = w.add_owner("Rodney").await;
    let cred = w.add_credential(owner, POLLER_KEY).await;
    w.add_station(owner, cred, "AA:BB:CC:00:00:01").await;
    let out = ingest(&w).await;
    assert_eq!(status(&out), RunStatus::Success, "{out:?}");
    assert_eq!(
        f.lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .device_calls
            .load(Ordering::SeqCst),
        3
    );
    assert_eq!(cred_status(&w, cred).await, "ok");
    assert_eq!(w.count("SELECT count(*) FROM app.observations").await, 288);
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn endless_429_gives_up_after_five_attempts_marks_rate_limited_and_other_credentials_continue(
    db: PgPool,
) {
    let c = slot::<CountingAwn>();
    let w = Builder::new(db).awn_counting(&c).build().await;
    // A is rate limited at AWN (the replay key for that); B is fine.
    let (a, b) = two_credentials(&w, true, "fixture-ratelimited-key", POLLER_KEY).await;
    let out = ingest(&w).await;
    assert_eq!(
        status(&out),
        RunStatus::Failed,
        "A's station could not be ingested"
    );
    let dt = detail(&out);
    assert_eq!(dt["rate_limited"], 1);
    assert_eq!(
        taken(&c).devices_count(),
        5 + 1,
        "five attempts for A, one call for B"
    );
    assert_eq!(cred_status(&w, a).await, "rate_limited");
    assert_eq!(cred_status(&w, b).await, "ok");
    assert_eq!(
        w.count("SELECT count(*) FROM app.observations").await,
        288,
        "B's station was ingested"
    );
    // A rate_limited credential is still used (and recovers) once AWN answers.
    sqlx::query("UPDATE app.credentials SET status = 'rate_limited' WHERE id = $1")
        .bind(b)
        .execute(&w.db)
        .await
        .unwrap();
    let again = ingest(&w).await;
    assert_eq!(
        detail(&again)["credentials"],
        2,
        "ok and rate_limited are both selected"
    );
    assert_eq!(
        cred_status(&w, b).await,
        "ok",
        "a later success sets status back to ok"
    );
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn unavailable_is_retried_three_times_then_that_unit_fails_while_the_others_continue(
    db: PgPool,
) {
    // The first station's history fails 4 times (1 attempt + 3 retries), then AWN recovers for the second.
    let f = Arc::new(std::sync::Mutex::new(None::<Arc<FlakyAwn>>));
    let f2 = f.clone();
    let w = Builder::new(db)
        .awn(move |inner| {
            let flaky = FlakyAwn::history(inner, AwnError::Unavailable, 4);
            *f2.lock().unwrap() = Some(flaky.clone());
            flaky
        })
        .build()
        .await;
    let owner = w.add_owner("Rodney").await;
    let cred = w.add_credential(owner, POLLER_KEY).await;
    let s1 = w.add_station(owner, cred, "AA:BB:CC:00:00:01").await;
    let s2 = w.add_station(owner, cred, "AA:BB:CC:00:00:02").await;
    let out = ingest(&w).await;
    assert_eq!(status(&out), RunStatus::Failed);
    assert_eq!(detail(&out)["failed_units"], 1);
    assert_eq!(
        f.lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .history_calls
            .load(Ordering::SeqCst),
        5,
        "4 failures for the first station, 1 success for the second"
    );
    let n = |s: Uuid| {
        let db = w.db.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM app.observations WHERE station_id = $1",
            )
            .bind(s)
            .fetch_one(&db)
            .await
            .unwrap()
        }
    };
    assert_eq!((n(s1).await, n(s2).await), (0, 288));
    // A failed unit is not a credential problem.
    assert_eq!(cred_status(&w, cred).await, "ok");
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn a_busy_gate_is_a_transient_unit_failure_and_the_key_gate_is_reserved_before_the_app_gate(
    db: PgPool,
) {
    let w = Builder::new(db).build().await;
    let owner = w.add_owner("Rodney").await;
    let cred = w.add_credential(owner, POLLER_KEY).await;
    w.add_station(owner, cred, "AA:BB:CC:00:00:01").await;
    // The app gate is booked for the next hour; the wait would exceed max_wait (30 s in tests).
    sqlx::query("INSERT INTO app.rate_gates (name, next_allowed_at) VALUES ('awn:app:poller', now() + interval '1 hour')").execute(&w.db).await.unwrap();
    let out = ingest(&w).await;
    assert_eq!(status(&out), RunStatus::Failed);
    assert_eq!(
        cred_status(&w, cred).await,
        "ok",
        "a busy gate never changes credential state"
    );
    assert_eq!(
        w.count(&format!(
            "SELECT count(*) FROM app.rate_gates WHERE name = 'awn:key:{cred}'"
        ))
        .await,
        1,
        "the key gate was reserved first"
    );
    assert_eq!(w.count("SELECT count(*) FROM app.observations").await, 0);
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn gate_reservations_space_the_requests_of_one_credential(db: PgPool) {
    // Real (test-overridden) spacing: 3 stations of one credential make 4 AWN calls (devices + 3 histories); the key
    // gate then advances by 4 slots of its spacing.
    let mut w = Builder::new(db).build().await;
    w.env = w.env.clone().with_gates(poller_lib::env::GateConfig {
        key_spacing: Duration::from_millis(40),
        app_spacing: Duration::from_millis(20),
        max_wait: Duration::from_secs(30),
    });
    let owner = w.add_owner("Rodney").await;
    let cred = w.add_credential(owner, POLLER_KEY).await;
    for m in [
        "AA:BB:CC:00:00:01",
        "AA:BB:CC:00:00:02",
        "AA:BB:CC:00:00:03",
    ] {
        w.add_station(owner, cred, m).await;
    }
    let started = std::time::Instant::now();
    let out = ingest(&w).await;
    assert_eq!(status(&out), RunStatus::Success);
    assert!(
        started.elapsed() >= Duration::from_millis(3 * 40),
        "four calls at 40 ms spacing take at least 120 ms: {:?}",
        started.elapsed()
    );
    w.close().await;
}

// ---------------------------------------------------------------------------------------------------------------
// Redaction (S7): no key, application key, query string or URL anywhere, with the real HTTP client

#[sqlx::test(migrations = false)]
async fn no_secret_url_or_query_string_reaches_logs_or_job_detail_whatever_awn_does(db: PgPool) {
    const USER_KEY: &str = "TESTKEY-abcdef123456";
    const APP_KEY: &str = "APPKEY-0123456789abcdef";
    const PING_ID: &str = "7d2f0c1e-1111-4222-8333-944455556666";
    let server = MockServer::start().await;
    let pings = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&pings)
        .await;

    let base = url::Url::parse(&format!("{}/", server.uri())).unwrap();
    let live = Arc::new(
        LiveAwn::with_base(
            base,
            SecretString::from(APP_KEY),
            Duration::from_millis(300),
        )
        .unwrap(),
    );
    let w = {
        let live = live.clone();
        Builder::new(db)
            .awn(move |_| live)
            .ping("AWN_INGEST", format!("{}/{PING_ID}", pings.uri()))
            .build()
            .await
    };
    let owner = w.add_owner("Rodney").await;
    let cred = w.add_credential(owner, USER_KEY).await;
    w.add_station(owner, cred, "AA:BB:CC:00:00:01").await;

    let (_g, logs) = capture_logs();
    // Debug output of the key and the client.
    let key = famweather_core::awn::ApiKey::new(SecretString::from(USER_KEY));
    let debug = format!("{key:?} {live:?}");

    let scenarios: Vec<(&str, ResponseTemplate)> = vec![
        ("server error", ResponseTemplate::new(500)),
        (
            "timeout",
            ResponseTemplate::new(200).set_delay(Duration::from_secs(2)),
        ),
        (
            "undecodable body",
            ResponseTemplate::new(200)
                .set_body_string("<html>TESTKEY-abcdef123456 http://x</html>"),
        ),
        (
            "unauthorized",
            ResponseTemplate::new(401).set_body_string("bad apiKey=TESTKEY-abcdef123456"),
        ),
        ("rate limited", ResponseTemplate::new(429)),
    ];
    let mut details = Vec::new();
    for (name, tpl) in scenarios {
        server.reset().await;
        Mock::given(path("/devices"))
            .respond_with(tpl)
            .mount(&server)
            .await;
        let out = ingest(&w).await;
        assert_ne!(status(&out), RunStatus::Success, "{name}");
        details.push(detail(&out).to_string());
        w.clock.advance(chrono::Duration::seconds(1));
    }
    // A connection that is refused outright.
    drop(server);
    let out = ingest(&w).await;
    details.push(detail(&out).to_string());

    let stored: Vec<Value> = sqlx::query_scalar("SELECT detail FROM app.job_runs")
        .fetch_all(&w.db)
        .await
        .unwrap();
    let everything = format!(
        "{}\n{}\n{}\n{debug}",
        logs.text(),
        details.join("\n"),
        stored
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
    // (hyper's own TRACE lines name the mock server's host:port; they are silenced by the default filter and carry no
    // path, query or key, so the host itself is not a needle.)
    for needle in [
        USER_KEY,
        APP_KEY,
        "apiKey=",
        "applicationKey=",
        "http://",
        "https://",
        PING_ID,
    ] {
        assert!(
            !everything.contains(needle),
            "{needle:?} leaked:\n{everything}"
        );
    }
    assert!(debug.contains("[REDACTED]"));
    w.close().await;
}
