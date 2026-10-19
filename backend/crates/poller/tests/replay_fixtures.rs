//! The committed AWN fixtures through `ReplayAwn` (03 section 10): depth, merged series, paging and time shifting.

mod common;

use std::collections::HashSet;
use std::sync::Arc;

use chrono::{Duration, TimeZone, Utc};
use common::{committed_fixtures, now};
use famweather_core::awn::{ApiKey, AwnApi, Mac, ReplayAwn, REPLAY_POLLER_KEY};
use famweather_core::clock::{Clock, FixedClock};
use secrecy::SecretString;

fn replay() -> ReplayAwn {
    let clock: Arc<dyn Clock> = Arc::new(FixedClock::new(now()));
    ReplayAwn::new(committed_fixtures(), clock).unwrap()
}

fn key() -> ApiKey {
    ApiKey::new(SecretString::from(REPLAY_POLLER_KEY))
}

#[tokio::test]
async fn the_poller_key_lists_the_three_seeded_devices_with_fuzzed_coordinates() {
    let r = replay();
    let devices = r.list_devices(&key()).await.unwrap();
    let macs: Vec<&str> = devices.iter().map(|d| d.mac.as_str()).collect();
    assert_eq!(
        macs,
        [
            "AA:BB:CC:00:00:01",
            "AA:BB:CC:00:00:02",
            "AA:BB:CC:00:00:03"
        ]
    );
    for d in &devices {
        let (lat, lon) = d.coords.expect("coordinates");
        assert!(
            ((lat * 10.0).round() - lat * 10.0).abs() < 1e-6
                && ((lon * 10.0).round() - lon * 10.0).abs() < 1e-6,
            "one decimal: {lat} {lon}"
        );
        assert_eq!(d.timezone.as_deref(), Some("America/Denver"));
    }
}

#[tokio::test]
async fn paging_a_station_walks_62_days_newest_first_then_ends_with_an_empty_page() {
    let r = replay();
    let mac = Mac::parse("AA:BB:CC:00:00:02").unwrap();
    let newest_expected = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap(); // floor_5min(12:03:30)
    let mut end = None;
    let mut seen = HashSet::new();
    let mut total = 0usize;
    let mut first_page_newest = None;
    let mut oldest_overall = i64::MAX;
    let mut pages = 0;
    loop {
        let page = r.history(&key(), &mac, end, 288).await.unwrap();
        if page.is_empty() {
            break;
        }
        pages += 1;
        assert!(page.len() <= 288);
        assert!(
            page.windows(2).all(|w| w[0].dateutc > w[1].dateutc),
            "newest first"
        );
        first_page_newest.get_or_insert(page[0].dateutc);
        for o in &page {
            assert!(seen.insert(o.dateutc), "no duplicates across pages");
            assert_eq!(o.dateutc % 300_000, 0, "5-minute grid");
        }
        total += page.len();
        oldest_overall = page.last().unwrap().dateutc;
        end = Some(Utc.timestamp_millis_opt(oldest_overall).unwrap() - Duration::milliseconds(1));
    }
    assert_eq!(first_page_newest, Some(newest_expected.timestamp_millis()));
    assert_eq!(pages, 62);
    assert_eq!(total, 62 * 288, "62 days of 5-minute data");
    let span_days = (newest_expected.timestamp_millis() - oldest_overall) / 86_400_000;
    assert!(span_days >= 60, "at least 60 days of history: {span_days}");
}

#[tokio::test]
async fn other_keys_are_unauthorized_and_unknown_stations_are_empty() {
    let r = replay();
    let bad = ApiKey::new(SecretString::from("some-other-key"));
    let mac = Mac::parse("AA:BB:CC:00:00:01").unwrap();
    assert!(r.history(&bad, &mac, None, 10).await.is_err());
    let unknown = Mac::parse("AA:BB:CC:00:00:09").unwrap();
    assert!(r
        .history(&key(), &unknown, None, 10)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn the_small_scenario_loads_from_its_committed_path() {
    let clock: Arc<dyn Clock> = Arc::new(FixedClock::new(
        Utc.with_ymd_and_hms(2026, 6, 1, 12, 3, 0).unwrap(),
    ));
    let r = ReplayAwn::new(committed_fixtures().join("test/small"), clock).unwrap();
    let mac = Mac::parse("AA:BB:CC:00:00:01").unwrap();
    let rows = r.history(&key(), &mac, None, 288).await.unwrap();
    assert_eq!(rows.len(), 6);
    assert_eq!(
        rows[0].dateutc, 1_780_315_200_000,
        "shift is zero when 'now' floors to the newest row"
    );
}
