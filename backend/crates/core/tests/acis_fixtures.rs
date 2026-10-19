//! The committed ACIS fixtures through `ReplayAcis` and the parsers (03 sections 7.5, 7.5a, 10 and 12.1).

use std::path::PathBuf;

use chrono::NaiveDate;
use famweather_core::acis::{
    parse_station_meta, AcisApi, AcisError, BBox, ReplayAcis, UNAVAILABLE_MARKER,
};

fn root() -> PathBuf {
    std::env::var_os("FIXTURES_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures"))
}

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

/// The box the API builds: half-width 0.9 degrees of latitude and the same distance in longitude.
fn api_box(lat: f64, lon: f64) -> BBox {
    let dlon = 0.9 / lat.to_radians().cos();
    BBox {
        west: lon - dlon,
        south: lat - 0.9,
        east: lon + dlon,
        north: lat + 0.9,
    }
}

#[test]
fn the_recorded_loveland_stnmeta_parses_as_documented() {
    let bytes = std::fs::read(root().join("acis/stnmeta__loveland.json")).unwrap();
    let all = parse_station_meta(&bytes).unwrap();
    let fnl = all
        .iter()
        .find(|s| s.ghcn_id.as_deref() == Some("USW00094035"))
        .expect("FNL is in the fixture");
    assert_eq!(fnl.icao.as_deref(), Some("KFNL"));
    assert_eq!(fnl.faa.as_deref(), Some("FNL"));
    assert_eq!(fnl.elevation_ft, Some(5015.0));
    assert_eq!((fnl.lat, fnl.lon), (40.45, -105.01667), "ll is [lon, lat]");
    assert!(fnl.pcpn.is_some() && fnl.maxt.is_some() && fnl.mint.is_some());
    assert_eq!(fnl.pcpn.unwrap().0, d("2011-10-01"));
    // Volunteer and closed stations are present too, and a rainfall-only station has no temperatures.
    let rain_only = all
        .iter()
        .find(|s| s.name.contains("RAINFALL-ONLY"))
        .unwrap();
    assert!(rain_only.pcpn.is_some() && rain_only.maxt.is_none() && rain_only.mint.is_none());
    assert!(rain_only.ghcn_id.is_none());
    let closed = all.iter().find(|s| s.name.contains("CLOSED")).unwrap();
    assert!(closed.maxt.unwrap().1 < d("2010-01-01"));
}

#[tokio::test]
async fn replay_serves_the_nearest_recorded_area_and_the_documented_fallbacks() {
    let r = ReplayAcis::new(root()).unwrap();
    // Loveland owner (the connect-flow devices sit near 40.4N 105.1W): the Loveland answer with FNL.
    let love = r.nearby_stations(api_box(40.4, -105.1)).await.unwrap();
    assert!(love
        .iter()
        .any(|s| s.ghcn_id.as_deref() == Some("USW00094035")));
    let helena = r.nearby_stations(api_box(46.6, -112.0)).await.unwrap();
    assert!(helena
        .iter()
        .any(|s| s.ghcn_id.as_deref() == Some("USW00024144")));
    let laramie = r.nearby_stations(api_box(41.3, -105.6)).await.unwrap();
    assert!(laramie
        .iter()
        .any(|s| s.ghcn_id.as_deref() == Some("USW00024022")));
    // Nothing recorded within a degree: the empty answer.
    assert!(r
        .nearby_stations(api_box(30.0, -90.0))
        .await
        .unwrap()
        .is_empty());
    // The nocandidates area: only volunteer and closed stations, none with a GHCN airport id.
    let none = r.nearby_stations(api_box(44.0, -108.0)).await.unwrap();
    assert!(!none.is_empty() && none.iter().all(|s| s.faa.is_none() && s.icao.is_none()));
    // The documented marker makes the service "unavailable".
    let (lat, lon) = UNAVAILABLE_MARKER;
    assert_eq!(
        r.nearby_stations(api_box(lat, lon)).await.unwrap_err(),
        AcisError::Unavailable
    );
}

#[tokio::test]
async fn the_demo_reference_stations_have_over_a_decade_of_complete_years() {
    let r = ReplayAcis::new(root()).unwrap();
    for sid in ["USW00099001", "USW00099002", "USW00099003"] {
        let days = r
            .station_data(sid, d("1991-01-01"), d("2026-10-16"))
            .await
            .unwrap();
        assert_eq!(days.first().unwrap().date, d("1991-01-01"), "{sid}");
        assert_eq!(days.last().unwrap().date, d("2026-09-30"), "{sid}");
        assert_eq!(days.len(), 13_057, "every day of 1991-01-01 ..= 2026-09-30");
        assert!(days.iter().any(|x| x.precip_trace), "trace days exist");
        assert!(days.iter().any(|x| x
            .quality_flags
            .as_deref()
            .is_some_and(|f| f.contains("p:A"))));
        assert!(days.iter().all(|x| x.unparsed == 0));
        // A slice keeps only the requested dates.
        let slice = r
            .station_data(sid, d("2020-02-28"), d("2020-03-01"))
            .await
            .unwrap();
        assert_eq!(slice.len(), 3, "2020 is a leap year");
    }
    // No fixture for a baseline picked from the StnMeta candidates: empty, not an error.
    assert!(r
        .station_data("USW00094035", d("1991-01-01"), d("2026-10-16"))
        .await
        .unwrap()
        .is_empty());
}
