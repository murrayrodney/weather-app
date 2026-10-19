//! Forecast-change view (02 5.6a): hourly Previous Runs values grouped into local days per lead (day offset).

use std::collections::BTreeMap;

use chrono::NaiveDate;
use chrono_tz::Tz;
use serde_json::{json, Value};

use super::PrevRunsRaw;

pub const LEADS: usize = 8;

/// `Some` only when every hour of the day has a value.
fn all(values: &[Option<f64>]) -> Option<Vec<f64>> {
    if values.is_empty() {
        return None;
    }
    values.iter().copied().collect()
}

fn round(x: f64, places: i32) -> f64 {
    let m = 10f64.powi(places);
    (x * m).round() / m
}

/// One JSON object per local date in `from..=to`, arrays indexed by lead.
pub fn assemble(raw: &PrevRunsRaw, tz: Tz, from: NaiveDate, to: NaiveDate) -> Vec<Value> {
    // date -> hour indexes
    let mut idx: BTreeMap<NaiveDate, Vec<usize>> = BTreeMap::new();
    for (i, t) in raw.times.iter().enumerate() {
        idx.entry(t.with_timezone(&tz).date_naive())
            .or_default()
            .push(i);
    }
    let pick = |series: &Vec<Option<f64>>, hours: &[usize]| -> Vec<Option<f64>> {
        hours
            .iter()
            .map(|i| series.get(*i).copied().flatten())
            .collect()
    };
    let mut days = Vec::new();
    let mut d = from;
    while d <= to {
        let hours = idx.get(&d).cloned().unwrap_or_default();
        let mut precip = Vec::new();
        let mut prob = Vec::new();
        let mut tmax = Vec::new();
        let mut tmin = Vec::new();
        for lead in 0..LEADS {
            precip.push(all(&pick(&raw.precip[lead], &hours)).map(|v| round(v.iter().sum(), 3)));
            prob.push(
                all(&pick(&raw.prob[lead], &hours))
                    .map(|v| v.iter().copied().fold(f64::MIN, f64::max).round() as i64),
            );
            let temps = all(&pick(&raw.temp[lead], &hours));
            tmax.push(
                temps
                    .as_ref()
                    .map(|v| round(v.iter().copied().fold(f64::MIN, f64::max), 1)),
            );
            tmin.push(
                temps
                    .as_ref()
                    .map(|v| round(v.iter().copied().fold(f64::MAX, f64::min), 1)),
            );
        }
        days.push(json!({
            "date": d,
            "precip_in": precip,
            "prob_precip_max": prob,
            "tmax_f": tmax,
            "tmin_f": tmin,
        }));
        d = d.succ_opt().expect("date in range");
    }
    days
}
