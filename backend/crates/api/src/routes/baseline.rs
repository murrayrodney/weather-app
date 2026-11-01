//! Baseline picker (02 4.15, 5.10): owner only; the id must come from the API's own ranked list.

use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::Json;
use famweather_core::ratelimit::reserve;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;

use crate::access::{OwnedStationPath, ViewableStation};
use crate::audit;
use crate::auth::AuthUser;
use crate::baseline::{self, Candidate};
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, NoQuery};
use crate::repo::stations;
use crate::state::AppState;

/// The `baseline` limiter; it sits before the station extractor so an over-limit call does no database or ACIS work.
pub struct BaselineLimit;

impl FromRequestParts<AppState> for BaselineLimit {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, st).await?;
        st.limiters
            .baseline
            .check(user.id)
            .map_err(|retry_after| ApiError::RateLimited { retry_after })?;
        Ok(BaselineLimit)
    }
}

/// Ranked candidates from the cache, or from ACIS behind the `acis:api` gate.
async fn ranked(st: &AppState, s: &ViewableStation) -> ApiResult<Vec<Candidate>> {
    let now = st.clock.now();
    let key = baseline::cache_key(s.lat(), s.lon());
    if let Some(l) = st.baseline_cache.get(key, now) {
        return Ok(l);
    }
    let wait = reserve(&st.db, "acis:api", st.gates.acis_spacing, st.gates.max_wait)
        .await
        .map_err(|_| ApiError::UpstreamBusy { retry_after: 2 })?;
    tokio::time::sleep(wait).await;
    let metas = st
        .acis
        .nearby_stations(baseline::search_box(s.lat(), s.lon()))
        .await
        .map_err(|_| ApiError::UpstreamUnavailable)?;
    let list = baseline::rank(&metas, s.lat(), s.lon(), st.clock.now().date_naive());
    st.baseline_cache.put(key, st.clock.now(), list.clone());
    Ok(list)
}

pub async fn candidates(
    State(st): State<AppState>,
    _user: AuthUser,
    _: BaselineLimit,
    OwnedStationPath(s): OwnedStationPath,
    _: NoQuery,
) -> ApiResult<Json<Value>> {
    let row =
        sqlx::query("SELECT elevation_ft, reference_station_id FROM app.stations WHERE id = $1")
            .bind(s.id())
            .fetch_one(&st.db)
            .await?;
    let station_elev: Option<i32> = row.try_get("elevation_ft")?;
    let current: Option<String> = row.try_get("reference_station_id")?;
    let list = ranked(&st, &s).await?;
    let items: Vec<Value> = list
        .iter()
        .take(baseline::RESPONSE_CAP)
        .map(|c| {
            let diff = c
                .elevation_ft
                .zip(station_elev)
                .map(|(a, b)| (a - f64::from(b)).round() as i64);
            json!({
                "reference_station_id": c.reference_station_id,
                "name": c.name,
                "airport_code": c.airport_code,
                "distance_mi": c.distance_mi,
                "elevation_ft": c.elevation_ft.map(|e| e.round() as i64),
                "elevation_diff_ft": diff,
                "record_start": c.record_start,
                "record_years": c.record_years,
                "short_record": c.record_years < baseline::SHORT_RECORD_YEARS,
            })
        })
        .collect();
    Ok(Json(json!({
        "station_id": s.id(),
        "station_elevation_ft": station_elev,
        "radius_mi": baseline::RADIUS_MI as i64,
        "current": current,
        "suggested": list.first().map(|c| c.reference_station_id.clone()),
        "candidates": items,
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutBody {
    reference_station_id: String,
}

fn valid_ghcn(id: &str) -> bool {
    id.len() == 11 && id.starts_with("USW") && id[3..].bytes().all(|b| b.is_ascii_digit())
}

pub async fn put(
    State(st): State<AppState>,
    user: AuthUser,
    _: BaselineLimit,
    OwnedStationPath(s): OwnedStationPath,
    ApiJson(body): ApiJson<PutBody>,
) -> ApiResult<Json<stations::StationOut>> {
    if !valid_ghcn(&body.reference_station_id) {
        return Err(ApiError::Validation("The station id is not valid."));
    }
    // Setting the baseline the station already has is a 200 no-op: no ACIS lookup, no write, no audit row (5.10).
    let current: Option<String> = sqlx::query_scalar(
        "SELECT reference_station_id FROM app.stations WHERE id = $1 AND owner_id = $2",
    )
    .bind(s.id())
    .bind(user.id)
    .fetch_one(&st.db)
    .await?;
    if current.as_deref() == Some(body.reference_station_id.as_str()) {
        let mut v = stations::by_ids(&st.db, user.id, &[s.id()]).await?;
        return v.pop().map(Json).ok_or(ApiError::NotFound);
    }
    let list = ranked(&st, &s).await?;
    let c = list
        .iter()
        .find(|c| c.reference_station_id == body.reference_station_id)
        .ok_or(ApiError::NotACandidate)?;
    let mut tx = st.db.begin().await?;
    let row = sqlx::query(
        "SELECT timezone, std_utc_offset_minutes, reference_station_id FROM app.stations WHERE id = $1 AND owner_id = $2",
    )
    .bind(s.id())
    .bind(user.id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    let current: Option<String> = row.try_get("reference_station_id")?;
    if current.as_deref() != Some(c.reference_station_id.as_str()) {
        sqlx::query(
            "INSERT INTO app.reference_stations (id, name, lat, lon, elevation_ft, timezone, std_utc_offset_minutes) \
             VALUES ($1, $2, $3::float8, $4::float8, $5, $6, $7) ON CONFLICT (id) DO NOTHING",
        )
        .bind(&c.reference_station_id)
        .bind(&c.name)
        .bind(c.lat)
        .bind(c.lon)
        .bind(c.elevation_ft.map(|e| e.round() as i32))
        .bind(row.try_get::<String, _>("timezone")?)
        .bind(row.try_get::<i16, _>("std_utc_offset_minutes")?)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE app.stations SET reference_station_id = $1 WHERE id = $2 AND owner_id = $3",
        )
        .bind(&c.reference_station_id)
        .bind(s.id())
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
        audit::record(
            &mut tx,
            user.id,
            audit::STATION_BASELINE_SET,
            "station",
            s.id(),
            json!({ "reference_station_id": c.reference_station_id }),
        )
        .await?;
    }
    tx.commit().await?;
    let mut v = stations::by_ids(&st.db, user.id, &[s.id()]).await?;
    v.pop().map(Json).ok_or(ApiError::NotFound)
}
