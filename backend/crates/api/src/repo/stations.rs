//! Station objects (02 5.3). Never returns mac, credential_id, email or sync columns.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgPool, Row};
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct Owner {
    pub id: Uuid,
    pub display_name: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct BaselineOut {
    pub reference_station_id: String,
    pub name: String,
    pub source: String,
    pub history_loaded: bool,
}

#[derive(Debug, Serialize)]
pub struct StationOut {
    pub id: Uuid,
    pub name: String,
    pub is_own: bool,
    pub owner: Owner,
    pub lat: f64,
    pub lon: f64,
    pub coordinates_rounded: bool,
    pub timezone: String,
    pub baseline: Option<BaselineOut>,
    pub latest_observation_at: Option<DateTime<Utc>>,
}

/// Expands to ONE static string literal (`concat!`): SQL is never assembled at run time.
macro_rules! station_sql {
    ($where_clause:literal) => {
        concat!(
            "SELECT s.id, s.name, s.owner_id, u.display_name AS owner_name, ",
            "s.lat::float8 AS lat, s.lon::float8 AS lon, s.timezone, s.last_observed_at, ",
            "s.reference_station_id, rs.name AS ref_name, rs.source AS ref_source, ",
            "(rs.last_synced_date IS NOT NULL) AS history_loaded ",
            "FROM app.stations s JOIN app.users u ON u.id = s.owner_id ",
            "LEFT JOIN app.reference_stations rs ON rs.id = s.reference_station_id ",
            $where_clause,
            " ORDER BY (s.owner_id <> $1), u.display_name, s.name"
        )
    };
}

fn map(viewer: Uuid, r: &sqlx::postgres::PgRow) -> Result<StationOut, sqlx::Error> {
    let owner_id: Uuid = r.try_get("owner_id")?;
    let baseline = match r.try_get::<Option<String>, _>("reference_station_id")? {
        Some(id) => Some(BaselineOut {
            reference_station_id: id,
            name: r.try_get("ref_name")?,
            source: r.try_get("ref_source")?,
            history_loaded: r
                .try_get::<Option<bool>, _>("history_loaded")?
                .unwrap_or(false),
        }),
        None => None,
    };
    Ok(StationOut {
        id: r.try_get("id")?,
        name: r.try_get("name")?,
        is_own: owner_id == viewer,
        owner: Owner {
            id: owner_id,
            display_name: r.try_get("owner_name")?,
        },
        lat: r.try_get("lat")?,
        lon: r.try_get("lon")?,
        coordinates_rounded: true,
        timezone: r.try_get("timezone")?,
        baseline,
        latest_observation_at: r.try_get("last_observed_at")?,
    })
}

/// Stations the viewer can see: `app.visible_station_ids`, never a join on follows.
pub async fn list_visible(db: &PgPool, viewer: Uuid) -> Result<Vec<StationOut>, sqlx::Error> {
    let rows = sqlx::query(station_sql!(
        "WHERE s.id IN (SELECT app.visible_station_ids($1))"
    ))
    .bind(viewer)
    .fetch_all(db)
    .await?;
    rows.iter().map(|r| map(viewer, r)).collect()
}

/// Station objects for ids the caller has ALREADY been authorized for (extractor or the caller's own writes).
pub async fn by_ids(
    db: &PgPool,
    viewer: Uuid,
    ids: &[Uuid],
) -> Result<Vec<StationOut>, sqlx::Error> {
    let rows = sqlx::query(station_sql!(
        "WHERE s.id = ANY($2) AND app.can_view($1, s.id)"
    ))
    .bind(viewer)
    .bind(ids)
    .fetch_all(db)
    .await?;
    rows.iter().map(|r| map(viewer, r)).collect()
}
