//! `/ytd` (02 5.5). Daily values come from SQL (GDD only through `app.gdd_day`, one parameter set for the station and its
//! baseline); running sums, eligibility and percentiles are computed here from those daily values.

use std::collections::{BTreeMap, HashMap};

use chrono::{Datelike, NaiveDate};
use serde::Serialize;
use sqlx::{PgPool, Row};

use crate::access::ViewableStation;
use crate::error::ApiError;
use crate::extract::Params;
use crate::repo::stations::BaselineOut;

pub const PRIOR_YEARS: usize = 5;
pub const MAX_MISSING: u32 = 7;
pub const MIN_BAND_YEARS: usize = 10;
pub const PERCENTILES: [u32; 5] = [10, 25, 50, 75, 90];

#[derive(Debug, Clone, Serialize)]
pub struct GddParams {
    pub method: String,
    pub base_f: i32,
    pub start: String,
}

#[derive(Debug, Clone)]
pub struct YtdParams {
    pub year: i32,
    pub start: String,
    pub end: String,
    pub gdd: GddParams,
}

fn md_valid(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 5 || b[2] != b'-' || !b[..2].iter().chain(&b[3..]).all(u8::is_ascii_digit) {
        return false;
    }
    let (m, d) = (
        s[..2].parse::<u32>().unwrap_or(0),
        s[3..].parse::<u32>().unwrap_or(0),
    );
    NaiveDate::from_ymd_opt(2000, m, d).is_some()
}

impl YtdParams {
    /// `p` must have been parsed with [`ytd_keys`] plus `station_ids`.
    pub fn parse(p: &Params, current_year: i32) -> Result<Self, ApiError> {
        let bad = |m: &'static str| ApiError::Validation(m);
        let year: i32 = p
            .get("year")
            .ok_or(bad("year is required."))?
            .parse()
            .map_err(|_| bad("year is not valid."))?;
        if !(2000..=current_year).contains(&year) {
            return Err(bad("year is out of range."));
        }
        let start = p.get("start").unwrap_or("04-01").to_owned();
        let end = p.get("end").unwrap_or("10-31").to_owned();
        if !md_valid(&start) || !md_valid(&end) || start > end {
            return Err(bad("start and end must be MM-DD with start <= end."));
        }
        let method = p.get("gdd_method").unwrap_or("simple_avg").to_owned();
        if method != "simple_avg" && method != "clamped_50_86" {
            return Err(bad("gdd_method is not valid."));
        }
        let base_f: i32 = p
            .get("gdd_base")
            .unwrap_or("50")
            .parse()
            .map_err(|_| bad("gdd_base is not valid."))?;
        if !(32..=70).contains(&base_f) {
            return Err(bad("gdd_base must be 32 to 70."));
        }
        let gstart = p.get("gdd_start").unwrap_or("04-01").to_owned();
        if !md_valid(&gstart) || gstart > end {
            return Err(bad("gdd_start must be MM-DD and not after end."));
        }
        Ok(Self {
            year,
            start,
            end,
            gdd: GddParams {
                method,
                base_f,
                start: gstart,
            },
        })
    }
}

pub const fn ytd_keys() -> [&'static str; 6] {
    [
        "year",
        "start",
        "end",
        "gdd_method",
        "gdd_base",
        "gdd_start",
    ]
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Point {
    pub md: String,
    pub v: f64,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct BandPoint {
    pub md: String,
    pub p: [f64; 5],
}

#[derive(Debug, Serialize, Clone, Copy, PartialEq)]
pub struct Missing {
    pub rain: u32,
    pub gdd: u32,
}

#[derive(Debug, Serialize, Clone, Copy)]
pub struct YearsIncluded {
    pub rain: usize,
    pub gdd: usize,
}

#[derive(Debug, Serialize)]
pub struct Current {
    pub first_date: Option<NaiveDate>,
    pub last_date: Option<NaiveDate>,
    pub last_complete_date: Option<NaiveDate>,
    pub missing_days: Missing,
    pub rain_cum_in: Vec<Point>,
    pub gdd_cum: Vec<Point>,
}

#[derive(Debug, Serialize)]
pub struct PriorYear {
    pub year: i32,
    pub missing_days: Missing,
    pub rain_cum_in: Option<Vec<Point>>,
    pub gdd_cum: Option<Vec<Point>>,
}

#[derive(Debug, Serialize)]
pub struct Bands {
    pub percentiles: [u32; 5],
    pub years_included: YearsIncluded,
    pub rain_cum_in: Option<Vec<BandPoint>>,
    pub gdd_cum: Option<Vec<BandPoint>>,
}

#[derive(Debug, Serialize)]
pub struct StationYtd {
    pub station_id: uuid::Uuid,
    pub baseline: Option<BaselineOut>,
    pub current: Current,
    pub prior_years: Vec<PriorYear>,
    pub bands: Option<Bands>,
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

fn md_of(d: NaiveDate) -> String {
    format!("{:02}-{:02}", d.month(), d.day())
}

struct Series {
    points: Vec<Point>,
    missing: u32,
}

/// Running sum over every calendar day in `from..=to`; a missing value adds 0 and is counted. No point for 02-29.
fn series(from: NaiveDate, to: NaiveDate, vals: &HashMap<NaiveDate, Option<f64>>) -> Series {
    let (mut sum, mut missing) = (0.0, 0);
    let mut points = Vec::new();
    let mut d = from;
    while d <= to {
        match vals.get(&d).copied().flatten() {
            Some(v) => sum += v,
            None => missing += 1,
        }
        let md = md_of(d);
        if md != "02-29" {
            points.push(Point { md, v: round3(sum) });
        }
        d = d.succ_opt().expect("date in range");
    }
    Series { points, missing }
}

/// First and last day of `year` whose month-day lies in `[start, end]`.
fn window(year: i32, start: &str, end: &str) -> Option<(NaiveDate, NaiveDate)> {
    let mut first = None;
    let mut last = None;
    let mut d = NaiveDate::from_ymd_opt(year, 1, 1)?;
    while d.year() == year {
        let md = md_of(d);
        if md.as_str() >= start && md.as_str() <= end {
            first.get_or_insert(d);
            last = Some(d);
        }
        d = d.succ_opt()?;
    }
    first.zip(last)
}

/// Type-7 percentile (PostgreSQL `percentile_cont`) of a sorted slice.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    let h = (sorted.len() - 1) as f64 * p;
    let lo = h.floor() as usize;
    let hi = (lo + 1).min(sorted.len() - 1);
    sorted[lo] + (h - lo as f64) * (sorted[hi] - sorted[lo])
}

fn bands(years: &[&Vec<Point>]) -> Vec<BandPoint> {
    let mut by_md: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    for y in years {
        for p in y.iter() {
            by_md.entry(p.md.as_str()).or_default().push(p.v);
        }
    }
    by_md
        .into_iter()
        .map(|(md, mut v)| {
            v.sort_by(|a, b| a.total_cmp(b));
            let mut p = [0.0; 5];
            for (i, q) in PERCENTILES.iter().enumerate() {
                p[i] = round3(percentile(&v, f64::from(*q) / 100.0));
            }
            BandPoint {
                md: md.to_owned(),
                p,
            }
        })
        .collect()
}

struct CurrentRow {
    date: NaiveDate,
    rain: Option<f64>,
    gdd: Option<f64>,
    complete: bool,
}

async fn current(db: &PgPool, st: &ViewableStation, p: &YtdParams) -> Result<Current, sqlx::Error> {
    let lo = p.start.as_str().min(p.gdd.start.as_str());
    let rows = sqlx::query(
        "SELECT local_date, rain_in::float8 AS rain, \
         app.gdd_day(tmax_f, tmin_f, $3, $4::float8::real)::float8 AS gdd, is_complete \
         FROM app.daily_rollups WHERE station_id = $1 \
         AND local_date BETWEEN make_date($2, 1, 1) AND make_date($2, 12, 31) \
         AND to_char(local_date, 'MM-DD') BETWEEN $5 AND $6 ORDER BY local_date",
    )
    .bind(st.id())
    .bind(p.year)
    .bind(&p.gdd.method)
    .bind(f64::from(p.gdd.base_f))
    .bind(lo)
    .bind(&p.end)
    .fetch_all(db)
    .await?;
    let mut data = Vec::with_capacity(rows.len());
    for r in &rows {
        data.push(CurrentRow {
            date: r.try_get("local_date")?,
            rain: r.try_get("rain")?,
            gdd: r.try_get("gdd")?,
            complete: r.try_get("is_complete")?,
        });
    }
    let in_window = |d: &CurrentRow, start: &str| {
        let md = md_of(d.date);
        md.as_str() >= start && md.as_str() <= p.end.as_str()
    };
    let build = |start: &str,
                 pick: fn(&CurrentRow) -> Option<f64>|
     -> (Option<(NaiveDate, NaiveDate)>, Series) {
        let sel: Vec<&CurrentRow> = data.iter().filter(|d| in_window(d, start)).collect();
        match (sel.first(), sel.last()) {
            (Some(a), Some(b)) => {
                let vals: HashMap<NaiveDate, Option<f64>> =
                    sel.iter().map(|d| (d.date, pick(d))).collect();
                (Some((a.date, b.date)), series(a.date, b.date, &vals))
            }
            _ => (
                None,
                Series {
                    points: Vec::new(),
                    missing: 0,
                },
            ),
        }
    };
    let (rain_span, rain) = build(&p.start, |d| d.rain);
    let (gdd_span, gdd) = build(&p.gdd.start, |d| d.gdd);
    Ok(Current {
        first_date: rain_span.map(|s| s.0),
        last_date: rain_span.or(gdd_span).map(|s| s.1),
        last_complete_date: data.iter().filter(|d| d.complete).map(|d| d.date).max(),
        missing_days: Missing {
            rain: rain.missing,
            gdd: gdd.missing,
        },
        rain_cum_in: rain.points,
        gdd_cum: gdd.points,
    })
}

struct YearSeries {
    year: i32,
    rain: Series,
    gdd: Series,
}

async fn baseline_years(
    db: &PgPool,
    ref_id: &str,
    p: &YtdParams,
    current_year: i32,
) -> Result<Vec<YearSeries>, sqlx::Error> {
    let lo = p.start.as_str().min(p.gdd.start.as_str());
    let rows = sqlx::query(
        "SELECT date, precip_in::float8 AS precip, precip_trace, \
         app.gdd_day(tmax_f, tmin_f, $3, $4::float8::real)::float8 AS gdd \
         FROM app.reference_daily WHERE reference_station_id = $1 \
         AND (date < make_date($2, 1, 1) OR date > make_date($2, 12, 31)) \
         AND to_char(date, 'MM-DD') BETWEEN $5 AND $6",
    )
    .bind(ref_id)
    .bind(p.year)
    .bind(&p.gdd.method)
    .bind(f64::from(p.gdd.base_f))
    .bind(lo)
    .bind(&p.end)
    .fetch_all(db)
    .await?;
    let mut rain: HashMap<NaiveDate, Option<f64>> = HashMap::new();
    let mut gdd: HashMap<NaiveDate, Option<f64>> = HashMap::new();
    let mut first_year = None::<i32>;
    for r in &rows {
        let d: NaiveDate = r.try_get("date")?;
        let precip: Option<f64> = r.try_get("precip")?;
        let trace: bool = r.try_get("precip_trace")?;
        rain.insert(d, precip.or(if trace { Some(0.0) } else { None }));
        gdd.insert(d, r.try_get("gdd")?);
        first_year = Some(first_year.map_or(d.year(), |y| y.min(d.year())));
    }
    let Some(first_year) = first_year else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for year in first_year..=current_year {
        if year == p.year {
            continue;
        }
        let (Some((rf, rt)), Some((gf, gt))) = (
            window(year, &p.start, &p.end),
            window(year, &p.gdd.start, &p.end),
        ) else {
            continue;
        };
        out.push(YearSeries {
            year,
            rain: series(rf, rt, &rain),
            gdd: series(gf, gt, &gdd),
        });
    }
    Ok(out)
}

pub async fn compute(
    db: &PgPool,
    st: &ViewableStation,
    p: &YtdParams,
    current_year: i32,
) -> Result<StationYtd, sqlx::Error> {
    let cur = current(db, st, p).await?;
    let mut baseline = None;
    if let Some(rid) = st.reference_station_id() {
        let r = sqlx::query(
            "SELECT name, source, (last_synced_date IS NOT NULL) AS loaded FROM app.reference_stations WHERE id = $1",
        )
        .bind(rid)
        .fetch_optional(db)
        .await?;
        if let Some(r) = r {
            baseline = Some(BaselineOut {
                reference_station_id: rid.to_owned(),
                name: r.try_get("name")?,
                source: r.try_get("source")?,
                history_loaded: r.try_get::<Option<bool>, _>("loaded")?.unwrap_or(false),
            });
        }
    }
    let mut prior_years = Vec::new();
    let mut band_out = None;
    if let Some(b) = baseline.as_ref().filter(|b| b.history_loaded) {
        let mut years = baseline_years(db, &b.reference_station_id, p, current_year).await?;
        years.sort_by_key(|y| std::cmp::Reverse(y.year));
        let rain_ok = |y: &YearSeries| y.rain.missing <= MAX_MISSING && !y.rain.points.is_empty();
        let gdd_ok = |y: &YearSeries| y.gdd.missing <= MAX_MISSING && !y.gdd.points.is_empty();
        let rain_years: Vec<&Vec<Point>> = years
            .iter()
            .filter(|y| rain_ok(y))
            .map(|y| &y.rain.points)
            .collect();
        let gdd_years: Vec<&Vec<Point>> = years
            .iter()
            .filter(|y| gdd_ok(y))
            .map(|y| &y.gdd.points)
            .collect();
        for y in years
            .iter()
            .filter(|y| rain_ok(y) || gdd_ok(y))
            .take(PRIOR_YEARS)
        {
            let pts = |s: &Series| {
                s.points
                    .iter()
                    .map(|q| Point {
                        md: q.md.clone(),
                        v: q.v,
                    })
                    .collect()
            };
            prior_years.push(PriorYear {
                year: y.year,
                missing_days: Missing {
                    rain: y.rain.missing,
                    gdd: y.gdd.missing,
                },
                rain_cum_in: rain_ok(y).then(|| pts(&y.rain)),
                gdd_cum: gdd_ok(y).then(|| pts(&y.gdd)),
            });
        }
        let rb = (rain_years.len() >= MIN_BAND_YEARS).then(|| bands(&rain_years));
        let gb = (gdd_years.len() >= MIN_BAND_YEARS).then(|| bands(&gdd_years));
        if rb.is_some() || gb.is_some() {
            band_out = Some(Bands {
                percentiles: PERCENTILES,
                years_included: YearsIncluded {
                    rain: rain_years.len(),
                    gdd: gdd_years.len(),
                },
                rain_cum_in: rb,
                gdd_cum: gb,
            });
        }
    }
    Ok(StationYtd {
        station_id: st.id(),
        baseline,
        current: cur,
        prior_years,
        bands: band_out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type7_percentiles() {
        let v: Vec<f64> = (1..=51).map(f64::from).collect();
        assert_eq!(percentile(&v, 0.10), 6.0);
        assert_eq!(percentile(&v, 0.50), 26.0);
        assert_eq!(percentile(&v, 0.90), 46.0);
    }

    #[test]
    fn md_validation() {
        assert!(md_valid("02-29"));
        assert!(!md_valid("13-01"));
        assert!(!md_valid("4-1"));
    }

    #[test]
    fn feb29_stays_in_the_sum_without_a_point() {
        let from = NaiveDate::from_ymd_opt(2024, 2, 28).unwrap();
        let to = NaiveDate::from_ymd_opt(2024, 3, 1).unwrap();
        let vals: HashMap<_, _> = (0..4)
            .map(|i| (from + chrono::Duration::days(i), Some(1.0)))
            .collect();
        let s = series(from, to, &vals);
        let mds: Vec<_> = s.points.iter().map(|p| p.md.as_str()).collect();
        assert_eq!(mds, ["02-28", "03-01"]);
        // 02-28, 02-29 and 03-01 each add 1.0: the leap day counts without having a point.
        assert_eq!(s.points[1].v, 3.0);
    }
}
