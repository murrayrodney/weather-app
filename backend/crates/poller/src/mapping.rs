//! AWN observation -> `app.observations` row (03 section 7.1 "Field mapping", exactly 04 section 5.6).
//!
//! Coercion happens here, before binding. A value that is out of range or not a JSON number becomes NULL and is
//! counted in `bad_values`; a bad value never fails the batch. Absent and `null` fields are NULL and not counted.

use std::collections::BTreeSet;

use chrono::{DateTime, Duration, TimeZone, Utc};
use famweather_core::awn::AwnObservation;
use serde_json::Value;

pub const MAX_FUTURE_MINUTES: i64 = 10;

/// Rows before this are rejected.
pub fn earliest_allowed() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2000, 1, 1, 0, 0, 0)
        .single()
        .expect("valid date")
}

/// `ts_utc` is `dateutc` verbatim; reject `ts_utc > now + 10 min` or before 2000-01-01.
pub fn ts_acceptable(ts: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    ts >= earliest_allowed() && ts <= now + Duration::minutes(MAX_FUTURE_MINUTES)
}

/// AWN fields that are deliberately not stored.
const IGNORED: &[&str] = &["date", "24hourrainin", "tz", "macAddress", "battin"];
const MAPPED: &[&str] = &[
    "dateutc",
    "tempf",
    "feelsLike",
    "dewPoint",
    "humidity",
    "windspeedmph",
    "windgustmph",
    "maxdailygust",
    "winddir",
    "baromrelin",
    "baromabsin",
    "hourlyrainin",
    "eventrainin",
    "dailyrainin",
    "weeklyrainin",
    "monthlyrainin",
    "yearlyrainin",
    "totalrainin",
    "lastRain",
    "solarradiation",
    "uv",
    "tempinf",
    "humidityin",
    "battout",
];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MappedRow {
    pub ts: DateTime<Utc>,
    pub temp_f: Option<f32>,
    pub feels_like_f: Option<f32>,
    pub dew_point_f: Option<f32>,
    pub humidity_pct: Option<i16>,
    pub wind_speed_mph: Option<f32>,
    pub wind_gust_mph: Option<f32>,
    pub max_daily_gust_mph: Option<f32>,
    pub wind_dir_deg: Option<i16>,
    pub baro_rel_inhg: Option<f32>,
    pub baro_abs_inhg: Option<f32>,
    pub rain_rate_in_hr: Option<f32>,
    pub rain_event_in: Option<f32>,
    pub rain_daily_in: Option<f32>,
    pub rain_weekly_in: Option<f32>,
    pub rain_monthly_in: Option<f32>,
    pub rain_yearly_in: Option<f32>,
    pub rain_total_in: Option<f32>,
    pub rain_last_at: Option<DateTime<Utc>>,
    pub solar_wm2: Option<f32>,
    pub uv_index: Option<i16>,
    pub temp_indoor_f: Option<f32>,
    pub humidity_indoor_pct: Option<i16>,
    pub battery_out: Option<i16>,
}

/// Counters collected while mapping a batch.
#[derive(Debug, Default)]
pub struct MapStats {
    pub bad_values: u32,
    /// Field names AWN sent that the guide neither maps nor ignores (names only, never values).
    pub unknown_fields: BTreeSet<String>,
}

struct Ctx<'a> {
    obs: &'a AwnObservation,
    stats: &'a mut MapStats,
}

impl Ctx<'_> {
    /// `None` for absent/null; a non-number counts as bad.
    fn number(&mut self, key: &str) -> Option<f64> {
        match self.obs.rest.get(key)? {
            Value::Null => None,
            Value::Number(n) => match n.as_f64().filter(|x| x.is_finite()) {
                Some(x) => Some(x),
                None => {
                    self.stats.bad_values += 1;
                    None
                }
            },
            _ => {
                self.stats.bad_values += 1;
                None
            }
        }
    }

    fn bad(&mut self) -> Option<f32> {
        self.stats.bad_values += 1;
        None
    }

    fn real(&mut self, key: &str, min: Option<f64>) -> Option<f32> {
        let x = self.number(key)?;
        if min.is_some_and(|m| x < m) || !(x as f32).is_finite() {
            return self.bad();
        }
        Some(x as f32)
    }

    fn small(&mut self, key: &str, lo: i32, hi: i32) -> Option<i16> {
        let x = self.number(key)?.round();
        if x < f64::from(lo) || x > f64::from(hi) {
            self.stats.bad_values += 1;
            return None;
        }
        Some(x as i16)
    }

    fn time(&mut self, key: &str) -> Option<DateTime<Utc>> {
        match self.obs.rest.get(key)? {
            Value::Null => None,
            Value::String(s) => match DateTime::parse_from_rfc3339(s.trim()) {
                Ok(t) => Some(t.with_timezone(&Utc)),
                Err(_) => {
                    self.stats.bad_values += 1;
                    None
                }
            },
            _ => {
                self.stats.bad_values += 1;
                None
            }
        }
    }
}

/// Maps one history row. `None` when `dateutc` is not a representable timestamp (the caller counts it rejected).
pub fn map_observation(obs: &AwnObservation, stats: &mut MapStats) -> Option<MappedRow> {
    let ts = Utc.timestamp_millis_opt(obs.dateutc).single()?;
    for k in obs.rest.keys() {
        if !MAPPED.contains(&k.as_str())
            && !IGNORED.contains(&k.as_str())
            && stats.unknown_fields.len() < crate::job::MAX_LIST_ITEMS
        {
            stats.unknown_fields.insert(k.clone());
        }
    }
    let mut c = Ctx { obs, stats };
    Some(MappedRow {
        ts,
        temp_f: c.real("tempf", None),
        feels_like_f: c.real("feelsLike", None),
        dew_point_f: c.real("dewPoint", None),
        humidity_pct: c.small("humidity", 0, 100),
        wind_speed_mph: c.real("windspeedmph", Some(0.0)),
        wind_gust_mph: c.real("windgustmph", Some(0.0)),
        max_daily_gust_mph: c.real("maxdailygust", Some(0.0)),
        wind_dir_deg: c.small("winddir", 0, 360),
        baro_rel_inhg: c.real("baromrelin", None),
        baro_abs_inhg: c.real("baromabsin", None),
        rain_rate_in_hr: c.real("hourlyrainin", Some(0.0)),
        rain_event_in: c.real("eventrainin", Some(0.0)),
        rain_daily_in: c.real("dailyrainin", Some(0.0)),
        rain_weekly_in: c.real("weeklyrainin", Some(0.0)),
        rain_monthly_in: c.real("monthlyrainin", Some(0.0)),
        rain_yearly_in: c.real("yearlyrainin", Some(0.0)),
        rain_total_in: c.real("totalrainin", Some(0.0)),
        rain_last_at: c.time("lastRain"),
        solar_wm2: c.real("solarradiation", Some(0.0)),
        uv_index: c.small("uv", 0, 32767),
        temp_indoor_f: c.real("tempinf", None),
        humidity_indoor_pct: c.small("humidityin", 0, 100),
        battery_out: c.small("battout", i32::from(i16::MIN), i32::from(i16::MAX)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obs(v: Value) -> AwnObservation {
        serde_json::from_value(v).unwrap()
    }

    fn map(v: Value) -> (MappedRow, MapStats) {
        let mut s = MapStats::default();
        let r = map_observation(&obs(v), &mut s).unwrap();
        (r, s)
    }

    #[test]
    fn maps_every_field() {
        let (r, s) = map(json!({
            "dateutc": 1_780_000_000_000i64, "tempf": 71.5, "feelsLike": 70.0, "dewPoint": 40.2, "humidity": 33,
            "windspeedmph": 4.5, "windgustmph": 9.0, "maxdailygust": 12.5, "winddir": 270,
            "baromrelin": 29.9, "baromabsin": 25.1, "hourlyrainin": 0.0, "eventrainin": 0.1, "dailyrainin": 0.2,
            "weeklyrainin": 0.3, "monthlyrainin": 0.4, "yearlyrainin": 5.5, "totalrainin": 12.25,
            "lastRain": "2026-06-01T12:00:00.000Z", "solarradiation": 640.0, "uv": 5, "tempinf": 68.0,
            "humidityin": 40, "battout": 1, "date": "x", "tz": "America/Denver", "macAddress": "AA:BB:CC:00:00:01",
            "battin": 1, "24hourrainin": 0.0
        }));
        assert_eq!(r.temp_f, Some(71.5));
        assert_eq!(r.humidity_pct, Some(33));
        assert_eq!(r.wind_dir_deg, Some(270));
        assert_eq!(r.rain_total_in, Some(12.25));
        assert_eq!(
            r.rain_last_at.unwrap().to_rfc3339(),
            "2026-06-01T12:00:00+00:00"
        );
        assert_eq!(r.uv_index, Some(5));
        assert_eq!(r.battery_out, Some(1));
        assert_eq!(s.bad_values, 0);
        assert!(s.unknown_fields.is_empty(), "{:?}", s.unknown_fields);
    }

    #[test]
    fn coercion_rules() {
        let (r, s) = map(json!({
            "dateutc": 1_780_000_000_000i64,
            "humidity": 100.4, "humidityin": 101, "winddir": 361, "uv": -2, "tempf": "71.5",
            "dailyrainin": -0.1, "windspeedmph": -1, "lastRain": "not a date", "solarradiation": true
        }));
        assert_eq!(r.humidity_pct, Some(100));
        assert_eq!(r.humidity_indoor_pct, None);
        assert_eq!(r.wind_dir_deg, None);
        assert_eq!(r.uv_index, None);
        assert_eq!(r.temp_f, None);
        assert_eq!(r.rain_daily_in, None);
        assert_eq!(r.wind_speed_mph, None);
        assert_eq!(r.rain_last_at, None);
        assert_eq!(r.solar_wm2, None);
        assert_eq!(s.bad_values, 8);
    }

    #[test]
    fn absent_and_null_are_not_bad() {
        let (r, s) = map(json!({"dateutc": 1_780_000_000_000i64, "tempf": null}));
        assert_eq!(r.temp_f, None);
        assert_eq!(s.bad_values, 0);
    }

    #[test]
    fn unknown_fields_are_named_not_valued_and_capped() {
        let mut v = json!({"dateutc": 1_780_000_000_000i64, "temp1f": 50.0, "secretish": "value"});
        for i in 0..40 {
            v[format!("extra{i}")] = json!(1);
        }
        let (_, s) = map(v);
        assert_eq!(s.unknown_fields.len(), crate::job::MAX_LIST_ITEMS);
    }

    #[test]
    fn timestamp_window() {
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();
        assert!(ts_acceptable(now, now));
        assert!(ts_acceptable(now + Duration::minutes(10), now));
        assert!(!ts_acceptable(now + Duration::minutes(11), now));
        assert!(!ts_acceptable(
            Utc.with_ymd_and_hms(1999, 12, 31, 23, 59, 59).unwrap(),
            now
        ));
    }
}
