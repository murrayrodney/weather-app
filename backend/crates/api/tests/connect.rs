//! Connect flow (02 4.6, 7.3): validate, save, reconnect, limits, gates, encryption, privileges.

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use common::*;
use famweather_core::awn::{ApiKey, AwnApi, AwnDevice, AwnError, AwnObservation, LiveAwn, Mac};
use secrecy::SecretString;
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "fixture-valid-key";

fn fast(b: &mut Build) {
    b.gates.awn_app_spacing = Duration::from_millis(5);
    b.gates.awn_key_spacing = Duration::from_millis(5);
}

fn dev(mac: &str, name: Option<&str>, tz: Option<&str>, coords: Option<(f64, f64)>) -> AwnDevice {
    AwnDevice {
        mac: Mac::parse(mac).unwrap(),
        name: name.map(str::to_owned),
        coords,
        elevation_ft: Some(5004.0),
        timezone: tz.map(str::to_owned),
    }
}

struct StubAwn(Mutex<Vec<AwnDevice>>);

impl StubAwn {
    fn new(d: Vec<AwnDevice>) -> Arc<Self> {
        Arc::new(Self(Mutex::new(d)))
    }
}

#[async_trait]
impl AwnApi for StubAwn {
    async fn list_devices(&self, _: &ApiKey) -> Result<Vec<AwnDevice>, AwnError> {
        Ok(self.0.lock().unwrap().clone())
    }
    async fn history(
        &self,
        _: &ApiKey,
        _: &Mac,
        _: Option<DateTime<Utc>>,
        _: u16,
    ) -> Result<Vec<AwnObservation>, AwnError> {
        Err(AwnError::Unavailable)
    }
}

fn save_body(key: &str, macs: &[&str]) -> Value {
    json!({ "api_key": key, "devices": macs.iter().map(|m| json!({ "mac": m })).collect::<Vec<_>>() })
}

async fn attempts(w: &World, who: &str) -> Vec<String> {
    sqlx::query_scalar("SELECT outcome FROM app.connect_attempts WHERE user_id = $1 ORDER BY id")
        .bind(w.p(who).id)
        .fetch_all(&w.db)
        .await
        .unwrap()
}

#[sqlx::test(migrations = false)]
async fn validate_outcomes(db: PgPool) {
    let w = World::with(db, fast).await;
    let r = w
        .post(
            "owner",
            "/api/v1/connect/validate",
            json!({ "api_key": KEY }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let j = r.json();
    assert_eq!(j["valid"], true);
    let macs: Vec<_> = j["devices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["mac"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(macs, ["AA:BB:CC:00:01:01", "AA:BB:CC:00:01:02"]);
    assert_eq!(j["devices"][0]["timezone"], "America/Denver");
    assert_eq!(j["devices"][0]["name"], "Fixture Station One");

    let r = w
        .post(
            "owner",
            "/api/v1/connect/validate",
            json!({ "api_key": "fixture-invalid-key" }),
        )
        .await;
    assert_eq!(
        (r.status, r.json()),
        (StatusCode::OK, json!({ "valid": false }))
    );

    let r = w
        .post(
            "owner",
            "/api/v1/connect/validate",
            json!({ "api_key": "fixture-ratelimited-key" }),
        )
        .await;
    assert_eq!(
        (r.status.as_u16(), r.code().as_str()),
        (503, "upstream_busy")
    );
    assert!(r.headers.contains_key("retry-after"));

    // Input validation; there is no way to point the call elsewhere.
    for body in [
        json!({ "api_key": "" }),
        json!({ "api_key": "has space" }),
        json!({ "api_key": "x".repeat(129) }),
        json!({ "api_key": KEY, "host": "evil.test" }),
        json!({ "api_key": KEY, "url": "http://evil.test" }),
        json!({}),
    ] {
        assert_eq!(
            w.post("owner", "/api/v1/connect/validate", body)
                .await
                .status
                .as_u16(),
            422
        );
    }
    // validate writes nothing but the attempt rows.
    let n: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM app.credentials) + (SELECT count(*) FROM app.stations) + (SELECT count(*) FROM app.audit_log)")
        .fetch_one(&w.db).await.unwrap();
    assert_eq!(n, 0);
    assert_eq!(
        attempts(&w, "owner").await,
        ["valid", "invalid", "rate_limited"]
    );
}

#[sqlx::test(migrations = false)]
async fn durable_attempt_limits(db: PgPool) {
    let w = World::with(db, fast).await;
    for _ in 0..5 {
        assert_eq!(
            w.post(
                "owner",
                "/api/v1/connect/validate",
                json!({ "api_key": KEY })
            )
            .await
            .status,
            StatusCode::OK
        );
    }
    let r = w
        .post(
            "owner",
            "/api/v1/connect/validate",
            json!({ "api_key": KEY }),
        )
        .await;
    assert_eq!(
        (r.status.as_u16(), r.code().as_str()),
        (429, "rate_limited")
    );
    let wait: u64 = r.headers["retry-after"].to_str().unwrap().parse().unwrap();
    // Retry-After is the whole seconds until the OLDEST counted row leaves the one-minute window.
    let expected: f64 = sqlx::query_scalar(
        "SELECT extract(epoch FROM (min(attempted_at) + interval '1 minute' - now()))::float8 FROM app.connect_attempts WHERE user_id = $1",
    )
    .bind(w.p("owner").id)
    .fetch_one(&w.db)
    .await
    .unwrap();
    assert!(
        (wait as f64 - expected.ceil()).abs() <= 1.0 && wait >= 1,
        "retry-after {wait} vs {expected}"
    );
    // Rows spread over the window: make the oldest 40 s old, the rest younger. The answer follows the oldest row.
    sqlx::query("UPDATE app.connect_attempts SET attempted_at = now() - interval '10 seconds' WHERE user_id = $1")
        .bind(w.p("owner").id)
        .execute(&w.db)
        .await
        .unwrap();
    sqlx::query("UPDATE app.connect_attempts SET attempted_at = now() - interval '40 seconds' WHERE id = (SELECT min(id) FROM app.connect_attempts WHERE user_id = $1)")
        .bind(w.p("owner").id)
        .execute(&w.db)
        .await
        .unwrap();
    let r = w
        .post(
            "owner",
            "/api/v1/connect/validate",
            json!({ "api_key": KEY }),
        )
        .await;
    let wait: u64 = r.headers["retry-after"].to_str().unwrap().parse().unwrap();
    assert!((19..=21).contains(&wait), "60 - 40 = 20 s, got {wait}");
    // Rejections are not recorded; other users are unaffected; the limit lives in the database, not in the process.
    assert_eq!(attempts(&w, "owner").await.len(), 5);
    assert_eq!(
        w.post(
            "stranger",
            "/api/v1/connect/validate",
            json!({ "api_key": KEY })
        )
        .await
        .status,
        StatusCode::OK
    );
    let again = api::app::build_router(Build::new(w.api.clone()).state(), axum::Router::new());
    let r = send(
        &again,
        "POST",
        "/api/v1/connect/validate",
        Some(&w.tok("owner")),
        Some(json!({ "api_key": KEY })),
    )
    .await;
    assert_eq!(r.status.as_u16(), 429);
    // The hourly window: 20 rows in the hour, none in the last minute.
    sqlx::query("UPDATE app.connect_attempts SET attempted_at = now() - interval '30 minutes' WHERE user_id = $1")
        .bind(w.p("owner").id)
        .execute(&w.db)
        .await
        .unwrap();
    for _ in 0..15 {
        sqlx::query("INSERT INTO app.connect_attempts (user_id, attempted_at, outcome) VALUES ($1, now() - interval '20 minutes', 'valid')")
            .bind(w.p("owner").id)
            .execute(&w.db)
            .await
            .unwrap();
    }
    let r = w
        .post(
            "owner",
            "/api/v1/connect/validate",
            json!({ "api_key": KEY }),
        )
        .await;
    assert_eq!(r.status.as_u16(), 429);
    let wait: u64 = r.headers["retry-after"].to_str().unwrap().parse().unwrap();
    assert!(wait > 60, "the hour window binds: {wait}");
}

#[sqlx::test(migrations = false)]
async fn parallel_validates_serialise_per_user(db: PgPool) {
    let w = World::with(db, fast).await;
    // One attempt below the limit of five: without the per-user advisory lock both parallel calls would count four
    // rows, both pass, and six rows would exist. With it exactly one gets in.
    for _ in 0..4 {
        sqlx::query("INSERT INTO app.connect_attempts (user_id, outcome) VALUES ($1, 'valid')")
            .bind(w.p("owner").id)
            .execute(&w.db)
            .await
            .unwrap();
    }
    let (a, b) = tokio::join!(
        w.post(
            "owner",
            "/api/v1/connect/validate",
            json!({ "api_key": KEY })
        ),
        w.post(
            "owner",
            "/api/v1/connect/validate",
            json!({ "api_key": KEY })
        ),
    );
    let mut codes = [a.status.as_u16(), b.status.as_u16()];
    codes.sort();
    assert_eq!(codes, [200, 429]);
    assert_eq!(attempts(&w, "owner").await.len(), 5);
}

async fn gate_next(w: &World, name: &str) -> Option<DateTime<Utc>> {
    sqlx::query_scalar("SELECT next_allowed_at FROM app.rate_gates WHERE name = $1")
        .bind(name)
        .fetch_optional(&w.db)
        .await
        .unwrap()
}

#[sqlx::test(migrations = false)]
async fn gates_are_reserved_by_the_caller_key_first(db: PgPool) {
    let w = World::new(db).await;
    // No stored credential: only the app gate.
    assert_eq!(
        w.post(
            "owner",
            "/api/v1/connect/validate",
            json!({ "api_key": KEY })
        )
        .await
        .status,
        StatusCode::OK
    );
    assert!(gate_next(&w, "awn:app:api").await.is_some());
    // With a live credential both gates advance.
    let cid = w.add_credential(w.p("stranger").id).await;
    assert_eq!(
        w.post(
            "stranger",
            "/api/v1/connect/validate",
            json!({ "api_key": KEY })
        )
        .await
        .status,
        StatusCode::OK
    );
    assert!(gate_next(&w, &format!("awn:key:{cid}")).await.is_some());
    // Two users in a row are at least one gate spacing apart.
    let before = gate_next(&w, "awn:app:api").await.unwrap();
    assert_eq!(
        w.post(
            "pending",
            "/api/v1/connect/validate",
            json!({ "api_key": KEY })
        )
        .await
        .status,
        StatusCode::OK
    );
    let after = gate_next(&w, "awn:app:api").await.unwrap();
    assert!((after - before).num_milliseconds() >= 1000);
}

#[sqlx::test(migrations = false)]
async fn a_busy_gate_answers_promptly_without_touching_the_app_gate(db: PgPool) {
    let w = World::new(db).await;
    let cid = w.add_credential(w.p("owner").id).await;
    sqlx::query(
        "INSERT INTO app.rate_gates (name, next_allowed_at) VALUES ($1, now() + interval '1 hour')",
    )
    .bind(format!("awn:key:{cid}"))
    .execute(&w.db)
    .await
    .unwrap();
    let t = Instant::now();
    let r = w
        .post(
            "owner",
            "/api/v1/connect/validate",
            json!({ "api_key": KEY }),
        )
        .await;
    assert!(
        t.elapsed() < Duration::from_millis(500),
        "{:?}",
        t.elapsed()
    );
    assert_eq!(
        (r.status.as_u16(), r.code().as_str()),
        (503, "upstream_busy")
    );
    assert_eq!(r.headers["retry-after"], "2");
    assert!(
        gate_next(&w, "awn:app:api").await.is_none(),
        "a busy key gate returns before the app gate is touched"
    );
    assert_eq!(attempts(&w, "owner").await, ["rate_limited"]);
    // The same with the app gate pre-loaded.
    sqlx::query("INSERT INTO app.rate_gates (name, next_allowed_at) VALUES ('awn:app:api', now() + interval '1 hour')")
        .execute(&w.db).await.unwrap();
    let before = gate_next(&w, "awn:app:api").await;
    let t = Instant::now();
    let r = w
        .post(
            "stranger",
            "/api/v1/connect/validate",
            json!({ "api_key": KEY }),
        )
        .await;
    assert!(t.elapsed() < Duration::from_millis(500));
    assert_eq!(r.status.as_u16(), 503);
    assert_eq!(gate_next(&w, "awn:app:api").await, before);
    // No pooled connection is left held. sqlx hands connections back asynchronously, so poll for a short while: a
    // connection that was really held (for example across the gate wait) would never come back.
    let mut idle = false;
    for _ in 0..40 {
        if w.api.num_idle() as u32 == w.api.size() {
            idle = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(idle, "all pooled connections are idle again");
}

#[sqlx::test(migrations = false)]
async fn live_awn_requests_and_failures(db: PgPool) {
    let server = MockServer::start().await;
    let devices = json!([{ "macAddress": "AA:BB:CC:00:09:09", "lastData": { "tz": "America/Denver" },
        "info": { "name": "Mock", "coords": { "coords": { "lat": 40.4, "lon": -105.1 }, "elevation": 5000.0 } } }]);
    Mock::given(method("GET"))
        .and(path("/v1/devices"))
        .respond_with(ResponseTemplate::new(200).set_body_json(devices))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    let base = url::Url::parse(&format!("{}/v1/", server.uri())).unwrap();
    let live: Arc<dyn AwnApi> = Arc::new(
        LiveAwn::with_base(
            base,
            SecretString::from("app-key-0123456789-abcdef".to_owned()),
            Duration::from_secs(1),
        )
        .unwrap(),
    );
    let w = World::with(db, |b| {
        fast(b);
        b.awn = Some(live);
    })
    .await;
    let r = w
        .post(
            "owner",
            "/api/v1/connect/validate",
            json!({ "api_key": "user-key-0123456789" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json()["devices"][0]["mac"], "AA:BB:CC:00:09:09");
    let reqs = server.received_requests().await.unwrap();
    let q = reqs[0].url.query().unwrap().to_owned();
    assert!(
        q.contains("apiKey=user-key-0123456789")
            && q.contains("applicationKey=app-key-0123456789-abcdef")
    );
    // The response never echoes either key.
    assert!(!r.text().contains("user-key") && !r.text().contains("app-key"));

    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    let r = w
        .post(
            "owner",
            "/api/v1/connect/validate",
            json!({ "api_key": "user-key-0123456789" }),
        )
        .await;
    assert_eq!(
        (r.status.as_u16(), r.code().as_str()),
        (502, "upstream_unavailable")
    );
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "7"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    let r = w
        .post(
            "owner",
            "/api/v1/connect/validate",
            json!({ "api_key": "user-key-0123456789" }),
        )
        .await;
    assert_eq!(
        (r.status.as_u16(), r.code().as_str()),
        (503, "upstream_busy")
    );
    assert_eq!(r.headers["retry-after"], "7");
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(3)))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    let r = w
        .post(
            "owner",
            "/api/v1/connect/validate",
            json!({ "api_key": "user-key-0123456789" }),
        )
        .await;
    assert_eq!(r.status.as_u16(), 502, "a timeout is upstream_unavailable");
}

#[sqlx::test(migrations = false)]
async fn first_save_creates_credential_and_stations(db: PgPool) {
    let w = World::with(db, fast).await;
    let r = w
        .post(
            "owner",
            "/api/v1/connect/save",
            save_body(KEY, &["aa:bb:cc:00:01:01", "AA-BB-CC-00-01-02"]),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    let j = r.json();
    assert_eq!(j["credential"], json!({ "status": "ok" }));
    assert_eq!(j["not_covered"], json!([]));
    let st = j["stations"].as_array().unwrap();
    assert_eq!(st.len(), 2);
    assert!(st
        .iter()
        .all(|s| s["baseline"].is_null() && s["is_own"] == true));
    assert_eq!(st[0]["name"], "Fixture Station One");
    assert!(!r.text().contains("AA:BB"), "no MAC is ever returned");

    let row = sqlx::query("SELECT mac, lat::float8 AS lat, lon::float8 AS lon, timezone, std_utc_offset_minutes, elevation_ft, reference_station_id, credential_id FROM app.stations ORDER BY mac")
        .fetch_all(&w.db).await.unwrap();
    assert_eq!(row[0].get::<String, _>("mac"), "AA:BB:CC:00:01:01");
    assert_eq!(row[0].get::<f64, _>("lat"), 40.4);
    assert_eq!(row[0].get::<String, _>("timezone"), "America/Denver");
    assert_eq!(row[0].get::<i16, _>("std_utc_offset_minutes"), -420);
    assert_eq!(row[0].get::<Option<i32>, _>("elevation_ft"), Some(5000));
    assert!(row[0]
        .get::<Option<String>, _>("reference_station_id")
        .is_none());

    // Encryption: ciphertext is the key plus a 16-byte tag, the encapsulated key is 32 bytes, and the key is nowhere else.
    let c = sqlx::query(
        "SELECT id, api_key_ciphertext, encapsulated_key, key_version, status FROM app.credentials",
    )
    .fetch_one(&w.db)
    .await
    .unwrap();
    assert_eq!(
        c.get::<Vec<u8>, _>("api_key_ciphertext").len(),
        KEY.len() + 16
    );
    assert_eq!(c.get::<Vec<u8>, _>("encapsulated_key").len(), 32);
    assert_eq!(c.get::<i16, _>("key_version"), 1);
    assert_eq!(
        c.get::<Uuid, _>("id"),
        row[0].get::<Uuid, _>("credential_id")
    );
    let audit: String = sqlx::query_scalar(
        "SELECT coalesce(string_agg(action || detail::text, ' '), '') FROM app.audit_log",
    )
    .fetch_one(&w.db)
    .await
    .unwrap();
    assert!(!audit.contains(KEY));
    assert_eq!(w.audit_count("credential.saved").await, 1);
    assert_eq!(w.audit_count("station.created").await, 2);
    assert_eq!(w.audit_count("credential.replaced").await, 0);

    let s = w.get("owner", "/api/v1/connect/status").await.json();
    assert_eq!(s["credential"]["status"], "ok");
    assert_eq!(s["credential"]["needs_reconnecting"], false);
    assert!(!s.to_string().contains("key_version"));
    assert!(w.get("stranger", "/api/v1/connect/status").await.json()["credential"].is_null());
}

#[sqlx::test(migrations = false)]
async fn save_rejections_write_nothing(db: PgPool) {
    let w = World::with(db, fast).await;
    let none = |w: &World| {
        let db = w.db.clone();
        async move {
            let n: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM app.credentials) + (SELECT count(*) FROM app.stations)")
                .fetch_one(&db).await.unwrap();
            n == 0
        }
    };
    let r = w
        .post(
            "owner",
            "/api/v1/connect/save",
            save_body(KEY, &["AA:BB:CC:00:01:01", "AA:BB:CC:EE:EE:EE"]),
        )
        .await;
    assert_eq!(
        (r.status.as_u16(), r.code().as_str()),
        (422, "unknown_device")
    );
    let r = w
        .post(
            "owner",
            "/api/v1/connect/save",
            save_body(KEY, &["AA:BB:CC:00:01:01", "aa:bb:cc:00:01:01"]),
        )
        .await;
    assert_eq!(r.status.as_u16(), 422);
    let r = w
        .post(
            "owner",
            "/api/v1/connect/save",
            save_body("fixture-invalid-key", &["AA:BB:CC:00:01:01"]),
        )
        .await;
    assert_eq!((r.status.as_u16(), r.code().as_str()), (422, "invalid_key"));
    for body in [
        save_body(KEY, &[]),
        save_body(KEY, &["nonsense"]),
        json!({ "api_key": KEY, "devices": [{ "mac": "AA:BB:CC:00:01:01", "coords": [1, 2] }] }),
    ] {
        assert_eq!(
            w.post("owner", "/api/v1/connect/save", body)
                .await
                .status
                .as_u16(),
            422
        );
    }
    let r = w.post("owner", "/api/v1/connect/save", json!({ "api_key": KEY, "devices": [{ "mac": "AA:BB:CC:00:01:01", "timezone": "Europe/Paris" }] })).await;
    assert_eq!(
        r.status.as_u16(),
        422,
        "a client time zone that disagrees with AWN's"
    );
    assert!(none(&w).await);
}

#[sqlx::test(migrations = false)]
async fn naming_timezone_and_coordinates_come_from_awn(db: PgPool) {
    let long = "N".repeat(120);
    let stub = StubAwn::new(vec![
        dev(
            "AA:BB:CC:00:02:01",
            None,
            Some("America/Denver"),
            Some((40.456, -105.123)),
        ),
        dev(
            "AA:BB:CC:00:02:02",
            Some(&long),
            Some("America/Denver"),
            Some((40.4, -105.1)),
        ),
        dev(
            "AA:BB:CC:00:02:03",
            Some("Plain"),
            None,
            Some((40.4, -105.1)),
        ),
        dev(
            "AA:BB:CC:00:02:04",
            Some("NoCoords"),
            Some("America/Denver"),
            None,
        ),
        dev(
            "AA:BB:CC:00:02:05",
            Some("Phoenix"),
            Some("America/Phoenix"),
            Some((33.4, -112.0)),
        ),
    ]);
    let w = World::with(db, |b| {
        fast(b);
        b.awn = Some(stub);
    })
    .await;
    let r = w
        .post(
            "owner",
            "/api/v1/connect/save",
            json!({ "api_key": KEY, "devices": [
        { "mac": "AA:BB:CC:00:02:01" },
        { "mac": "AA:BB:CC:00:02:02", "name": "   " },
        { "mac": "AA:BB:CC:00:02:05" },
    ] }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    let names: Vec<String> = sqlx::query_scalar("SELECT name FROM app.stations ORDER BY mac")
        .fetch_all(&w.db)
        .await
        .unwrap();
    assert_eq!(names[0], "Station 0201");
    assert_eq!(names[1].chars().count(), 80);
    let row = sqlx::query("SELECT lat::float8 AS lat, lon::float8 AS lon FROM app.stations WHERE mac = 'AA:BB:CC:00:02:01'").fetch_one(&w.db).await.unwrap();
    assert_eq!(
        (row.get::<f64, _>("lat"), row.get::<f64, _>("lon")),
        (40.46, -105.12)
    );
    let off: i16 = sqlx::query_scalar(
        "SELECT std_utc_offset_minutes FROM app.stations WHERE mac = 'AA:BB:CC:00:02:05'",
    )
    .fetch_one(&w.db)
    .await
    .unwrap();
    assert_eq!(off, -420, "Phoenix has no DST: its base offset is -7 h");
    // No time zone anywhere -> timezone_required; a client fallback is accepted; no coordinates is refused.
    let r = w
        .post(
            "pending",
            "/api/v1/connect/save",
            save_body(KEY, &["AA:BB:CC:00:02:03"]),
        )
        .await;
    assert_eq!(
        (r.status.as_u16(), r.code().as_str()),
        (422, "timezone_required")
    );
    let r = w.post("pending", "/api/v1/connect/save", json!({ "api_key": KEY, "devices": [{ "mac": "AA:BB:CC:00:02:03", "timezone": "America/Chicago" }] })).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    let off: i16 = sqlx::query_scalar(
        "SELECT std_utc_offset_minutes FROM app.stations WHERE mac = 'AA:BB:CC:00:02:03'",
    )
    .fetch_one(&w.db)
    .await
    .unwrap();
    assert_eq!(off, -360);
    let r = w
        .post(
            "declined",
            "/api/v1/connect/save",
            save_body(KEY, &["AA:BB:CC:00:02:04"]),
        )
        .await;
    assert_eq!(r.status.as_u16(), 422);
}

#[sqlx::test(migrations = false)]
async fn reconnect_replaces_the_credential_and_reports_uncovered_stations(db: PgPool) {
    let stub = StubAwn::new(vec![
        dev(
            "AA:BB:CC:00:03:01",
            Some("One"),
            Some("America/Denver"),
            Some((40.4, -105.1)),
        ),
        dev(
            "AA:BB:CC:00:03:02",
            Some("Two"),
            Some("America/Denver"),
            Some((40.4, -105.0)),
        ),
    ]);
    let w = World::with(db, |b| {
        fast(b);
        b.awn = Some(stub.clone());
    })
    .await;
    let r = w
        .post(
            "owner",
            "/api/v1/connect/save",
            save_body(KEY, &["AA:BB:CC:00:03:01", "AA:BB:CC:00:03:02"]),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let first: Uuid = sqlx::query_scalar("SELECT id FROM app.credentials")
        .fetch_one(&w.db)
        .await
        .unwrap();
    // A station the new key will not list, still on the first credential.
    sqlx::query("INSERT INTO app.stations (mac, name, owner_id, credential_id, lat, lon, timezone, std_utc_offset_minutes) VALUES ('AA:BB:CC:00:03:99', 'Old', $1, $2, 40.4, -105.1, 'America/Denver', -420)")
        .bind(w.p("owner").id).bind(first).execute(&w.db).await.unwrap();

    let r = w
        .post(
            "owner",
            "/api/v1/connect/save",
            save_body(KEY, &["AA:BB:CC:00:03:01"]),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    let j = r.json();
    assert_eq!(j["not_covered"].as_array().unwrap().len(), 1);
    assert_eq!(j["not_covered"][0]["name"], "Old");
    let creds = sqlx::query("SELECT id, status FROM app.credentials ORDER BY created_at")
        .fetch_all(&w.db)
        .await
        .unwrap();
    assert_eq!(creds.len(), 2);
    assert_eq!(creds[0].get::<String, _>("status"), "replaced");
    assert_eq!(creds[1].get::<String, _>("status"), "ok");
    let second: Uuid = creds[1].get("id");
    // Covered stations moved to the new credential (both 03:01 and 03:02 are in the AWN list); the uncovered one stayed.
    let on_new: Vec<String> =
        sqlx::query_scalar("SELECT mac FROM app.stations WHERE credential_id = $1 ORDER BY mac")
            .bind(second)
            .fetch_all(&w.db)
            .await
            .unwrap();
    assert_eq!(on_new, ["AA:BB:CC:00:03:01", "AA:BB:CC:00:03:02"]);
    assert_eq!(w.audit_count("credential.saved").await, 2);
    assert_eq!(w.audit_count("credential.replaced").await, 1);
    let subject: Uuid = sqlx::query_scalar(
        "SELECT subject_id FROM app.audit_log WHERE action = 'credential.replaced'",
    )
    .fetch_one(&w.db)
    .await
    .unwrap();
    assert_eq!(subject, first);
    assert_eq!(
        w.get("owner", "/api/v1/connect/status").await.status,
        StatusCode::OK
    );

    // A third key that lists the old station recovers it.
    stub.0.lock().unwrap().push(dev(
        "AA:BB:CC:00:03:99",
        Some("Old"),
        Some("America/Denver"),
        Some((40.4, -105.1)),
    ));
    let r = w
        .post(
            "owner",
            "/api/v1/connect/save",
            save_body(KEY, &["AA:BB:CC:00:03:01"]),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    assert_eq!(r.json()["not_covered"], json!([]));
    let live: i64 =
        sqlx::query_scalar("SELECT count(*) FROM app.credentials WHERE status <> 'replaced'")
            .fetch_one(&w.db)
            .await
            .unwrap();
    assert_eq!(live, 1);
    let stranded: i64 = sqlx::query_scalar("SELECT count(*) FROM app.stations s JOIN app.credentials c ON c.id = s.credential_id WHERE c.status = 'replaced'").fetch_one(&w.db).await.unwrap();
    assert_eq!(stranded, 0);
}

#[sqlx::test(migrations = false)]
async fn another_users_mac_is_a_409_and_races_never_500(db: PgPool) {
    let w = World::with(db, fast).await;
    let r = w
        .post(
            "owner",
            "/api/v1/connect/save",
            save_body(KEY, &["AA:BB:CC:00:01:01"]),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let r = w
        .post(
            "stranger",
            "/api/v1/connect/save",
            save_body(KEY, &["AA:BB:CC:00:01:01"]),
        )
        .await;
    assert_eq!(
        (r.status.as_u16(), r.code().as_str()),
        (409, "device_already_registered")
    );
    assert!(!r.text().contains("Olive"), "no owner information");

    // Two different users racing for one MAC: one 201, one 409.
    let (a, b) = tokio::join!(
        w.post(
            "pending",
            "/api/v1/connect/save",
            save_body(KEY, &["AA:BB:CC:00:01:02"])
        ),
        w.post(
            "declined",
            "/api/v1/connect/save",
            save_body(KEY, &["AA:BB:CC:00:01:02"])
        ),
    );
    let mut codes = [a.status.as_u16(), b.status.as_u16()];
    codes.sort();
    assert_eq!(codes, [201, 409]);
    let loser = if a.status == StatusCode::CONFLICT {
        &a
    } else {
        &b
    };
    assert_eq!(loser.code(), "device_already_registered");

    // The same (losing) user twice at once: 201 or 409, never 500.
    let (a, b) = tokio::join!(
        w.post(
            "stranger",
            "/api/v1/connect/save",
            save_body(KEY, &["AA:BB:CC:00:01:02"])
        ),
        w.post(
            "stranger",
            "/api/v1/connect/save",
            save_body(KEY, &["AA:BB:CC:00:01:02"])
        ),
    );
    for r in [&a, &b] {
        assert!(matches!(r.status.as_u16(), 201 | 409), "{}", r.text());
    }
}

#[sqlx::test(migrations = false)]
async fn the_same_user_racing_themselves_ends_with_one_live_credential(db: PgPool) {
    let stub = StubAwn::new(vec![dev(
        "AA:BB:CC:00:04:01",
        Some("Race"),
        Some("America/Denver"),
        Some((40.4, -105.1)),
    )]);
    let w = World::with(db, |b| {
        fast(b);
        b.awn = Some(stub);
    })
    .await;
    let (a, b) = tokio::join!(
        w.post(
            "owner",
            "/api/v1/connect/save",
            save_body(KEY, &["AA:BB:CC:00:04:01"])
        ),
        w.post(
            "owner",
            "/api/v1/connect/save",
            save_body(KEY, &["AA:BB:CC:00:04:01"])
        ),
    );
    for r in [&a, &b] {
        assert!(matches!(r.status.as_u16(), 201 | 409), "{}", r.text());
        if r.status == StatusCode::CONFLICT {
            assert_eq!(r.code(), "conflict");
        }
    }
    let live: i64 =
        sqlx::query_scalar("SELECT count(*) FROM app.credentials WHERE status <> 'replaced'")
            .fetch_one(&w.db)
            .await
            .unwrap();
    let stations: i64 =
        sqlx::query_scalar("SELECT count(*) FROM app.stations WHERE mac = 'AA:BB:CC:00:04:01'")
            .fetch_one(&w.db)
            .await
            .unwrap();
    assert_eq!((live, stations), (1, 1));
}

#[sqlx::test(migrations = false)]
async fn a_failed_station_insert_rolls_everything_back(db: PgPool) {
    let w = World::with(db, fast).await;
    // A second owner already holds the MAC the first device list contains as its SECOND entry: the credential insert,
    // the first station and the audit rows of this request must not survive.
    let other = w.p("stranger").id;
    let cid = w.add_credential(other).await;
    sqlx::query("INSERT INTO app.stations (mac, name, owner_id, credential_id, lat, lon, timezone, std_utc_offset_minutes) VALUES ('AA:BB:CC:00:01:02', 'Theirs', $1, $2, 40.4, -105.1, 'America/Denver', -420)")
        .bind(other).bind(cid).execute(&w.db).await.unwrap();
    let r = w
        .post(
            "owner",
            "/api/v1/connect/save",
            save_body(KEY, &["AA:BB:CC:00:01:01", "AA:BB:CC:00:01:02"]),
        )
        .await;
    assert_eq!(r.status.as_u16(), 409);
    let mine: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM app.credentials WHERE owner_id = $1) + (SELECT count(*) FROM app.stations WHERE owner_id = $1) + (SELECT count(*) FROM app.audit_log)")
        .bind(w.p("owner").id).fetch_one(&w.db).await.unwrap();
    assert_eq!(mine, 0);
}

#[sqlx::test(migrations = false)]
async fn the_api_role_cannot_read_or_overwrite_ciphertext(db: PgPool) {
    let w = World::with(db, fast).await;
    assert_eq!(
        w.post(
            "owner",
            "/api/v1/connect/save",
            save_body(KEY, &["AA:BB:CC:00:01:01"])
        )
        .await
        .status,
        StatusCode::CREATED
    );
    for sql in [
        "SELECT api_key_ciphertext FROM app.credentials",
        "SELECT * FROM app.credentials",
        "UPDATE app.credentials SET api_key_ciphertext = '\\x00'::bytea",
        "INSERT INTO app.credentials (id, owner_id, api_key_ciphertext, encapsulated_key, key_version) VALUES (gen_random_uuid(), gen_random_uuid(), '\\x00'::bytea, '\\x00'::bytea, 1) RETURNING api_key_ciphertext",
        "INSERT INTO app.credentials (id, owner_id, api_key_ciphertext, encapsulated_key, key_version) SELECT id, owner_id, api_key_ciphertext, encapsulated_key, key_version FROM app.credentials ON CONFLICT (id) DO UPDATE SET key_version = 2",
    ] {
        let e = sqlx::query(sql).execute(&w.api).await.unwrap_err();
        assert!(e.to_string().contains("permission denied"), "{sql}: {e}");
    }
    assert!(sqlx::query("SELECT email FROM app.users")
        .execute(&w.api)
        .await
        .is_err());
}

/// AWN stand-in that takes its time, so a test can walk away mid-call.
struct SlowAwn(Duration);

#[async_trait]
impl AwnApi for SlowAwn {
    async fn list_devices(&self, _: &ApiKey) -> Result<Vec<AwnDevice>, AwnError> {
        tokio::time::sleep(self.0).await;
        Ok(vec![])
    }
    async fn history(
        &self,
        _: &ApiKey,
        _: &Mac,
        _: Option<DateTime<Utc>>,
        _: u16,
    ) -> Result<Vec<AwnObservation>, AwnError> {
        Err(AwnError::Unavailable)
    }
}

#[sqlx::test(migrations = false)]
async fn a_client_that_disconnects_mid_call_leaves_a_counted_started_row(db: PgPool) {
    let w = World::with(db, |b| {
        fast(b);
        b.awn = Some(Arc::new(SlowAwn(Duration::from_secs(3))));
    })
    .await;
    // The caller gives up after 400 ms: the request future (and with it the handler) is dropped mid-call.
    let gone = tokio::time::timeout(
        Duration::from_millis(400),
        w.post(
            "owner",
            "/api/v1/connect/validate",
            json!({ "api_key": KEY }),
        ),
    )
    .await;
    assert!(gone.is_err(), "the call was still running");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        attempts(&w, "owner").await,
        ["started"],
        "the attempt row was written BEFORE the AWN call and still counts"
    );
    for _ in 0..4 {
        sqlx::query("INSERT INTO app.connect_attempts (user_id, outcome) VALUES ($1, 'valid')")
            .bind(w.p("owner").id)
            .execute(&w.db)
            .await
            .unwrap();
    }
    let r = w
        .post(
            "owner",
            "/api/v1/connect/validate",
            json!({ "api_key": KEY }),
        )
        .await;
    assert_eq!(
        r.status.as_u16(),
        429,
        "the abandoned attempt counts towards the limit"
    );
}

#[sqlx::test(migrations = false)]
async fn an_injected_failure_on_the_station_insert_rolls_everything_back(db: PgPool) {
    let w = World::with(db, fast).await;
    let r = w
        .post(
            "owner",
            "/api/v1/connect/save",
            save_body(KEY, &["AA:BB:CC:00:01:01"]),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let before: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM app.credentials), (SELECT count(*) FROM app.stations), (SELECT count(*) FROM app.audit_log)",
    )
    .fetch_one(&w.db)
    .await
    .unwrap();
    // From now on every INSERT into stations fails inside the database.
    sqlx::raw_sql(
        "CREATE FUNCTION app.inject_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected'; END $$; \
         CREATE TRIGGER inject_failure BEFORE INSERT ON app.stations FOR EACH ROW EXECUTE FUNCTION app.inject_failure();",
    )
    .execute(&w.db)
    .await
    .unwrap();
    // A reconnect that must insert a new station: the credential insert and the `replaced` update come first.
    let r = w
        .post(
            "owner",
            "/api/v1/connect/save",
            save_body(KEY, &["AA:BB:CC:00:01:01", "AA:BB:CC:00:01:02"]),
        )
        .await;
    assert_eq!(r.status.as_u16(), 500, "{}", r.text());
    assert_eq!(r.code(), "internal");
    assert!(
        !r.text().contains("injected"),
        "no database text reaches the client"
    );
    let after: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM app.credentials), (SELECT count(*) FROM app.stations), (SELECT count(*) FROM app.audit_log)",
    )
    .fetch_one(&w.db)
    .await
    .unwrap();
    assert_eq!(
        before, after,
        "no credential, station or audit row from the failed request"
    );
    let status: String = sqlx::query_scalar("SELECT status FROM app.credentials")
        .fetch_one(&w.db)
        .await
        .unwrap();
    assert_eq!(status, "ok", "the old credential was not marked replaced");
}
