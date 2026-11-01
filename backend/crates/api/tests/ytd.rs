//! `/ytd` (02 5.5, 7.8): query-time GDD, eligibility, bands.

mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

async fn tiny_station(w: &World, reference: Option<&str>) -> Uuid {
    let s = w
        .add_station(
            "owner",
            "AA:BB:CC:00:00:01",
            "Backyard",
            40.4,
            -105.1,
            reference,
        )
        .await;
    // 95/60, 45/30, 70/40, then a day with no tmax (and no rain value, not complete).
    for (d, rain, tmax, tmin, complete) in [
        ("2026-04-01", Some(0.1), Some(95.0), Some(60.0), true),
        ("2026-04-02", Some(0.0), Some(45.0), Some(30.0), true),
        ("2026-04-03", Some(0.2), Some(70.0), Some(40.0), true),
        ("2026-04-04", None, None, Some(40.0), false),
    ] {
        sqlx::query(
            "INSERT INTO app.daily_rollups (station_id, local_date, rain_in, tmax_f, tmin_f, is_complete) \
             VALUES ($1, $2::date, $3::float8, $4::float8, $5::float8, $6)",
        )
        .bind(s)
        .bind(d)
        .bind(rain)
        .bind(tmax)
        .bind(tmin)
        .bind(complete)
        .execute(&w.db)
        .await
        .unwrap();
    }
    s
}

fn series(v: &Value) -> Vec<f64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|p| p["v"].as_f64().unwrap())
        .collect()
}

async fn ytd(w: &World, who: &str, s: Uuid, extra: &str) -> Value {
    let r = w
        .get(
            who,
            &format!("/api/v1/ytd?station_ids={s}&year=2026{extra}"),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    r.json()
}

#[sqlx::test(migrations = false)]
async fn current_series_and_gdd_parameters(db: PgPool) {
    let w = World::new(db).await;
    let s = tiny_station(&w, None).await;

    let j = ytd(&w, "owner", s, "").await;
    assert_eq!(
        j["gdd"],
        serde_json::json!({ "method": "simple_avg", "base_f": 50, "start": "04-01" })
    );
    assert_eq!(
        j["day_definition"],
        "local_standard_time_midnight_to_midnight"
    );
    let st = &j["stations"][0];
    assert!(st["baseline"].is_null());
    assert_eq!(st["prior_years"].as_array().unwrap().len(), 0);
    assert!(st["bands"].is_null());
    let cur = &st["current"];
    assert_eq!(series(&cur["gdd_cum"]), [27.5, 27.5, 32.5, 32.5]);
    assert_eq!(series(&cur["rain_cum_in"]), [0.1, 0.1, 0.3, 0.3]);
    assert_eq!(cur["first_date"], "2026-04-01");
    assert_eq!(cur["last_date"], "2026-04-04");
    assert_eq!(cur["last_complete_date"], "2026-04-03");
    assert_eq!(
        cur["missing_days"],
        serde_json::json!({ "rain": 1, "gdd": 1 })
    );
    assert_eq!(cur["gdd_cum"][0]["md"], "04-01");

    let j = ytd(&w, "owner", s, "&gdd_method=clamped_50_86").await;
    assert_eq!(
        series(&j["stations"][0]["current"]["gdd_cum"]),
        [23.0, 23.0, 33.0, 33.0]
    );
    let j = ytd(&w, "owner", s, "&gdd_base=45").await;
    assert_eq!(series(&j["stations"][0]["current"]["gdd_cum"])[0], 32.5);
    assert_eq!(j["gdd"]["base_f"], 45);
    // A different accumulation start changes the echo; the station series still begins at its first rollup row.
    let j = ytd(&w, "owner", s, "&gdd_start=01-01").await;
    assert_eq!(j["gdd"]["start"], "01-01");
    assert_eq!(j["stations"][0]["current"]["rain_cum_in"][0]["md"], "04-01");
    let j = ytd(&w, "owner", s, "&gdd_start=04-03").await;
    assert_eq!(series(&j["stations"][0]["current"]["gdd_cum"]), [5.0, 5.0]);
    // Rain keeps its own window.
    let j = ytd(&w, "owner", s, "&start=04-03").await;
    assert_eq!(
        series(&j["stations"][0]["current"]["rain_cum_in"]),
        [0.2, 0.2]
    );
}

#[sqlx::test(migrations = false)]
async fn invalid_parameters_are_422(db: PgPool) {
    let w = World::new(db).await;
    let s = tiny_station(&w, None).await;
    for q in [
        "&gdd_method=avg_50",
        "&gdd_base=31",
        "&gdd_base=71",
        "&gdd_base=abc",
        "&gdd_start=13-01",
        "&gdd_start=11-01&end=10-31",
        "&start=10-31&end=04-01",
        "&start=4-1",
        "&bogus=1",
        "&year=2027",
    ] {
        let r = w
            .get(
                "owner",
                &format!("/api/v1/ytd?station_ids={s}&year=2026{q}"),
            )
            .await;
        assert_eq!(r.status.as_u16(), 422, "{q}: {}", r.text());
        assert_eq!(r.code(), "validation_failed");
    }
    let r = w
        .get("owner", &format!("/api/v1/ytd?station_ids={s}"))
        .await;
    assert_eq!(r.status.as_u16(), 422, "year is required");
}

async fn seed_history(w: &World) {
    // 2014..=2025 inclusive, every day 04-01..10-31, simple deterministic weather.
    sqlx::query(
        "INSERT INTO app.reference_daily (reference_station_id, date, tmax_f, tmin_f, precip_in) \
         SELECT 'USW00099001', d::date, 70 + (extract(doy FROM d)::int % 10), 40 + (extract(doy FROM d)::int % 7), \
                CASE WHEN extract(doy FROM d)::int % 3 = 0 THEN 0.1 ELSE 0 END \
         FROM generate_series(DATE '2014-04-01', DATE '2025-10-31', interval '1 day') g(d) \
         WHERE to_char(d, 'MM-DD') BETWEEN '04-01' AND '10-31'",
    )
    .execute(&w.db)
    .await
    .unwrap();
    // 2019: 8 days with NULL precip and no trace (rain ineligible, GDD fine).
    sqlx::query("UPDATE app.reference_daily SET precip_in = NULL WHERE reference_station_id = 'USW00099001' AND date BETWEEN '2019-05-01' AND '2019-05-08'")
        .execute(&w.db).await.unwrap();
    // 2018: 8 trace days (NULL precip, trace = true) count as 0.00 and are NOT missing.
    sqlx::query("UPDATE app.reference_daily SET precip_in = NULL, precip_trace = true WHERE reference_station_id = 'USW00099001' AND date BETWEEN '2018-05-01' AND '2018-05-08'")
        .execute(&w.db).await.unwrap();
    // 2020: ten absent days (both series ineligible).
    sqlx::query("DELETE FROM app.reference_daily WHERE reference_station_id = 'USW00099001' AND date BETWEEN '2020-06-01' AND '2020-06-10'")
        .execute(&w.db).await.unwrap();
}

#[sqlx::test(migrations = false)]
async fn baseline_years_eligibility_and_bands(db: PgPool) {
    let w = World::new(db).await;
    w.add_reference("USW00099001", true).await;
    let s = tiny_station(&w, Some("USW00099001")).await;
    seed_history(&w).await;

    let j = ytd(&w, "owner", s, "").await;
    let st = &j["stations"][0];
    assert_eq!(st["baseline"]["reference_station_id"], "USW00099001");
    assert_eq!(st["baseline"]["history_loaded"], true);
    let years: Vec<i64> = st["prior_years"]
        .as_array()
        .unwrap()
        .iter()
        .map(|y| y["year"].as_i64().unwrap())
        .collect();
    assert_eq!(
        years,
        [2025, 2024, 2023, 2022, 2021],
        "newest first, 2020 is ineligible"
    );
    let b = &st["bands"];
    // 12 candidate years: 2020 out of both; 2019 out of rain only.
    assert_eq!(
        b["years_included"],
        serde_json::json!({ "rain": 10, "gdd": 11 })
    );
    assert_eq!(b["percentiles"], serde_json::json!([10, 25, 50, 75, 90]));
    let first = &b["rain_cum_in"][0];
    assert_eq!(first["md"], "04-01");
    assert_eq!(first["p"].as_array().unwrap().len(), 5);
    let mid = &b["gdd_cum"].as_array().unwrap()[100]["p"];
    let p: Vec<f64> = mid
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect();
    assert!(
        p.windows(2).all(|w| w[0] <= w[1]),
        "percentiles are ordered: {p:?}"
    );
    assert_eq!(
        st["prior_years"][0]["missing_days"],
        serde_json::json!({ "rain": 0, "gdd": 0 })
    );
    assert!(
        st["prior_years"][0]["rain_cum_in"]
            .as_array()
            .unwrap()
            .len()
            >= 200
    );

    // A historical `year` includes later years, and a year ineligible for one series has that array null.
    let r = w
        .get("owner", &format!("/api/v1/ytd?station_ids={s}&year=2019"))
        .await
        .json();
    let st = &r["stations"][0];
    assert_eq!(st["prior_years"][0]["year"], 2025);
    let y2020 = st["prior_years"]
        .as_array()
        .unwrap()
        .iter()
        .find(|y| y["year"] == 2020);
    assert!(y2020.is_none() || y2020.unwrap()["rain_cum_in"].is_null());
}

#[sqlx::test(migrations = false)]
async fn a_year_without_ten_eligible_years_has_no_band_for_that_series(db: PgPool) {
    let w = World::new(db).await;
    w.add_reference("USW00099001", true).await;
    let s = tiny_station(&w, Some("USW00099001")).await;
    seed_history(&w).await;
    // Remove 2014..2018 temperatures: GDD eligibility drops below ten years, rain stays at ten or more.
    sqlx::query("UPDATE app.reference_daily SET tmax_f = NULL WHERE reference_station_id = 'USW00099001' AND date < '2019-01-01'")
        .execute(&w.db).await.unwrap();
    let st = &ytd(&w, "owner", s, "").await["stations"][0].clone();
    assert!(st["bands"]["gdd_cum"].is_null());
    assert!(!st["bands"]["rain_cum_in"].is_null());
}

#[sqlx::test(migrations = false)]
async fn station_and_baseline_share_the_gdd_parameters(db: PgPool) {
    let w = World::new(db).await;
    w.add_reference("USW00099001", true).await;
    let s = w
        .add_station(
            "owner",
            "AA:BB:CC:00:00:01",
            "Backyard",
            40.4,
            -105.1,
            Some("USW00099001"),
        )
        .await;
    seed_history(&w).await;
    // Copy 2025 into the station's 2026 so both sides hold identical rows.
    sqlx::query(
        "INSERT INTO app.daily_rollups (station_id, local_date, rain_in, tmax_f, tmin_f, is_complete) \
         SELECT $1, (date + interval '1 year')::date, precip_in, tmax_f, tmin_f, true FROM app.reference_daily \
         WHERE reference_station_id = 'USW00099001' AND date BETWEEN '2025-04-01' AND '2025-10-31'",
    )
    .bind(s)
    .execute(&w.db)
    .await
    .unwrap();
    for q in [
        "",
        "&gdd_method=clamped_50_86",
        "&gdd_base=40",
        "&gdd_method=clamped_50_86&gdd_base=60&gdd_start=05-01",
    ] {
        let j = ytd(&w, "owner", s, q).await;
        let st = &j["stations"][0];
        assert_eq!(
            st["current"]["gdd_cum"], st["prior_years"][0]["gdd_cum"],
            "{q}"
        );
        assert_eq!(
            st["current"]["rain_cum_in"], st["prior_years"][0]["rain_cum_in"],
            "{q}"
        );
        assert!(j.get("gdd_overlay").is_none());
    }
    // Changing only the parameters changes the output; nothing was stored.
    let a = ytd(&w, "owner", s, "").await;
    let b = ytd(&w, "owner", s, "&gdd_base=60").await;
    assert_ne!(
        a["stations"][0]["current"]["gdd_cum"],
        b["stations"][0]["current"]["gdd_cum"]
    );
}

#[sqlx::test(migrations = false)]
async fn history_still_loading_gives_the_station_line_only(db: PgPool) {
    let w = World::new(db).await;
    w.add_reference("USW00099001", false).await;
    let s = tiny_station(&w, Some("USW00099001")).await;
    seed_history(&w).await;
    let st = &ytd(&w, "owner", s, "").await["stations"][0].clone();
    assert_eq!(st["baseline"]["history_loaded"], false);
    assert_eq!(st["prior_years"].as_array().unwrap().len(), 0);
    assert!(st["bands"].is_null());
    assert_eq!(st["current"]["rain_cum_in"].as_array().unwrap().len(), 4);
}

#[sqlx::test(migrations = false)]
async fn a_follower_sees_the_ytd_of_an_approved_owner(db: PgPool) {
    let w = World::new(db).await;
    let s = tiny_station(&w, None).await;
    let j = ytd(&w, "approved_follower", s, "").await;
    assert_eq!(j["stations"].as_array().unwrap().len(), 1);
    let r = w
        .get(
            "stranger",
            &format!("/api/v1/ytd?station_ids={s}&year=2026"),
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}
