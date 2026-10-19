//! Baseline picker (02 4.15, 5.10, 7.7).

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use api::baseline::{self, BaselineCache};
use async_trait::async_trait;
use axum::http::StatusCode;
use chrono::{Duration as Cd, NaiveDate};
use common::*;
use famweather_core::acis::{AcisApi, AcisDay, AcisError, AcisStationMeta, BBox, ReplayAcis};
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use uuid::Uuid;

struct CountingAcis {
    inner: ReplayAcis,
    calls: AtomicUsize,
}

#[async_trait]
impl AcisApi for CountingAcis {
    async fn station_data(
        &self,
        sid: &str,
        a: NaiveDate,
        b: NaiveDate,
    ) -> Result<Vec<AcisDay>, AcisError> {
        self.inner.station_data(sid, a, b).await
    }
    async fn nearby_stations(&self, bbox: BBox) -> Result<Vec<AcisStationMeta>, AcisError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.nearby_stations(bbox).await
    }
}

fn counting() -> Arc<CountingAcis> {
    Arc::new(CountingAcis {
        inner: ReplayAcis::new(fixtures()).unwrap(),
        calls: AtomicUsize::new(0),
    })
}

/// Most tests make many baseline calls: they get a relaxed baseline limiter. The limiter test uses the real quota.
async fn world(db: PgPool) -> (World, Arc<CountingAcis>) {
    build(db, true).await
}

async fn build(db: PgPool, relaxed: bool) -> (World, Arc<CountingAcis>) {
    let acis = counting();
    let a = acis.clone();
    let w = World::with(db, move |b| {
        b.acis = Some(a);
        b.gates.awn_app_spacing = Duration::from_millis(5);
        let clock = governor::clock::FakeRelativeClock::default();
        b.limiters = Some(Arc::new(if relaxed {
            api::limits::UserLimiters::with_baseline_quota(clock, 6000, 6000)
        } else {
            api::limits::UserLimiters::with_clock(clock)
        }));
    })
    .await;
    (w, acis)
}

async fn connect(w: &World) -> Vec<Uuid> {
    let r = w
        .post(
            "owner",
            "/api/v1/connect/save",
            json!({ "api_key": "fixture-valid-key", "devices": [{ "mac": "AA:BB:CC:00:01:01" }, { "mac": "AA:BB:CC:00:01:02" }] }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    r.json()["stations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| Uuid::parse_str(s["id"].as_str().unwrap()).unwrap())
        .collect()
}

fn cand(id: &str, ll: (f64, f64), range: (&str, &str), faa: bool) -> AcisStationMeta {
    let d = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
    let r = Some((d(range.0), d(range.1)));
    AcisStationMeta {
        ghcn_id: Some(id.to_owned()),
        icao: faa.then(|| "KXXX".to_owned()),
        faa: None,
        name: format!("Station {id}"),
        lat: ll.0,
        lon: ll.1,
        elevation_ft: Some(5000.0),
        pcpn: r,
        maxt: r,
        mint: r,
    }
}

#[test]
fn filter_and_ranking_rules() {
    let today = NaiveDate::from_ymd_opt(2026, 10, 17).unwrap();
    let (lat, lon) = (40.4, -105.1);
    let fresh = ("2000-01-01", "2026-10-10");
    let mut coop = cand("USC00053005", (40.5, -105.1), fresh, true);
    coop.faa = Some("FCL".into());
    let mut stale = cand(
        "USW00000002",
        (40.41, -105.1),
        ("2000-01-01", "2026-08-01"),
        true,
    );
    stale.name = "stale".into();
    let mut no_precip = cand("USW00000003", (40.41, -105.1), fresh, true);
    no_precip.pcpn = None;
    let mut no_code = cand("USW00000004", (40.41, -105.1), fresh, true);
    no_code.icao = None;
    let metas = vec![
        cand("USW00000010", (40.6, -105.1), fresh, true),
        cand(
            "USW00000011",
            (40.5, -105.1),
            ("2015-01-01", "2026-10-10"),
            true,
        ),
        cand("USW00000012", (40.5, -105.1), fresh, true), // same distance, longer record: ranks before 11
        cand(
            "USW00000013",
            (40.45, -105.1),
            ("2020-01-01", "2026-10-10"),
            true,
        ), // 6 years: too short
        cand("USW00000014", (41.6, -105.1), fresh, true), // about 83 miles: too far
        coop,
        stale,
        no_precip,
        no_code,
    ];
    let ranked = baseline::rank(&metas, lat, lon, today);
    let ids: Vec<_> = ranked
        .iter()
        .map(|c| c.reference_station_id.as_str())
        .collect();
    assert_eq!(ids, ["USW00000012", "USW00000011", "USW00000010"]);
    assert!(ranked[0].distance_mi > 6.0 && ranked[0].distance_mi < 8.0);
    assert_eq!(ranked[0].record_years, 26);
    // The record start is the LATEST of the three ranges.
    let mut m = cand("USW00000020", (40.45, -105.1), fresh, true);
    m.mint = Some((
        NaiveDate::from_ymd_opt(2010, 1, 1).unwrap(),
        m.mint.unwrap().1,
    ));
    assert_eq!(baseline::rank(&[m], lat, lon, today)[0].record_years, 16);
    // At most ten are kept.
    let many: Vec<_> = (0..15)
        .map(|i| {
            cand(
                &format!("USW000001{i:02}"),
                (40.4 + f64::from(i) * 0.01, -105.1),
                fresh,
                true,
            )
        })
        .collect();
    assert_eq!(baseline::rank(&many, lat, lon, today).len(), 10);
}

#[test]
fn cache_ttls_and_bound() {
    let t0 = now();
    let c = BaselineCache::new();
    let one = vec![baseline::rank(
        &[cand(
            "USW00000010",
            (40.5, -105.1),
            ("2000-01-01", "2026-10-10"),
            true,
        )],
        40.4,
        -105.1,
        t0.date_naive(),
    )[0]
    .clone()];
    c.put((1, 1), t0, one.clone());
    c.put((2, 2), t0, vec![]);
    assert!(c.get((1, 1), t0 + Cd::hours(23)).is_some());
    assert!(c.get((1, 1), t0 + Cd::hours(25)).is_none());
    assert!(c.get((2, 2), t0 + Cd::minutes(59)).is_some());
    assert!(
        c.get((2, 2), t0 + Cd::minutes(61)).is_none(),
        "empty results live one hour"
    );
    for i in 0..65 {
        c.put((100 + i, 0), t0 + Cd::seconds(i), one.clone());
    }
    assert_eq!(c.len(), 64);
    assert!(
        c.get((100, 0), t0 + Cd::seconds(66)).is_none(),
        "the oldest entry was evicted"
    );
    assert!(c.get((164, 0), t0 + Cd::seconds(66)).is_some());
}

#[sqlx::test(migrations = false)]
async fn save_never_calls_acis_then_the_flow_works(db: PgPool) {
    let (w, acis) = world(db).await;
    let ids = connect(&w).await;
    assert_eq!(
        acis.calls.load(Ordering::SeqCst),
        0,
        "connect/save makes zero ACIS requests"
    );
    let path = |id: Uuid| format!("/api/v1/stations/{id}/baseline/candidates");

    let r = w.get("owner", &path(ids[0])).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let j = r.json();
    assert_eq!(j["suggested"], "USW00094035");
    assert_eq!(j["radius_mi"], 60);
    assert!(j["current"].is_null());
    assert_eq!(j["station_elevation_ft"], 5000);
    let c = &j["candidates"];
    assert_eq!(
        c.as_array().unwrap().len(),
        1,
        "coop and rainfall-only and closed stations are excluded"
    );
    assert_eq!(c[0]["airport_code"], "KFNL");
    assert_eq!(c[0]["elevation_diff_ft"], 15);
    assert_eq!(c[0]["record_start"], "2011-10-01");
    assert_eq!(c[0]["record_years"], 15);
    assert_eq!(c[0]["short_record"], true);
    let d = c[0]["distance_mi"].as_f64().unwrap();
    assert!(d > 3.0 && d < 12.0, "{d}");
    assert_eq!(acis.calls.load(Ordering::SeqCst), 1);

    // Same station again within the TTL: no ACIS call and the gate does not move.
    let gate: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT next_allowed_at FROM app.rate_gates WHERE name = 'acis:api'")
            .fetch_one(&w.db)
            .await
            .unwrap();
    assert_eq!(w.get("owner", &path(ids[0])).await.status, StatusCode::OK);
    assert_eq!(acis.calls.load(Ordering::SeqCst), 1);
    let gate2: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT next_allowed_at FROM app.rate_gates WHERE name = 'acis:api'")
            .fetch_one(&w.db)
            .await
            .unwrap();
    assert_eq!(gate, gate2);
    // A station at another location misses.
    assert_eq!(w.get("owner", &path(ids[1])).await.status, StatusCode::OK);
    assert_eq!(acis.calls.load(Ordering::SeqCst), 2);
    // After 25 hours (injected clock) the entry has expired.
    w.clock.advance(Cd::hours(25));
    assert_eq!(w.get("owner", &path(ids[0])).await.status, StatusCode::OK);
    assert_eq!(acis.calls.load(Ordering::SeqCst), 3);

    // PUT the suggestion.
    let put = |id: Uuid| format!("/api/v1/stations/{id}/baseline");
    let r = w
        .put(
            "owner",
            &put(ids[0]),
            json!({ "reference_station_id": "USW00094035" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let b = &r.json()["baseline"];
    assert_eq!(b["reference_station_id"], "USW00094035");
    assert_eq!(b["name"], "FORT COLLINS LOVELAND AP");
    assert_eq!(b["history_loaded"], false);
    let row = sqlx::query("SELECT name, lat::float8 AS lat, lon::float8 AS lon, elevation_ft, timezone, std_utc_offset_minutes, last_synced_date FROM app.reference_stations WHERE id = 'USW00094035'")
        .fetch_one(&w.db).await.unwrap();
    assert_eq!(row.get::<String, _>("name"), "FORT COLLINS LOVELAND AP");
    assert_eq!(
        (row.get::<f64, _>("lat"), row.get::<f64, _>("lon")),
        (40.45, -105.01667)
    );
    assert_eq!(row.get::<Option<i32>, _>("elevation_ft"), Some(5015));
    assert_eq!(row.get::<String, _>("timezone"), "America/Denver");
    assert_eq!(row.get::<i16, _>("std_utc_offset_minutes"), -420);
    assert!(row
        .get::<Option<chrono::NaiveDate>, _>("last_synced_date")
        .is_none());
    assert_eq!(w.audit_count("station.baseline_set").await, 1);
    let detail: Value = sqlx::query_scalar(
        "SELECT detail FROM app.audit_log WHERE action = 'station.baseline_set'",
    )
    .fetch_one(&w.db)
    .await
    .unwrap();
    assert_eq!(detail["reference_station_id"], "USW00094035");
    // Repeating it is a no-op without an audit row; it shows as the station's current baseline.
    assert_eq!(
        w.put(
            "owner",
            &put(ids[0]),
            json!({ "reference_station_id": "USW00094035" })
        )
        .await
        .status,
        StatusCode::OK
    );
    assert_eq!(w.audit_count("station.baseline_set").await, 1);
    assert_eq!(
        w.get("owner", &path(ids[0])).await.json()["current"],
        "USW00094035"
    );
    assert_eq!(
        w.get("owner", &format!("/api/v1/stations/{}", ids[0]))
            .await
            .json()["baseline"]["reference_station_id"],
        "USW00094035"
    );
    // A second station reuses the reference row.
    w.clock.advance(Cd::seconds(1));
    assert_eq!(
        w.put(
            "owner",
            &put(ids[1]),
            json!({ "reference_station_id": "USW00094035" })
        )
        .await
        .status,
        StatusCode::OK
    );
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM app.reference_stations")
        .fetch_one(&w.db)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

#[sqlx::test(migrations = false)]
async fn putting_the_current_baseline_is_a_no_op_even_if_it_is_not_in_the_list(db: PgPool) {
    let (w, acis) = world(db).await;
    // An id the ranked list would never contain (a distant airport, set by an admin).
    w.add_reference("USW00024144", true).await;
    let s = w
        .add_station(
            "owner",
            "AA:BB:CC:00:06:01",
            "Admin set",
            40.4,
            -105.1,
            Some("USW00024144"),
        )
        .await;
    let r = w
        .put(
            "owner",
            &format!("/api/v1/stations/{s}/baseline"),
            json!({ "reference_station_id": "USW00024144" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json()["baseline"]["reference_station_id"], "USW00024144");
    assert_eq!(acis.calls.load(Ordering::SeqCst), 0, "no ACIS lookup");
    assert_eq!(
        w.audit_count("station.baseline_set").await,
        0,
        "no audit row"
    );
    // Any other id is still checked against the list.
    let r = w
        .put(
            "owner",
            &format!("/api/v1/stations/{s}/baseline"),
            json!({ "reference_station_id": "USW00012345" }),
        )
        .await;
    assert_eq!(r.code(), "not_a_candidate");
}

#[sqlx::test(migrations = false)]
async fn arbitrary_ids_are_rejected(db: PgPool) {
    let (w, _) = world(db).await;
    let ids = connect(&w).await;
    let put = format!("/api/v1/stations/{}/baseline", ids[0]);
    for (body, code, name) in [
        (
            json!({ "reference_station_id": "USW00024144" }),
            422,
            "not_a_candidate",
        ),
        (
            json!({ "reference_station_id": "USW00012345" }),
            422,
            "not_a_candidate",
        ),
        (
            json!({ "reference_station_id": "nope" }),
            422,
            "validation_failed",
        ),
        (
            json!({ "reference_station_id": "USW00094035", "name": "Mine" }),
            422,
            "validation_failed",
        ),
    ] {
        let r = w.put("owner", &put, body).await;
        assert_eq!((r.status.as_u16(), r.code().as_str()), (code, name));
    }
    let n: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM app.reference_stations) + (SELECT count(*) FROM app.audit_log WHERE action = 'station.baseline_set')").fetch_one(&w.db).await.unwrap();
    assert_eq!(n, 0);
}

#[sqlx::test(migrations = false)]
async fn only_the_owner_gets_in_and_others_cause_no_acis_request(db: PgPool) {
    let (w, acis) = world(db).await;
    let ids = connect(&w).await;
    let c = format!("/api/v1/stations/{}/baseline/candidates", ids[0]);
    let p = format!("/api/v1/stations/{}/baseline", ids[0]);
    let missing = w
        .get(
            "stranger",
            &format!("/api/v1/stations/{}/baseline/candidates", Uuid::new_v4()),
        )
        .await;
    for who in [
        "approved_follower",
        "stranger",
        "pending",
        "declined",
        "revoked",
    ] {
        let r = w.get(who, &c).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{who}");
        assert_eq!(r.body, missing.body);
        let r = w
            .put(who, &p, json!({ "reference_station_id": "USW00094035" }))
            .await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{who}");
    }
    assert_eq!(acis.calls.load(Ordering::SeqCst), 0);
}

#[sqlx::test(migrations = false)]
async fn empty_down_and_busy(db: PgPool) {
    let (w, acis) = world(db).await;
    // Nothing qualifies near this station: a normal 200.
    let quiet = w
        .add_station("owner", "AA:BB:CC:00:05:01", "Quiet", 44.0, -108.0, None)
        .await;
    let j = w
        .get(
            "owner",
            &format!("/api/v1/stations/{quiet}/baseline/candidates"),
        )
        .await
        .json();
    assert_eq!(j["candidates"], json!([]));
    assert!(j["suggested"].is_null());
    // ACIS down (replay marker at 0,0): 502, and the failure is not cached.
    let down = w
        .add_station("owner", "AA:BB:CC:00:05:02", "Down", 0.0, 0.0, None)
        .await;
    let p = format!("/api/v1/stations/{down}/baseline/candidates");
    let before = acis.calls.load(Ordering::SeqCst);
    for i in 1..=2 {
        let r = w.get("owner", &p).await;
        assert_eq!(
            (r.status.as_u16(), r.code().as_str()),
            (502, "upstream_unavailable")
        );
        assert_eq!(acis.calls.load(Ordering::SeqCst), before + i, "not cached");
    }
    let r = w
        .put(
            "owner",
            &format!("/api/v1/stations/{down}/baseline"),
            json!({ "reference_station_id": "USW00094035" }),
        )
        .await;
    assert_eq!(r.status.as_u16(), 502);
    // The ytd of a station without a baseline is still a 200.
    let r = w
        .get(
            "owner",
            &format!("/api/v1/ytd?station_ids={down}&year=2026"),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.json()["stations"][0]["baseline"].is_null());
}

#[sqlx::test(migrations = false)]
async fn a_busy_acis_gate_is_prompt(db: PgPool) {
    let (w, acis) = world(db).await;
    let s = w
        .add_station("owner", "AA:BB:CC:00:05:03", "Busy", 40.4, -105.1, None)
        .await;
    sqlx::query("INSERT INTO app.rate_gates (name, next_allowed_at) VALUES ('acis:api', now() + interval '1 hour')").execute(&w.db).await.unwrap();
    let t = Instant::now();
    let r = w
        .get(
            "owner",
            &format!("/api/v1/stations/{s}/baseline/candidates"),
        )
        .await;
    assert!(t.elapsed() < Duration::from_millis(500));
    assert_eq!(
        (r.status.as_u16(), r.code().as_str()),
        (503, "upstream_busy")
    );
    assert_eq!(r.headers["retry-after"], "2");
    assert_eq!(acis.calls.load(Ordering::SeqCst), 0);
}

#[sqlx::test(migrations = false)]
async fn the_baseline_limiter_runs_before_any_work(db: PgPool) {
    let (w, acis) = build(db, false).await;
    let s = w
        .add_station("owner", "AA:BB:CC:00:05:04", "Limited", 40.4, -105.1, None)
        .await;
    let p = format!("/api/v1/stations/{s}/baseline/candidates");
    for _ in 0..3 {
        assert_eq!(w.get("owner", &p).await.status, StatusCode::OK);
    }
    let calls = acis.calls.load(Ordering::SeqCst);
    let r = w.get("owner", &p).await;
    assert_eq!(
        (r.status.as_u16(), r.code().as_str()),
        (429, "rate_limited")
    );
    assert!(r.headers.contains_key("retry-after"));
    assert_eq!(acis.calls.load(Ordering::SeqCst), calls);
    // Other users keep their own budget.
    assert_eq!(
        w.get("approved_follower", "/api/v1/me").await.status,
        StatusCode::OK
    );
}

#[sqlx::test(migrations = false)]
async fn grants_for_the_picker(db: PgPool) {
    let (w, _) = world(db).await;
    let ids = connect(&w).await;
    sqlx::query("INSERT INTO app.reference_stations (id, name, lat, lon, elevation_ft, timezone, std_utc_offset_minutes) VALUES ('USW00000001', 'X', 40, -105, 1, 'America/Denver', -420)")
        .execute(&w.api).await.unwrap();
    for sql in [
        "INSERT INTO app.reference_stations (id, name, lat, lon, timezone, std_utc_offset_minutes, last_synced_date) VALUES ('USW00000002', 'X', 40, -105, 'America/Denver', -420, now())",
        "UPDATE app.reference_stations SET name = 'Y'",
        "UPDATE app.stations SET owner_id = gen_random_uuid()",
    ] {
        let e = sqlx::query(sql).execute(&w.api).await.unwrap_err();
        assert!(e.to_string().contains("permission denied"), "{sql}: {e}");
    }
    sqlx::query("UPDATE app.stations SET reference_station_id = 'USW00000001' WHERE id = $1")
        .bind(ids[0])
        .execute(&w.api)
        .await
        .unwrap();
}
