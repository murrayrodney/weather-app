//! Baseline lookup (ACIS) filter, ranking and cache (02 4.15). Pure functions plus an in-memory TTL cache.

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, Duration, NaiveDate, Utc};
use famweather_core::acis::{AcisStationMeta, BBox};

pub const RADIUS_MI: f64 = 60.0;
pub const MAX_STALE_DAYS: i64 = 30;
pub const MIN_RECORD_YEARS: u32 = 10;
/// Candidates kept in the cache and accepted by PUT.
pub const INTERNAL_CAP: usize = 10;
/// Candidates returned by GET.
pub const RESPONSE_CAP: usize = 5;
pub const SHORT_RECORD_YEARS: u32 = 20;
pub const CACHE_MAX_ENTRIES: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub reference_station_id: String,
    pub name: String,
    pub airport_code: String,
    pub lat: f64,
    pub lon: f64,
    pub elevation_ft: Option<f64>,
    pub distance_mi: f64,
    pub record_start: NaiveDate,
    pub record_years: u32,
}

pub fn haversine_mi(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dphi = (lat2 - lat1).to_radians();
    let dl = (lon2 - lon1).to_radians();
    let a = (dphi / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    3958.8 * 2.0 * a.sqrt().asin()
}

/// Half-width 0.9 degrees of latitude around the station, and the same distance in longitude.
pub fn search_box(lat: f64, lon: f64) -> BBox {
    let half_lon = 0.9 / lat.to_radians().cos().max(0.2);
    BBox {
        west: lon - half_lon,
        south: lat - 0.9,
        east: lon + half_lon,
        north: lat + 0.9,
    }
}

fn clean(s: &str) -> String {
    let c: String = s.chars().filter(|c| !c.is_control()).collect();
    c.trim().chars().take(80).collect()
}

/// Filter and rank (nearest first, then longer record, then id), capped at [`INTERNAL_CAP`].
pub fn rank(metas: &[AcisStationMeta], lat: f64, lon: f64, today: NaiveDate) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = metas
        .iter()
        .filter_map(|m| {
            let id = m.ghcn_id.as_deref().filter(|i| i.starts_with("USW"))?;
            let code = m.icao.clone().or_else(|| m.faa.clone())?;
            let (p, mx, mn) = (m.pcpn?, m.maxt?, m.mint?);
            let fresh = |end: NaiveDate| (today - end).num_days() <= MAX_STALE_DAYS;
            if !(fresh(p.1) && fresh(mx.1) && fresh(mn.1)) {
                return None;
            }
            let start = p.0.max(mx.0).max(mn.0);
            let years = today.years_since(start)?;
            if years < MIN_RECORD_YEARS {
                return None;
            }
            let distance = haversine_mi(lat, lon, m.lat, m.lon);
            if distance > RADIUS_MI {
                return None;
            }
            Some(Candidate {
                reference_station_id: id.to_owned(),
                name: clean(&m.name),
                airport_code: code,
                lat: m.lat,
                lon: m.lon,
                elevation_ft: m.elevation_ft,
                distance_mi: (distance * 10.0).round() / 10.0,
                record_start: start,
                record_years: years,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        a.distance_mi
            .total_cmp(&b.distance_mi)
            .then(b.record_years.cmp(&a.record_years))
            .then(a.reference_station_id.cmp(&b.reference_station_id))
    });
    out.truncate(INTERNAL_CAP);
    out
}

struct Entry {
    at: DateTime<Utc>,
    ttl: Duration,
    list: Vec<Candidate>,
}

/// Key = the station's stored 2-decimal coordinates in hundredths of a degree.
pub type CacheKey = (i64, i64);

pub fn cache_key(lat: f64, lon: f64) -> CacheKey {
    ((lat * 100.0).round() as i64, (lon * 100.0).round() as i64)
}

#[derive(Default)]
pub struct BaselineCache {
    map: Mutex<HashMap<CacheKey, Entry>>,
}

impl BaselineCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, key: CacheKey, now: DateTime<Utc>) -> Option<Vec<Candidate>> {
        let g = self.map.lock().unwrap_or_else(|p| p.into_inner());
        g.get(&key)
            .filter(|e| now - e.at < e.ttl)
            .map(|e| e.list.clone())
    }

    /// Empty results live 1 h, others 24 h; at most 64 entries (the oldest is evicted).
    pub fn put(&self, key: CacheKey, now: DateTime<Utc>, list: Vec<Candidate>) {
        let ttl = if list.is_empty() {
            Duration::hours(1)
        } else {
            Duration::hours(24)
        };
        let mut g = self.map.lock().unwrap_or_else(|p| p.into_inner());
        g.retain(|_, e| now - e.at < e.ttl);
        if !g.contains_key(&key) && g.len() >= CACHE_MAX_ENTRIES {
            if let Some(oldest) = g.iter().min_by_key(|(_, e)| e.at).map(|(k, _)| *k) {
                g.remove(&oldest);
            }
        }
        g.insert(key, Entry { at: now, ttl, list });
    }

    pub fn len(&self) -> usize {
        self.map.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
