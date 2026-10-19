//! ACIS client (03 sections 7.5 and 7.5a): daily station data for `reference_sync` and the `StnMeta` search for
//! the API's baseline picker. Keyless. Neither implementation reserves rate gates (callers do, `acis:api`).

mod live;
mod parse;
mod replay;

use async_trait::async_trait;
use chrono::NaiveDate;

pub use live::LiveAcis;
pub use parse::{parse_station_data, parse_station_meta, ParsedStationData};
pub use replay::{ReplayAcis, UNAVAILABLE_MARKER};

/// No URL, no source chain, no upstream body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AcisError {
    /// 5xx, 429, timeout or connection error.
    #[error("ACIS unavailable")]
    Unavailable,
    /// Undecodable or oversize body, or an unexpected status.
    #[error("ACIS returned an unusable response")]
    BadResponse,
}

/// One day of a reference station (03 section 7.5 parser table).
#[derive(Debug, Clone, PartialEq)]
pub struct AcisDay {
    pub date: NaiveDate,
    pub tmax_f: Option<f64>,
    pub tmin_f: Option<f64>,
    pub precip_in: Option<f64>,
    /// ACIS `T`: precip is NULL and this is true.
    pub precip_trace: bool,
    /// Comma-joined `<m|n|p>:<flag>` entries, `?` for an unparseable element.
    pub quality_flags: Option<String>,
    /// Number of elements of this day that could not be parsed.
    pub unparsed: u8,
}

/// `west,south,east,north` in degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BBox {
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
}

impl BBox {
    pub fn centre(&self) -> (f64, f64) {
        (
            (self.south + self.north) / 2.0,
            (self.west + self.east) / 2.0,
        )
    }
}

impl std::fmt::Display for BBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{},{},{},{}",
            self.west, self.south, self.east, self.north
        )
    }
}

/// One station of an ACIS `StnMeta` answer.
#[derive(Debug, Clone, PartialEq)]
pub struct AcisStationMeta {
    /// `sids` entry of type 6.
    pub ghcn_id: Option<String>,
    /// type 5
    pub icao: Option<String>,
    /// type 3
    pub faa: Option<String>,
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    pub elevation_ft: Option<f64>,
    /// `valid_daterange` per element, in the order of the request's `elems` (pcpn, maxt, mint).
    pub pcpn: Option<(NaiveDate, NaiveDate)>,
    pub maxt: Option<(NaiveDate, NaiveDate)>,
    pub mint: Option<(NaiveDate, NaiveDate)>,
}

#[async_trait]
pub trait AcisApi: Send + Sync {
    /// Daily maxt, mint and pcpn for `sid` between `sdate` and `edate` inclusive.
    async fn station_data(
        &self,
        sid: &str,
        sdate: NaiveDate,
        edate: NaiveDate,
    ) -> Result<Vec<AcisDay>, AcisError>;

    /// Stations inside `bbox` with their sids and valid date ranges. An empty `meta` is `Ok(vec![])`.
    async fn nearby_stations(&self, bbox: BBox) -> Result<Vec<AcisStationMeta>, AcisError>;
}

/// Body caps (03 7.5a): StnMeta answers are small; StnData for 35 years of one station is below 1 MiB.
pub(crate) const MAX_META_BYTES: usize = 512 * 1024;
pub(crate) const MAX_DATA_BYTES: usize = 8 * 1024 * 1024;
