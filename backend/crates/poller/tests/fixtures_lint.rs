//! Fixture lint (03 section 10): the repository is public, so fixtures must hold no real identifiers.
//! The lint runs against the committed `backend/fixtures` and, to prove it bites, against tempdir trees.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use famweather_core::acis::parse_station_data;
use regex::Regex;
use serde_json::Value;

const SIZE_BUDGET_BYTES: u64 = 25 * 1024 * 1024;
const MIN_HISTORY_DAYS: i64 = 60;
const MIN_COMPLETE_YEARS: usize = 12;
const OPEN_METEO_MAX_DEG: f64 = 0.25;

fn fixtures_root() -> PathBuf {
    std::env::var_os("FIXTURES_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures"))
}

fn seed_path() -> PathBuf {
    std::env::var_os("OPS_DIR")
        .filter(|v| !v.is_empty())
        .map(|d| PathBuf::from(d).join("seed/seed.toml"))
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../ops/db/seed/seed.toml")
        })
}

#[derive(Debug, Default)]
struct Seed {
    /// reference id -> (lat, lon)
    reference: BTreeMap<String, (f64, f64)>,
    /// every seeded city-centre coordinate (stations and reference stations)
    centres: Vec<(f64, f64)>,
}

fn load_seed(path: &Path) -> Seed {
    let doc: toml::Table = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("reading the seed file {}: {e}", path.display()))
        .parse()
        .expect("seed.toml parses");
    let f = |t: &toml::Value, k: &str| {
        t.get(k)
            .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
    };
    let mut seed = Seed::default();
    for t in doc
        .get("reference_station")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        let (lat, lon) = (f(t, "lat").unwrap(), f(t, "lon").unwrap());
        seed.reference
            .insert(t["id"].as_str().unwrap().to_owned(), (lat, lon));
        seed.centres.push((lat, lon));
    }
    for t in doc
        .get("station")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        seed.centres
            .push((f(t, "lat").unwrap(), f(t, "lon").unwrap()));
    }
    seed
}

fn read_text(path: &Path) -> Vec<u8> {
    let raw = std::fs::read(path).unwrap();
    if path.extension().is_some_and(|e| e == "gz") {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(raw.as_slice())
            .read_to_end(&mut out)
            .unwrap();
        out
    } else {
        raw
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn decimals(n: &serde_json::Number) -> usize {
    let s = n.to_string();
    s.split_once('.')
        .map_or(0, |(_, frac)| frac.trim_end_matches('0').len())
}

/// Calls `f(key, value)` for every object member and `f("", value)` is not used.
fn visit(v: &Value, f: &mut dyn FnMut(&str, &Value)) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                f(k, x);
                visit(x, f);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| visit(x, f)),
        _ => {}
    }
}

const LAT_LON_KEYS: [&str; 4] = ["lat", "lon", "latitude", "longitude"];

/// An allow-listed public station or a seed city centre.
fn coordinate_allowed(seed: &Seed, lon: f64, lat: f64) -> bool {
    PUBLIC_STATIONS.iter().any(|(_, plon, plat)| {
        (plon - lon).abs() < CITY_CENTRE_TOLERANCE_DEG
            && (plat - lat).abs() < CITY_CENTRE_TOLERANCE_DEG
    }) || seed.centres.iter().any(|(clat, clon)| {
        (clat - lat).abs() < CITY_CENTRE_TOLERANCE_DEG
            && (clon - lon).abs() < CITY_CENTRE_TOLERANCE_DEG
    })
}

/// A two-number array that could be a coordinate pair, anywhere except as the value of an `ll` member.
fn has_stray_pair(v: &Value, key: &str) -> bool {
    match v {
        Value::Array(a) => {
            let numbers = a.len() == 2 && a.iter().all(Value::is_number);
            (numbers && key != "ll") || a.iter().any(|x| has_stray_pair(x, ""))
        }
        Value::Object(m) => m.iter().any(|(k, x)| has_stray_pair(x, k)),
        _ => false,
    }
}

const CITY_CENTRE_TOLERANCE_DEG: f64 = 0.001;

/// Coordinates allowed in `acis/stnmeta__*.json`: public ACIS stations (airports) and invented demo stations.
/// Adding a line here is the review gate: never a family home. `(name, lon, lat)`.
const PUBLIC_STATIONS: [(&str, f64, f64); 10] = [
    (
        "Fort Collins Loveland AP (FNL, USW00094035)",
        -105.01667,
        40.45,
    ),
    ("Helena Regional AP (HLN, USW00024144)", -111.97, 46.6),
    ("Laramie Regional AP (LAR, USW00024022)", -105.68, 41.32),
    ("demo COOP station A", -105.1, 40.55),
    ("demo rainfall-only volunteer 1", -105.2, 40.35),
    ("demo closed COOP station B", -104.9, 40.25),
    ("demo rainfall-only volunteer 2", -112.1, 46.7),
    ("demo COOP station C", -105.5, 41.2),
    ("demo rainfall-only volunteer 3", -108.0, 44.0),
    ("demo closed COOP station D", -108.1, 44.1),
];

fn lint(root: &Path, seed: &Seed) -> Vec<String> {
    let mut problems = Vec::new();
    let mut files = Vec::new();
    walk(root, &mut files);
    files.retain(|p| p.file_name().is_some_and(|n| n != "README.md"));
    files.sort();
    let hex = Regex::new(r"[0-9a-fA-F]{32,}").unwrap();
    let mac = Regex::new(r"(?i)\b(?:[0-9a-f]{2}[:-]){5}[0-9a-f]{2}\b").unwrap();
    let rel = |p: &Path| p.strip_prefix(root).unwrap().display().to_string();

    let mut total = 0u64;
    for p in &files {
        total += std::fs::metadata(p).unwrap().len();
        let name = rel(p);
        let text = String::from_utf8_lossy(&read_text(p)).into_owned();
        if hex.is_match(&text) || hex.is_match(&name) {
            problems.push(format!("{name}: contains a 32+ character hex string"));
        }
        for m in mac.find_iter(&format!("{name}\n{text}")) {
            if !m.as_str().to_ascii_uppercase().starts_with("AA:BB:CC:00:") {
                problems.push(format!(
                    "{name}: MAC outside AA:BB:CC:00:.. ({} chars)",
                    m.as_str().len()
                ));
            }
        }
    }
    if total > SIZE_BUDGET_BYTES {
        problems.push(format!(
            "total size {total} bytes exceeds the {SIZE_BUDGET_BYTES} byte budget"
        ));
    }

    for p in &files {
        let name = rel(p);
        let is_json = name.ends_with(".json") || name.ends_with(".json.gz");
        if !is_json {
            continue;
        }
        let v: Value = match serde_json::from_slice(&read_text(p)) {
            Ok(v) => v,
            Err(e) => {
                problems.push(format!("{name}: invalid JSON ({e})"));
                continue;
            }
        };
        // Every awn tree counts, including the scenario trees at test/<case>/awn/**.
        if name.starts_with("awn/") || name.contains("/awn/") {
            let mut bad = false;
            visit(&v, &mut |k, x| {
                if LAT_LON_KEYS.contains(&k) {
                    if let Value::Number(n) = x {
                        bad |= decimals(n) > 1;
                    }
                }
                if k == "coordinates" {
                    if let Value::Array(a) = x {
                        bad |= a
                            .iter()
                            .any(|c| matches!(c, Value::Number(n) if decimals(n) > 1));
                    }
                }
            });
            if bad {
                problems.push(format!("{name}: coordinates with more than 1 decimal"));
            }
        }
        if name.starts_with("open-meteo/") || name.starts_with("test/open-meteo/") {
            visit(&v, &mut |k, x| {
                let Some(deg) = x.as_f64() else { return };
                let near = |pick: fn(&(f64, f64)) -> f64| {
                    seed.centres
                        .iter()
                        .any(|c| (pick(c) - deg).abs() <= OPEN_METEO_MAX_DEG)
                };
                match k {
                    "latitude" | "lat" if !near(|c| c.0) => problems.push(format!("{name}: {k} is not within {OPEN_METEO_MAX_DEG} degrees of a seed city centre")),
                    "longitude" | "lon" if !near(|c| c.1) => problems.push(format!("{name}: {k} is not within {OPEN_METEO_MAX_DEG} degrees of a seed city centre")),
                    _ => {}
                }
            });
        }
        let acis_base = name
            .strip_prefix("acis/")
            .or_else(|| name.split_once("/acis/").map(|(_, rest)| rest));
        if let Some(base) = acis_base {
            // Any file under acis/: no lat/lon-style members and no stray coordinate-looking number pairs; the
            // only coordinates are the `ll` members checked below.
            let mut stray = false;
            visit(&v, &mut |k, _| {
                stray |= LAT_LON_KEYS.contains(&k) || k == "coords" || k == "coordinates"
            });
            if stray || has_stray_pair(&v, "") {
                problems.push(format!(
                    "{name}: lat/lon-style member or coordinate-looking number pair outside `ll`"
                ));
            }
            // The recorded search box must be centred on an allowed point too.
            if let Some(bbox) = v.get("recorded_bbox") {
                let nums: Vec<f64> = bbox
                    .as_str()
                    .map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).collect())
                    .unwrap_or_default();
                if nums.len() != 4 {
                    problems.push(format!(
                        "{name}: recorded_bbox is not `west,south,east,north`"
                    ));
                } else {
                    let (lon, lat) = ((nums[0] + nums[2]) / 2.0, (nums[1] + nums[3]) / 2.0);
                    if !coordinate_allowed(seed, lon, lat) {
                        problems.push(format!(
                            "{name}: recorded_bbox centre ({lon}, {lat}) is neither on the public-station allow-list nor a seed city centre"
                        ));
                    }
                }
            }
            if base.starts_with("stnmeta__") {
                // Public stations only: every coordinate pair belongs to a named station entry with ids.
                let metas = v
                    .get("meta")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for m in &metas {
                    let ok = m
                        .get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|n| !n.is_empty())
                        && m.get("sids")
                            .and_then(Value::as_array)
                            .is_some_and(|s| !s.is_empty());
                    if !ok {
                        problems.push(format!("{name}: a station entry without name and sids"));
                    }
                    if let Some(ll) = m.get("ll").and_then(Value::as_array) {
                        // Public coordinate of an approved station, or a seed city centre: never a home.
                        if let (Some(lon), Some(lat)) = (
                            ll.first().and_then(Value::as_f64),
                            ll.get(1).and_then(Value::as_f64),
                        ) {
                            let allowed = PUBLIC_STATIONS.iter().any(|(_, plon, plat)| {
                                (plon - lon).abs() < 1e-6 && (plat - lat).abs() < 1e-6
                            }) || seed.centres.iter().any(|(clat, clon)| {
                                (clat - lat).abs() < CITY_CENTRE_TOLERANCE_DEG
                                    && (clon - lon).abs() < CITY_CENTRE_TOLERANCE_DEG
                            });
                            if !allowed {
                                problems.push(format!(
                                    "{name}: coordinate ({lon}, {lat}) is neither on the public-station allow-list nor a seed city centre"
                                ));
                            }
                        }
                        if ll
                            .iter()
                            .any(|c| matches!(c, Value::Number(n) if decimals(n) > 5))
                        {
                            problems.push(format!("{name}: coordinate with more than 5 decimals"));
                        }
                    }
                }
            } else if let Some((sid, _)) = base.split_once("__") {
                match seed.reference.get(sid) {
                    None => {
                        problems.push(format!("{name}: {sid} is not a seeded reference station"))
                    }
                    Some((lat, lon)) => {
                        let ll = v.pointer("/meta/ll").and_then(Value::as_array);
                        let ok = ll.is_some_and(|a| {
                            a.len() == 2
                                && (a[0].as_f64().unwrap_or(f64::NAN) - lon).abs() < 1e-6
                                && (a[1].as_f64().unwrap_or(f64::NAN) - lat).abs() < 1e-6
                        });
                        if !ok {
                            problems.push(format!(
                                "{name}: meta.ll is not the seed coordinate of {sid}"
                            ));
                        }
                        let mut stray = false;
                        visit(&v, &mut |k, _| {
                            stray |= LAT_LON_KEYS.contains(&k) || k == "coordinates"
                        });
                        if stray {
                            problems.push(format!("{name}: unexpected coordinate member"));
                        }
                    }
                }
                match parse_station_data(&read_text(p)) {
                    Err(_) => problems.push(format!("{name}: not a StnData answer")),
                    Ok(parsed) => {
                        let mut per_year: BTreeMap<i32, (u32, u32)> = BTreeMap::new();
                        for d in &parsed.days {
                            let e = per_year.entry(chrono::Datelike::year(&d.date)).or_default();
                            e.0 += 1;
                            e.1 += u32::from(d.tmax_f.is_some() && d.tmin_f.is_some());
                        }
                        let complete = per_year
                            .values()
                            .filter(|(n, ok)| *n >= 365 && *ok as f64 >= 0.9 * f64::from(*n))
                            .count();
                        if complete < MIN_COMPLETE_YEARS {
                            problems.push(format!("{name}: only {complete} complete years (need {MIN_COMPLETE_YEARS})"));
                        }
                    }
                }
            }
        }
    }

    // Station history depth and the exact set of seeded station directories.
    let stations = root.join("awn/stations");
    if stations.is_dir() {
        let mut dirs: Vec<String> = std::fs::read_dir(&stations)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        dirs.sort();
        if dirs
            != [
                "AA:BB:CC:00:00:01",
                "AA:BB:CC:00:00:02",
                "AA:BB:CC:00:00:03",
            ]
        {
            problems.push(format!(
                "awn/stations holds {dirs:?}, expected exactly the three seeded MACs"
            ));
        }
        for d in &dirs {
            let mut pages = Vec::new();
            walk(&stations.join(d).join("history"), &mut pages);
            let (mut lo, mut hi) = (i64::MAX, i64::MIN);
            for p in pages {
                let rows: Vec<Value> = serde_json::from_slice(&read_text(&p)).unwrap_or_default();
                for r in rows {
                    if let Some(t) = r.get("dateutc").and_then(Value::as_i64) {
                        lo = lo.min(t);
                        hi = hi.max(t);
                    }
                }
            }
            let days = if hi >= lo { (hi - lo) / 86_400_000 } else { 0 };
            if days < MIN_HISTORY_DAYS {
                problems.push(format!(
                    "awn/stations/{d}: {days} days of history (need {MIN_HISTORY_DAYS})"
                ));
            }
        }
    }
    problems
}

#[test]
fn committed_fixtures_pass_the_lint() {
    let problems = lint(&fixtures_root(), &load_seed(&seed_path()));
    assert!(
        problems.is_empty(),
        "fixture lint:\n{}",
        problems.join("\n")
    );
}

// ---- the lint must bite ----

fn tree_with(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (path, body) in files {
        let p = dir.path().join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
    dir
}

fn seed() -> Seed {
    let mut s = Seed::default();
    s.reference.insert("USW00099001".into(), (46.6, -112.0));
    s.centres.push((46.6, -112.0));
    s
}

fn problems_of(files: &[(&str, &str)]) -> String {
    lint(tree_with(files).path(), &seed()).join("\n")
}

#[test]
fn lint_catches_hex_strings_and_foreign_macs() {
    let p = problems_of(&[("awn/x.json", r#"{"k": "0123456789abcdef0123456789abcdef"}"#)]);
    assert!(p.contains("hex string"), "{p}");
    let p = problems_of(&[("awn/x.json", r#"{"macAddress": "00:11:22:33:44:55"}"#)]);
    assert!(p.contains("MAC outside"), "{p}");
    let p = problems_of(&[("awn/stations/00-11-22-33-44-55/device.json", "{}")]);
    assert!(p.contains("MAC outside"), "{p}");
    let p = problems_of(&[("awn/x.json", r#"{"macAddress": "aa:bb:cc:00:00:09"}"#)]);
    assert!(!p.contains("MAC outside"), "{p}");
}

#[test]
fn lint_catches_precise_coordinates_in_awn() {
    let p = problems_of(&[(
        "awn/x.json",
        r#"{"info": {"coords": {"lat": 40.123456, "lon": -105.1}}}"#,
    )]);
    assert!(p.contains("more than 1 decimal"), "{p}");
    let p = problems_of(&[(
        "awn/x.json",
        r#"{"info": {"coords": {"lat": 40.1, "lon": -105.0, "elevation": 5000.25}}}"#,
    )]);
    assert!(!p.contains("decimal"), "{p}");
}

#[test]
fn lint_checks_open_meteo_against_the_seed_cities() {
    let p = problems_of(&[(
        "open-meteo/f.json",
        r#"{"latitude": 46.7, "longitude": -112.1}"#,
    )]);
    assert!(!p.contains("seed city centre"), "{p}");
    let p = problems_of(&[(
        "open-meteo/f.json",
        r#"{"latitude": 47.5, "longitude": -112.0}"#,
    )]);
    assert!(p.contains("latitude is not within"), "{p}");
}

#[test]
fn lint_checks_acis_coordinates_and_depth() {
    let p = problems_of(&[(
        "acis/USW00099001__1991-01-01__1991-01-02.json",
        r#"{"meta":{"ll":[-111.0,46.0]},"data":[]}"#,
    )]);
    assert!(p.contains("meta.ll is not the seed coordinate"), "{p}");
    assert!(p.contains("complete years"), "{p}");
    let p = problems_of(&[(
        "acis/USW00012345__1991-01-01__1991-01-02.json",
        r#"{"meta":{"ll":[-111.0,46.0]},"data":[]}"#,
    )]);
    assert!(p.contains("not a seeded reference station"), "{p}");
    let p = problems_of(&[(
        "acis/stnmeta__x.json",
        r#"{"meta":[{"ll":[-105.0,40.0],"name":"","sids":[]}]}"#,
    )]);
    assert!(p.contains("without name and sids"), "{p}");
}

#[test]
fn lint_checks_stnmeta_coordinates_against_the_allow_list_and_seed_centres() {
    let station = |lon: f64, lat: f64| {
        format!(r#"{{"meta":[{{"ll":[{lon},{lat}],"name":"X","sids":["A 6"]}}]}}"#)
    };
    // allow-listed public station, and a seed city centre (46.6, -112.0 in the test seed)
    let p = problems_of(&[("acis/stnmeta__a.json", &station(-111.97, 46.6))]);
    assert!(!p.contains("allow-list"), "{p}");
    let p = problems_of(&[("acis/stnmeta__b.json", &station(-112.0, 46.6))]);
    assert!(!p.contains("allow-list"), "{p}");
    // a coordinate that is neither: could be a home
    let p = problems_of(&[("acis/stnmeta__c.json", &station(-105.1234, 40.4321))]);
    assert!(
        p.contains("neither on the public-station allow-list"),
        "{p}"
    );
    // the same rule applies to scenario trees
    let p = problems_of(&[(
        "test/case/acis/stnmeta__c.json",
        &station(-105.1234, 40.4321),
    )]);
    assert!(
        p.contains("neither on the public-station allow-list"),
        "{p}"
    );
}

#[test]
fn lint_checks_the_recorded_bbox_centre() {
    // tight box around a point that is neither allow-listed nor a seed centre
    let p = problems_of(&[(
        "acis/stnmeta__a.json",
        r#"{"recorded_bbox":"-105.1236,40.4319,-105.1232,40.4323","meta":[]}"#,
    )]);
    assert!(p.contains("recorded_bbox centre"), "{p}");
    // centred on the seed city (46.6, -112.0): fine
    let p = problems_of(&[(
        "acis/stnmeta__b.json",
        r#"{"recorded_bbox":"-112.9,45.7,-111.1,47.5","meta":[]}"#,
    )]);
    assert!(!p.contains("recorded_bbox"), "{p}");
    let p = problems_of(&[(
        "acis/stnmeta__c.json",
        r#"{"recorded_bbox":"nope","meta":[]}"#,
    )]);
    assert!(p.contains("recorded_bbox is not"), "{p}");
}

#[test]
fn lint_rejects_lat_lon_style_members_and_stray_number_pairs_anywhere_in_acis() {
    let stray = "lat/lon-style member or coordinate-looking number pair";
    let p = problems_of(&[(
        "acis/stnmeta__a.json",
        r#"{"meta":[{"ll":[-111.97,46.6],"name":"X","sids":["A 6"],"lat":40.4321,"lon":-105.1234}]}"#,
    )]);
    assert!(p.contains(stray), "{p}");
    let p = problems_of(&[(
        "acis/stnmeta__b.json",
        r#"{"meta":[{"ll":[-111.97,46.6],"name":"X","sids":["A 6"],"coords":"40.4,-105.1"}]}"#,
    )]);
    assert!(p.contains(stray), "{p}");
    let p = problems_of(&[(
        "acis/stnmeta__c.json",
        r#"{"meta":[{"ll":[-111.97,46.6],"name":"X","sids":["A 6"],"home":[40.4321,-105.1234]}]}"#,
    )]);
    assert!(p.contains(stray), "{p}");
    let p = problems_of(&[(
        "test/case/acis/USW00099001__1991-01-01__1991-01-02.json",
        r#"{"meta":{"ll":[-112.0,46.6]},"data":[],"extra":{"latitude":40.1}}"#,
    )]);
    assert!(p.contains(stray), "{p}");
    // a plain `ll` is fine
    let p = problems_of(&[(
        "acis/stnmeta__d.json",
        r#"{"meta":[{"ll":[-111.97,46.6],"name":"X","sids":["A 6"]}]}"#,
    )]);
    assert!(!p.contains(stray), "{p}");
}

#[test]
fn lint_checks_one_decimal_coordinates_in_every_awn_tree_including_scenarios() {
    let body = r#"{"info": {"coords": {"coords": {"lat": 40.1234, "lon": -105.1}}}}"#;
    for path in [
        "awn/stations/AA:BB:CC:00:00:01/device.json",
        "test/case/awn/stations/AA:BB:CC:00:00:01/device.json",
        "deep/er/awn/devices.json",
    ] {
        let p = problems_of(&[(path, body)]);
        assert!(p.contains("more than 1 decimal"), "{path}: {p}");
    }
}

#[test]
fn lint_checks_station_history_depth_and_directory_set() {
    let p = problems_of(&[
        ("awn/stations/AA:BB:CC:00:00:01/device.json", "{}"),
        (
            "awn/stations/AA:BB:CC:00:00:01/history/page-0001.json",
            r#"[{"dateutc": 1780315200000}, {"dateutc": 1780314900000}]"#,
        ),
    ]);
    assert!(p.contains("days of history"), "{p}");
    assert!(p.contains("expected exactly the three seeded MACs"), "{p}");
}

#[test]
fn lint_enforces_the_size_budget() {
    let big = "x".repeat((SIZE_BUDGET_BYTES + 10) as usize);
    let p = problems_of(&[("awn/big.txt", &big)]);
    assert!(p.contains("budget"), "{p}");
}
