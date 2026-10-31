//! Accumulated precipitation from "now" (02 5.6, proposal 07 section 3). Pure.
//!
//! The deterministic total is the running sum of its own hourly series. The ensemble total is built per member
//! first and only then given to [`super::percentiles::hourly`]; percentiles of hourly sums are never summed.

use chrono::{DateTime, Utc};

/// Index of the first hourly step not in the past: the first step at or after the start of the current UTC hour, so
/// the hour in progress is included. `times.len()` when every step is in the past.
pub fn start_step(times: &[DateTime<Utc>], now: DateTime<Utc>) -> usize {
    let secs = now.timestamp();
    let hour_start = secs - secs.rem_euclid(3600);
    times
        .iter()
        .position(|t| t.timestamp() >= hour_start)
        .unwrap_or(times.len())
}

/// Running sum of one hourly series from `start`. `out[i]` is the total of the steps `start..=i`. It is `None` before
/// `start`, and at a step whose own value is missing (the sum carries on past it).
pub fn running_sum(series: &[Option<f64>], start: usize) -> Vec<Option<f64>> {
    let mut total = 0.0;
    series
        .iter()
        .enumerate()
        .map(|(i, v)| match v {
            Some(x) if i >= start => {
                total += x;
                Some(total)
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openmeteo::percentiles::hourly;
    use chrono::TimeZone;

    fn hours(start: DateTime<Utc>, n: i64) -> Vec<DateTime<Utc>> {
        (0..n).map(|h| start + chrono::Duration::hours(h)).collect()
    }

    #[test]
    fn starts_at_the_hour_in_progress() {
        let t0 = Utc.with_ymd_and_hms(2026, 10, 17, 6, 0, 0).unwrap();
        let times = hours(t0, 24);
        // 12:30 is inside the 12:00 step, which is still accumulating: it is the first step counted.
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 30, 0).unwrap();
        assert_eq!(start_step(&times, now), 6);
        // Exactly on the hour: that step is counted.
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        assert_eq!(start_step(&times, now), 6);
        // Before the series: the first step. After it: none.
        let early = Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap();
        assert_eq!(start_step(&times, early), 0);
        let late = Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap();
        assert_eq!(start_step(&times, late), times.len());
    }

    #[test]
    fn running_sum_starts_at_now_and_skips_missing_steps() {
        let s = vec![Some(0.1), Some(0.2), None, Some(0.3), Some(9.0)];
        let out = running_sum(&s, 1);
        assert_eq!(out[0], None, "before now");
        assert!((out[1].unwrap() - 0.2).abs() < 1e-12);
        assert_eq!(out[2], None, "a missing step has no total");
        assert!(
            (out[3].unwrap() - 0.5).abs() < 1e-12,
            "the sum carries on past it"
        );
        assert!((out[4].unwrap() - 9.5).abs() < 1e-12);
    }

    /// Hand-built: three members, two steps. Member sums are A = [1, 1], B = [0, 1], C = [0, 0].
    /// The median of the member totals at step 1 is 1.0. The median of the hourly values at step 1 is 0.0 and the
    /// median at step 0 is 0.0, so summing hourly percentiles gives 0.0. The two methods must differ.
    #[test]
    fn per_member_sums_differ_from_summing_hourly_percentiles() {
        let a = vec![Some(1.0), Some(0.0)];
        let b = vec![Some(0.0), Some(1.0)];
        let c = vec![Some(0.0), Some(0.0)];
        let members = vec![a.clone(), b.clone(), c.clone()];

        // The method used by the API: accumulate each member, then take percentiles across members.
        let accum: Vec<Vec<Option<f64>>> = members.iter().map(|m| running_sum(m, 0)).collect();
        let [_, per_member_p50, _] = hourly(&accum, 2);
        assert_eq!(per_member_p50, vec![Some(0.0), Some(1.0)]);

        // The rejected method: percentiles of the hourly values, then summed along the time axis.
        let [_, hourly_p50, _] = hourly(&members, 2);
        let summed: f64 = hourly_p50.iter().map(|x| x.unwrap()).sum();
        assert_eq!(summed, 0.0);
        assert_ne!(per_member_p50[1].unwrap(), summed);
    }

    #[test]
    fn member_totals_keep_the_80_percent_rule() {
        // 51 members, all dry at step 0; the last step is null for all members, so the band is null there.
        let members: Vec<Vec<Option<f64>>> =
            (0..51).map(|_| vec![Some(0.0), Some(0.1), None]).collect();
        let accum: Vec<Vec<Option<f64>>> = members.iter().map(|m| running_sum(m, 0)).collect();
        let [p10, p50, p90] = hourly(&accum, 3);
        assert_eq!(p10[0], Some(0.0));
        assert!((p50[1].unwrap() - 0.1).abs() < 1e-12);
        assert!((p90[1].unwrap() - 0.1).abs() < 1e-12);
        assert_eq!(p10[2], None);
    }
}
