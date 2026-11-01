//! Types shared by the live and replay AWN clients.

use std::collections::HashMap;
use std::fmt;

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::redact;

/// An AWN user API key. `Debug` never shows the value. While the key is alive its value is in the log scrub set
/// (scoped, so the set does not grow with every connect-flow key); structural redaction (no URLs or keys in
/// error types) is the primary defence.
pub struct ApiKey {
    secret: SecretString,
    _scrub: redact::ScrubGuard,
}

impl ApiKey {
    pub fn new(key: SecretString) -> Self {
        let _scrub = redact::register_scoped(&key);
        Self {
            secret: key,
            _scrub,
        }
    }

    pub(crate) fn expose(&self) -> &str {
        self.secret.expose_secret()
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid MAC address")]
pub struct MacError;

/// Station MAC in upper-case colon form `AA:BB:CC:DD:EE:FF`. `Debug` shows only the last four digits.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Mac(String);

impl fmt::Debug for Mac {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Mac(..{})", self.last4())
    }
}

impl Mac {
    /// Accepts 12 bare hex digits, or six hex pairs joined by one consistent separator (`:` or `-`) at exactly
    /// the pair boundaries. Any case; the result is upper-case colon form.
    pub fn parse(s: &str) -> Result<Mac, MacError> {
        let t = s.trim().as_bytes();
        let hex: Vec<u8> = match t.len() {
            12 => t.to_vec(),
            17 => {
                let sep = t[2];
                if sep != b':' && sep != b'-' {
                    return Err(MacError);
                }
                let mut out = Vec::with_capacity(12);
                for (i, b) in t.iter().enumerate() {
                    if i % 3 == 2 {
                        if *b != sep {
                            return Err(MacError);
                        }
                    } else {
                        out.push(*b);
                    }
                }
                out
            }
            _ => return Err(MacError),
        };
        if !hex.iter().all(u8::is_ascii_hexdigit) {
            return Err(MacError);
        }
        let hex = String::from_utf8(hex)
            .map_err(|_| MacError)?
            .to_ascii_uppercase();
        let parts: Vec<&str> = (0..6).map(|i| &hex[i * 2..i * 2 + 2]).collect();
        Ok(Mac(parts.join(":")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Last four hex digits (for logs: "MAC only as last 4 characters").
    pub fn last4(&self) -> String {
        let digits: String = self.0.chars().filter(|c| *c != ':').collect();
        digits[digits.len() - 4..].to_owned()
    }
}

impl fmt::Display for Mac {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for Mac {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Mac {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Mac::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// One device of `GET /devices`. `Debug` hides the name and rounds coordinates (it is logged by accident easily).
#[derive(Clone, PartialEq)]
pub struct AwnDevice {
    pub mac: Mac,
    pub name: Option<String>,
    /// `(latitude, longitude)` in that order, degrees.
    pub coords: Option<(f64, f64)>,
    /// `info.coords.elevation`; field name and unit unverified until T7's first recording.
    pub elevation_ft: Option<f64>,
    /// `lastData.tz`
    pub timezone: Option<String>,
}

impl fmt::Debug for AwnDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AwnDevice")
            .field("mac", &self.mac)
            .field("name", &self.name.as_ref().map(|_| ".."))
            .field(
                "coords",
                &self
                    .coords
                    .map(|(a, b)| (format!("{a:.1}"), format!("{b:.1}"))),
            )
            .field("elevation_ft", &self.elevation_ft)
            .field("timezone", &self.timezone)
            .finish()
    }
}

/// One history row: `dateutc` (epoch ms) plus every other AWN field untouched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AwnObservation {
    pub dateutc: i64,
    #[serde(flatten)]
    pub rest: HashMap<String, Value>,
}

/// Decodes history rows. Rows without a numeric `dateutc` cannot be stored and are dropped; the count is
/// logged (`warn`) and returned so live and replay behave the same way.
pub(crate) fn decode_rows(rows: Vec<Value>) -> (Vec<AwnObservation>, usize) {
    let total = rows.len();
    let kept: Vec<AwnObservation> = rows
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect();
    let dropped = total - kept.len();
    if dropped > 0 {
        tracing::warn!(dropped, total, "dropped undecodable AWN history rows");
    }
    (kept, dropped)
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|x| x.is_finite())
}

/// Reads one element of the `/devices` array. `None` when it has no usable `macAddress`.
/// Coordinates are read from `info.coords.coords.{lat,lon}`, else `info.coords.{lat,lon}`, else
/// `info.coords.geo.coordinates` (`[lon, lat]`); the real nesting is unverified until T7.
pub fn parse_device(v: &Value) -> Option<AwnDevice> {
    let mac = Mac::parse(v.get("macAddress")?.as_str()?).ok()?;
    let info = v.get("info");
    let name = info
        .and_then(|i| i.get("name"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let c = info.and_then(|i| i.get("coords"));
    let latlon =
        |o: &Value| -> Option<(f64, f64)> { Some((num(o.get("lat")?)?, num(o.get("lon")?)?)) };
    let coords = c.and_then(|c| {
        c.get("coords")
            .and_then(latlon)
            .or_else(|| latlon(c))
            .or_else(|| {
                let g = c.get("geo")?.get("coordinates")?.as_array()?;
                Some((num(g.get(1)?)?, num(g.first()?)?))
            })
    });
    let elevation_ft = c.and_then(|c| c.get("elevation")).and_then(num);
    let timezone = v
        .get("lastData")
        .and_then(|l| l.get("tz"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    Some(AwnDevice {
        mac,
        name,
        coords,
        elevation_ft,
        timezone,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mac_parse_normalises() {
        assert_eq!(
            Mac::parse("aa:bb:cc:00:01:01").unwrap().as_str(),
            "AA:BB:CC:00:01:01"
        );
        assert_eq!(
            Mac::parse("AA-BB-CC-00-01-02").unwrap().to_string(),
            "AA:BB:CC:00:01:02"
        );
        assert_eq!(
            Mac::parse("aabbcc000103").unwrap().as_str(),
            "AA:BB:CC:00:01:03"
        );
        assert_eq!(Mac::parse("AA:BB:CC:00:01:01").unwrap().last4(), "0101");
        for bad in [
            "",
            "AA:BB:CC:00:01",
            "AA:BB:CC:00:01:0G",
            "AA:BB:CC:00:01:01:02",
            "AABB:CC:00:01:01:0",
            "AA:BB:CC:00:01:011",
            // right length, separators in the wrong places
            "AAB:BCC:00:01:01:01",
            "A:ABB:CC:00:01:01",
            "AA:BBB:C:00:01:01:",
            ":A:BB:CC:00:01:01",
            // mixed or unsupported separators
            "AA:BB-CC:00:01:01",
            "AA BB CC 00 01 01",
            "AA.BB.CC.00.01.01",
            "AA::BB:CC:00:01:01",
            // not ASCII hex
            "AA:BB:CC:00:01:0\u{e9}",
        ] {
            assert!(Mac::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn mac_and_device_debug_hide_identifying_detail() {
        let mac = Mac::parse("AA:BB:CC:00:01:01").unwrap();
        assert_eq!(format!("{mac:?}"), "Mac(..0101)");
        let d = AwnDevice {
            mac,
            name: Some("Family Home Station".into()),
            coords: Some((40.412_345, -105.098_765)),
            elevation_ft: Some(5000.0),
            timezone: Some("America/Denver".into()),
        };
        let s = format!("{d:?} {d:#?}");
        for leaked in ["AA:BB:CC:00:01:01", "Family", "40.412", "105.098"] {
            assert!(!s.contains(leaked), "{leaked} in {s}");
        }
        assert!(s.contains("40.4") && s.contains("-105.1"));
    }

    #[test]
    fn undecodable_history_rows_are_counted_and_logged() {
        use std::io::Write;
        use std::sync::{Arc, Mutex};
        #[derive(Clone, Default)]
        struct Cap(Arc<Mutex<Vec<u8>>>);
        impl Write for Cap {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Cap {
            type Writer = Cap;
            fn make_writer(&'a self) -> Cap {
                self.clone()
            }
        }
        let cap = Cap::default();
        let sub = tracing_subscriber::fmt()
            .json()
            .with_writer(cap.clone())
            .finish();
        let rows = vec![
            json!({"dateutc": 1}),
            json!({"tempf": 1}),
            json!({"dateutc": "x"}),
            json!(7),
            json!({"dateutc": 2}),
        ];
        let (kept, dropped) = tracing::subscriber::with_default(sub, || {
            // Other tests run decode_rows on parallel threads with no subscriber and can leave this callsite cached as
            // "never interested". Rebuilding the interest cache under our dispatcher makes the capture deterministic.
            tracing::callsite::rebuild_interest_cache();
            decode_rows(rows)
        });
        assert_eq!((kept.len(), dropped), (2, 3));
        let log = String::from_utf8(cap.0.lock().unwrap().clone()).unwrap();
        assert!(
            log.contains("\"dropped\":3") && log.contains("\"total\":5"),
            "{log}"
        );
    }

    #[test]
    fn api_key_debug_is_redacted() {
        let k = ApiKey::new(SecretString::from("TESTKEY-abcdef123456"));
        let s = format!("{k:?}");
        assert!(s.contains("[REDACTED]") && !s.contains("TESTKEY"));
    }

    #[test]
    fn device_parsing() {
        let d = parse_device(&json!({
            "macAddress": "aa:bb:cc:00:01:01",
            "lastData": {"tz": "America/Denver", "tempf": 50.1},
            "info": {"name": "Test", "coords": {"coords": {"lat": 40.4, "lon": -105.1}, "elevation": 5000.0}}
        }))
        .unwrap();
        assert_eq!(d.mac.as_str(), "AA:BB:CC:00:01:01");
        assert_eq!(d.coords, Some((40.4, -105.1)));
        assert_eq!(d.elevation_ft, Some(5000.0));
        assert_eq!(d.timezone.as_deref(), Some("America/Denver"));
        assert_eq!(d.name.as_deref(), Some("Test"));

        let geo = parse_device(&json!({"macAddress": "AA:BB:CC:00:01:02", "info": {"coords": {"geo": {"coordinates": [-105.1, 40.4]}}}})).unwrap();
        assert_eq!(geo.coords, Some((40.4, -105.1)));
        let flat = parse_device(&json!({"macAddress": "AA:BB:CC:00:01:02", "info": {"coords": {"lat": "40.4", "lon": "-105.1"}}})).unwrap();
        assert_eq!(flat.coords, Some((40.4, -105.1)));

        let bare = parse_device(&json!({"macAddress": "AA:BB:CC:00:01:02"})).unwrap();
        assert_eq!(
            (bare.name, bare.coords, bare.elevation_ft, bare.timezone),
            (None, None, None, None)
        );
        assert!(parse_device(&json!({"macAddress": "nope"})).is_none());
        assert!(parse_device(&json!({})).is_none());
    }

    #[test]
    fn observation_flattens_unknown_fields() {
        let o: AwnObservation = serde_json::from_value(
            json!({"dateutc": 1_700_000_000_000i64, "tempf": 41.2, "weird": "x"}),
        )
        .unwrap();
        assert_eq!(o.dateutc, 1_700_000_000_000);
        assert_eq!(o.rest.len(), 2);
    }
}
