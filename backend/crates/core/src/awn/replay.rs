//! `ReplayAwn`: serves recorded fixtures (03 section 10). No gates, no database, no network.
//!
//! Layout under the fixtures root:
//! `awn/devices_ok.json`, `awn/stations/<MAC>/device.json`, `awn/stations/<MAC>/history/page-NNNN.json[.gz]`.

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Duration as ChronoDuration, SecondsFormat, Utc};
use serde_json::Value;

use super::models::{decode_rows, parse_device, ApiKey, AwnDevice, AwnObservation, Mac};
use super::{AwnApi, AwnError};
use crate::clock::Clock;

/// Connect-flow keys (conventions "Fixtures").
pub const REPLAY_VALID_KEY: &str = "fixture-valid-key";
pub const REPLAY_INVALID_KEY: &str = "fixture-invalid-key";
pub const REPLAY_RATELIMITED_KEY: &str = "fixture-ratelimited-key";
/// The seeded poller credential's key.
pub const REPLAY_POLLER_KEY: &str = "fixture-poller-key";

const FIVE_MIN_MS: i64 = 300_000;

pub struct ReplayAwn {
    root: PathBuf,
    /// Added to every returned timestamp; fixed for the process lifetime.
    shift_ms: i64,
    stations: BTreeMap<Mac, PathBuf>,
    /// Unshifted, newest first, deduplicated; loaded on first use.
    series: Mutex<HashMap<Mac, Arc<Vec<AwnObservation>>>>,
}

impl std::fmt::Debug for ReplayAwn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReplayAwn")
            .field("shift_ms", &self.shift_ms)
            .field("stations", &self.stations.len())
            .finish()
    }
}

fn read_bytes(path: &Path) -> std::io::Result<Vec<u8>> {
    let raw = std::fs::read(path)?;
    if path.extension().is_some_and(|e| e == "gz") {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(raw.as_slice()).read_to_end(&mut out)?;
        Ok(out)
    } else {
        Ok(raw)
    }
}

/// History page files of a station, in file-name order.
fn page_files(station_dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(station_dir.join("history"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            n.starts_with("page-") && (n.ends_with(".json") || n.ends_with(".json.gz"))
        })
        .collect();
    v.sort();
    v
}

fn load_series(station_dir: &Path) -> Result<Vec<AwnObservation>, AwnError> {
    let mut all: Vec<AwnObservation> = Vec::new();
    for p in page_files(station_dir) {
        let bytes = read_bytes(&p).map_err(|_| AwnError::BadResponse)?;
        let page: Vec<Value> = serde_json::from_slice(&bytes).map_err(|_| AwnError::BadResponse)?;
        all.extend(decode_rows(page).0);
    }
    all.sort_by_key(|o| std::cmp::Reverse(o.dateutc));
    all.dedup_by_key(|o| o.dateutc);
    Ok(all)
}

fn shift_iso(rest: &mut serde_json::Map<String, Value>, field: &str, shift_ms: i64) {
    if let Some(Value::String(s)) = rest.get_mut(field) {
        if let Ok(t) = DateTime::parse_from_rfc3339(s) {
            let moved = t.with_timezone(&Utc) + ChronoDuration::milliseconds(shift_ms);
            *s = moved.to_rfc3339_opts(SecondsFormat::Millis, true);
        }
    }
}

/// Shifts `lastData.dateutc`, `lastData.date` and `lastData.lastRain` of one `/devices` element (03 section 10).
pub(crate) fn shift_device(device: &mut Value, shift_ms: i64) {
    let Some(last) = device.get_mut("lastData").and_then(Value::as_object_mut) else {
        return;
    };
    if let Some(Value::Number(n)) = last.get("dateutc") {
        if let Some(ms) = n.as_i64() {
            last.insert("dateutc".into(), Value::from(ms + shift_ms));
        }
    }
    shift_iso(last, "date", shift_ms);
    shift_iso(last, "lastRain", shift_ms);
}

fn shifted(o: &AwnObservation, shift_ms: i64) -> AwnObservation {
    let mut o = o.clone();
    o.dateutc += shift_ms;
    // `rest` is a HashMap; reuse the Map-based helper through a temporary Map.
    let mut m: serde_json::Map<String, Value> = std::mem::take(&mut o.rest).into_iter().collect();
    shift_iso(&mut m, "lastRain", shift_ms);
    shift_iso(&mut m, "date", shift_ms);
    o.rest = m.into_iter().collect();
    o
}

impl ReplayAwn {
    /// Scans the station history once to fix the time shift
    /// `floor_5min(clock.now()) - max(dateutc over all pages of all stations)`.
    /// A missing or empty `awn/stations/` gives shift 0 and never fails, so connect-flow fixtures
    /// work before any station history exists.
    pub fn new(fixtures_root: PathBuf, clock: Arc<dyn Clock>) -> Result<Self, AwnError> {
        let mut stations = BTreeMap::new();
        let mut max_ts: Option<i64> = None;
        if let Ok(rd) = std::fs::read_dir(fixtures_root.join("awn").join("stations")) {
            for e in rd.flatten() {
                let Some(mac) = e.file_name().to_str().and_then(|n| Mac::parse(n).ok()) else {
                    continue;
                };
                for p in page_files(&e.path()) {
                    let bytes = read_bytes(&p).map_err(|_| AwnError::BadResponse)?;
                    let rows: Vec<Value> =
                        serde_json::from_slice(&bytes).map_err(|_| AwnError::BadResponse)?;
                    if let Some(m) = rows
                        .iter()
                        .filter_map(|r| r.get("dateutc").and_then(Value::as_i64))
                        .max()
                    {
                        max_ts = Some(max_ts.map_or(m, |c| c.max(m)));
                    }
                }
                stations.insert(mac, e.path());
            }
        }
        let now_ms = clock.now().timestamp_millis();
        let shift_ms = max_ts.map_or(0, |m| now_ms - now_ms.rem_euclid(FIVE_MIN_MS) - m);
        Ok(Self {
            root: fixtures_root,
            shift_ms,
            stations,
            series: Mutex::new(HashMap::new()),
        })
    }

    async fn read_json(&self, path: PathBuf) -> Result<Value, AwnError> {
        let bytes = tokio::task::spawn_blocking(move || read_bytes(&path))
            .await
            .map_err(|_| AwnError::BadResponse)?
            .map_err(|_| AwnError::BadResponse)?;
        serde_json::from_slice(&bytes).map_err(|_| AwnError::BadResponse)
    }

    async fn devices_from(&self, path: PathBuf) -> Result<Vec<AwnDevice>, AwnError> {
        match self.read_json(path).await? {
            Value::Array(mut a) => {
                a.iter_mut().for_each(|d| shift_device(d, self.shift_ms));
                Ok(a.iter().filter_map(parse_device).collect())
            }
            _ => Err(AwnError::BadResponse),
        }
    }

    async fn station_series(
        &self,
        mac: &Mac,
    ) -> Result<Option<Arc<Vec<AwnObservation>>>, AwnError> {
        let Some(dir) = self.stations.get(mac) else {
            return Ok(None);
        };
        if let Some(s) = self
            .series
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(mac)
        {
            return Ok(Some(s.clone()));
        }
        let dir = dir.clone();
        let loaded = tokio::task::spawn_blocking(move || load_series(&dir))
            .await
            .map_err(|_| AwnError::BadResponse)??;
        let arc = Arc::new(loaded);
        self.series
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(mac.clone(), arc.clone());
        Ok(Some(arc))
    }
}

#[async_trait]
impl AwnApi for ReplayAwn {
    async fn list_devices(&self, key: &ApiKey) -> Result<Vec<AwnDevice>, AwnError> {
        match key.expose() {
            REPLAY_VALID_KEY => {
                self.devices_from(self.root.join("awn/devices_ok.json"))
                    .await
            }
            REPLAY_INVALID_KEY => Err(AwnError::Unauthorized),
            REPLAY_RATELIMITED_KEY => Err(AwnError::RateLimited { retry_after: None }),
            REPLAY_POLLER_KEY => {
                let mut out = Vec::new();
                for dir in self.stations.values() {
                    let mut v = self.read_json(dir.join("device.json")).await?;
                    shift_device(&mut v, self.shift_ms);
                    out.extend(parse_device(&v));
                }
                Ok(out)
            }
            _ => Err(AwnError::Unauthorized),
        }
    }

    /// Merged series of all pages of the station, newest first, deduplicated; returns the `limit` newest rows
    /// with `dateutc <= end_date` (unshifted). Only `fixture-poller-key` may read history.
    async fn history(
        &self,
        key: &ApiKey,
        mac: &Mac,
        end_date: Option<DateTime<Utc>>,
        limit: u16,
    ) -> Result<Vec<AwnObservation>, AwnError> {
        if key.expose() != REPLAY_POLLER_KEY {
            return Err(AwnError::Unauthorized);
        }
        let Some(series) = self.station_series(mac).await? else {
            return Ok(Vec::new());
        };
        let upper = end_date.map_or(i64::MAX, |e| e.timestamp_millis() - self.shift_ms);
        Ok(series
            .iter()
            .filter(|o| o.dateutc <= upper)
            .take(limit as usize)
            .map(|o| shifted(o, self.shift_ms))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::FixedClock;
    use chrono::TimeZone;
    use secrecy::SecretString;
    use serde_json::json;
    use std::io::Write;

    fn key(s: &str) -> ApiKey {
        ApiKey::new(SecretString::from(s))
    }
    fn committed_fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
    }
    fn clock() -> Arc<dyn Clock> {
        Arc::new(FixedClock::new(
            Utc.with_ymd_and_hms(2026, 10, 17, 12, 3, 30).unwrap(),
        ))
    }

    #[tokio::test]
    async fn connect_flow_fixtures_list_devices() {
        let r = ReplayAwn::new(committed_fixtures(), clock()).unwrap();
        let devs = r.list_devices(&key(REPLAY_VALID_KEY)).await.unwrap();
        let macs: Vec<&str> = devs.iter().map(|d| d.mac.as_str()).collect();
        assert_eq!(macs, ["AA:BB:CC:00:01:01", "AA:BB:CC:00:01:02"]);
        for d in &devs {
            assert!(d.coords.is_some(), "info.coords present");
            assert!(d.elevation_ft.is_some());
            assert_eq!(d.timezone.as_deref(), Some("America/Denver"));
        }
        assert_eq!(
            r.list_devices(&key(REPLAY_INVALID_KEY)).await.unwrap_err(),
            AwnError::Unauthorized
        );
        assert_eq!(
            r.list_devices(&key(REPLAY_RATELIMITED_KEY))
                .await
                .unwrap_err(),
            AwnError::RateLimited { retry_after: None }
        );
        assert_eq!(
            r.list_devices(&key("some-other-key")).await.unwrap_err(),
            AwnError::Unauthorized
        );
        // The seeded station fixtures (03 T7) exist now: the poller key lists exactly the three seeded devices.
        let seeded: Vec<String> = r
            .list_devices(&key(REPLAY_POLLER_KEY))
            .await
            .unwrap()
            .iter()
            .map(|d| d.mac.to_string())
            .collect();
        assert_eq!(
            seeded,
            [
                "AA:BB:CC:00:00:01",
                "AA:BB:CC:00:00:02",
                "AA:BB:CC:00:00:03"
            ]
        );
    }

    #[tokio::test]
    async fn without_station_history_the_poller_key_lists_nothing_and_construction_works() {
        let dir = tempfile::tempdir().unwrap();
        let r = ReplayAwn::new(dir.path().to_owned(), clock()).unwrap();
        assert!(r
            .list_devices(&key(REPLAY_POLLER_KEY))
            .await
            .unwrap()
            .is_empty());
    }

    #[test]
    fn committed_fixtures_use_only_synthetic_macs() {
        let macs = regex::Regex::new(r"(?i)([0-9a-f]{2}:){5}[0-9a-f]{2}").unwrap();
        for f in [
            "devices_ok.json",
            "devices_unauthorized.json",
            "devices_rate_limited.json",
        ] {
            let text = std::fs::read_to_string(committed_fixtures().join("awn").join(f)).unwrap();
            for m in macs.find_iter(&text) {
                assert!(
                    m.as_str().to_ascii_uppercase().starts_with("AA:BB:CC:00:"),
                    "{f}: {}",
                    m.as_str()
                );
            }
        }
    }

    #[test]
    fn shift_device_moves_last_data_timestamps() {
        let mut d = json!({"macAddress": "AA:BB:CC:00:01:01", "lastData": {
            "dateutc": 1_000_000i64, "date": "2025-10-17T12:00:00.000Z", "lastRain": "2025-10-16T00:00:00.000Z", "tempf": 50.0}});
        shift_device(&mut d, 60_000);
        assert_eq!(d["lastData"]["dateutc"], 1_060_000);
        assert_eq!(d["lastData"]["date"], "2025-10-17T12:01:00.000Z");
        assert_eq!(d["lastData"]["lastRain"], "2025-10-16T00:01:00.000Z");
        assert_eq!(d["lastData"]["tempf"], 50.0);
        // Devices without lastData, or with unparsable dates, are left alone.
        let mut bare = json!({"macAddress": "AA:BB:CC:00:01:01"});
        shift_device(&mut bare, 5);
        assert_eq!(bare, json!({"macAddress": "AA:BB:CC:00:01:01"}));
        let mut odd = json!({"lastData": {"date": "yesterday"}});
        shift_device(&mut odd, 5);
        assert_eq!(odd["lastData"]["date"], "yesterday");
    }

    #[tokio::test]
    async fn rows_without_dateutc_are_dropped_like_the_live_client_does() {
        let root = tempfile::tempdir().unwrap();
        let t = 1_700_000_000_000i64;
        write_station(
            root.path(),
            "AA:BB:CC:00:00:01",
            &[(
                "page-0001.json",
                json!([{"dateutc": t}, {"tempf": 1.0}, {"dateutc": "x"}, {"dateutc": t - FIVE_MIN_MS}]),
            )],
        );
        let r = ReplayAwn::new(root.path().to_path_buf(), clock()).unwrap();
        let mac = Mac::parse("AA:BB:CC:00:00:01").unwrap();
        assert_eq!(
            r.history(&key(REPLAY_POLLER_KEY), &mac, None, 288)
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn missing_fixture_root_never_fails_construction() {
        let r = ReplayAwn::new(PathBuf::from("/nonexistent/fixtures"), clock()).unwrap();
        assert_eq!(r.shift_ms, 0);
        assert_eq!(
            r.list_devices(&key(REPLAY_VALID_KEY)).await.unwrap_err(),
            AwnError::BadResponse
        );
    }

    fn write_station(root: &Path, mac: &str, pages: &[(&str, Value)]) {
        let dir = root.join("awn/stations").join(mac);
        std::fs::create_dir_all(dir.join("history")).unwrap();
        std::fs::write(
            dir.join("device.json"),
            json!({"macAddress": mac, "lastData": {"tz": "America/Denver"}}).to_string(),
        )
        .unwrap();
        for (name, rows) in pages {
            let path = dir.join("history").join(name);
            if name.ends_with(".gz") {
                let mut enc =
                    flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
                enc.write_all(rows.to_string().as_bytes()).unwrap();
                std::fs::write(path, enc.finish().unwrap()).unwrap();
            } else {
                std::fs::write(path, rows.to_string()).unwrap();
            }
        }
    }

    #[tokio::test]
    async fn history_is_shifted_merged_and_paged() {
        let root = tempfile::tempdir().unwrap();
        let t = 1_700_000_000_000i64; // newest fixture reading
        let row = |k: i64| json!({"dateutc": t - k * FIVE_MIN_MS, "tempf": 40.0 + k as f64, "lastRain": "2023-11-14T00:00:00.000Z"});
        // Pages overlap on purpose (duplicate dateutc), one is gzipped, order on disk is not by time.
        write_station(
            root.path(),
            "AA:BB:CC:00:00:01",
            &[
                ("page-0001.json", json!([row(0), row(1), row(2)])),
                ("page-0002.json.gz", json!([row(2), row(3), row(4)])),
            ],
        );
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 3, 30).unwrap();
        let r = ReplayAwn::new(root.path().to_path_buf(), Arc::new(FixedClock::new(now))).unwrap();
        let floor = now.timestamp_millis() - now.timestamp_millis() % FIVE_MIN_MS;
        assert_eq!(r.shift_ms, floor - t);

        let mac = Mac::parse("AA:BB:CC:00:00:01").unwrap();
        let k = key(REPLAY_POLLER_KEY);
        let all = r.history(&k, &mac, None, 288).await.unwrap();
        assert_eq!(all.len(), 5, "deduplicated");
        assert_eq!(
            all[0].dateutc, floor,
            "newest reading equals now (floored to 5 minutes)"
        );
        assert!(
            all.windows(2).all(|w| w[0].dateutc > w[1].dateutc),
            "newest first"
        );
        // lastRain moved by the same shift.
        let lr = all[0].rest["lastRain"].as_str().unwrap();
        assert_ne!(lr, "2023-11-14T00:00:00.000Z");
        assert!(lr.starts_with("2026-"));

        // limit and endDate (cursor = oldest - 1 ms terminates with an empty page).
        let first = r.history(&k, &mac, None, 2).await.unwrap();
        assert_eq!(first.len(), 2);
        let end = DateTime::from_timestamp_millis(first[1].dateutc - 1).unwrap();
        let next = r.history(&k, &mac, Some(end), 288).await.unwrap();
        assert_eq!(next.len(), 3);
        let end = DateTime::from_timestamp_millis(all[4].dateutc - 1).unwrap();
        assert!(r
            .history(&k, &mac, Some(end), 288)
            .await
            .unwrap()
            .is_empty());

        // Keys and unknown stations.
        assert_eq!(
            r.history(&key("other"), &mac, None, 10).await.unwrap_err(),
            AwnError::Unauthorized
        );
        let other = Mac::parse("AA:BB:CC:00:00:09").unwrap();
        assert!(r.history(&k, &other, None, 10).await.unwrap().is_empty());
        // The poller key lists the station's device.json.
        assert_eq!(r.list_devices(&k).await.unwrap().len(), 1);
    }
}
