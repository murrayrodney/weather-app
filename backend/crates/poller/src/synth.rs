//! Deterministic synthetic AWN and ACIS data (feature `test-support`): the committed replay fixtures are made with
//! this (`examples/record_fixtures.rs synth ...`), and the integration tests build their scenarios with it.
//! Nothing here resembles a real station: names, coordinates and weather are invented.

use std::io::Write;
use std::path::Path;

use chrono::{DateTime, Datelike, Duration, NaiveDate, Timelike, Utc};
use serde_json::{json, Map, Value};

/// xorshift64*: small, deterministic, good enough for weather-shaped noise.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// In `[0, 1)`.
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }
}

/// Scenario markers inside the generated rain counter, in days after the start of the series.
#[derive(Debug, Clone, Default)]
pub struct Anomalies {
    /// The console total drops to zero and counts on (a counter reset).
    pub reset_at_day: Option<f64>,
    /// Three readings with a low total, then back (a glitch).
    pub glitch_at_day: Option<f64>,
    /// A single jump of 2 inches that stays (an impossible jump that is capped).
    pub spike_at_day: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct StationSpec {
    /// Time of the newest reading (a multiple of the interval).
    pub newest: DateTime<Utc>,
    pub days: u32,
    pub interval_min: i64,
    pub seed: u64,
    /// Constant standard offset used for the daily/weekly/monthly/yearly counter resets.
    pub std_offset_min: i32,
    pub base_temp_f: f64,
    pub rainy: bool,
    pub anomalies: Anomalies,
}

impl StationSpec {
    pub fn simple(newest: DateTime<Utc>, days: u32, interval_min: i64, seed: u64) -> Self {
        StationSpec {
            newest,
            days,
            interval_min,
            seed,
            std_offset_min: -420,
            base_temp_f: 52.0,
            rainy: true,
            anomalies: Anomalies::default(),
        }
    }
}

fn r2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}
fn r1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

fn local_date(ts: DateTime<Utc>, off: i32) -> NaiveDate {
    (ts + Duration::minutes(i64::from(off))).date_naive()
}

/// Generates the rows oldest first.
fn generate_oldest_first(spec: &StationSpec) -> Vec<Value> {
    let step = Duration::minutes(spec.interval_min);
    let n = (i64::from(spec.days) * 24 * 60 / spec.interval_min) as usize;
    let start = spec.newest - step * (n as i32 - 1);
    let mut rng = Rng::new(spec.seed);

    // Rain events: one roughly every 2-4 days, 40-200 minutes at 0.03-0.35 in/hr.
    let mut events: Vec<(usize, usize, f64)> = Vec::new();
    if spec.rainy {
        let per_day = (24 * 60 / spec.interval_min) as f64;
        let mut at = rng.range(0.5, 2.0) * per_day;
        while (at as usize) < n {
            let len = (rng.range(40.0, 200.0) / spec.interval_min as f64)
                .ceil()
                .max(1.0) as usize;
            events.push((at as usize, len, rng.range(0.03, 0.35)));
            at += rng.range(2.0, 4.0) * per_day;
        }
    }
    let mut rate = vec![0.0f64; n];
    for (s, len, r) in &events {
        for slot in rate.iter_mut().skip(*s).take(*len) {
            *slot = *r;
        }
    }

    let day_idx = |d: f64| (d * (24 * 60 / spec.interval_min) as f64) as usize;
    let reset_at = spec.anomalies.reset_at_day.map(day_idx);
    let glitch_at = spec.anomalies.glitch_at_day.map(day_idx);
    let spike_at = spec.anomalies.spike_at_day.map(day_idx);

    let mut cum = 0.0f64; // fractional inches since the start
    let mut reset_sub = 0.0f64;
    let mut spike_add = 0.0f64;
    let (mut origin_day, mut origin_week, mut origin_month, mut origin_year) = (0.0, 0.0, 0.0, 0.0);
    let mut prev_date = local_date(start, spec.std_offset_min);
    let mut last_rain: Option<DateTime<Utc>> = None;
    let mut event_cum = 0.0f64;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let ts = start + step * i as i32;
        let local = ts + Duration::minutes(i64::from(spec.std_offset_min));
        let date = local.date_naive();
        if date != prev_date {
            origin_day = cum;
            if date.weekday() == chrono::Weekday::Sun {
                origin_week = cum;
            }
            if date.day() == 1 {
                origin_month = cum;
                if date.month() == 1 {
                    origin_year = cum;
                }
            }
            prev_date = date;
        }
        let inc = rate[i] * spec.interval_min as f64 / 60.0;
        if inc > 0.0 {
            last_rain = Some(ts);
            event_cum += inc;
        } else if i > 0 && rate[i - 1] > 0.0 {
            event_cum = 0.0;
        }
        cum += inc;
        if reset_at == Some(i) {
            reset_sub = 41.0 + cum;
        }
        if spike_at == Some(i) {
            spike_add = 2.0;
        }

        let hour = f64::from(local.hour()) + f64::from(local.minute()) / 60.0;
        let doy = f64::from(date.ordinal());
        let diurnal = ((hour - 9.0) / 24.0 * std::f64::consts::TAU).sin();
        let synoptic = ((i as f64) / (24.0 * 60.0 / spec.interval_min as f64) / 5.0).sin() * 6.0;
        let temp = spec.base_temp_f
            + 18.0 * ((doy - 110.0) / 365.0 * std::f64::consts::TAU).sin()
            + 14.0 * diurnal
            + synoptic
            + rng.range(-0.8, 0.8);
        let hum = (60.0 - 25.0 * diurnal + rng.range(-3.0, 3.0))
            .clamp(8.0, 99.0)
            .round();
        let dew = temp - (100.0 - hum) / 5.0;
        let wind = (4.0 + 3.0 * rng.unit() + 2.0 * (1.0 + diurnal)).max(0.0);
        let gust = wind + rng.range(1.0, 7.0);
        let total_disp = {
            let mut t = 41.0 + cum - reset_sub + spike_add;
            if glitch_at.is_some_and(|g| i >= g && i < g + 3) {
                t = (t - 1.1).max(0.0);
            }
            r2(t.max(0.0))
        };
        let solar = if (6.0..18.0).contains(&hour) {
            900.0 * ((hour - 6.0) / 12.0 * std::f64::consts::PI).sin()
        } else {
            0.0
        };
        let mut row = Map::new();
        row.insert("dateutc".into(), json!(ts.timestamp_millis()));
        row.insert(
            "date".into(),
            json!(ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
        );
        row.insert("tempf".into(), json!(r1(temp)));
        row.insert("feelsLike".into(), json!(r1(temp - wind * 0.1)));
        row.insert("dewPoint".into(), json!(r1(dew)));
        row.insert("humidity".into(), json!(hum as i64));
        row.insert("windspeedmph".into(), json!(r1(wind)));
        row.insert("windgustmph".into(), json!(r1(gust)));
        row.insert("maxdailygust".into(), json!(r1(gust + 3.0)));
        row.insert(
            "winddir".into(),
            json!(((rng.range(0.0, 360.0)) as i64) % 360),
        );
        row.insert("baromrelin".into(), json!(r2(29.9 + 0.2 * synoptic / 6.0)));
        row.insert("baromabsin".into(), json!(r2(25.1 + 0.2 * synoptic / 6.0)));
        row.insert("hourlyrainin".into(), json!(r2(rate[i])));
        row.insert("eventrainin".into(), json!(r2(event_cum)));
        row.insert("dailyrainin".into(), json!(r2(cum - origin_day)));
        row.insert("weeklyrainin".into(), json!(r2(cum - origin_week)));
        row.insert("monthlyrainin".into(), json!(r2(cum - origin_month)));
        row.insert("yearlyrainin".into(), json!(r2(3.2 + cum - origin_year)));
        row.insert("totalrainin".into(), json!(total_disp));
        if let Some(t) = last_rain {
            row.insert(
                "lastRain".into(),
                json!(t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
            );
        }
        row.insert("solarradiation".into(), json!(r1(solar)));
        row.insert("uv".into(), json!((solar / 150.0).round() as i64));
        row.insert("tempinf".into(), json!(r1(69.0 + rng.range(-0.5, 0.5))));
        row.insert("humidityin".into(), json!(42));
        row.insert("battout".into(), json!(1));
        row.insert("battin".into(), json!(1));
        out.push(Value::Object(row));
    }
    out
}

/// Rows newest first, as AWN returns them.
pub fn generate(spec: &StationSpec) -> Vec<Value> {
    let mut v = generate_oldest_first(spec);
    v.reverse();
    v
}

/// `derive`: shifts temperatures and scales the rain counters; timestamps are kept (deterministic).
pub fn derive(rows: &[Value], temp_offset_f: f64, rain_scale: f64) -> Vec<Value> {
    const TEMPS: [&str; 4] = ["tempf", "feelsLike", "dewPoint", "tempinf"];
    const RAINS: [&str; 7] = [
        "hourlyrainin",
        "eventrainin",
        "dailyrainin",
        "weeklyrainin",
        "monthlyrainin",
        "yearlyrainin",
        "totalrainin",
    ];
    rows.iter()
        .map(|r| {
            let mut m = r.as_object().cloned().unwrap_or_default();
            for k in TEMPS {
                if k != "tempinf" {
                    if let Some(x) = m.get(k).and_then(Value::as_f64) {
                        m.insert(k.into(), json!(r1(x + temp_offset_f)));
                    }
                }
            }
            for k in RAINS {
                if let Some(x) = m.get(k).and_then(Value::as_f64) {
                    m.insert(k.into(), json!(r2(x * rain_scale)));
                }
            }
            Value::Object(m)
        })
        .collect()
}

/// A `/devices` element for a station (coordinates already fuzzed by the caller).
pub fn device_json(
    mac: &str,
    name: &str,
    lat: f64,
    lon: f64,
    elevation_ft: f64,
    tz: &str,
    newest: &Value,
) -> Value {
    let mut last = newest.as_object().cloned().unwrap_or_default();
    last.insert("tz".into(), json!(tz));
    json!({
        "macAddress": mac,
        "lastData": Value::Object(last),
        "info": {"name": name, "coords": {"coords": {"lat": r1(lat), "lon": r1(lon)}, "elevation": r1(elevation_ft)}},
    })
}

fn gz(bytes: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    e.write_all(bytes).expect("in-memory write");
    e.finish().expect("in-memory finish")
}

/// Writes `awn/stations/<MAC>/device.json` and `history/page-NNNN.json[.gz]` (288 rows per page, newest first).
pub fn write_station(
    root: &Path,
    mac: &str,
    device: &Value,
    rows_newest_first: &[Value],
    gzip: bool,
) -> std::io::Result<()> {
    let dir = root.join("awn").join("stations").join(mac);
    let hist = dir.join("history");
    std::fs::create_dir_all(&hist)?;
    std::fs::write(dir.join("device.json"), serde_json::to_vec_pretty(device)?)?;
    for (i, page) in rows_newest_first.chunks(288).enumerate() {
        let bytes = serde_json::to_vec(page)?;
        if gzip {
            std::fs::write(hist.join(format!("page-{:04}.json.gz", i + 1)), gz(&bytes))?;
        } else {
            std::fs::write(hist.join(format!("page-{:04}.json", i + 1)), bytes)?;
        }
    }
    Ok(())
}

/// A StnData answer for fixtures: `{"meta": {...}, "data": [[date, [maxt, flag], [mint, flag], [pcpn, flag]], ...]}`.
#[allow(clippy::too_many_arguments)]
pub fn acis_fixture(
    sid: &str,
    name: &str,
    lat: f64,
    lon: f64,
    elev_ft: f64,
    start: NaiveDate,
    end: NaiveDate,
    seed: u64,
) -> Value {
    let mut rng = Rng::new(seed);
    let mut data: Vec<Value> = Vec::new();
    let mut day = start;
    let mut accumulating = 0u32;
    while day <= end {
        let doy = f64::from(day.ordinal());
        let season = ((doy - 110.0) / 365.25 * std::f64::consts::TAU).sin();
        let tmax = 58.0 + 27.0 * season + rng.range(-9.0, 9.0);
        let tmin = tmax - 22.0 + rng.range(-5.0, 5.0);
        let pick = rng.unit();
        let temp = |v: f64, miss: bool| -> Value {
            if miss {
                json!("M")
            } else {
                json!([format!("{}", v.round() as i64), " "])
            }
        };
        let miss_t = pick < 0.01;
        let wet = rng.unit();
        let pcpn: Value = if wet < 0.015 {
            json!("M")
        } else if wet < 0.05 {
            json!("T")
        } else if wet < 0.24 {
            let amount = r2(-0.35 * rng.unit().max(1e-6).ln());
            if amount > 1.0 && rng.unit() < 0.3 {
                // a multi-day total: this day "S" (subsequent), the next one carries the accumulation "A"
                accumulating = 1;
                json!("S")
            } else {
                json!([format!("{amount:.2}"), " "])
            }
        } else {
            json!(["0.00", " "])
        };
        let pcpn = if accumulating == 2 {
            accumulating = 0;
            json!([format!("{:.2}", r2(rng.range(1.0, 2.5))), "A"])
        } else {
            if accumulating == 1 {
                accumulating = 2;
            }
            pcpn
        };
        data.push(json!([
            day.format("%Y-%m-%d").to_string(),
            temp(tmax, miss_t),
            temp(tmin, miss_t),
            pcpn
        ]));
        day += Duration::days(1);
    }
    json!({
        "meta": {"name": name, "ll": [lon, lat], "sids": [format!("{sid} 6")], "elev": elev_ft, "uid": 0},
        "data": data,
    })
}

/// Gzipped JSON bytes for fixture files.
pub fn gzip_json(v: &Value) -> Vec<u8> {
    gz(&serde_json::to_vec(v).expect("serialisable"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn newest() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 15, 12, 0, 0).unwrap()
    }

    #[test]
    fn generation_is_deterministic_and_newest_first() {
        let spec = StationSpec::simple(newest(), 3, 5, 7);
        let a = generate(&spec);
        let b = generate(&spec);
        assert_eq!(a, b);
        assert_eq!(a.len(), 3 * 288);
        assert_eq!(a[0]["dateutc"], newest().timestamp_millis());
        assert_eq!(a[1]["dateutc"], newest().timestamp_millis() - 300_000);
        let other = generate(&StationSpec { seed: 8, ..spec });
        assert_ne!(a, other);
    }

    #[test]
    fn anomalies_show_up_in_the_total_counter() {
        let mut spec = StationSpec::simple(newest(), 20, 5, 3);
        spec.anomalies = Anomalies {
            reset_at_day: Some(5.0),
            glitch_at_day: Some(10.0),
            spike_at_day: Some(15.0),
        };
        let rows: Vec<f64> = generate_oldest_first(&spec)
            .iter()
            .map(|r| r["totalrainin"].as_f64().unwrap())
            .collect();
        let drops = rows.windows(2).filter(|w| w[1] < w[0] - 0.05).count();
        let jumps = rows.windows(2).filter(|w| w[1] > w[0] + 1.5).count();
        assert!(
            drops >= 2,
            "reset and glitch both drop the counter: {drops}"
        );
        assert_eq!(
            jumps, 1,
            "exactly the spike: the glitch recovers by about 1.1 only"
        );
    }

    #[test]
    fn derive_keeps_timestamps_and_scales_rain() {
        let rows = generate(&StationSpec::simple(newest(), 5, 5, 1));
        let d = derive(&rows, -3.0, 2.0);
        assert_eq!(rows[10]["dateutc"], d[10]["dateutc"]);
        let t0 = rows[10]["tempf"].as_f64().unwrap();
        assert!((d[10]["tempf"].as_f64().unwrap() - (t0 - 3.0)).abs() < 0.11);
        assert!(
            (d[10]["totalrainin"].as_f64().unwrap()
                - rows[10]["totalrainin"].as_f64().unwrap() * 2.0)
                .abs()
                < 0.011
        );
    }

    #[test]
    fn acis_has_the_documented_forms() {
        let v = acis_fixture(
            "USW00099001",
            "Demo",
            46.6,
            -112.0,
            3830.0,
            NaiveDate::from_ymd_opt(2020, 1, 1).unwrap(),
            NaiveDate::from_ymd_opt(2021, 12, 31).unwrap(),
            5,
        );
        let text = v.to_string();
        let parsed = famweather_core::acis::parse_station_data(text.as_bytes()).unwrap();
        assert_eq!(parsed.days.len(), 731);
        assert_eq!(parsed.unparsed, 0);
        assert!(parsed.days.iter().any(|d| d.precip_trace));
        assert!(parsed.days.iter().any(|d| d.tmax_f.is_none()));
    }
}
