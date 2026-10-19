//! Rain increments from cumulative counters (F6, 03 section 7.3). Pure functions, no I/O.
//!
//! The console reports cumulative counters (`totalrainin`, `yearlyrainin`, `dailyrainin`). The increment of a
//! reading is the difference to the previous reading, with three corrections: counter resets (the counter drops
//! and stays low), glitches (a short dip that recovers), and impossible jumps (capped).

use chrono::{DateTime, Utc};

/// Thresholds (all proposals, Q3).
pub mod consts {
    pub const MAX_INC_PER_5MIN_IN: f64 = 0.5;
    pub const MAX_INC_ABS_IN: f64 = 4.0;
    pub const GLITCH_MIN_DROP_IN: f64 = 0.05;
    pub const GLITCH_LOOKAHEAD_READINGS: usize = 6;
    pub const NOISE_IN: f64 = 0.0005;
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
    pub glitch: bool,
    pub capped: bool,
}

pub fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

fn cap_for(gap: chrono::Duration) -> f64 {
    let five = chrono::Duration::minutes(5).num_milliseconds() as f64;
    let steps = (gap.num_milliseconds().max(0) as f64 / five)
        .ceil()
        .max(1.0);
    (MAX_INC_PER_5MIN_IN * steps).min(MAX_INC_ABS_IN)
}

/// Increments over a contiguous, time-ordered series using the already chosen counter.
/// A pair contributes only when both rows have a counter value.
pub fn increments(rows: &[RainRow]) -> Vec<Inc> {
    let n = rows.len();
    let mut out: Vec<Inc> = rows
        .iter()
        .map(|r| Inc {
            ts: r.ts,
            inc: None,
            reset: false,
            glitch: false,
            capped: false,
        })
        .collect();
    let val = |i: usize| rows[i].counter.map(round3);
    let mut i = 1;
    while i < n {
        let (Some(prev), Some(cur)) = (val(i - 1), val(i)) else {
            i += 1;
            continue;
        };
        let d = round3(cur - prev);
        let gap = rows[i].ts - rows[i - 1].ts;
        if d.abs() < NOISE_IN {
            out[i].inc = Some(0.0);
        } else if d >= 0.0 {
            out[i].inc = Some(d);
            apply_cap(&mut out[i], gap);
        } else {
            // Counter dropped.
            let glitch_end = if -d >= GLITCH_MIN_DROP_IN {
                (i + 1..n.min(i + 1 + GLITCH_LOOKAHEAD_READINGS))
                    .find(|&j| val(j).is_some_and(|c| c >= prev - NOISE_IN))
            } else {
                None
            };
            if let Some(j) = glitch_end {
                for item in out.iter_mut().take(j).skip(i) {
                    item.inc = Some(0.0);
                    item.glitch = true;
                }
                let cj = val(j).expect("checked above");
                out[j].inc = Some(round3((cj - prev).max(0.0)));
                // The recovery row carries `c_j - c_{i-1}`, a difference over the whole glitch, so the cap is
                // measured over the same span (03 7.3 defines the gap per pair; this is the reading that keeps
                // real rain that fell during a glitch).
                let span = rows[j].ts - rows[i - 1].ts;
                apply_cap(&mut out[j], span);
                i = j + 1;
                continue;
            }
            out[i].inc = Some(cur.max(0.0));
            out[i].reset = true;
            apply_cap(&mut out[i], gap);
        }
        i += 1;
    }
    out
}

fn apply_cap(item: &mut Inc, gap: chrono::Duration) {
    if let Some(v) = item.inc {
        if v > cap_for(gap) {
            item.inc = Some(0.0);
            item.capped = true;
        }
    }
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
    fn glitch_that_recovers_is_not_rain_and_not_a_reset() {
        let r = increments(&s(&[1.20, 0.30, 1.20]));
        let v: Vec<_> = r.iter().map(|i| i.inc).collect();
        assert_eq!(v, [None, Some(0.0), Some(0.0)]);
        assert!(r[1].glitch && !r.iter().any(|i| i.reset));
    }

    #[test]
    fn glitch_that_recovers_higher_counts_the_difference() {
        assert_eq!(incs(&s(&[1.20, 0.30, 1.25])), [None, Some(0.0), Some(0.05)]);
    }

    #[test]
    fn rain_that_fell_during_a_glitch_is_capped_against_the_whole_span() {
        // 0.6 in over 15 minutes (three 5-minute steps): within the 1.5 in cap of the span, not zeroed.
        let r = increments(&s(&[1.00, 0.30, 0.30, 1.60]));
        assert_eq!(r[3].inc, Some(0.6));
        assert!(!r[3].capped && r[1].glitch && r[2].glitch);
        // Still capped when it exceeds the span's cap.
        let r = increments(&s(&[1.00, 0.30, 0.30, 3.00]));
        assert!(r[3].capped && r[3].inc == Some(0.0));
    }

    #[test]
    fn small_drop_is_a_reset() {
        let r = increments(&s(&[1.00, 0.97, 0.97]));
        assert!(r[1].reset, "{r:?}");
    }

    #[test]
    fn drop_that_never_returns_within_six_readings_is_a_reset() {
        // 7 low readings then recovery: lookahead of 6 does not see it.
        let mut vals = vec![1.20];
        vals.extend([0.30; 7]);
        vals.push(1.20);
        let r = increments(&s(&vals));
        assert!(r[1].reset && !r[1].glitch);
        // Recovery within 6 readings is a glitch.
        let mut vals = vec![1.20];
        vals.extend([0.30; 5]);
        vals.push(1.20);
        let r = increments(&s(&vals));
        assert!(r[1].glitch && !r[1].reset, "{r:?}");
    }

    #[test]
    fn jumps_are_capped() {
        let r = increments(&s(&[0.0, 5.0]));
        assert_eq!(r[1].inc, Some(0.0));
        assert!(r[1].capped);
        let r = increments(&s(&[0.0, 0.6]));
        assert!(r[1].capped);
        let r = increments(&s(&[0.0, 0.5]));
        assert!(!r[1].capped && r[1].inc == Some(0.5));
        // 4.0 in over a 4 h gap: cap min(0.5 * 48, 4.0) = 4.0, accepted.
        let r = increments(&series(&[Some(0.0), Some(4.0)], 240));
        assert!(!r[1].capped && r[1].inc == Some(4.0));
        let r = increments(&series(&[Some(0.0), Some(4.5)], 240));
        assert!(r[1].capped);
    }

    #[test]
    fn null_ends_give_null() {
        let r = incs(&series(&[None, Some(1.0), Some(1.1), None, Some(1.2)], 5));
        assert_eq!(r, [None, None, Some(0.1), None, None]);
    }

    #[test]
    fn noise_is_ignored() {
        assert_eq!(incs(&s(&[1.0, 1.0004, 1.0])), [None, Some(0.0), Some(0.0)]);
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
