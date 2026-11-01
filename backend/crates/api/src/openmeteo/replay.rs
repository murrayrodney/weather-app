//! Replay client: recorded Open-Meteo answers from `$FIXTURES_DIR/open-meteo/`, shifted so the first hour of each series is
//! local midnight of the station's current local date (02 5.6; the previous-runs window starts at its `from` date). No network.
//!
//! The shift is elapsed time, not whole days: a DST change between the recording and the replay date makes the local
//! midnight 1 hour different in UTC, so whole-day shifts would start the series at 23:00 the day before (proposal 07 phase 3).

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, NaiveDate, Utc};
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

/// The local calendar date of the first instant of a series.
fn first_local_date(times: &[DateTime<Utc>], loc: &ViewableStation) -> Result<NaiveDate, OmError> {
    times
        .first()
        .map(|t| t.with_timezone(&loc.timezone()).date_naive())
        .ok_or(OmError::Unavailable)
}

/// Local midnight of `day` in the station's zone, as a UTC instant.
fn local_midnight(day: NaiveDate, loc: &ViewableStation) -> Result<DateTime<Utc>, OmError> {
    day.and_hms_opt(0, 0, 0)
        .and_then(|n| n.and_local_timezone(loc.timezone()).earliest())
        .map(|t| t.with_timezone(&Utc))
        .ok_or(OmError::Unavailable)
}

/// The elapsed shift that puts the first hour of a recorded series at local midnight of `day`. The recorded series starts
/// at local midnight of its own first day, and the hourly series are regular, so one constant shift moves all of it.
fn shift_to_midnight(
    times: &[DateTime<Utc>],
    day: NaiveDate,
    loc: &ViewableStation,
) -> Result<Duration, OmError> {
    let first = *times.first().ok_or(OmError::Unavailable)?;
    Ok(local_midnight(day, loc)? - first)
}

#[async_trait]
impl OpenMeteoApi for ReplayOpenMeteo {
    async fn forecast(&self, loc: &ViewableStation) -> Result<ForecastRaw, OmError> {
        let mut raw = parse_forecast(&self.nearest("forecast__", loc)?, loc.timezone())?;
        let today = self.today(loc);
        let delta = shift_to_midnight(&raw.times, today, loc)?;
        let days = Duration::days((today - first_local_date(&raw.times, loc)?).num_days());
        raw.times.iter_mut().for_each(|t| *t += delta);
        raw.daily.iter_mut().for_each(|d| {
            d.date += days;
            d.sunrise
                .iter_mut()
                .chain(d.sunset.iter_mut())
                .for_each(|t| *t += delta);
        });
        Ok(raw)
    }

    async fn ensemble(&self, loc: &ViewableStation) -> Result<EnsembleRaw, OmError> {
        let mut raw = parse_ensemble(
            &self.nearest("ensemble_ecmwf_ifs025__", loc)?,
            loc.timezone(),
        )?;
        let delta = shift_to_midnight(&raw.times, self.today(loc), loc)?;
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
        let delta = shift_to_midnight(&raw.times, from, loc)?;
        raw.times.iter_mut().for_each(|t| *t += delta);
        Ok(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Timelike};
    use chrono_tz::America::Denver;
    use famweather_core::clock::FixedClock;

    fn replay_at(now: DateTime<Utc>) -> ReplayOpenMeteo {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
        ReplayOpenMeteo::new(root, Arc::new(FixedClock::new(now)))
    }

    fn station() -> ViewableStation {
        ViewableStation::for_test(Denver, 46.61, -111.99)
    }

    fn assert_hourly(times: &[DateTime<Utc>]) {
        for w in times.windows(2) {
            assert_eq!(w[1] - w[0], Duration::hours(1), "series must stay hourly");
        }
    }

    /// Before the change (Oct 30, MDT): the series starts at 00:00 local, i.e. 06:00 UTC.
    #[tokio::test]
    async fn forecast_starts_at_local_midnight_before_the_dst_change() {
        let loc = station();
        let r = replay_at(Utc.with_ymd_and_hms(2026, 10, 30, 18, 0, 0).unwrap());
        let f = r.forecast(&loc).await.unwrap();
        assert_eq!(
            f.times[0],
            Utc.with_ymd_and_hms(2026, 10, 30, 6, 0, 0).unwrap()
        );
        assert_hourly(&f.times);
    }

    /// After the change (Nov 3, MST): the first hour is still 00:00 local (07:00 UTC), not 23:00 the day before, for
    /// the forecast, the ensemble and the daily rows; the first 7 days are then 7 full local days.
    #[tokio::test]
    async fn replay_across_the_dst_change_starts_at_local_midnight_on_every_series() {
        let loc = station();
        let r = replay_at(Utc.with_ymd_and_hms(2026, 11, 3, 18, 0, 0).unwrap());
        let midnight = Utc.with_ymd_and_hms(2026, 11, 3, 7, 0, 0).unwrap();

        let f = r.forecast(&loc).await.unwrap();
        assert_eq!(f.times[0], midnight);
        assert_eq!(f.times[0].with_timezone(&Denver).hour(), 0);
        assert_eq!(
            f.daily[0].date,
            NaiveDate::from_ymd_opt(2026, 11, 3).unwrap()
        );
        assert_hourly(&f.times);
        // sunrise moves with the same elapsed shift as the hourly series, so it lands on the replay date
        let sunrise = f.daily[0].sunrise.unwrap().with_timezone(&Denver);
        assert_eq!(
            sunrise.date_naive(),
            NaiveDate::from_ymd_opt(2026, 11, 3).unwrap(),
            "sunrise {sunrise}"
        );

        let e = r.ensemble(&loc).await.unwrap();
        assert_eq!(e.times[0], midnight);
        assert_hourly(&e.times);

        // a week from the replay date, the local dates run Nov 3 to Nov 9 with 24 hours each
        let week: Vec<_> = f.times.iter().take(7 * 24).collect();
        let days: std::collections::BTreeSet<_> = week
            .iter()
            .map(|t| t.with_timezone(&Denver).date_naive())
            .collect();
        assert_eq!(days.len(), 7);
        for d in &days {
            let hours = week
                .iter()
                .filter(|t| t.with_timezone(&Denver).date_naive() == *d)
                .count();
            assert_eq!(hours, 24, "every local day in the window is whole: {d}");
        }
    }

    /// A previous-runs window that starts before the change ends after it; its first hour is local midnight of `from`.
    #[tokio::test]
    async fn previous_runs_window_across_the_dst_change_starts_at_local_midnight() {
        let loc = station();
        let r = replay_at(Utc.with_ymd_and_hms(2026, 10, 30, 18, 0, 0).unwrap());
        let from = NaiveDate::from_ymd_opt(2026, 10, 28).unwrap();
        let p = r.previous_runs(&loc, from, from).await.unwrap();
        assert_eq!(
            p.times[0],
            Utc.with_ymd_and_hms(2026, 10, 28, 6, 0, 0).unwrap()
        );
        assert_hourly(&p.times);
    }
}
