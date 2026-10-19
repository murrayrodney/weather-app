//! Replay client: recorded Open-Meteo answers from `$FIXTURES_DIR/open-meteo/`, shifted by whole days so the first
//! forecast day is the station's current local date (02 5.6). No network.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Duration, NaiveDate};
use famweather_core::clock::Clock;
use serde::Deserialize;

use super::client::{parse_ensemble, parse_forecast, parse_previous_runs};
use super::{EnsembleRaw, ForecastRaw, OmError, OpenMeteoApi, PrevRunsRaw};
use crate::access::ViewableStation;

pub struct ReplayOpenMeteo {
    dir: PathBuf,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for ReplayOpenMeteo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReplayOpenMeteo { .. }")
    }
}

#[derive(Deserialize)]
struct LatLon {
    latitude: f64,
    longitude: f64,
}

impl ReplayOpenMeteo {
    pub fn new(fixtures_root: PathBuf, clock: Arc<dyn Clock>) -> Self {
        Self {
            dir: fixtures_root.join("open-meteo"),
            clock,
        }
    }

    /// The recorded file with `prefix` whose echoed coordinates are nearest to the station.
    fn nearest(&self, prefix: &str, loc: &ViewableStation) -> Result<Vec<u8>, OmError> {
        let mut best: Option<(f64, Vec<u8>)> = None;
        let mut names: Vec<_> = std::fs::read_dir(&self.dir)
            .map_err(|_| OmError::Unavailable)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(prefix) && n.ends_with(".json"))
            })
            .collect();
        names.sort();
        for p in names {
            let bytes = std::fs::read(&p).map_err(|_| OmError::Unavailable)?;
            let Ok(ll) = serde_json::from_slice::<LatLon>(&bytes) else {
                continue;
            };
            let d = (ll.latitude - loc.lat()).powi(2) + (ll.longitude - loc.lon()).powi(2);
            if best.as_ref().is_none_or(|(bd, _)| d < *bd) {
                best = Some((d, bytes));
            }
        }
        best.map(|(_, b)| b).ok_or(OmError::Unavailable)
    }

    fn today(&self, loc: &ViewableStation) -> NaiveDate {
        self.clock.now().with_timezone(&loc.timezone()).date_naive()
    }
}

fn first_day(
    times: &[chrono::DateTime<chrono::Utc>],
    loc: &ViewableStation,
) -> Result<NaiveDate, OmError> {
    times
        .first()
        .map(|t| t.with_timezone(&loc.timezone()).date_naive())
        .ok_or(OmError::Unavailable)
}

#[async_trait]
impl OpenMeteoApi for ReplayOpenMeteo {
    async fn forecast(&self, loc: &ViewableStation) -> Result<ForecastRaw, OmError> {
        let mut raw = parse_forecast(&self.nearest("forecast__", loc)?, loc.timezone())?;
        let delta = Duration::days((self.today(loc) - first_day(&raw.times, loc)?).num_days());
        raw.times.iter_mut().for_each(|t| *t += delta);
        raw.daily.iter_mut().for_each(|d| d.date += delta);
        Ok(raw)
    }

    async fn ensemble(&self, loc: &ViewableStation) -> Result<EnsembleRaw, OmError> {
        let mut raw = parse_ensemble(
            &self.nearest("ensemble_ecmwf_ifs025__", loc)?,
            loc.timezone(),
        )?;
        let delta = Duration::days((self.today(loc) - first_day(&raw.times, loc)?).num_days());
        raw.times.iter_mut().for_each(|t| *t += delta);
        Ok(raw)
    }

    async fn previous_runs(
        &self,
        loc: &ViewableStation,
        from: NaiveDate,
        _to: NaiveDate,
    ) -> Result<PrevRunsRaw, OmError> {
        let mut raw = parse_previous_runs(&self.nearest("previous_runs__", loc)?, loc.timezone())?;
        let delta = Duration::days((from - first_day(&raw.times, loc)?).num_days());
        raw.times.iter_mut().for_each(|t| *t += delta);
        Ok(raw)
    }
}
