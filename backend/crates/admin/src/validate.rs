//! Input validation and normalisation shared by the commands.

use anyhow::{bail, Result};
use famweather_core::awn::Mac;

/// Email syntax: one `@`, a non-empty local part, a dotted domain, no whitespace or control characters, at most
/// 254 bytes. Deliberately simple: GoTrue is the authority on deliverability.
pub fn email(s: &str) -> Result<String> {
    let t = s.trim();
    let ok = t.len() <= 254
        && !t.chars().any(|c| c.is_whitespace() || c.is_control())
        && t.matches('@').count() == 1
        && t.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && local.len() <= 64
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
                && !domain.contains("..")
        });
    if !ok {
        bail!("invalid email address");
    }
    Ok(t.to_owned())
}

/// 1 to 80 characters after trimming (mirrors the `app.users` CHECK).
pub fn display_name(s: &str) -> Result<String> {
    let t = s.trim();
    let n = t.chars().count();
    if !(1..=80).contains(&n) || t.chars().any(char::is_control) {
        bail!("display name must be 1 to 80 characters");
    }
    Ok(t.to_owned())
}

/// Upper-case colon form `AA:BB:CC:DD:EE:FF`; anything malformed is rejected.
pub fn mac(s: &str) -> Result<String> {
    match Mac::parse(s) {
        Ok(m) => Ok(m.to_string()),
        Err(_) => bail!("invalid MAC address"),
    }
}

/// Coordinates are persisted rounded to 2 decimals (S10); the column also rounds.
pub fn coordinate(value: f64, limit: f64, name: &str) -> Result<f64> {
    if !value.is_finite() || value.abs() > limit {
        bail!("{name} must be between -{limit} and {limit}");
    }
    Ok((value * 100.0).round() / 100.0)
}

pub fn timezone(s: &str) -> Result<String> {
    if s.parse::<chrono_tz::Tz>().is_err() {
        bail!("unknown IANA time zone");
    }
    Ok(s.to_owned())
}

/// Standard (non-DST) offset from UTC in minutes, as the `stations` CHECK allows.
pub fn std_offset(minutes: i16) -> Result<i16> {
    if !(-720..=840).contains(&minutes) {
        bail!("standard UTC offset must be between -720 and 840 minutes");
    }
    Ok(minutes)
}

/// AWN reports elevation to the foot; the schema stores it rounded to 10 ft (04 5.5).
pub fn elevation_ft(feet: Option<i32>) -> Result<Option<i32>> {
    match feet {
        None => Ok(None),
        Some(f) if (-500..=20_000).contains(&f) => {
            Ok(Some(((f as f64) / 10.0).round() as i32 * 10))
        }
        Some(_) => bail!("elevation must be between -500 and 20000 feet"),
    }
}

/// GHCN-Daily station id, as the `reference_stations` CHECK requires.
pub fn ghcn_id(s: &str) -> Result<String> {
    if s.len() == 11
        && s.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    {
        Ok(s.to_owned())
    } else {
        bail!("reference station id must be an 11-character GHCN-Daily id such as USW00094035")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_syntax() {
        assert_eq!(email(" a@example.test ").unwrap(), "a@example.test");
        for bad in [
            "",
            "a",
            "a@",
            "@x.test",
            "a@b",
            "a b@x.test",
            "a@@x.test",
            "a@x..test",
            "a@.x.test",
            "a@x.test.",
        ] {
            assert!(email(bad).is_err(), "{bad:?}");
        }
        assert!(email(&format!("{}@x.test", "a".repeat(260))).is_err());
    }

    #[test]
    fn display_name_bounds() {
        assert_eq!(display_name("  Alex ").unwrap(), "Alex");
        assert!(display_name("   ").is_err());
        assert!(display_name(&"x".repeat(81)).is_err());
        assert!(display_name(&"x".repeat(80)).is_ok());
        assert!(display_name("bad\nname").is_err());
    }

    #[test]
    fn mac_is_normalised_and_malformed_ones_rejected() {
        assert_eq!(mac("aa:bb:cc:00:01:02").unwrap(), "AA:BB:CC:00:01:02");
        assert_eq!(mac("aa-bb-cc-00-01-02").unwrap(), "AA:BB:CC:00:01:02");
        assert_eq!(mac("aabbcc000102").unwrap(), "AA:BB:CC:00:01:02");
        for bad in [
            "",
            "AA:BB:CC:00:01",
            "AA:BB:CC:00:01:0G",
            "AA:BB:CC:00:01:02:03",
            "AA.BB.CC.00.01.02",
        ] {
            assert!(mac(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn coordinates_round_to_two_decimals() {
        assert_eq!(coordinate(46.58765, 90.0, "lat").unwrap(), 46.59);
        assert_eq!(coordinate(-112.03999, 180.0, "lon").unwrap(), -112.04);
        assert!(coordinate(91.0, 90.0, "lat").is_err());
        assert!(coordinate(f64::NAN, 90.0, "lat").is_err());
    }

    #[test]
    fn timezone_offset_elevation_and_ghcn() {
        assert!(timezone("America/Denver").is_ok());
        assert!(timezone("Mars/Olympus").is_err());
        assert_eq!(std_offset(-420).unwrap(), -420);
        assert!(std_offset(900).is_err());
        assert_eq!(elevation_ft(Some(5004)).unwrap(), Some(5000));
        assert_eq!(elevation_ft(Some(5005)).unwrap(), Some(5010));
        assert_eq!(elevation_ft(None).unwrap(), None);
        assert!(elevation_ft(Some(30_000)).is_err());
        assert!(ghcn_id("USW00094035").is_ok());
        assert!(ghcn_id("usw00094035").is_err());
        assert!(ghcn_id("USW0009403").is_err());
    }
}
