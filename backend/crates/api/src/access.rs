//! Authorization: `can_view` cannot be skipped (02 section 4.5, S10, G7).
//!
//! `ViewableStation(s)` are constructible only here, by extractors that require an `AuthUser` and run one statement
//! calling `app.can_view`. Every repo function and every upstream-location function takes them, never a bare UUID.

use axum::extract::{FromRequestParts, Path};
use axum::http::request::Parts;
use chrono_tz::Tz;
use sqlx::Row;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::ApiError;
use crate::extract::Params;
use crate::state::AppState;

/// A station the caller may see. Coordinates are the rounded 2-decimal stored values (04 5.5).
#[derive(Debug, Clone)]
pub struct ViewableStation {
    id: Uuid,
    owner_id: Uuid,
    timezone: Tz,
    reference_station_id: Option<String>,
    lat: f64,
    lon: f64,
}

impl ViewableStation {
    /// Unit-test constructor (no database, never in a release build): the replay tests need a zone and coordinates.
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_test(timezone: Tz, lat: f64, lon: f64) -> Self {
        Self {
            id: Uuid::nil(),
            owner_id: Uuid::nil(),
            timezone,
            reference_station_id: None,
            lat,
            lon,
        }
    }

    pub fn id(&self) -> Uuid {
        self.id
    }
    pub fn owner_id(&self) -> Uuid {
        self.owner_id
    }
    pub fn timezone(&self) -> Tz {
        self.timezone
    }
    pub fn reference_station_id(&self) -> Option<&str> {
        self.reference_station_id.as_deref()
    }
    pub fn lat(&self) -> f64 {
        self.lat
    }
    pub fn lon(&self) -> f64 {
        self.lon
    }
}

/// 1 to 5 distinct viewable stations.
#[derive(Debug, Clone)]
pub struct ViewableStations(Vec<ViewableStation>);

impl ViewableStations {
    pub fn iter(&self) -> std::slice::Iter<'_, ViewableStation> {
        self.0.iter()
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

pub const MAX_STATIONS: usize = 5;

const LOAD_VIEWABLE_SQL: &str = "SELECT s.id, s.owner_id, s.timezone, s.reference_station_id, \
     s.lat::float8 AS lat, s.lon::float8 AS lon \
     FROM app.stations s WHERE s.id = ANY($1) AND app.can_view($2, s.id)";

/// The one function that builds viewable stations. Fewer rows than distinct ids requested = 404, no partial results.
async fn load_viewable(
    st: &AppState,
    viewer: Uuid,
    ids: &[Uuid],
) -> Result<Vec<ViewableStation>, ApiError> {
    let mut distinct: Vec<Uuid> = Vec::with_capacity(ids.len());
    for id in ids {
        if !distinct.contains(id) {
            distinct.push(*id);
        }
    }
    let rows = sqlx::query(LOAD_VIEWABLE_SQL)
        .bind(&distinct)
        .bind(viewer)
        .fetch_all(&st.db)
        .await?;
    if rows.len() != distinct.len() {
        return Err(ApiError::NotFound);
    }
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let tz: String = r.try_get("timezone")?;
        out.push(ViewableStation {
            id: r.try_get("id")?,
            owner_id: r.try_get("owner_id")?,
            timezone: tz.parse::<Tz>().map_err(|_| {
                tracing::error!("a station row has an unknown time zone");
                ApiError::Internal
            })?,
            reference_station_id: r.try_get("reference_station_id")?,
            lat: r.try_get("lat")?,
            lon: r.try_get("lon")?,
        });
    }
    // Keep the caller's order.
    out.sort_by_key(|s| {
        distinct
            .iter()
            .position(|d| *d == s.id)
            .unwrap_or(usize::MAX)
    });
    Ok(out)
}

/// The `{station_id}` path segment, authorized with `can_view`. Malformed, missing and hidden are the same 404.
pub struct ViewableStationPath(pub ViewableStation);

impl FromRequestParts<AppState> for ViewableStationPath {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, st).await?;
        let Path(raw) = Path::<String>::from_request_parts(parts, st)
            .await
            .map_err(|_| ApiError::NotFound)?;
        let id = Uuid::parse_str(&raw).map_err(|_| ApiError::NotFound)?;
        let mut v = load_viewable(st, user.id, &[id]).await?;
        Ok(ViewableStationPath(v.remove(0)))
    }
}

/// Like [`ViewableStationPath`], and the caller must also be the owner (baseline routes). A viewer who is not the
/// owner gets the same 404.
pub struct OwnedStationPath(pub ViewableStation);

impl FromRequestParts<AppState> for OwnedStationPath {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, st).await?;
        let ViewableStationPath(s) = ViewableStationPath::from_request_parts(parts, st).await?;
        if s.owner_id != user.id {
            return Err(ApiError::NotFound);
        }
        Ok(OwnedStationPath(s))
    }
}

/// `station_ids` (comma separated, 1 to 5 UUIDs) read from the raw query string. Other keys may share the query;
/// routes parse them with [`Params`] through [`ViewableStationsQuery::split`].
pub struct ViewableStationsQuery(pub ViewableStations);

/// Parses the raw query: `station_ids` plus the route's own allow-listed keys (anything else is 422).
pub fn split_query(raw: Option<&str>, own_keys: &[&str]) -> Result<(Vec<Uuid>, Params), ApiError> {
    let mut allowed: Vec<&str> = vec!["station_ids"];
    allowed.extend_from_slice(own_keys);
    let params = Params::parse(raw, &allowed)?;
    let ids_raw = params
        .get("station_ids")
        .ok_or(ApiError::Validation("station_ids is required."))?;
    let mut ids = Vec::new();
    for part in ids_raw.split(',') {
        // A malformed id is the same 404 as an unknown one.
        let id = Uuid::parse_str(part.trim()).map_err(|_| ApiError::NotFound)?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    if ids.is_empty() || ids.len() > MAX_STATIONS {
        return Err(ApiError::Validation("Give between 1 and 5 stations."));
    }
    Ok((ids, params))
}

/// Authorizes the ids of an already-parsed `station_ids` list.
pub async fn authorize_stations(
    st: &AppState,
    user: &AuthUser,
    ids: &[Uuid],
) -> Result<ViewableStations, ApiError> {
    Ok(ViewableStations(load_viewable(st, user.id, ids).await?))
}

impl FromRequestParts<AppState> for ViewableStationsQuery {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, st).await?;
        let (ids, _) = split_query(parts.uri.query(), &[])?;
        Ok(ViewableStationsQuery(
            authorize_stations(st, &user, &ids).await?,
        ))
    }
}
