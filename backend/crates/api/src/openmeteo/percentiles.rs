//! Ensemble percentiles (02 5.6): pure, type-7, unit-tested.

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;

use crate::repo::ytd::percentile;

/// A step is emitted only when at least this share of the series has a value.
pub const MIN_SHARE: f64 = 0.8;

/// p10, p50, p90 of the values at one step; `None` when fewer than 80 percent of the series have a value.
pub fn p10_50_90(values: &[Option<f64>]) -> Option<[f64; 3]> {
    let mut v: Vec<f64> = values
        .iter()
        .flatten()
        .copied()
        .filter(|x| x.is_finite())
        .collect();
    if values.is_empty() || (v.len() as f64) / (values.len() as f64) < MIN_SHARE {
        return None;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    Some([
        percentile(&v, 0.1),
        percentile(&v, 0.5),
        percentile(&v, 0.9),
    ])
}

/// Per-step percentiles across members. `members[m][i]` is member m at step i.
pub fn hourly(members: &[Vec<Option<f64>>], steps: usize) -> [Vec<Option<f64>>; 3] {
    let mut out = [vec![None; steps], vec![None; steps], vec![None; steps]];
    for i in 0..steps {
        let col: Vec<Option<f64>> = members
            .iter()
            .map(|m| m.get(i).copied().flatten())
            .collect();
        if let Some(p) = p10_50_90(&col) {
            for (series, v) in out.iter_mut().zip(p) {
                series[i] = Some(v);
            }
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemberDay {
    pub tmax: f64,
    pub tmin: f64,
    pub precip: f64,
}

/// Real length of local day `d` in whole hours (24 normally).
pub fn hours_in_day(d: NaiveDate, tz: Tz) -> Option<usize> {
    use chrono::TimeZone;
    let start = tz
        .from_local_datetime(&d.and_hms_opt(0, 0, 0)?)
        .earliest()?;
    let end = tz
        .from_local_datetime(&d.succ_opt()?.and_hms_opt(0, 0, 0)?)
        .earliest()?;
    usize::try_from((end - start).num_hours()).ok()
}

/// One member's complete local days: every timestamp of the day has both values.
pub fn member_days(
    times: &[DateTime<Utc>],
    temp: &[Option<f64>],
    precip: &[Option<f64>],
    tz: Tz,
) -> BTreeMap<NaiveDate, MemberDay> {
    // date -> (hours present in `times`, temps, precips)
    let mut by_day: BTreeMap<NaiveDate, (usize, Vec<f64>, Vec<f64>)> = BTreeMap::new();
    for (i, t) in times.iter().enumerate() {
        let d = t.with_timezone(&tz).date_naive();
        let e = by_day.entry(d).or_default();
        e.0 += 1;
        if let (Some(Some(a)), Some(Some(p))) = (temp.get(i), precip.get(i)) {
            e.1.push(*a);
            e.2.push(*p);
        }
    }
    by_day
        .into_iter()
        // A member-day needs every hour of the local day: 24, or 23 or 25 on the two DST change days.
        .filter(|(d, (n, t, _))| Some(*n) == hours_in_day(*d, tz) && t.len() == *n)
        .map(|(d, (_, t, p))| {
            (
                d,
                MemberDay {
                    tmax: t.iter().copied().fold(f64::MIN, f64::max),
                    tmin: t.iter().copied().fold(f64::MAX, f64::min),
                    precip: p.iter().sum(),
                },
            )
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct DayBand {
    pub date: NaiveDate,
    pub tmax: [f64; 3],
    pub tmin: [f64; 3],
    pub precip: [f64; 3],
    pub prob_precip: u32,
}

/// Daily values are computed per member first, then across members.
pub fn daily(members: &[BTreeMap<NaiveDate, MemberDay>]) -> Vec<DayBand> {
    let mut dates: Vec<NaiveDate> = members.iter().flat_map(|m| m.keys().copied()).collect();
    dates.sort();
    dates.dedup();
    let mut out = Vec::new();
    for d in dates {
        let days: Vec<&MemberDay> = members.iter().filter_map(|m| m.get(&d)).collect();
        if (days.len() as f64) / (members.len() as f64) < MIN_SHARE {
            continue;
        }
        let col = |f: fn(&MemberDay) -> f64| -> Option<[f64; 3]> {
            p10_50_90(&days.iter().map(|x| Some(f(x))).collect::<Vec<_>>())
        };
        let (Some(tmax), Some(tmin), Some(precip)) =
            (col(|x| x.tmax), col(|x| x.tmin), col(|x| x.precip))
        else {
            continue;
        };
        let wet = days.iter().filter(|x| x.precip >= 0.01).count();
        out.push(DayBand {
            date: d,
            tmax,
            tmin,
            precip,
            prob_precip: (100.0 * wet as f64 / days.len() as f64).round() as u32,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn type7_at_a_step() {
        let v: Vec<Option<f64>> = (1..=51).map(|x| Some(f64::from(x))).collect();
        assert_eq!(p10_50_90(&v), Some([6.0, 26.0, 46.0]));
    }

    #[test]
    fn eighty_percent_rule() {
        let mut v: Vec<Option<f64>> = vec![None; 51];
        for x in v.iter_mut().take(41) {
            *x = Some(1.0);
        }
        assert!(p10_50_90(&v).is_some());
        v[40] = None;
        assert!(p10_50_90(&v).is_none());
    }

    #[test]
    fn all_null_final_hours_are_none() {
        let members = vec![vec![Some(1.0), None]; 51];
        let [p10, _, _] = hourly(&members, 2);
        assert_eq!(p10, vec![Some(1.0), None]);
    }

    #[test]
    fn daily_sums_per_member_then_percentiles() {
        let tz: Tz = "America/Denver".parse().unwrap();
        let start = tz
            .with_ymd_and_hms(2026, 10, 7, 0, 0, 0)
            .unwrap()
            .with_timezone(&Utc);
        let times: Vec<DateTime<Utc>> = (0..24)
            .map(|h| start + chrono::Duration::hours(h))
            .collect();
        // Member A rains 1.0 in one hour; member B rains 0.1 in each of 24 hours (total 2.4); C is dry.
        let a: Vec<Option<f64>> = (0..24)
            .map(|h| Some(if h == 3 { 1.0 } else { 0.0 }))
            .collect();
        let b = vec![Some(0.1); 24];
        let c = vec![Some(0.0); 24];
        let temp = vec![Some(50.0); 24];
        let ms: Vec<_> = [a, b, c]
            .iter()
            .map(|p| member_days(&times, &temp, p, tz))
            .collect();
        let d = daily(&ms);
        assert_eq!(d.len(), 1);
        // sorted totals 0.0, 1.0, 2.4 -> median 1.0 (the median of hourly medians would be 0.0)
        assert!((d[0].precip[1] - 1.0).abs() < 1e-9);
        assert_eq!(d[0].prob_precip, 67);
    }

    #[test]
    fn a_member_with_a_missing_hour_is_excluded_and_truncated_days_drop() {
        let tz: Tz = "America/Denver".parse().unwrap();
        let start = tz
            .with_ymd_and_hms(2026, 10, 7, 0, 0, 0)
            .unwrap()
            .with_timezone(&Utc);
        let times: Vec<DateTime<Utc>> = (0..30)
            .map(|h| start + chrono::Duration::hours(h))
            .collect();
        let mut t = vec![Some(40.0); 30];
        t[5] = None;
        let p = vec![Some(0.0); 30];
        let m = member_days(&times, &t, &p, tz);
        assert!(m.is_empty(), "day 1 has a hole, day 2 is truncated");
    }

    #[test]
    fn an_ordinary_day_needs_all_24_hours_but_dst_days_need_their_real_length() {
        let tz: Tz = "America/Denver".parse().unwrap();
        let day = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();
        assert_eq!(hours_in_day(day(2026, 10, 7), tz), Some(24));
        assert_eq!(hours_in_day(day(2026, 11, 1), tz), Some(25));
        assert_eq!(hours_in_day(day(2026, 3, 8), tz), Some(23));
        let local = |y, m, d| {
            tz.with_ymd_and_hms(y, m, d, 0, 0, 0)
                .unwrap()
                .with_timezone(&Utc)
        };
        let vals = |n: usize| vec![Some(1.0); n];
        // A 23-hour run on an ordinary day is incomplete.
        let start = local(2026, 10, 7);
        let times: Vec<_> = (0..23)
            .map(|h| start + chrono::Duration::hours(h))
            .collect();
        assert!(member_days(&times, &vals(23), &vals(23), tz).is_empty());
        // 24 hours is complete.
        let times: Vec<_> = (0..24)
            .map(|h| start + chrono::Duration::hours(h))
            .collect();
        assert_eq!(member_days(&times, &vals(24), &vals(24), tz).len(), 1);
        // The 25-hour fall-back day is complete with 25 steps and incomplete with 24.
        let start = local(2026, 11, 1);
        let times: Vec<_> = (0..25)
            .map(|h| start + chrono::Duration::hours(h))
            .collect();
        assert_eq!(member_days(&times, &vals(25), &vals(25), tz).len(), 1);
        assert!(member_days(&times[..24], &vals(24), &vals(24), tz).is_empty());
    }
}
