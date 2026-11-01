//! Rain increments from cumulative counters (F6, 03 section 7.3). Pure functions, no I/O.
//!
//! The console reports cumulative counters (`totalrainin`, `yearlyrainin`, `dailyrainin`). The increment of a
//! reading is its rise above the running peak since the last reset (a high-water mark): a drop that is not a reset
//! is ignored and the climb back to the old peak is not counted twice (Rodney, Oct 31, 2026, from real data). A
//! counter reset, a drop to near zero (below [`consts::RESET_NEAR_ZERO_IN`]), counts the new value as rain from zero
//! and restarts the peak. Nothing else: no caps, glitch windows or noise thresholds (Oct 18, 2026).

use chrono::{DateTime, Utc};

/// Thresholds (all proposals, Q3).
pub mod consts {
    /// A counter that drops to a value below this is a reset (the new value counts as rain from zero).
    pub const RESET_NEAR_ZERO_IN: f64 = 0.05;
    pub const RECON_ABS_IN: f64 = 0.05;
    pub const RECON_REL: f64 = 0.10;
    /// A counter must be non-null in at least this share of a month's rows to be chosen.
    pub const COUNTER_MIN_SHARE: f64 = 0.9;
}

use consts::*;

/// Which stored counter column feeds the increments, in order of preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Counter {
    Total,
    Yearly,
    Daily,
}

impl Counter {
    pub const PREFERENCE: [Counter; 3] = [Counter::Total, Counter::Yearly, Counter::Daily];
}

/// The three stored counters of one reading.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Counters {
    pub total: Option<f64>,
    pub yearly: Option<f64>,
    pub daily: Option<f64>,
}

impl Counters {
    pub fn get(&self, c: Counter) -> Option<f64> {
        match c {
            Counter::Total => self.total,
            Counter::Yearly => self.yearly,
            Counter::Daily => self.daily,
        }
    }
}

/// Counter choice per station-month: the first of total, yearly, daily that is non-null in at least 90% of the
/// month's stored rows. `None` when no counter qualifies (or there are no rows).
pub fn choose_counter(month_rows: &[Counters]) -> Option<Counter> {
    let n = month_rows.len();
    if n == 0 {
        return None;
    }
    Counter::PREFERENCE.into_iter().find(|c| {
        let have = month_rows.iter().filter(|r| r.get(*c).is_some()).count();
        have as f64 >= COUNTER_MIN_SHARE * n as f64
    })
}

/// One reading of the series: timestamp and the value of the chosen counter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RainRow {
    pub ts: DateTime<Utc>,
    pub counter: Option<f64>,
}

/// Result per reading.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Inc {
    pub ts: DateTime<Utc>,
    /// `None` when the pair has no usable predecessor value.
    pub inc: Option<f64>,
    pub reset: bool,
}

pub fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// Increments over a contiguous, time-ordered series using the already chosen counter, starting with no history
/// before the first row. See [`increments_seeded`].
pub fn increments(rows: &[RainRow]) -> Vec<Inc> {
    increments_seeded(None, rows)
}

/// Increments with a high-water mark (Rodney, Oct 31, 2026, replacing plain differences).
///
/// The peak is the highest counter value since the last reset. Per reading with counter `c`:
/// - a drop to a value below `RESET_NEAR_ZERO_IN` is a reset: `inc = c` (rain from zero) and the peak becomes `c`;
/// - otherwise `inc = max(0, c - peak)` and the peak becomes `max(peak, c)`, so a drop that is not a reset is
///   ignored and the rise back to the old peak is not counted twice.
///
/// A reading counts only when it and the previous reading both have a counter value (the first row of the series
/// has no previous reading, so its `inc` is `None`). A reading with no counter leaves the peak unchanged.
///
/// `seed_peak` is the peak reached before `rows[0]`: for a window that starts mid-history, the maximum counter value
/// from the last reset up to and including the first row (the predecessor), see the poller's seed query. `None`
/// means no history, and the peak starts at the first row's value.
pub fn increments_seeded(seed_peak: Option<f64>, rows: &[RainRow]) -> Vec<Inc> {
    let mut out: Vec<Inc> = rows
        .iter()
        .map(|r| Inc {
            ts: r.ts,
            inc: None,
            reset: false,
        })
        .collect();
    let mut peak: Option<f64> = seed_peak.map(round3);
    let mut prev_has_value = false;
    for (i, (row, item)) in rows.iter().zip(out.iter_mut()).enumerate() {
        let Some(cur) = row.counter.map(round3) else {
            prev_has_value = false;
            continue;
        };
        match peak {
            None => peak = Some(cur),
            Some(p) if i == 0 => peak = Some(p.max(cur)),
            Some(p) => {
                let is_reset = cur < p && cur < RESET_NEAR_ZERO_IN;
                if is_reset {
                    peak = Some(cur);
                    if prev_has_value {
                        item.inc = Some(cur);
                        item.reset = true;
                    }
                } else {
                    if prev_has_value {
                        item.inc = Some(round3((cur - p).max(0.0)));
                    }
                    peak = Some(p.max(cur));
                }
            }
        }
        prev_has_value = true;
    }
    out
}

/// Reconciliation status strings of `daily_rollups.rain_reconcile_status`.
pub fn reconcile(
    console_in: f64,
    awn_daily_max: Option<f64>,
    counter_is_daily: bool,
) -> (&'static str, Option<f64>) {
    if counter_is_daily {
        return ("unchecked", None);
    }
    let Some(awn) = awn_daily_max else {
        return ("no_awn_total", None);
    };
    let diff = round3(console_in - awn);
    let tol = RECON_ABS_IN.max(RECON_REL * awn);
    if diff.abs() > tol {
        ("mismatch", Some(diff))
    } else {
        ("ok", Some(diff))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    fn series(vals: &[Option<f64>], step_min: i64) -> Vec<RainRow> {
        let t0 = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        vals.iter()
            .enumerate()
            .map(|(i, v)| RainRow {
                ts: t0 + Duration::minutes(step_min * i as i64),
                counter: *v,
            })
            .collect()
    }
    fn s(vals: &[f64]) -> Vec<RainRow> {
        series(&vals.iter().map(|v| Some(*v)).collect::<Vec<_>>(), 5)
    }
    fn incs(rows: &[RainRow]) -> Vec<Option<f64>> {
        increments(rows).iter().map(|i| i.inc).collect()
    }

    #[test]
    fn monotone_counter() {
        assert_eq!(
            incs(&s(&[0.0, 0.01, 0.01, 0.05])),
            [None, Some(0.01), Some(0.0), Some(0.04)]
        );
    }

    #[test]
    fn reset_is_detected() {
        let r = increments(&s(&[1.20, 1.25, 0.00, 0.04]));
        let v: Vec<_> = r.iter().map(|i| i.inc).collect();
        assert_eq!(v, [None, Some(0.05), Some(0.0), Some(0.04)]);
        assert_eq!(r.iter().filter(|i| i.reset).count(), 1);
        assert!(r[2].reset);
    }

    #[test]
    fn plain_differences_with_no_other_rule() {
        // Large jumps and tiny steps are taken as they are (no cap, no noise rule).
        assert_eq!(
            incs(&s(&[0.0, 5.0, 5.0004, 5.0014])),
            [None, Some(5.0), Some(0.0), Some(0.001)]
        );
        let r = increments(&s(&[0.0, 0.6, 1.0]));
        assert!(!r.iter().any(|i| i.reset));
        assert_eq!(r[1].inc, Some(0.6));
    }

    #[test]
    fn drop_to_near_zero_is_a_reset_and_the_new_value_counts() {
        // Below the threshold: reset.
        let r = increments(&s(&[3.00, 0.04, 0.10]));
        assert_eq!(
            r.iter().map(|i| i.inc).collect::<Vec<_>>(),
            [None, Some(0.04), Some(0.06)]
        );
        assert!(!r[0].reset && r[1].reset && !r[2].reset);
        // Exactly zero.
        let r = increments(&s(&[0.20, 0.0]));
        assert_eq!(r[1].inc, Some(0.0));
        assert!(r[1].reset);
        // At the threshold itself it is not near zero: not a reset, the drop is ignored (no rain, never negative).
        let r = increments(&s(&[1.00, RESET_NEAR_ZERO_IN]));
        assert!(!r[1].reset);
        assert_eq!(r[1].inc, Some(0.0));
    }

    /// The first half hour of the real station: one tip reported, retracted, then reported again.
    const REAL_FIRST_HALF_HOUR: [f64; 10] = [
        0.051, 0.051, 0.059, 0.051, 0.051, 0.063, 0.063, 0.071, 0.071, 0.083,
    ];

    #[test]
    fn real_retracted_tip_is_not_double_counted() {
        let r = increments(&s(&REAL_FIRST_HALF_HOUR));
        assert_eq!(
            r.iter().map(|i| i.inc).collect::<Vec<_>>(),
            [
                None,
                Some(0.0),
                Some(0.008),
                Some(0.0),
                Some(0.0),
                Some(0.004),
                Some(0.0),
                Some(0.008),
                Some(0.0),
                Some(0.012)
            ]
        );
        assert!(!r.iter().any(|i| i.reset));
        let total: f64 = r.iter().filter_map(|i| i.inc).sum();
        assert_eq!(round3(total), 0.032);
        assert_eq!(round3(total), round3(0.083 - 0.051));
    }

    #[test]
    fn a_reset_after_a_peak_restarts_the_peak() {
        // Peak 17.551, a dip (ignored), a reset to 0.00 (new value counts, peak restarts), then new rain.
        let r = increments(&s(&[17.50, 17.551, 17.40, 17.551, 0.00, 0.02, 0.02, 0.07]));
        assert_eq!(
            r.iter().map(|i| i.inc).collect::<Vec<_>>(),
            [
                None,
                Some(0.051),
                Some(0.0),
                Some(0.0),
                Some(0.0),
                Some(0.02),
                Some(0.0),
                Some(0.05)
            ]
        );
        assert_eq!(r.iter().filter(|i| i.reset).count(), 1);
        assert!(r[4].reset);
        // After the reset the old peak no longer matters: 0.02 -> 0.01 is a drop to near zero, a reset of its own.
        let r = increments(&s(&[5.0, 0.02, 0.01]));
        assert!(r[1].reset && r[2].reset);
        assert_eq!(r[2].inc, Some(0.01));
    }

    #[test]
    fn seeded_peak_from_before_the_window_ignores_a_dip_inside_it() {
        // The window starts at the predecessor (0.051, after an earlier peak of 0.059); the dip to 0.051 is inside.
        let rows = s(&[0.051, 0.051, 0.063, 0.063]);
        let seeded = increments_seeded(Some(0.059), &rows);
        assert_eq!(
            seeded.iter().map(|i| i.inc).collect::<Vec<_>>(),
            [None, Some(0.0), Some(0.004), Some(0.0)]
        );
        // Without the seed the predecessor's own value would be the peak and the 0.051 -> 0.059 climb would be
        // counted again by an earlier window; with it the two windows agree with one pass over the whole series.
        let whole = increments(&s(&[0.051, 0.059, 0.051, 0.051, 0.063, 0.063]));
        let tail: Vec<_> = whole[3..].iter().map(|i| i.inc).collect();
        let seeded_tail: Vec<_> = seeded[1..].iter().map(|i| i.inc).collect();
        assert_eq!(tail, seeded_tail);
        // No seed: same as `increments`.
        assert_eq!(increments_seeded(None, &rows), increments(&rows));
    }

    #[test]
    fn a_null_counter_leaves_the_peak_alone() {
        let r = incs(&series(
            &[Some(1.0), Some(1.2), None, Some(1.1), Some(1.3)],
            5,
        ));
        // the reading after the gap has no usable predecessor; the peak (1.2) survives, so 1.3 is 0.1 above it
        assert_eq!(r, [None, Some(0.2), None, None, Some(0.1)]);
    }

    #[test]
    fn reset_across_a_day_and_a_month_boundary() {
        // One series spanning 30 Jun 23:40 -> 1 Jul 00:10 in 10-minute steps; the reset lands on each boundary.
        let t0 = Utc.with_ymd_and_hms(2026, 6, 30, 23, 40, 0).unwrap();
        let mk = |vals: &[f64]| -> Vec<RainRow> {
            vals.iter()
                .enumerate()
                .map(|(i, v)| RainRow {
                    ts: t0 + Duration::minutes(10 * i as i64),
                    counter: Some(*v),
                })
                .collect()
        };
        // reset exactly at the boundary row (00:00)
        let r = increments(&mk(&[2.00, 2.10, 0.00, 0.02]));
        assert_eq!(
            r.iter().map(|i| i.inc).collect::<Vec<_>>(),
            [None, Some(0.10), Some(0.0), Some(0.02)]
        );
        assert!(r[2].reset);
        assert_eq!(r[2].ts, Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap());
        // reset one row before the boundary, with rain after it
        let r = increments(&mk(&[2.00, 0.03, 0.05, 0.09]));
        assert_eq!(
            r.iter().map(|i| i.inc).collect::<Vec<_>>(),
            [None, Some(0.03), Some(0.02), Some(0.04)]
        );
        assert!(r[1].reset && !r[2].reset);
    }

    #[test]
    fn season_total_is_last_minus_first_without_resets() {
        let vals = [0.00, 0.00, 0.01, 0.25, 0.25, 1.10, 1.11, 3.40, 3.40, 7.125];
        let r = increments(&s(&vals));
        assert!(!r.iter().any(|i| i.reset));
        let total: f64 = r.iter().filter_map(|i| i.inc).sum();
        assert_eq!(round3(total), round3(vals[vals.len() - 1] - vals[0]));
        // Same with a non-zero start.
        let vals = [12.34, 12.34, 12.50, 13.00];
        let total: f64 = increments(&s(&vals)).iter().filter_map(|i| i.inc).sum();
        assert_eq!(round3(total), 0.66);
    }

    #[test]
    fn null_ends_give_null() {
        let r = incs(&series(&[None, Some(1.0), Some(1.1), None, Some(1.2)], 5));
        assert_eq!(r, [None, None, Some(0.1), None, None]);
    }

    #[test]
    fn counter_choice_prefers_total_then_yearly_then_daily() {
        let full = |t: Option<f64>, y: Option<f64>, d: Option<f64>| Counters {
            total: t,
            yearly: y,
            daily: d,
        };
        let rows: Vec<Counters> = (0..10)
            .map(|_| full(Some(1.0), Some(1.0), Some(1.0)))
            .collect();
        assert_eq!(choose_counter(&rows), Some(Counter::Total));
        // total null in 2 of 10 rows: below 90%
        let mut rows: Vec<Counters> = (0..10)
            .map(|_| full(Some(1.0), Some(1.0), Some(1.0)))
            .collect();
        rows[0].total = None;
        rows[1].total = None;
        assert_eq!(choose_counter(&rows), Some(Counter::Yearly));
        for r in rows.iter_mut().take(2) {
            r.yearly = None;
        }
        assert_eq!(choose_counter(&rows), Some(Counter::Daily));
        for r in rows.iter_mut().take(2) {
            r.daily = None;
        }
        assert_eq!(choose_counter(&rows), None);
        // exactly 90% non-null is enough
        let mut rows: Vec<Counters> = (0..10).map(|_| full(Some(1.0), None, None)).collect();
        rows[0].total = None;
        assert_eq!(choose_counter(&rows), Some(Counter::Total));
        assert_eq!(choose_counter(&[]), None);
    }

    #[test]
    fn reconcile_statuses() {
        assert_eq!(reconcile(1.00, Some(1.02), false).0, "ok");
        assert_eq!(reconcile(1.00, Some(1.20), false).0, "mismatch");
        assert_eq!(reconcile(0.10, None, false).0, "no_awn_total");
        assert_eq!(reconcile(0.10, Some(0.10), true).0, "unchecked");
        // tolerance is max(0.05, 10% of awn)
        assert_eq!(reconcile(5.4, Some(5.0), false).0, "ok");
        assert_eq!(reconcile(5.6, Some(5.0), false).0, "mismatch");
    }
}
