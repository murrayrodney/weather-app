use axum::extract::{Request, State};
use axum::Json;
use chrono::Datelike;
use serde_json::{json, Value};

use crate::access::{authorize_stations, split_query};
use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::repo::ytd::{self, YtdParams};
use crate::state::AppState;

pub async fn ytd(
    State(st): State<AppState>,
    user: AuthUser,
    req: Request,
) -> ApiResult<Json<Value>> {
    // `station_ids` is read ONLY through access::split_query; the other keys through the explicit allow-list.
    let (ids, params) = split_query(req.uri().query(), &ytd::ytd_keys())?;
    let current_year = st.clock.now().year();
    let p = YtdParams::parse(&params, current_year)?;
    let stations = authorize_stations(&st, &user, &ids).await?;
    let mut out = Vec::with_capacity(stations.len());
    for s in stations.iter() {
        out.push(ytd::compute(&st.db, s, &p, current_year).await?);
    }
    Ok(Json(json!({
        "year": p.year,
        "start": p.start,
        "end": p.end,
        "gdd": p.gdd,
        "day_definition": "local_standard_time_midnight_to_midnight",
        "stations": out,
    })))
}
