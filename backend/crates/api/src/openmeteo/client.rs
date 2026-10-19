//! Live Open-Meteo client and response parsers (shared with the replay client).

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde_json::Value;
use url::Url;

use super::{
    DailyRaw, EnsembleRaw, ForecastRaw, OmError, OpenMeteoApi, PrevRunsRaw, ENSEMBLE_HOST,
    ENSEMBLE_MEMBERS, FORECAST_HOST, PREVIOUS_RUNS_HOST,
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

/// Local ISO strings (`2026-10-07T06:00`) of an hourly series to UTC. The series is regular, so only the first
/// instant is converted and the rest follow hour by hour (this stays correct across DST changes).
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
        });
    }
    Ok(ForecastRaw {
        times,
        temp: get("temperature_2m")?,
        precip: get("precipitation")?,
        prob: get("precipitation_probability")?,
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
        p.push((
            "hourly",
            "temperature_2m,precipitation,precipitation_probability".to_owned(),
        ));
        p.push((
            "daily",
            "temperature_2m_max,temperature_2m_min,precipitation_sum,precipitation_probability_max"
                .to_owned(),
        ));
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
