//! Local standard time days (F5, 03 section 7.3). A station's rollup day is a local **standard** time day: the
//! constant `stations.std_utc_offset_minutes` is used all year, so every day is exactly 24 h and
//! `standard_date` equals SQL `app.lst_date`.

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, Offset, TimeZone, Utc};
use chrono_tz::{OffsetComponents, Tz};

/// `(ts + offset).date()`; equals `app.lst_date(ts, offset_minutes)`.
pub fn standard_date(off_minutes: i32, ts: DateTime<Utc>) -> NaiveDate {
    (ts + Duration::minutes(i64::from(off_minutes))).date_naive()
}

/// `[date 00:00 - offset, +24 h)` in UTC.
pub fn standard_day_bounds(off_minutes: i32, date: NaiveDate) -> (DateTime<Utc>, DateTime<Utc>) {
    let local_midnight: NaiveDateTime = date.and_hms_opt(0, 0, 0).expect("midnight exists");
    let start =
        Utc.from_utc_datetime(&(local_midnight - Duration::minutes(i64::from(off_minutes))));
    (start, start + Duration::hours(24))
}

/// The zone's standard (non-DST) offset from UTC in minutes at instant `at`. `None` for an unknown zone name.
pub fn base_offset_minutes(tz: &str, at: DateTime<Utc>) -> Option<i32> {
    let tz: Tz = tz.parse().ok()?;
    let off = tz.offset_from_utc_datetime(&at.naive_utc());
    Some((off.base_utc_offset().num_seconds() / 60) as i32)
}

/// Wall-clock day bounds `[local midnight, next local midnight)` of `date` in `tz` (23 h or 25 h on DST days).
/// Used only by rain reconciliation (T16). `None` for an unknown zone or a midnight that does not exist.
pub fn wall_day_bounds(tz: &str, date: NaiveDate) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let tz: Tz = tz.parse().ok()?;
    let at = |d: NaiveDate| -> Option<DateTime<Utc>> {
        let naive = d.and_hms_opt(0, 0, 0)?;
        // Earliest = the first instant the local clock showed this wall time (or, when midnight falls in a
        // gap, the first instant after it).
        let mut probe = naive;
        for _ in 0..4 {
            if let Some(t) = tz.from_local_datetime(&probe).earliest() {
                return Some(t.with_timezone(&Utc));
            }
            probe += Duration::hours(1);
        }
        None
    };
    Some((at(date)?, at(date.succ_opt()?)?))
}

/// Offset in minutes of a fixed zone as `i32`, convenient for tests.
pub fn offset_of(tz: &str, at: DateTime<Utc>) -> Option<i32> {
    let tz: Tz = tz.parse().ok()?;
    Some(
        tz.offset_from_utc_datetime(&at.naive_utc())
            .fix()
            .local_minus_utc()
            / 60,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }
    fn utc(y: i32, m: u32, day: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, day, h, mi, s).unwrap()
    }

    #[test]
    fn denver_dst_days_are_still_24h_standard_days() {
        for date in [d(2026, 3, 8), d(2026, 11, 1)] {
            let (s, e) = standard_day_bounds(-420, date);
            assert_eq!(s, utc(date.year(), date.month(), date.day(), 7, 0, 0));
            assert_eq!(e - s, Duration::hours(24));
        }
    }

    #[test]
    fn wall_days_are_23_and_25_hours_on_dst_days() {
        let (s, e) = wall_day_bounds("America/Denver", d(2026, 3, 8)).unwrap();
        assert_eq!(e - s, Duration::hours(23));
        let (s, e) = wall_day_bounds("America/Denver", d(2026, 11, 1)).unwrap();
        assert_eq!(e - s, Duration::hours(25));
        let (s, e) = wall_day_bounds("America/Denver", d(2026, 6, 1)).unwrap();
        assert_eq!(e - s, Duration::hours(24));
        assert!(wall_day_bounds("Nowhere/Land", d(2026, 6, 1)).is_none());
    }

    #[test]
    fn boundary_seconds_belong_to_the_right_date() {
        assert_eq!(
            standard_date(-420, utc(2026, 6, 2, 6, 59, 59)),
            d(2026, 6, 1)
        );
        assert_eq!(standard_date(-420, utc(2026, 6, 2, 7, 0, 0)), d(2026, 6, 2));
    }

    #[test]
    fn base_offsets() {
        let summer = utc(2026, 7, 1, 12, 0, 0);
        let winter = utc(2026, 1, 1, 12, 0, 0);
        assert_eq!(base_offset_minutes("America/Denver", summer), Some(-420));
        assert_eq!(base_offset_minutes("America/Denver", winter), Some(-420));
        assert_eq!(base_offset_minutes("America/Phoenix", summer), Some(-420));
        assert_eq!(base_offset_minutes("Australia/Sydney", summer), Some(600));
        assert_eq!(base_offset_minutes("Australia/Sydney", winter), Some(600));
        assert_eq!(base_offset_minutes("Nope/Nope", winter), None);
        // The DST-adjusted offset differs, which is exactly why the base offset is used.
        assert_eq!(offset_of("America/Denver", summer), Some(-360));
    }
}
