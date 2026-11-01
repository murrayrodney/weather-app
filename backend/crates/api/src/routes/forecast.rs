//! Live forecast with ensemble bands and the forecast-change view (02 5.6, 5.6a). Nothing is stored.

use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::Json;
use chrono::{DateTime, Duration, NaiveDate, SecondsFormat, Utc};
use serde_json::{json, Map, Value};

use crate::access::ViewableStationPath;
use crate::auth::AuthUser;
use crate::error::{ApiError, ApiResult};
use crate::extract::{NoQuery, Params};
use crate::openmeteo::accumulation::{running_sum, start_step};
use crate::openmeteo::changes::{self, LEADS};
use crate::openmeteo::percentiles::{daily, hourly, member_days};
use crate::openmeteo::{EnsembleRaw, ForecastRaw, OmError, ENSEMBLE_MEMBERS};
use crate::state::AppState;

/// The per-user `forecast` limiter, checked before the station lookup and before any upstream call.
pub struct ForecastLimit;

impl FromRequestParts<AppState> for ForecastLimit {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, st).await?;
        st.limiters
            .forecast
            .check(user.id)
            .map_err(|retry_after| ApiError::RateLimited { retry_after })?;
        Ok(ForecastLimit)
    }
}

const ATTRIBUTION: &str = "Weather data by Open-Meteo.com (CC BY 4.0)";

fn map_err(e: OmError) -> ApiError {
    match e {
        OmError::Busy { retry_after } => ApiError::UpstreamBusy { retry_after },
        OmError::Unavailable => ApiError::UpstreamUnavailable,
    }
}

fn r(x: f64, places: i32) -> f64 {
    let m = 10f64.powi(places);
    (x * m).round() / m
}

fn opt(v: &[Option<f64>], places: i32) -> Vec<Value> {
    v.iter()
        .map(|x| x.map_or(Value::Null, |x| json!(r(x, places))))
        .collect()
}

fn iso(t: &[DateTime<Utc>]) -> Vec<String> {
    t.iter()
        .map(|t| t.to_rfc3339_opts(SecondsFormat::Secs, true))
        .collect()
}

/// Whole numbers (rounded), `null` when missing.
fn ints(v: &[Option<f64>]) -> Vec<Value> {
    v.iter()
        .map(|x| x.map_or(Value::Null, |x| json!(x.round() as i64)))
        .collect()
}

fn utc(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// `now` decides where the precipitation totals start (02 5.6): they are computed per request, not cached.
fn deterministic(f: &ForecastRaw, now: DateTime<Utc>) -> Value {
    let accum = running_sum(&f.precip, start_step(&f.times, now));
    json!({
        "model": "gfs_seamless",
        "hourly": {
            "time": iso(&f.times),
            "temperature_f": opt(&f.temp, 1),
            "precip_in": opt(&f.precip, 3),
            "prob_precip": f.prob.iter().map(|x| x.map_or(Value::Null, |x| json!(x.round() as i64))).collect::<Vec<_>>(),
            "precip_accum_in": opt(&accum, 3),
            "apparent_temp_f": opt(&f.apparent_f, 1),
            "dew_point_f": opt(&f.dew_point_f, 1),
            "cloud_pct": ints(&f.cloud_pct),
            "humidity_pct": ints(&f.humidity_pct),
            "pressure_inhg": opt(&f.pressure_inhg, 2),
            "wind_mph": opt(&f.wind_mph, 1),
            "wind_dir_deg": ints(&f.wind_dir_deg),
            "gust_mph": opt(&f.gust_mph, 1),
            "weather_code": ints(&f.weather_code),
            "snow_in": opt(&f.snow_in, 3),
        },
        "daily": f.daily.iter().map(|d| json!({
            "date": d.date,
            "tmax_f": d.tmax.map(|x| r(x, 1)),
            "tmin_f": d.tmin.map(|x| r(x, 1)),
            "precip_in": d.precip.map(|x| r(x, 3)),
            "prob_precip_max": d.prob_max.map(|x| x.round() as i64),
            "weather_code": d.weather_code.map(|x| x.round() as i64),
            "snow_in": d.snow.map(|x| r(x, 3)),
            "sunrise": d.sunrise.map(utc),
            "sunset": d.sunset.map(utc),
        })).collect::<Vec<_>>(),
    })
}

fn bands(a: &[Vec<Option<f64>>], steps: usize, places: i32) -> Value {
    let [p10, p50, p90] = hourly(a, steps);
    json!({ "p10": opt(&p10, places), "p50": opt(&p50, places), "p90": opt(&p90, places) })
}

fn triple(p: [f64; 3], places: i32) -> Value {
    json!({ "p10": r(p[0], places), "p50": r(p[1], places), "p90": r(p[2], places) })
}

/// The ensemble total is accumulated per member first, then percentiles are taken (never the reverse).
fn ensemble(e: &EnsembleRaw, tz: chrono_tz::Tz, now: DateTime<Utc>) -> Value {
    let steps = e.times.len();
    let members: Vec<_> = e
        .temp
        .iter()
        .zip(&e.precip)
        .map(|(t, p)| member_days(&e.times, t, p, tz))
        .collect();
    let from = start_step(&e.times, now);
    let accum: Vec<Vec<Option<f64>>> = e.precip.iter().map(|p| running_sum(p, from)).collect();
    json!({
        "model": "ecmwf_ifs025",
        "members": ENSEMBLE_MEMBERS,
        "hourly": {
            "time": iso(&e.times),
            "temperature_f": bands(&e.temp, steps, 1),
            "precip_in": bands(&e.precip, steps, 3),
            "precip_accum_in": bands(&accum, steps, 3),
        },
        "daily": daily(&members).iter().map(|d| json!({
            "date": d.date,
            "tmax_f": triple(d.tmax, 1),
            "tmin_f": triple(d.tmin, 1),
            "precip_in": triple(d.precip, 3),
            "prob_precip": d.prob_precip,
        })).collect::<Vec<_>>(),
    })
}

pub async fn forecast(
    State(st): State<AppState>,
    _: ForecastLimit,
    ViewableStationPath(s): ViewableStationPath,
    _: NoQuery,
) -> ApiResult<Json<Value>> {
    let now = st.clock.now();
    let (f, e) = tokio::join!(st.openmeteo.forecast(&s), st.openmeteo.ensemble(&s));
    if let (Err(a), Err(b)) = (&f, &e) {
        // Rate-limited upstream is "busy"; anything else is "unavailable".
        return Err(if matches!(a, OmError::Busy { .. }) {
            map_err(*a)
        } else {
            map_err(*b)
        });
    }
    if let Err(err) = &e {
        tracing::warn!(error = %err, "ensemble unavailable; serving the forecast without bands");
    }
    if let Err(err) = &f {
        tracing::warn!(error = %err, "forecast unavailable; serving the ensemble only");
    }
    let mut generated: Vec<DateTime<Utc>> = Vec::new();
    let mut stale = false;
    let det = f.ok().map(|c| {
        generated.push(c.fetched_at);
        stale |= c.stale;
        deterministic(&c.value, now)
    });
    let ens = e.ok().map(|c| {
        generated.push(c.fetched_at);
        stale |= c.stale;
        ensemble(&c.value, s.timezone(), now)
    });
    let generated_at = generated
        .into_iter()
        .min()
        .unwrap_or_else(|| st.clock.now());
    let mut out = Map::new();
    out.insert("station_id".into(), json!(s.id()));
    out.insert("timezone".into(), json!(s.timezone().name()));
    out.insert(
        "generated_at".into(),
        json!(generated_at.to_rfc3339_opts(SecondsFormat::Secs, true)),
    );
    out.insert("stale".into(), json!(stale));
    // Additive keys (proposal 07 phase 2): wind, pressure, snow and the percentages (cloud, humidity, probability).
    out.insert(
        "units".into(),
        json!({
            "temperature": "F",
            "precipitation": "in",
            "wind": "mph",
            "pressure": "inHg",
            "snow": "in",
            "percent": "%"
        }),
    );
    out.insert("attribution".into(), json!(ATTRIBUTION));
    out.insert("deterministic".into(), det.unwrap_or(Value::Null));
    out.insert("ensemble".into(), ens.unwrap_or(Value::Null));
    Ok(Json(Value::Object(out)))
}

fn date(p: &Params, key: &str) -> ApiResult<NaiveDate> {
    p.get(key)
        .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        .ok_or(ApiError::Validation(
            "from and to must be YYYY-MM-DD dates.",
        ))
}

pub async fn changes(
    State(st): State<AppState>,
    _: ForecastLimit,
    ViewableStationPath(s): ViewableStationPath,
    req: Request,
) -> ApiResult<Json<Value>> {
    let p = Params::parse(req.uri().query(), &["from", "to"])?;
    let (from, to) = (date(&p, "from")?, date(&p, "to")?);
    let today = st.clock.now().with_timezone(&s.timezone()).date_naive();
    let earliest = NaiveDate::from_ymd_opt(2024, 1, 1).expect("static date");
    if from > to
        || (to - from).num_days() + 1 > 14
        || from < earliest
        || to > today + Duration::days(15)
    {
        return Err(ApiError::Validation("That date range is not allowed."));
    }
    let c = st
        .openmeteo
        .previous_runs(&s, from, to)
        .await
        .map_err(map_err)?;
    Ok(Json(json!({
        "station_id": s.id(),
        "timezone": s.timezone().name(),
        "model": "gfs_seamless",
        "generated_at": c.fetched_at.to_rfc3339_opts(SecondsFormat::Secs, true),
        "units": { "temperature": "F", "precipitation": "in" },
        "attribution": ATTRIBUTION,
        "lead_days": (0..LEADS).collect::<Vec<_>>(),
        "days": changes::assemble(&c.value, s.timezone(), from, to),
    })))
}
