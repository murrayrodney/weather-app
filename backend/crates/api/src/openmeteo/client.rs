//! Live Open-Meteo client and response parsers (shared with the replay client).

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde_json::Value;
use url::Url;

use super::{
    DailyRaw, EnsembleRaw, ForecastRaw, OmError, OpenMeteoApi, PrevRunsRaw, ENSEMBLE_HOST,
    ENSEMBLE_MEMBERS, FORECAST_DAILY, FORECAST_HOST, FORECAST_HOURLY, FORECAST_WIND_SPEED_UNIT,
    PREVIOUS_RUNS_HOST,
};
use crate::access::ViewableStation;
use crate::openmeteo::changes::LEADS;

pub const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
const FORECAST_TIMEOUT: Duration = Duration::from_secs(10);
const ENSEMBLE_TIMEOUT: Duration = Duration::from_secs(15);
const PREVIOUS_RUNS_TIMEOUT: Duration = Duration::from_secs(20);

// ---------------------------------------------------------------------------------------------------------------
// Parsing

fn series(v: &Value) -> Option<Vec<Option<f64>>> {
    Some(
        v.as_array()?
            .iter()
            .map(|x| x.as_f64().filter(|f| f.is_finite()))
            .collect(),
    )
}

/// Local ISO strings (`2026-10-07T06:00`) of an hourly series to UTC. Open-Meteo labels every hour with the response's
/// fixed `utc_offset_seconds` (the first day's offset, not the offset of each day), and the data is in regular elapsed
/// hours. So the first label fixes the base instant and each later hour is one elapsed hour after it. That stays correct
/// across a DST change; the labels after the change are not wall-clock time and must not be converted day by day.
fn hourly_times(h: &Value, tz: Tz) -> Option<Vec<DateTime<Utc>>> {
    let times = h.get("time")?.as_array()?;
    let first = NaiveDateTime::parse_from_str(times.first()?.as_str()?, "%Y-%m-%dT%H:%M").ok()?;
    let base = tz
        .from_local_datetime(&first)
        .earliest()?
        .with_timezone(&Utc);
    Some(
        (0..times.len())
            .map(|i| base + chrono::Duration::hours(i as i64))
            .collect(),
    )
}

fn field(h: &Value, name: &str, len: usize) -> Option<Vec<Option<f64>>> {
    let s = series(h.get(name)?)?;
    (s.len() == len).then_some(s)
}

fn json(bytes: &[u8]) -> Result<Value, OmError> {
    serde_json::from_slice(bytes).map_err(|_| OmError::Unavailable)
}

/// Multiplies hPa by this to get inches of mercury (1 inHg = 33.8638866667 hPa).
const INHG_PER_HPA: f64 = 0.029_529_983_071_4;

/// Sea-level pressure in hPa to inHg (02 5.6, proposal 07 section 3). Open-Meteo has no inHg option.
pub fn hpa_to_inhg(hpa: f64) -> f64 {
    hpa * INHG_PER_HPA
}

/// A wall-clock label (`2026-10-07T07:12`, as Open-Meteo returns sunrise and sunset) to a UTC instant. Every label in a
/// response carries the response's fixed `utc_offset_seconds`, including dates after a DST change, so the offset is
/// subtracted directly rather than looked up per day in the zone.
fn local_instant(v: &Value, offset_secs: i64) -> Option<DateTime<Utc>> {
    let naive = NaiveDateTime::parse_from_str(v.as_str()?, "%Y-%m-%dT%H:%M").ok()?;
    Some(naive.and_utc() - chrono::Duration::seconds(offset_secs))
}

pub fn parse_forecast(bytes: &[u8], tz: Tz) -> Result<ForecastRaw, OmError> {
    let v = json(bytes)?;
    let h = v.get("hourly").ok_or(OmError::Unavailable)?;
    let times = hourly_times(h, tz).ok_or(OmError::Unavailable)?;
    let n = times.len();
    let get = |name| field(h, name, n).ok_or(OmError::Unavailable);
    let d = v.get("daily").ok_or(OmError::Unavailable)?;
    let dates = d
        .get("time")
        .and_then(Value::as_array)
        .ok_or(OmError::Unavailable)?;
    let dn = dates.len();
    let dget = |name| field(d, name, dn).ok_or(OmError::Unavailable);
    let (tmax, tmin, psum, pmax) = (
        dget("temperature_2m_max")?,
        dget("temperature_2m_min")?,
        dget("precipitation_sum")?,
        dget("precipitation_probability_max")?,
    );
    let (wcode, snow) = (dget("weather_code")?, dget("snowfall_sum")?);
    // Sunrise and sunset are strings, one per day, so they are read directly rather than as numbers.
    let offset = v
        .get("utc_offset_seconds")
        .and_then(Value::as_i64)
        .ok_or(OmError::Unavailable)?;
    let instants = |name: &str| -> Result<Vec<Option<DateTime<Utc>>>, OmError> {
        let a = d
            .get(name)
            .and_then(Value::as_array)
            .filter(|a| a.len() == dn)
            .ok_or(OmError::Unavailable)?;
        Ok(a.iter().map(|x| local_instant(x, offset)).collect())
    };
    let (sunrise, sunset) = (instants("sunrise")?, instants("sunset")?);
    let mut daily = Vec::with_capacity(dn);
    for (i, s) in dates.iter().enumerate() {
        let date = s
            .as_str()
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
            .ok_or(OmError::Unavailable)?;
        daily.push(DailyRaw {
            date,
            tmax: tmax[i],
            tmin: tmin[i],
            precip: psum[i],
            prob_max: pmax[i],
            weather_code: wcode[i],
            snow: snow[i],
            sunrise: sunrise[i],
            sunset: sunset[i],
        });
    }
    Ok(ForecastRaw {
        times,
        temp: get("temperature_2m")?,
        precip: get("precipitation")?,
        prob: get("precipitation_probability")?,
        apparent_f: get("apparent_temperature")?,
        dew_point_f: get("dew_point_2m")?,
        cloud_pct: get("cloud_cover")?,
        humidity_pct: get("relative_humidity_2m")?,
        pressure_inhg: get("pressure_msl")?
            .into_iter()
            .map(|x| x.map(hpa_to_inhg))
            .collect(),
        wind_mph: get("wind_speed_10m")?,
        wind_dir_deg: get("wind_direction_10m")?,
        gust_mph: get("wind_gusts_10m")?,
        weather_code: get("weather_code")?,
        snow_in: get("snowfall")?,
        daily,
    })
}

fn members(h: &Value, var: &str, n: usize) -> Vec<Vec<Option<f64>>> {
    let mut out = Vec::new();
    if let Some(c) = field(h, var, n) {
        out.push(c);
    }
    for m in 1..=99 {
        match field(h, &format!("{var}_member{m:02}"), n) {
            Some(s) => out.push(s),
            None => break,
        }
    }
    out
}

pub fn parse_ensemble(bytes: &[u8], tz: Tz) -> Result<EnsembleRaw, OmError> {
    let v = json(bytes)?;
    let h = v.get("hourly").ok_or(OmError::Unavailable)?;
    let times = hourly_times(h, tz).ok_or(OmError::Unavailable)?;
    let n = times.len();
    let temp = members(h, "temperature_2m", n);
    let precip = members(h, "precipitation", n);
    if temp.len() != ENSEMBLE_MEMBERS || precip.len() != ENSEMBLE_MEMBERS {
        tracing::error!(
            temperature = temp.len(),
            precipitation = precip.len(),
            expected = ENSEMBLE_MEMBERS,
            "ensemble member count mismatch"
        );
        return Err(OmError::Unavailable);
    }
    Ok(EnsembleRaw {
        times,
        temp,
        precip,
    })
}

fn leads(h: &Value, var: &str, n: usize) -> Result<Vec<Vec<Option<f64>>>, OmError> {
    let mut out = Vec::with_capacity(LEADS);
    for lead in 0..LEADS {
        let name = if lead == 0 {
            var.to_owned()
        } else {
            format!("{var}_previous_day{lead}")
        };
        out.push(field(h, &name, n).ok_or(OmError::Unavailable)?);
    }
    Ok(out)
}

pub fn parse_previous_runs(bytes: &[u8], tz: Tz) -> Result<PrevRunsRaw, OmError> {
    let v = json(bytes)?;
    let h = v.get("hourly").ok_or(OmError::Unavailable)?;
    let times = hourly_times(h, tz).ok_or(OmError::Unavailable)?;
    let n = times.len();
    Ok(PrevRunsRaw {
        temp: leads(h, "temperature_2m", n)?,
        precip: leads(h, "precipitation", n)?,
        prob: leads(h, "precipitation_probability", n)?,
        times,
    })
}

// ---------------------------------------------------------------------------------------------------------------
// Live client

pub struct LiveOpenMeteo {
    client: reqwest::Client,
    forecast: Url,
    ensemble: Url,
    previous: Url,
}

impl std::fmt::Debug for LiveOpenMeteo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LiveOpenMeteo { .. }")
    }
}

impl LiveOpenMeteo {
    pub fn new() -> Result<Self, OmError> {
        Self::build(
            Url::parse(FORECAST_HOST).map_err(|_| OmError::Unavailable)?,
            Url::parse(ENSEMBLE_HOST).map_err(|_| OmError::Unavailable)?,
            Url::parse(PREVIOUS_RUNS_HOST).map_err(|_| OmError::Unavailable)?,
            true,
        )
    }

    /// Test-only: point the three endpoints at a stub server (plain HTTP allowed).
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_bases(forecast: Url, ensemble: Url, previous: Url) -> Result<Self, OmError> {
        Self::build(forecast, ensemble, previous, false)
    }

    fn build(
        forecast: Url,
        ensemble: Url,
        previous: Url,
        https_only: bool,
    ) -> Result<Self, OmError> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .gzip(true)
            .deflate(true)
            .redirect(reqwest::redirect::Policy::none())
            .https_only(https_only)
            .user_agent(concat!("family-weather/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| OmError::Unavailable)?;
        Ok(Self {
            client,
            forecast,
            ensemble,
            previous,
        })
    }

    async fn get(
        &self,
        mut url: Url,
        params: &[(&str, String)],
        timeout: Duration,
    ) -> Result<Vec<u8>, OmError> {
        {
            let mut q = url.query_pairs_mut();
            for (k, v) in params {
                q.append_pair(k, v);
            }
        }
        if url.as_str().to_ascii_lowercase().contains("apikey") {
            return Err(OmError::Unavailable);
        }
        let mut resp = self
            .client
            .get(url)
            .timeout(timeout)
            .send()
            .await
            .map_err(|e| {
                tracing::warn!(error = %famweather_core::redact::http_error(e), "open-meteo call failed");
                OmError::Unavailable
            })?;
        let status = resp.status();
        if status.as_u16() == 429 {
            return Err(OmError::Busy { retry_after: 60 });
        }
        if !status.is_success() {
            tracing::warn!(status = status.as_u16(), "open-meteo call failed");
            return Err(OmError::Unavailable);
        }
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| {
            tracing::warn!(error = %famweather_core::redact::http_error(e), "open-meteo call failed");
            OmError::Unavailable
        })? {
            if body.len() + chunk.len() > MAX_BODY_BYTES {
                tracing::warn!("open-meteo response too large");
                return Err(OmError::Unavailable);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

fn common(loc: &ViewableStation) -> Vec<(&'static str, String)> {
    vec![
        ("latitude", loc.lat().to_string()),
        ("longitude", loc.lon().to_string()),
        ("temperature_unit", "fahrenheit".to_owned()),
        ("precipitation_unit", "inch".to_owned()),
        ("timezone", loc.timezone().name().to_owned()),
    ]
}

#[async_trait]
impl OpenMeteoApi for LiveOpenMeteo {
    async fn forecast(&self, loc: &ViewableStation) -> Result<ForecastRaw, OmError> {
        let mut p = common(loc);
        p.push(("models", "gfs_seamless".to_owned()));
        p.push(("hourly", FORECAST_HOURLY.to_owned()));
        p.push(("daily", FORECAST_DAILY.to_owned()));
        p.push(("wind_speed_unit", FORECAST_WIND_SPEED_UNIT.to_owned()));
        p.push(("forecast_days", "16".to_owned()));
        let body = self
            .get(self.forecast.clone(), &p, FORECAST_TIMEOUT)
            .await?;
        parse_forecast(&body, loc.timezone())
    }

    async fn ensemble(&self, loc: &ViewableStation) -> Result<EnsembleRaw, OmError> {
        let mut p = common(loc);
        p.push(("models", "ecmwf_ifs025".to_owned()));
        p.push(("hourly", "temperature_2m,precipitation".to_owned()));
        p.push(("forecast_days", "15".to_owned()));
        let body = self
            .get(self.ensemble.clone(), &p, ENSEMBLE_TIMEOUT)
            .await?;
        parse_ensemble(&body, loc.timezone())
    }

    async fn previous_runs(
        &self,
        loc: &ViewableStation,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<PrevRunsRaw, OmError> {
        let mut p = common(loc);
        p.push(("models", "gfs_seamless".to_owned()));
        let mut vars = Vec::new();
        for var in [
            "temperature_2m",
            "precipitation",
            "precipitation_probability",
        ] {
            vars.push(var.to_owned());
            for lead in 1..LEADS {
                vars.push(format!("{var}_previous_day{lead}"));
            }
        }
        p.push(("hourly", vars.join(",")));
        p.push(("start_date", from.to_string()));
        p.push(("end_date", to.to_string()));
        let body = self
            .get(self.previous.clone(), &p, PREVIOUS_RUNS_TIMEOUT)
            .await?;
        parse_previous_runs(&body, loc.timezone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    /// A forecast body carrying every variable of the request (built from the request constants), two hourly steps
    /// from local midnight on 7 October and two days (7 and 2 November, one in MDT and one in MST).
    fn body(hpa: f64) -> Vec<u8> {
        let mut h = serde_json::Map::new();
        h.insert(
            "time".into(),
            json!(["2026-10-07T00:00", "2026-10-07T01:00"]),
        );
        for name in FORECAST_HOURLY.split(',') {
            let v = if name == "pressure_msl" {
                json!([hpa, hpa])
            } else {
                json!([1.0, 2.0])
            };
            h.insert(name.into(), v);
        }
        let mut d = serde_json::Map::new();
        d.insert("time".into(), json!(["2026-10-07", "2026-11-02"]));
        for name in FORECAST_DAILY.split(',') {
            let v = match name {
                "sunrise" => json!(["2026-10-07T07:12", "2026-11-02T06:20"]),
                "sunset" => json!(["2026-10-07T18:30", "2026-11-02T16:40"]),
                _ => json!([1.0, 2.0]),
            };
            d.insert(name.into(), v);
        }
        json!({ "utc_offset_seconds": -21600, "hourly": h, "daily": d })
            .to_string()
            .into_bytes()
    }

    fn denver() -> Tz {
        "America/Denver".parse().unwrap()
    }

    #[test]
    fn sunrise_and_sunset_become_utc_instants_with_each_days_own_offset() {
        let f = parse_forecast(&body(1013.25), denver()).unwrap();
        // 07:12 MDT (UTC-6) is 13:12 UTC; 18:30 MDT is 00:30 UTC on the next day.
        assert_eq!(
            f.daily[0].sunrise,
            Some(Utc.with_ymd_and_hms(2026, 10, 7, 13, 12, 0).unwrap())
        );
        assert_eq!(
            f.daily[0].sunset,
            Some(Utc.with_ymd_and_hms(2026, 10, 8, 0, 30, 0).unwrap())
        );
        // The response's fixed offset (-6 h, from the first day) labels 2 November too, so 06:20 is 12:20 UTC and
        // 16:40 is 22:40 UTC. The labels are not converted with 2 November's own MST offset.
        assert_eq!(
            f.daily[1].sunrise,
            Some(Utc.with_ymd_and_hms(2026, 11, 2, 12, 20, 0).unwrap())
        );
        assert_eq!(
            f.daily[1].sunset,
            Some(Utc.with_ymd_and_hms(2026, 11, 2, 22, 40, 0).unwrap())
        );
    }

    /// Regression: a label after the DST change (2 Nov, 08:11) is read with the response's fixed -6 h offset, so it is
    /// 14:11 UTC. Converting it with that day's MST offset would give 15:11 UTC, an hour late.
    #[test]
    fn a_sunrise_after_the_dst_change_uses_the_fixed_offset_not_that_days_offset() {
        let mut v: Value = serde_json::from_slice(&body(1013.25)).unwrap();
        v["daily"]["sunrise"][1] = json!("2026-11-02T08:11");
        let f = parse_forecast(v.to_string().as_bytes(), denver()).unwrap();
        assert_eq!(
            f.daily[1].sunrise,
            Some(Utc.with_ymd_and_hms(2026, 11, 2, 14, 11, 0).unwrap())
        );
    }

    /// A response without its offset is refused: the labels cannot be read without it.
    #[test]
    fn a_response_without_utc_offset_seconds_is_unavailable() {
        let mut v: Value = serde_json::from_slice(&body(1013.25)).unwrap();
        v.as_object_mut().unwrap().remove("utc_offset_seconds");
        assert!(parse_forecast(v.to_string().as_bytes(), denver()).is_err());
    }

    #[test]
    fn pressure_is_converted_from_hpa_to_inhg() {
        let f = parse_forecast(&body(1013.25), denver()).unwrap();
        // 1013.25 hPa is 29.92 inHg.
        assert!((f.pressure_inhg[0].unwrap() - 29.92).abs() < 0.005);
        assert!((hpa_to_inhg(1013.25) - 29.9213).abs() < 0.0001);
        assert_eq!(f.pressure_inhg.len(), 2);
    }

    #[test]
    fn new_hourly_and_daily_columns_are_parsed_in_order() {
        let f = parse_forecast(&body(1013.25), denver()).unwrap();
        let steps = vec![Some(1.0), Some(2.0)];
        assert_eq!(f.apparent_f, steps);
        assert_eq!(f.dew_point_f, steps);
        assert_eq!(f.cloud_pct, steps);
        assert_eq!(f.humidity_pct, steps);
        assert_eq!(f.wind_mph, steps);
        assert_eq!(f.wind_dir_deg, steps);
        assert_eq!(f.gust_mph, steps);
        assert_eq!(f.weather_code, steps);
        assert_eq!(f.snow_in, steps);
        assert_eq!(f.daily[0].weather_code, Some(1.0));
        assert_eq!(f.daily[1].snow, Some(2.0));
    }
}
