//! Rain increments from cumulative counters (F6, 03 section 7.3). Pure functions, no I/O.
//!
//! The console reports cumulative counters (`totalrainin`, `yearlyrainin`, `dailyrainin`). The increment of a
//! reading is the plain difference to the previous reading. The one exception is a counter reset: when the counter
//! drops to near zero (below [`consts::RESET_NEAR_ZERO_IN`]) the new value counts as rain from zero and the drop
//! itself is not counted. Nothing else (Rodney, Oct 18, 2026).

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

/// Increments over a contiguous, time-ordered series using the already chosen counter.
/// A pair contributes only when both rows have a counter value. The increment is `cur - prev`; a drop to a value
/// below `RESET_NEAR_ZERO_IN` is a reset and yields `cur` (rain from zero).
pub fn increments(rows: &[RainRow]) -> Vec<Inc> {
    let mut out: Vec<Inc> = rows
        .iter()
        .map(|r| Inc {
            ts: r.ts,
            inc: None,
            reset: false,
        })
        .collect();
    for (pair, item) in rows.windows(2).zip(out.iter_mut().skip(1)) {
        let (Some(prev), Some(cur)) = (pair[0].counter.map(round3), pair[1].counter.map(round3))
        else {
            continue;
        };
        if cur < prev && cur < RESET_NEAR_ZERO_IN {
            item.inc = Some(cur);
            item.reset = true;
        } else {
            item.inc = Some(round3(cur - prev));
        }
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
        // At the threshold itself it is not near zero: plain (negative) difference, not a reset.
        let r = increments(&s(&[1.00, RESET_NEAR_ZERO_IN]));
        assert!(!r[1].reset);
        assert_eq!(r[1].inc, Some(round3(RESET_NEAR_ZERO_IN - 1.0)));
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
