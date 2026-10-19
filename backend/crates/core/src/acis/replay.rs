//! `ReplayAcis`: serves recorded ACIS fixtures (03 section 10). No network, no database, no gates, no shifting.
//!
//! * `acis/<SID>__<sdate>__<edate>.json[.gz]`: a recorded `StnData` answer; rows are sliced by the requested dates.
//!   A `sid` with no file replays an empty result (logged once per `sid` per process at `info`).
//! * `acis/stnmeta__<tag>.json[.gz]`: a recorded `StnMeta` answer plus a `recorded_bbox` member
//!   (`"west,south,east,north"`); the file whose centre is nearest to the requested centre (within 1 degree) wins,
//!   else `stnmeta__empty.json`. A request centred within 0.05 degrees of [`UNAVAILABLE_MARKER`] fails with
//!   `AcisError::Unavailable` (how the API's ACIS-down tests run without a mock server).

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::NaiveDate;
use serde_json::Value;

use super::parse::{parse_station_data, parse_station_meta};
use super::{AcisApi, AcisDay, AcisError, AcisStationMeta, BBox};

/// `(latitude, longitude)`: a request centred here gets `AcisError::Unavailable` (documented in the fixtures README).
pub const UNAVAILABLE_MARKER: (f64, f64) = (0.0, 0.0);
const MARKER_TOLERANCE_DEG: f64 = 0.05;
const NEAREST_WITHIN_DEG: f64 = 1.0;

pub struct ReplayAcis {
    dir: PathBuf,
    warned: Mutex<HashSet<String>>,
    days: Mutex<HashMap<String, Arc<Vec<AcisDay>>>>,
    metas: Mutex<Option<Arc<Vec<MetaFile>>>>,
}

struct MetaFile {
    centre: Option<(f64, f64)>,
    empty_fallback: bool,
    stations: Vec<AcisStationMeta>,
}

impl std::fmt::Debug for ReplayAcis {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReplayAcis { .. }")
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

fn file_name(p: &Path) -> &str {
    p.file_name().and_then(|n| n.to_str()).unwrap_or("")
}

fn is_json(name: &str) -> bool {
    name.ends_with(".json") || name.ends_with(".json.gz")
}

fn parse_bbox(s: &str) -> Option<BBox> {
    let v: Vec<f64> = s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
    (v.len() == 4).then(|| BBox {
        west: v[0],
        south: v[1],
        east: v[2],
        north: v[3],
    })
}

impl ReplayAcis {
    pub fn new(fixtures_root: PathBuf) -> Result<Self, AcisError> {
        Ok(Self {
            dir: fixtures_root.join("acis"),
            warned: Mutex::new(HashSet::new()),
            days: Mutex::new(HashMap::new()),
            metas: Mutex::new(None),
        })
    }

    fn files(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| is_json(file_name(p)))
            .collect();
        v.sort();
        v
    }

    fn load_days(&self, sid: &str) -> Result<Option<Arc<Vec<AcisDay>>>, AcisError> {
        if let Some(d) = self.days.lock().unwrap_or_else(|p| p.into_inner()).get(sid) {
            return Ok(Some(d.clone()));
        }
        let prefix = format!("{sid}__");
        let Some(path) = self
            .files()
            .into_iter()
            .find(|p| file_name(p).starts_with(&prefix))
        else {
            return Ok(None);
        };
        let bytes = read_bytes(&path).map_err(|_| AcisError::BadResponse)?;
        let parsed = Arc::new(parse_station_data(&bytes)?.days);
        self.days
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(sid.to_owned(), parsed.clone());
        Ok(Some(parsed))
    }

    fn load_metas(&self) -> Result<Arc<Vec<MetaFile>>, AcisError> {
        let mut guard = self.metas.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(m) = guard.as_ref() {
            return Ok(m.clone());
        }
        let mut out = Vec::new();
        for p in self.files() {
            let name = file_name(&p).to_owned();
            if !name.starts_with("stnmeta__") {
                continue;
            }
            let bytes = read_bytes(&p).map_err(|_| AcisError::BadResponse)?;
            let centre = serde_json::from_slice::<Value>(&bytes)
                .ok()
                .and_then(|v| v.get("recorded_bbox")?.as_str().and_then(parse_bbox))
                .map(|b| b.centre());
            out.push(MetaFile {
                centre,
                empty_fallback: name.starts_with("stnmeta__empty."),
                stations: parse_station_meta(&bytes)?,
            });
        }
        let arc = Arc::new(out);
        *guard = Some(arc.clone());
        Ok(arc)
    }
}

#[async_trait]
impl AcisApi for ReplayAcis {
    async fn station_data(
        &self,
        sid: &str,
        sdate: NaiveDate,
        edate: NaiveDate,
    ) -> Result<Vec<AcisDay>, AcisError> {
        match self.load_days(sid)? {
            Some(all) => Ok(all
                .iter()
                .filter(|d| d.date >= sdate && d.date <= edate)
                .cloned()
                .collect()),
            None => {
                let first = self
                    .warned
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .insert(sid.to_owned());
                if first {
                    tracing::info!(sid, "no ACIS fixture for sid, replaying empty");
                }
                Ok(Vec::new())
            }
        }
    }

    async fn nearby_stations(&self, bbox: BBox) -> Result<Vec<AcisStationMeta>, AcisError> {
        let (lat, lon) = bbox.centre();
        if (lat - UNAVAILABLE_MARKER.0).abs() <= MARKER_TOLERANCE_DEG
            && (lon - UNAVAILABLE_MARKER.1).abs() <= MARKER_TOLERANCE_DEG
        {
            return Err(AcisError::Unavailable);
        }
        let metas = self.load_metas()?;
        let best = metas
            .iter()
            .filter_map(|m| {
                let (clat, clon) = m.centre?;
                let dist = ((clat - lat).powi(2) + (clon - lon).powi(2)).sqrt();
                (dist <= NEAREST_WITHIN_DEG).then_some((dist, m))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((_, m)) = best {
            return Ok(m.stations.clone());
        }
        Ok(metas
            .iter()
            .find(|m| m.empty_fallback)
            .map(|m| m.stations.clone())
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let acis = dir.path().join("acis");
        std::fs::create_dir_all(&acis).unwrap();
        let data = json!({"meta": {}, "data": [
            ["2020-01-01", "30", "10", "0.1"], ["2020-01-02", "31", "11", "T"], ["2020-01-03", "32", "12", "0"]]});
        std::fs::write(
            acis.join("USW00099001__2020-01-01__2020-01-03.json"),
            data.to_string(),
        )
        .unwrap();
        let mk = |name: &str, bbox: Option<&str>, n: usize| {
            let meta: Vec<Value> = (0..n)
                .map(|i| {
                    json!({"ll": [-105.0, 40.4], "sids": [format!("USW0009900{i} 6")], "name": name,
                    "valid_daterange": [[], [], []]})
                })
                .collect();
            let mut v = json!({"meta": meta});
            if let Some(b) = bbox {
                v["recorded_bbox"] = json!(b);
            }
            v.to_string()
        };
        std::fs::write(
            acis.join("stnmeta__loveland.json"),
            mk("LOVE", Some("-105.9,39.5,-104.1,41.3"), 2),
        )
        .unwrap();
        std::fs::write(
            acis.join("stnmeta__helena.json"),
            mk("HELENA", Some("-112.9,45.7,-111.1,47.5"), 1),
        )
        .unwrap();
        // gzip is read too
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(mk("EMPTYISH", None, 0).as_bytes()).unwrap();
        std::fs::write(acis.join("stnmeta__empty.json.gz"), gz.finish().unwrap()).unwrap();
        dir
    }

    #[tokio::test]
    async fn station_data_slices_by_date() {
        let t = tree();
        let r = ReplayAcis::new(t.path().to_owned()).unwrap();
        let rows = r
            .station_data("USW00099001", d("2020-01-02"), d("2020-01-03"))
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].precip_trace);
    }

    #[tokio::test]
    async fn unknown_sid_is_empty_ok_and_logged_once() {
        use std::sync::Mutex as M;
        #[derive(Clone, Default)]
        struct Cap(Arc<M<Vec<u8>>>);
        impl Write for Cap {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Cap {
            type Writer = Cap;
            fn make_writer(&'a self) -> Cap {
                self.clone()
            }
        }
        let cap = Cap::default();
        let sub = tracing_subscriber::fmt()
            .with_writer(cap.clone())
            .with_ansi(false)
            .finish();
        let t = tree();
        let r = ReplayAcis::new(t.path().to_owned()).unwrap();
        let _g = tracing::subscriber::set_default(sub);
        for _ in 0..2 {
            let rows = r
                .station_data("USW00094035", d("2020-01-01"), d("2020-01-03"))
                .await
                .unwrap();
            assert!(rows.is_empty());
        }
        let log = String::from_utf8(cap.0.lock().unwrap().clone()).unwrap();
        assert_eq!(log.matches("no ACIS fixture for sid").count(), 1, "{log}");
    }

    #[tokio::test]
    async fn nearest_stnmeta_file_wins_within_one_degree() {
        let t = tree();
        let r = ReplayAcis::new(t.path().to_owned()).unwrap();
        let near = |lat: f64, lon: f64| BBox {
            west: lon - 0.9,
            south: lat - 0.9,
            east: lon + 0.9,
            north: lat + 0.9,
        };
        assert_eq!(
            r.nearby_stations(near(40.4, -105.0)).await.unwrap().len(),
            2
        );
        assert_eq!(
            r.nearby_stations(near(46.6, -112.0)).await.unwrap().len(),
            1
        );
        // too far from both: the empty fallback
        assert!(r
            .nearby_stations(near(30.0, -90.0))
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn marker_centre_is_unavailable() {
        let t = tree();
        let r = ReplayAcis::new(t.path().to_owned()).unwrap();
        let b = BBox {
            west: -0.9,
            south: -0.9,
            east: 0.9,
            north: 0.9,
        };
        assert_eq!(
            r.nearby_stations(b).await.unwrap_err(),
            AcisError::Unavailable
        );
    }

    #[tokio::test]
    async fn missing_acis_dir_replays_empty() {
        let dir = tempfile::tempdir().unwrap();
        let r = ReplayAcis::new(dir.path().to_owned()).unwrap();
        assert!(r
            .station_data("X", d("2020-01-01"), d("2020-01-02"))
            .await
            .unwrap()
            .is_empty());
        let b = BBox {
            west: -1.0,
            south: 40.0,
            east: 1.0,
            north: 41.0,
        };
        assert!(r.nearby_stations(b).await.unwrap().is_empty());
    }
}
