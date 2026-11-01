//! Parsers for ACIS `StnData` and `StnMeta` responses (03 sections 7.5 and 7.5a).

use chrono::NaiveDate;
use serde_json::Value;

use super::{AcisDay, AcisError, AcisStationMeta, MAX_META_BYTES};

/// Parsed `StnData` answer.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedStationData {
    pub days: Vec<AcisDay>,
    /// Elements that could not be parsed (also counted per day).
    pub unparsed: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Elem {
    Max,
    Min,
    Precip,
}

impl Elem {
    fn letter(self) -> char {
        match self {
            Elem::Max => 'm',
            Elem::Min => 'n',
            Elem::Precip => 'p',
        }
    }
}

#[derive(Default)]
struct Cell {
    value: Option<f64>,
    trace: bool,
    /// `quality_flags` entry (without the element letter), if any.
    flag: Option<&'static str>,
    unparsed: bool,
}

fn finite(x: f64) -> Option<f64> {
    x.is_finite().then_some(x)
}

/// Parses one element: a bare string/number or `[value, flag]`.
fn parse_cell(v: &Value, elem: Elem) -> Cell {
    let (raw, flag_raw): (&Value, Option<&str>) = match v {
        Value::Array(a) => (
            a.first().unwrap_or(&Value::Null),
            a.get(1).and_then(Value::as_str),
        ),
        other => (other, None),
    };
    let mut cell = Cell::default();
    let mut text: Option<String> = match raw {
        Value::Number(n) => {
            cell.value = n.as_f64().and_then(finite);
            if cell.value.is_none() {
                cell.unparsed = true;
            }
            None
        }
        Value::String(s) => Some(s.trim().to_owned()),
        Value::Null => Some(String::new()),
        _ => {
            cell.unparsed = true;
            None
        }
    };
    let mut accumulated = flag_raw.is_some_and(|f| f.trim().eq_ignore_ascii_case("a"));
    if let Some(s) = text.take() {
        match s.as_str() {
            "" | "M" => cell.flag = Some("M"),
            "T" if elem == Elem::Precip => cell.trace = true,
            "S" if elem == Elem::Precip => cell.flag = Some("S"),
            other => {
                // "1.20", "1.20A": a number with an optional trailing accumulation letter.
                let (digits, suffix) = match other.strip_suffix(['A', 'a']) {
                    Some(d) => (d, true),
                    None => (other, false),
                };
                match digits.trim().parse::<f64>().ok().and_then(finite) {
                    Some(x) => {
                        cell.value = Some(x);
                        accumulated |= suffix;
                    }
                    None => cell.unparsed = true,
                }
            }
        }
    }
    if cell.trace {
        cell.flag = Some("T");
    }
    if accumulated && cell.value.is_some() && elem == Elem::Precip {
        cell.flag = Some("A");
    }
    if elem == Elem::Precip && cell.value.is_some_and(|x| x < 0.0) {
        cell.value = None;
        cell.unparsed = true;
    }
    cell
}

fn day_of(row: &[Value]) -> Option<AcisDay> {
    let date = NaiveDate::parse_from_str(row.first()?.as_str()?, "%Y-%m-%d").ok()?;
    let cells = [
        (
            Elem::Max,
            parse_cell(row.get(1).unwrap_or(&Value::Null), Elem::Max),
        ),
        (
            Elem::Min,
            parse_cell(row.get(2).unwrap_or(&Value::Null), Elem::Min),
        ),
        (
            Elem::Precip,
            parse_cell(row.get(3).unwrap_or(&Value::Null), Elem::Precip),
        ),
    ];
    let mut flags: Vec<String> = Vec::new();
    let mut unparsed = 0u8;
    for (e, c) in &cells {
        if let Some(f) = c.flag {
            flags.push(format!("{}:{f}", e.letter()));
        }
        if c.unparsed {
            unparsed += 1;
        }
    }
    if unparsed > 0 {
        flags.push("?".to_owned());
    }
    Some(AcisDay {
        date,
        tmax_f: cells[0].1.value,
        tmin_f: cells[1].1.value,
        precip_in: cells[2].1.value,
        precip_trace: cells[2].1.trace,
        quality_flags: (!flags.is_empty()).then(|| flags.join(",")),
        unparsed,
    })
}

/// `{"meta": {...}, "data": [["1991-01-01", maxt, mint, pcpn], ...]}` with elements in the order of the request
/// (`maxt`, `mint`, `pcpn`). Rows with an unreadable date are dropped. An `error` member or a missing `data` array
/// is `BadResponse`.
pub fn parse_station_data(body: &[u8]) -> Result<ParsedStationData, AcisError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| AcisError::BadResponse)?;
    if v.get("error").is_some() {
        return Err(AcisError::BadResponse);
    }
    let rows = v
        .get("data")
        .and_then(Value::as_array)
        .ok_or(AcisError::BadResponse)?;
    let mut days = Vec::with_capacity(rows.len());
    let mut unparsed = 0usize;
    for r in rows {
        let Some(arr) = r.as_array() else { continue };
        if let Some(d) = day_of(arr) {
            unparsed += usize::from(d.unparsed);
            days.push(d);
        }
    }
    Ok(ParsedStationData { days, unparsed })
}

fn range(v: Option<&Value>) -> Option<(NaiveDate, NaiveDate)> {
    let a = v?.as_array()?;
    let d = |x: &Value| NaiveDate::parse_from_str(x.as_str()?, "%Y-%m-%d").ok();
    Some((d(a.first()?)?, d(a.get(1)?)?))
}

/// `{"meta":[{"ll":[lon,lat],"sids":["<id> <type>",..],"elev":ft,"valid_daterange":[[s,e],..],"name":..}]}`.
pub fn parse_station_meta(body: &[u8]) -> Result<Vec<AcisStationMeta>, AcisError> {
    if body.len() > MAX_META_BYTES {
        return Err(AcisError::BadResponse);
    }
    let v: Value = serde_json::from_slice(body).map_err(|_| AcisError::BadResponse)?;
    let metas = v
        .get("meta")
        .and_then(Value::as_array)
        .ok_or(AcisError::BadResponse)?;
    let mut out = Vec::with_capacity(metas.len());
    for m in metas {
        let Some(ll) = m.get("ll").and_then(Value::as_array) else {
            continue;
        };
        let (Some(lon), Some(lat)) = (
            ll.first().and_then(Value::as_f64),
            ll.get(1).and_then(Value::as_f64),
        ) else {
            continue;
        };
        let (mut ghcn_id, mut icao, mut faa) = (None, None, None);
        for s in m
            .get("sids")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some((id, ty)) = s.as_str().and_then(|s| s.rsplit_once(' ')) else {
                continue;
            };
            let slot = match ty.trim() {
                "6" => &mut ghcn_id,
                "5" => &mut icao,
                "3" => &mut faa,
                _ => continue,
            };
            if slot.is_none() {
                *slot = Some(id.trim().to_owned());
            }
        }
        let ranges = m.get("valid_daterange").and_then(Value::as_array);
        let at = |i: usize| range(ranges.and_then(|r| r.get(i)));
        out.push(AcisStationMeta {
            ghcn_id,
            icao,
            faa,
            name: m
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            lat,
            lon,
            elevation_ft: m.get("elev").and_then(Value::as_f64),
            pcpn: at(0),
            maxt: at(1),
            mint: at(2),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn one(row: Value) -> AcisDay {
        let body = json!({"data": [row]}).to_string();
        parse_station_data(body.as_bytes()).unwrap().days.remove(0)
    }

    #[test]
    fn bare_numbers_and_strings() {
        let d = one(json!(["2020-01-02", "28", 7, "0.12"]));
        assert_eq!(
            (d.tmax_f, d.tmin_f, d.precip_in),
            (Some(28.0), Some(7.0), Some(0.12))
        );
        assert!(!d.precip_trace && d.quality_flags.is_none() && d.unparsed == 0);
    }

    #[test]
    fn value_flag_pairs_and_accumulated() {
        let d = one(json!(["2020-01-02", ["28", " "], ["7", ""], ["1.20", "A"]]));
        assert_eq!(d.precip_in, Some(1.20));
        assert_eq!(d.quality_flags.as_deref(), Some("p:A"));
        let d = one(json!(["2020-01-02", "28", "7", "1.20A"]));
        assert_eq!(
            (d.precip_in, d.quality_flags.as_deref()),
            (Some(1.20), Some("p:A"))
        );
    }

    #[test]
    fn missing_trace_and_subsequent() {
        let d = one(json!(["2020-01-02", "M", "", "T"]));
        assert_eq!((d.tmax_f, d.tmin_f, d.precip_in), (None, None, None));
        assert!(d.precip_trace);
        assert_eq!(d.quality_flags.as_deref(), Some("m:M,n:M,p:T"));
        let d = one(json!(["2020-01-02", "30", "10", "S"]));
        assert_eq!((d.precip_in, d.precip_trace), (None, false));
        assert_eq!(d.quality_flags.as_deref(), Some("p:S"));
        let d = one(json!(["2020-01-02", "30", "10", "M"]));
        assert_eq!(d.quality_flags.as_deref(), Some("p:M"));
    }

    #[test]
    fn malformed_values_are_null_and_flagged() {
        let d = one(json!(["2020-01-02", "abc", "7", "-1"]));
        assert_eq!((d.tmax_f, d.tmin_f, d.precip_in), (None, Some(7.0), None));
        assert_eq!(d.unparsed, 2);
        assert_eq!(d.quality_flags.as_deref(), Some("?"));
        // a temperature "T" is not trace
        let d = one(json!(["2020-01-02", "T", "7", "0"]));
        assert_eq!(d.tmax_f, None);
        assert!(!d.precip_trace && d.unparsed == 1);
    }

    #[test]
    fn rows_with_bad_dates_are_dropped_and_errors_rejected() {
        let body =
            json!({"data": [["nope", "1", "1", "1"], ["2020-01-03", "1", "1", "0"]]}).to_string();
        assert_eq!(parse_station_data(body.as_bytes()).unwrap().days.len(), 1);
        assert_eq!(
            parse_station_data(b"{\"error\":\"bad sid\"}"),
            Err(AcisError::BadResponse)
        );
        assert_eq!(parse_station_data(b"{}"), Err(AcisError::BadResponse));
        assert_eq!(parse_station_data(b"<html>"), Err(AcisError::BadResponse));
    }

    const FNL: &str = r#"{"meta":[{"ll":[-105.01667,40.45],"sids":["94035 1","FNL 3","KFNL 5","USW00094035 6","USW00094035 32"],"elev":5015.0,
        "valid_daterange":[["2011-10-01","2026-10-05"],["2011-10-01","2026-10-06"],["2011-10-01","2026-10-06"]],"name":"FORT COLLINS LOVELAND AP"},
        {"ll":[-105.1,40.3],"sids":["XYZ 10"],"elev":5200,"valid_daterange":[["2015-01-01","2026-10-01"],[],[]],"name":"RAIN ONLY"},
        {"sids":["NOLL 6"],"name":"no coordinates"}]}"#;

    #[test]
    fn station_meta() {
        let m = parse_station_meta(FNL.as_bytes()).unwrap();
        assert_eq!(m.len(), 2, "an entry without ll is skipped");
        let f = &m[0];
        assert_eq!(f.ghcn_id.as_deref(), Some("USW00094035"));
        assert_eq!(
            (f.icao.as_deref(), f.faa.as_deref()),
            (Some("KFNL"), Some("FNL"))
        );
        assert_eq!(f.elevation_ft, Some(5015.0));
        assert_eq!((f.lat, f.lon), (40.45, -105.01667));
        let d = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        assert_eq!(f.pcpn, Some((d("2011-10-01"), d("2026-10-05"))));
        assert_eq!(f.maxt, Some((d("2011-10-01"), d("2026-10-06"))));
        assert_eq!(f.mint, Some((d("2011-10-01"), d("2026-10-06"))));
        let r = &m[1];
        assert!(r.pcpn.is_some() && r.maxt.is_none() && r.mint.is_none());
        assert_eq!(r.ghcn_id, None, "type 10 (CoCoRaHS) is ignored");
    }

    #[test]
    fn empty_and_bad_meta() {
        assert_eq!(parse_station_meta(b"{\"meta\":[]}").unwrap(), vec![]);
        assert_eq!(parse_station_meta(b"{}"), Err(AcisError::BadResponse));
        assert_eq!(parse_station_meta(b"nope"), Err(AcisError::BadResponse));
        let big = vec![b' '; MAX_META_BYTES + 1];
        assert_eq!(parse_station_meta(&big), Err(AcisError::BadResponse));
    }
}
