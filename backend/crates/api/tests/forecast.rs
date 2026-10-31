//! Forecast and forecast changes (02 5.6, 5.6a, 7.9). A stub HTTP server serves the committed fixtures and counts
//! upstream hits; no test calls the real Open-Meteo.

mod common;

use std::sync::Arc;
use std::time::Duration;

use api::openmeteo::{LiveOpenMeteo, OpenMeteoApi};
use axum::http::StatusCode;
use chrono::Duration as Cd;
use common::*;
use serde_json::{json, Value};
use sqlx::PgPool;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn fixture(prefix: &str) -> Vec<u8> {
    let dir = fixtures().join("open-meteo");
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().unwrap().to_str().unwrap().starts_with(prefix)
                && p.to_str().unwrap().contains("helena")
        })
        .collect();
    names.sort();
    std::fs::read(names.remove(0)).unwrap()
}

struct Stub {
    server: MockServer,
}

impl Stub {
    async fn start() -> Stub {
        let server = MockServer::start().await;
        let s = Stub { server };
        s.ok("/forecast", fixture("forecast__")).await;
        s.ok("/ensemble", fixture("ensemble_ecmwf_ifs025__")).await;
        s.ok("/previous", fixture("previous_runs__")).await;
        s
    }

    async fn ok(&self, p: &str, body: Vec<u8>) {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(body, "application/json")
                    .set_delay(Duration::from_millis(30)),
            )
            .mount(&self.server)
            .await;
    }

    async fn status(&self, p: &str, code: u16, body: &str) {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(code).set_body_string(body.to_owned()))
            .with_priority(1)
            .mount(&self.server)
            .await;
    }

    fn api(&self) -> Arc<dyn OpenMeteoApi> {
        let u = |p: &str| url::Url::parse(&format!("{}{p}", self.server.uri())).unwrap();
        Arc::new(LiveOpenMeteo::with_bases(u("/forecast"), u("/ensemble"), u("/previous")).unwrap())
    }

    async fn hits(&self, p: &str) -> usize {
        self.server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path() == p)
            .count()
    }

    async fn requests(&self, p: &str) -> Vec<Request> {
        self.server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.url.path() == p)
            .collect()
    }
}

/// Most tests make many forecast calls: they get a relaxed per-user forecast limiter.
async fn world(db: PgPool, stub: &Stub) -> (World, uuid::Uuid) {
    limited_world(db, stub, true).await
}

async fn limited_world(db: PgPool, stub: &Stub, relaxed: bool) -> (World, uuid::Uuid) {
    let api = stub.api();
    let w = World::with(db, move |b| {
        b.om = Some(api);
        let clock = governor::clock::FakeRelativeClock::default();
        b.limiters = Some(Arc::new(if relaxed {
            api::limits::UserLimiters::with_forecast_quota(clock, 6000, 6000)
        } else {
            api::limits::UserLimiters::with_clock(clock)
        }));
    })
    .await;
    let s = w
        .add_station("owner", "AA:BB:CC:00:00:01", "Helena", 46.6, -112.0, None)
        .await;
    (w, s)
}

fn fpath(s: uuid::Uuid) -> String {
    format!("/api/v1/stations/{s}/forecast")
}

fn changes(s: uuid::Uuid, q: &str) -> String {
    format!("/api/v1/stations/{s}/forecast/changes?{q}")
}

#[sqlx::test(migrations = false)]
async fn forecast_shape_units_and_requests(db: PgPool) {
    let stub = Stub::start().await;
    let (w, s) = world(db, &stub).await;
    let r = w.get("owner", &fpath(s)).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let j = r.json();
    assert_eq!(j["timezone"], "America/Denver");
    assert_eq!(j["stale"], false);
    assert_eq!(
        j["units"],
        json!({
            "temperature": "F",
            "precipitation": "in",
            "wind": "mph",
            "pressure": "inHg",
            "snow": "in",
            "percent": "%"
        })
    );
    assert_eq!(
        j["attribution"],
        "Weather data by Open-Meteo.com (CC BY 4.0)"
    );
    let d = &j["deterministic"];
    assert_eq!(d["model"], "gfs_seamless");
    assert_eq!(d["daily"].as_array().unwrap().len(), 16);
    // The stub serves the recorded forecast unshifted, so its first day is the day it was recorded (25 Oct 2026).
    assert_eq!(d["daily"][0]["date"], "2026-10-25");
    let n = d["hourly"]["time"].as_array().unwrap().len();
    let hourly_keys = [
        "temperature_f",
        "precip_in",
        "prob_precip",
        "precip_accum_in",
        "apparent_temp_f",
        "dew_point_f",
        "cloud_pct",
        "humidity_pct",
        "pressure_inhg",
        "wind_mph",
        "wind_dir_deg",
        "gust_mph",
        "weather_code",
        "snow_in",
    ];
    for k in hourly_keys {
        assert_eq!(d["hourly"][k].as_array().unwrap().len(), n, "{k}");
    }
    assert!(d["hourly"]["time"][0].as_str().unwrap().ends_with('Z'));

    // The fixture as recorded: pressure in hPa, precipitation in inches.
    let raw: Value = serde_json::from_slice(&fixture("forecast__")).unwrap();
    let raw_h = &raw["hourly"];
    // Pressure: hPa converted to inHg in the API, rounded to two places (1022.3 hPa is 30.19 inHg).
    let inhg = d["hourly"]["pressure_inhg"].as_array().unwrap();
    assert_eq!(inhg[0].as_f64(), Some(30.19));
    for (hpa, got) in raw_h["pressure_msl"].as_array().unwrap().iter().zip(inhg) {
        let want = (hpa.as_f64().unwrap() * 0.0295299830714 * 100.0).round() / 100.0;
        assert!((got.as_f64().unwrap() - want).abs() < 1e-9, "{hpa} hPa");
    }
    // The running total starts at the first step not in the past. This fixture begins after the test clock
    // (17 Oct 2026 12:00 UTC), so every step is counted: each entry is the sum of the hourly values so far.
    let accum = d["hourly"]["precip_accum_in"].as_array().unwrap();
    let mut run = 0.0;
    for (k, p) in raw_h["precipitation"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        run += p.as_f64().unwrap();
        assert!(
            (accum[k].as_f64().unwrap() - run).abs() < 0.0006,
            "step {k}"
        );
    }
    // Daily rows carry the new fields: sunrise and sunset as UTC instants, weather code, snow in inches.
    let day0 = &d["daily"][0];
    assert!(day0["weather_code"].is_i64());
    assert!(day0["snow_in"].is_number());
    for k in ["sunrise", "sunset"] {
        assert!(day0[k].as_str().unwrap().ends_with('Z'), "{k}");
    }
    assert_eq!(day0["sunrise"], "2026-10-25T13:59:00Z", "07:59 MDT");
    let e = &j["ensemble"];
    assert_eq!(
        (e["model"].as_str().unwrap(), e["members"].as_u64().unwrap()),
        ("ecmwf_ifs025", 51)
    );
    let en = e["hourly"]["time"].as_array().unwrap().len();
    assert_eq!(
        e["hourly"]["temperature_f"]["p50"]
            .as_array()
            .unwrap()
            .len(),
        en
    );
    assert!(
        e["hourly"]["precip_in"]["p10"][en - 1].is_null(),
        "the last precipitation hours are null, not an error"
    );
    // The ensemble horizon ends with null steps for every member (the recorded hourly series ends before its last steps have
    // member values), so the percentiles are null there too; the first step has values.
    assert!(e["hourly"]["temperature_f"]["p10"][en - 1].is_null());
    assert!(e["hourly"]["temperature_f"]["p50"][0].is_number());
    // The ensemble total: each member is accumulated first, then percentiles are taken. Every member's total never
    // falls, so the percentiles never fall either; the final hours are null for every member.
    let ea = &e["hourly"]["precip_accum_in"];
    for k in ["p10", "p50", "p90"] {
        assert_eq!(ea[k].as_array().unwrap().len(), en, "{k}");
    }
    assert!(ea["p50"][en - 1].is_null());
    let p50: Vec<f64> = ea["p50"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(Value::as_f64)
        .collect();
    assert!(!p50.is_empty() && p50.windows(2).all(|w| w[0] <= w[1] + 1e-9));
    for i in 0..en {
        if let (Some(a), Some(b), Some(c)) = (
            ea["p10"][i].as_f64(),
            ea["p50"][i].as_f64(),
            ea["p90"][i].as_f64(),
        ) {
            assert!(a <= b && b <= c, "step {i}");
        }
    }
    // Route level, per-member method: the last ensemble total must not equal the sum of the hourly medians over the
    // same steps. On this fixture (17 Oct to the end of the series) the per-member total is 0.24 in and the summed
    // medians are 0.0 in, so this would fail if the route summed percentiles.
    let ens_precip_p50 = e["hourly"]["precip_in"]["p50"].as_array().unwrap();
    let from = ea["p50"]
        .as_array()
        .unwrap()
        .iter()
        .position(Value::is_number)
        .unwrap();
    let summed: f64 = ens_precip_p50[from..].iter().flat_map(Value::as_f64).sum();
    let per_member = ea["p50"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find_map(Value::as_f64)
        .unwrap();
    assert!(
        (per_member - summed).abs() > 0.01,
        "per-member {per_member} vs summed hourly medians {summed}"
    );
    let days = e["daily"].as_array().unwrap();
    // A member-day needs every member's hours: the recording has missing member values from 6 Nov, so the complete days
    // are 25 Oct to 5 Nov (12 days). The old 15-day recording had none missing.
    assert_eq!(days.len(), 12, "complete member-days");
    for day in days {
        for k in ["tmax_f", "tmin_f", "precip_in"] {
            let (a, b, c) = (
                day[k]["p10"].as_f64().unwrap(),
                day[k]["p50"].as_f64().unwrap(),
                day[k]["p90"].as_f64().unwrap(),
            );
            assert!(a <= b && b <= c, "{k}: {a} {b} {c}");
        }
        assert!(day["prob_precip"].as_u64().unwrap() <= 100);
    }

    // What was sent upstream: pinned models, fixed units, the station's rounded coordinates, no key.
    let f = stub.requests("/forecast").await;
    let q = f[0].url.query().unwrap();
    for needle in [
        "models=gfs_seamless",
        "temperature_unit=fahrenheit",
        "precipitation_unit=inch",
        "wind_speed_unit=mph",
        "timezone=America%2FDenver",
        "latitude=46.6",
        "longitude=-112",
        "forecast_days=16",
    ] {
        assert!(q.contains(needle), "{needle} in {q}");
    }
    // Every added variable is requested (parsed from the query, so the encoding of commas does not matter).
    let pairs: std::collections::HashMap<String, String> = f[0]
        .url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let hourly: Vec<&str> = pairs["hourly"].split(',').collect();
    for v in [
        "apparent_temperature",
        "dew_point_2m",
        "cloud_cover",
        "relative_humidity_2m",
        "pressure_msl",
        "wind_speed_10m",
        "wind_direction_10m",
        "wind_gusts_10m",
        "weather_code",
        "snowfall",
    ] {
        assert!(hourly.contains(&v), "hourly {v}");
    }
    let daily: Vec<&str> = pairs["daily"].split(',').collect();
    for v in ["sunrise", "sunset", "weather_code", "snowfall_sum"] {
        assert!(daily.contains(&v), "daily {v}");
    }
    let q = stub.requests("/ensemble").await[0]
        .url
        .query()
        .unwrap()
        .to_owned();
    assert!(q.contains("models=ecmwf_ifs025") && q.contains("forecast_days=15"));
    for p in ["/forecast", "/ensemble"] {
        assert!(!stub.requests(p).await[0]
            .url
            .as_str()
            .to_lowercase()
            .contains("apikey"));
    }
}

#[sqlx::test(migrations = false)]
async fn caching_single_flight_and_staleness(db: PgPool) {
    let stub = Stub::start().await;
    let (w, s) = world(db, &stub).await;
    // 20 concurrent cold requests cause exactly one hit per kind.
    let path = fpath(s);
    let rs = futures::future::join_all((0..20).map(|_| w.get("owner", &path))).await;
    assert!(rs.iter().all(|r| r.status == StatusCode::OK));
    assert_eq!(
        (stub.hits("/forecast").await, stub.hits("/ensemble").await),
        (1, 1)
    );
    // Within 30 minutes: no hits. A second station at the same rounded coordinates shares the entry.
    w.clock.advance(Cd::minutes(29));
    let twin = w
        .add_station("owner", "AA:BB:CC:00:00:02", "Twin", 46.6, -112.0, None)
        .await;
    assert_eq!(w.get("owner", &fpath(twin)).await.status, StatusCode::OK);
    assert_eq!(stub.hits("/forecast").await, 1);
    // After the TTL a refresh hits again.
    w.clock.advance(Cd::minutes(2));
    assert_eq!(w.get("owner", &fpath(s)).await.json()["stale"], false);
    assert_eq!(stub.hits("/forecast").await, 2);
    // A failing refresh serves the old entry as stale, up to six hours.
    stub.status("/forecast", 500, "boom").await;
    stub.status("/ensemble", 500, "boom").await;
    w.clock.advance(Cd::minutes(31));
    let r = w.get("owner", &fpath(s)).await.json();
    assert_eq!(r["stale"], true);
    assert!(r["deterministic"].is_object() && r["ensemble"].is_object());
    w.clock.advance(Cd::hours(6));
    let r = w.get("owner", &fpath(s)).await;
    assert_eq!(
        (r.status.as_u16(), r.code().as_str()),
        (502, "upstream_unavailable")
    );
}

#[sqlx::test(migrations = false)]
async fn partial_and_total_upstream_failures(db: PgPool) {
    let stub = Stub::start().await;
    stub.status("/ensemble", 500, "canary-secret-ensemble-body")
        .await;
    let (w, s) = world(db, &stub).await;
    let r = w.get("owner", &fpath(s)).await;
    assert_eq!(r.status, StatusCode::OK);
    let j = r.json();
    assert!(j["ensemble"].is_null() && j["deterministic"].is_object());
    assert!(!r.text().contains("canary-secret"));
}

#[sqlx::test(migrations = false)]
async fn only_the_deterministic_call_failing(db: PgPool) {
    let stub = Stub::start().await;
    stub.status("/forecast", 500, "x").await;
    let (w, s) = world(db, &stub).await;
    let j = w.get("owner", &fpath(s)).await.json();
    assert!(j["deterministic"].is_null() && j["ensemble"].is_object());
}

#[sqlx::test(migrations = false)]
async fn every_failure_mode_maps_to_the_envelope(db: PgPool) {
    let stub = Stub::start().await;
    stub.status("/forecast", 429, "canary-secret-limit").await;
    stub.status("/ensemble", 500, "canary-secret-server").await;
    let (w, s) = world(db, &stub).await;
    let r = w.get("owner", &fpath(s)).await;
    assert_eq!(
        (r.status.as_u16(), r.code().as_str()),
        (503, "upstream_busy")
    );
    assert_eq!(r.headers["retry-after"], "60");
    assert!(!r.text().contains("canary-secret"));
}

#[sqlx::test(migrations = false)]
async fn ensemble_member_count_mismatch_leaves_the_forecast_intact(db: PgPool) {
    let stub = Stub::start().await;
    let mut v: Value = serde_json::from_slice(&fixture("ensemble_ecmwf_ifs025__")).unwrap();
    v["hourly"]
        .as_object_mut()
        .unwrap()
        .remove("temperature_2m_member50");
    stub.status("/ensemble", 200, &v.to_string()).await;
    let (w, s) = world(db, &stub).await;
    let j = w.get("owner", &fpath(s)).await.json();
    assert!(j["ensemble"].is_null());
    assert_eq!(j["deterministic"]["daily"].as_array().unwrap().len(), 16);
}

#[sqlx::test(migrations = false)]
async fn hidden_stations_cost_no_upstream_call_and_no_location_parameters(db: PgPool) {
    let stub = Stub::start().await;
    let (w, s) = world(db, &stub).await;
    for who in ["stranger", "pending", "declined", "revoked"] {
        assert_eq!(
            w.get(who, &fpath(s)).await.status,
            StatusCode::NOT_FOUND,
            "{who}"
        );
        assert_eq!(
            w.get(who, &changes(s, "from=2026-10-12&to=2026-10-14"))
                .await
                .status,
            StatusCode::NOT_FOUND,
            "{who}"
        );
    }
    assert_eq!(stub.server.received_requests().await.unwrap().len(), 0);
    for q in ["lat=1", "lon=1", "latitude=1", "location_id=x", "x=1"] {
        assert_eq!(
            w.get("owner", &format!("{}?{q}", fpath(s)))
                .await
                .status
                .as_u16(),
            422,
            "{q}"
        );
    }
    assert_eq!(
        w.get("approved_follower", &fpath(s)).await.status,
        StatusCode::OK
    );
}

#[sqlx::test(migrations = false)]
async fn the_call_budget_is_enforced(db: PgPool) {
    let stub = Stub::start().await;
    let api = stub.api();
    let w = World::with(db, move |b| {
        b.om = Some(api);
        b.om_limits = Some((3, 3));
    })
    .await;
    // Three upstream calls are allowed: forecast + ensemble for one place, then forecast for a second.
    let a = w
        .add_station("owner", "AA:BB:CC:00:00:01", "A", 46.6, -112.0, None)
        .await;
    let b = w
        .add_station("owner", "AA:BB:CC:00:00:02", "B", 41.3, -105.6, None)
        .await;
    assert_eq!(w.get("owner", &fpath(a)).await.status, StatusCode::OK);
    let r = w.get("owner", &fpath(b)).await;
    // Forecast got the last token, the ensemble found the bucket empty: bands are absent, the forecast is served.
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert!(r.json()["ensemble"].is_null());
    let r = w
        .get("owner", &changes(a, "from=2026-10-12&to=2026-10-14"))
        .await;
    assert_eq!(
        (r.status.as_u16(), r.code().as_str()),
        (503, "upstream_busy")
    );
    assert!(r.headers.contains_key("retry-after"));
    // Time refills the bucket.
    w.clock.advance(Cd::minutes(1));
    assert_eq!(
        w.get("owner", &changes(a, "from=2026-10-12&to=2026-10-14"))
            .await
            .status,
        StatusCode::OK
    );
}

#[sqlx::test(migrations = false)]
async fn forecast_changes_shape_validation_and_cache(db: PgPool) {
    let stub = Stub::start().await;
    let (w, s) = world(db, &stub).await;
    let r = w
        .get("owner", &changes(s, "from=2026-10-12&to=2026-10-14"))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let j = r.json();
    assert_eq!(j["model"], "gfs_seamless");
    assert_eq!(j["lead_days"], json!([0, 1, 2, 3, 4, 5, 6, 7]));
    let days = j["days"].as_array().unwrap();
    assert_eq!(days.len(), 3);
    assert_eq!(days[0]["date"], "2026-10-12");
    for d in days {
        for k in ["precip_in", "prob_precip_max", "tmax_f", "tmin_f"] {
            assert_eq!(d[k].as_array().unwrap().len(), 8, "{k}");
        }
        assert!(d["precip_in"][0].is_number() && d["tmax_f"][7].is_number());
    }
    let q = stub.requests("/previous").await[0]
        .url
        .query()
        .unwrap()
        .to_owned();
    assert!(
        q.contains("models=gfs_seamless")
            && q.contains("start_date=2026-10-11")
            && q.contains("end_date=2026-10-15"),
        "{q}"
    );
    assert_eq!(q.matches("previous_day").count(), 21, "{q}");
    for v in [
        "temperature_2m",
        "precipitation",
        "precipitation_probability",
        "temperature_2m_previous_day7",
    ] {
        assert!(q.contains(v), "{v}");
    }
    // Two hours of caching.
    w.clock.advance(Cd::minutes(119));
    assert_eq!(
        w.get("owner", &changes(s, "from=2026-10-12&to=2026-10-14"))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(stub.hits("/previous").await, 1);
    w.clock.advance(Cd::minutes(2));
    assert_eq!(
        w.get("owner", &changes(s, "from=2026-10-12&to=2026-10-14"))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(stub.hits("/previous").await, 2);

    for q in [
        "from=2026-10-14&to=2026-10-12",
        "from=2026-10-03&to=2026-10-17",
        "from=2023-12-31&to=2024-01-02",
        "from=2026-10-12&to=2026-11-05",
        "from=2026-10-12",
        "to=2026-10-12",
        "from=bad&to=2026-10-12",
        "from=2026-10-12&to=2026-10-14&x=1",
    ] {
        assert_eq!(
            w.get("owner", &changes(s, q)).await.status.as_u16(),
            422,
            "{q}"
        );
    }
    // The widest allowed window (14 days) works.
    assert_eq!(
        w.get("owner", &changes(s, "from=2026-10-11&to=2026-10-24"))
            .await
            .status,
        StatusCode::OK
    );
}

/// A synthetic previous-runs body: `hours` hourly steps from `start` (local), constant values, with optional holes.
fn synthetic(start: &str, hours: usize, prob_null: bool, hole: Option<(usize, usize)>) -> String {
    let times: Vec<String> = (0..hours).map(|i| format!("{start}-{i}")).collect();
    let mut first = start.to_owned();
    first.push_str("T00:00");
    let mut times = times;
    times[0] = first;
    let series = |v: f64, null: bool, lead: usize| -> Vec<Value> {
        (0..hours)
            .map(|i| {
                if null || hole == Some((lead, i)) {
                    Value::Null
                } else {
                    json!(v)
                }
            })
            .collect()
    };
    let mut h = serde_json::Map::new();
    h.insert("time".into(), json!(times));
    for lead in 0..8 {
        let sfx = if lead == 0 {
            String::new()
        } else {
            format!("_previous_day{lead}")
        };
        h.insert(
            format!("temperature_2m{sfx}"),
            json!(series(50.0 + lead as f64, false, lead)),
        );
        h.insert(
            format!("precipitation{sfx}"),
            json!(series(0.01, false, lead)),
        );
        h.insert(
            format!("precipitation_probability{sfx}"),
            json!(series(30.0, prob_null, lead)),
        );
    }
    json!({ "latitude": 46.5, "longitude": -112.0, "hourly": h }).to_string()
}

#[sqlx::test(migrations = false)]
async fn dst_days_missing_hours_and_probability_gaps(db: PgPool) {
    let stub = Stub::start().await;
    // 2026-11-01 in America/Denver has 25 hours. The window is padded by a day each side: 31 Oct to 2 Nov.
    // Lead 3 has a hole inside 1 Nov; the probability series is entirely null.
    let hole_hour = 24 + 5;
    stub.status(
        "/previous",
        200,
        &synthetic("2026-10-31", 72, true, Some((3, hole_hour))),
    )
    .await;
    let (w, s) = world(db, &stub).await;
    let r = w
        .get("owner", &changes(s, "from=2026-11-01&to=2026-11-01"))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let d = &r.json()["days"][0];
    assert_eq!(d["date"], "2026-11-01");
    let precip: Vec<f64> = d["precip_in"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap_or(-1.0))
        .collect();
    assert_eq!(precip[0], 0.25, "25 hours of 0.01 in");
    assert_eq!(precip[3], -1.0, "a lead with a missing hour is null");
    assert_eq!(precip[1], 0.25);
    assert!(
        d["prob_precip_max"]
            .as_array()
            .unwrap()
            .iter()
            .all(Value::is_null),
        "no probability before its start date"
    );
    assert_eq!(d["tmax_f"][2], 52.0);
}

#[sqlx::test(migrations = false)]
async fn one_user_cannot_drain_the_forecast_budget(db: PgPool) {
    let stub = Stub::start().await;
    let (w, s) = limited_world(db, &stub, false).await;
    // Burst of ten across both routes, then 429 with Retry-After and no further upstream traffic.
    for i in 0..10 {
        let path = if i % 2 == 0 {
            fpath(s)
        } else {
            changes(s, "from=2026-10-12&to=2026-10-14")
        };
        assert_eq!(w.get("owner", &path).await.status, StatusCode::OK, "{i}");
    }
    let hits = stub.server.received_requests().await.unwrap().len();
    for path in [fpath(s), changes(s, "from=2026-10-12&to=2026-10-14")] {
        let r = w.get("owner", &path).await;
        assert_eq!(
            (r.status.as_u16(), r.code().as_str()),
            (429, "rate_limited")
        );
        assert!(r.headers.contains_key("retry-after"));
    }
    assert_eq!(stub.server.received_requests().await.unwrap().len(), hits);
    // Another user keeps their own budget.
    assert_eq!(
        w.get("approved_follower", &fpath(s)).await.status,
        StatusCode::OK
    );
}

#[sqlx::test(migrations = false)]
async fn upstream_errors_are_cached_briefly_so_waiters_fail_fast(db: PgPool) {
    let stub = Stub::start().await;
    // Both calls hang 400 ms and then fail. Without a negative cache, N serial waiters would take N times as long.
    for p in ["/forecast", "/ensemble"] {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(500).set_delay(Duration::from_millis(400)))
            .with_priority(1)
            .mount(&stub.server)
            .await;
    }
    let (w, s) = world(db, &stub).await;
    let started = std::time::Instant::now();
    let p = fpath(s);
    let rs = futures::future::join_all((0..6).map(|_| w.get("owner", &p))).await;
    assert!(rs.iter().all(|r| r.status.as_u16() == 502));
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        (stub.hits("/forecast").await, stub.hits("/ensemble").await),
        (1, 1)
    );
    // The failure memory is short: after ten seconds the upstream is asked again.
    w.clock.advance(Cd::seconds(11));
    assert_eq!(w.get("owner", &p).await.status.as_u16(), 502);
    assert_eq!(stub.hits("/forecast").await, 2);
}

#[sqlx::test(migrations = false)]
async fn compressed_upstream_bodies_are_requested_and_decoded(db: PgPool) {
    use std::io::Write;
    let stub = Stub::start().await;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&fixture("forecast__")).unwrap();
    let body = gz.finish().unwrap();
    Mock::given(method("GET"))
        .and(path("/forecast"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-encoding", "gzip")
                .insert_header("content-type", "application/json")
                .set_body_bytes(body),
        )
        .with_priority(1)
        .mount(&stub.server)
        .await;
    let (w, s) = world(db, &stub).await;
    let j = w.get("owner", &fpath(s)).await.json();
    assert_eq!(j["deterministic"]["daily"].as_array().unwrap().len(), 16);
    for p in ["/forecast", "/ensemble"] {
        let enc = stub.requests(p).await[0]
            .headers
            .get("accept-encoding")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert!(
            enc.contains("gzip") && enc.contains("deflate"),
            "{p}: {enc}"
        );
    }
}

#[sqlx::test(migrations = false)]
async fn replay_serves_a_forecast_starting_today(db: PgPool) {
    let w = World::new(db).await;
    let s = w
        .add_station("owner", "AA:BB:CC:00:00:01", "Loveland", 40.4, -105.1, None)
        .await;
    let r = w.get("owner", &fpath(s)).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let j = r.json();
    assert_eq!(j["deterministic"]["daily"][0]["date"], "2026-10-17");
    assert!(j["ensemble"]["daily"][0]["date"].as_str().unwrap() >= "2026-10-17");
    // Replay shifts sunrise and sunset with the days: each one falls on its own local date.
    let tz: chrono_tz::Tz = "America/Denver".parse().unwrap();
    for day in j["deterministic"]["daily"]
        .as_array()
        .unwrap()
        .iter()
        .take(3)
    {
        let date: chrono::NaiveDate = day["date"].as_str().unwrap().parse().unwrap();
        for k in ["sunrise", "sunset"] {
            let t = chrono::DateTime::parse_from_rfc3339(day[k].as_str().unwrap())
                .unwrap()
                .with_timezone(&tz);
            assert_eq!(t.date_naive(), date, "{k} on {date}");
        }
    }
    // The running total starts at the first step not in the past: 17 Oct 12:00 UTC (the test clock) is step 6 of
    // the shifted series, which starts at 06:00 UTC (local midnight). Earlier steps are null.
    let d = &j["deterministic"]["hourly"];
    let step = d["time"]
        .as_array()
        .unwrap()
        .iter()
        .position(|t| t == "2026-10-17T12:00:00Z")
        .unwrap();
    assert_eq!(step, 6);
    assert!(d["precip_accum_in"].as_array().unwrap()[..step]
        .iter()
        .all(Value::is_null));
    assert_eq!(d["precip_accum_in"][step], d["precip_in"][step]);
    let r = w
        .get("owner", &changes(s, "from=2026-10-10&to=2026-10-16"))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json()["days"].as_array().unwrap().len(), 7);
    // Hidden station: 404, and this works for a follower.
    assert_eq!(
        w.get("stranger", &fpath(s)).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        w.get("approved_follower", &fpath(s)).await.status,
        StatusCode::OK
    );
}
