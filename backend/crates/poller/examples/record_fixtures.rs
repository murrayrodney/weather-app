//! Fixture recorder and generator (03 section 10). Run from the repository root:
//!
//! ```text
//! cargo run -p poller --features test-support --example record_fixtures -- <command>
//!
//!   synth-all                         regenerate every committed synthetic fixture (stations, ACIS)
//!   awn   --mac <real MAC> --as AA:BB:CC:00:00:01 [--days 60]   record one real station (live)
//!   derive --from AA:BB:CC:00:00:01 --as AA:BB:CC:00:00:02 --temp-offset-f -3.0 --rain-scale 1.15
//!   acis  --sid <real GHCN id> --as USW00099001                 record one reference station (live)
//! ```
//!
//! `awn` and `acis` talk to the real services: the AWN user key is typed at a hidden prompt (never an argument,
//! never an environment variable), the application key is the developer's own `AWN_APPLICATION_KEY`, and both
//! refuse `APP_ENV=production`. Output is sanitised: placeholder MAC, coordinates and name from the seed, no
//! addresses. The committed fixtures are synthetic (`synth-all`); Rodney replaces them with recordings (T7).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use clap::{Parser, Subcommand};
use famweather_core::awn::{ApiKey, AwnApi, LiveAwn, Mac};
use poller_lib::synth::{self, Anomalies, StationSpec};
use secrecy::SecretString;
use serde_json::{json, Value};

#[derive(Parser)]
struct Cli {
    /// Fixtures root
    #[arg(long, default_value = "backend/fixtures", global = true)]
    root: PathBuf,
    /// Seed file naming the stations and reference stations (names, coordinates)
    #[arg(long, default_value = "ops/db/seed/seed.toml", global = true)]
    seed: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    SynthAll,
    Awn {
        #[arg(long)]
        mac: String,
        #[arg(long = "as")]
        as_mac: String,
        #[arg(long, default_value_t = 60)]
        days: u32,
    },
    Derive {
        #[arg(long)]
        from: String,
        #[arg(long = "as")]
        as_mac: String,
        #[arg(long, default_value_t = -3.0)]
        temp_offset_f: f64,
        #[arg(long, default_value_t = 1.15)]
        rain_scale: f64,
    },
    Acis {
        #[arg(long)]
        sid: String,
        #[arg(long = "as")]
        as_sid: String,
    },
}

struct SeedStation {
    name: String,
    lat: f64,
    lon: f64,
    elevation_ft: f64,
    timezone: String,
}

fn seed_station(seed: &Path, mac: &str) -> Result<SeedStation> {
    let doc: toml::Table = std::fs::read_to_string(seed)
        .with_context(|| format!("reading {}", seed.display()))?
        .parse()?;
    let st = doc
        .get("station")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .find(|s| s.get("mac").and_then(|m| m.as_str()) == Some(mac))
        .ok_or_else(|| anyhow!("{mac} is not a [[station]] in the seed file"))?;
    let f = |k: &str| {
        st.get(k)
            .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
    };
    Ok(SeedStation {
        name: st
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("Demo Station")
            .to_owned(),
        lat: f("lat").ok_or_else(|| anyhow!("station lat"))?,
        lon: f("lon").ok_or_else(|| anyhow!("station lon"))?,
        elevation_ft: f("elevation_ft").unwrap_or(0.0),
        timezone: st
            .get("timezone")
            .and_then(|v| v.as_str())
            .unwrap_or("America/Denver")
            .to_owned(),
    })
}

fn seed_reference(seed: &Path, id: &str) -> Result<(String, f64, f64, f64)> {
    let doc: toml::Table = std::fs::read_to_string(seed)?.parse()?;
    let r = doc
        .get("reference_station")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .find(|s| s.get("id").and_then(|m| m.as_str()) == Some(id))
        .ok_or_else(|| anyhow!("{id} is not a [[reference_station]] in the seed file"))?;
    let f = |k: &str| {
        r.get(k)
            .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
            .unwrap_or(0.0)
    };
    Ok((
        r.get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("Demo")
            .to_owned(),
        f("lat"),
        f("lon"),
        f("elevation_ft"),
    ))
}

fn newest_fixed() -> DateTime<Utc> {
    // Replay shifts the data so the newest reading equals "now"; the absolute date does not matter.
    Utc.with_ymd_and_hms(2026, 10, 15, 12, 0, 0)
        .single()
        .expect("valid")
}

fn write(cli: &Cli, mac: &str, rows: &[Value]) -> Result<()> {
    let s = seed_station(&cli.seed, mac)?;
    let device = synth::device_json(
        mac,
        &s.name,
        s.lat,
        s.lon,
        s.elevation_ft,
        &s.timezone,
        &rows[0],
    );
    // Start clean so a shorter regeneration leaves no stale pages.
    let dir = cli.root.join("awn").join("stations").join(mac);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    synth::write_station(&cli.root, mac, &device, rows, true)?;
    println!("wrote {} rows for {mac}", rows.len());
    Ok(())
}

fn read_rows(root: &Path, mac: &str) -> Result<Vec<Value>> {
    use std::io::Read;
    let hist = root.join("awn").join("stations").join(mac).join("history");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&hist)?
        .flatten()
        .map(|e| e.path())
        .collect();
    files.sort();
    let mut rows = Vec::new();
    for f in files {
        let raw = std::fs::read(&f)?;
        let bytes = if f.extension().is_some_and(|e| e == "gz") {
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(raw.as_slice()).read_to_end(&mut out)?;
            out
        } else {
            raw
        };
        rows.extend(serde_json::from_slice::<Vec<Value>>(&bytes)?);
    }
    Ok(rows)
}

fn derive_cmd(cli: &Cli, from: &str, to: &str, temp: f64, rain: f64) -> Result<()> {
    let rows = read_rows(&cli.root, from)?;
    write(cli, to, &synth::derive(&rows, temp, rain))
}

fn acis_synth(cli: &Cli, id: &str, seed: u64) -> Result<()> {
    let (name, lat, lon, elev) = seed_reference(&cli.seed, id)?;
    let start = NaiveDate::from_ymd_opt(1991, 1, 1).expect("valid");
    let end = NaiveDate::from_ymd_opt(2026, 9, 30).expect("valid");
    let v = synth::acis_fixture(id, &name, lat, lon, elev, start, end, seed);
    let dir = cli.root.join("acis");
    std::fs::create_dir_all(&dir)?;
    let name = format!("{id}__{start}__{end}.json.gz");
    std::fs::write(dir.join(&name), synth::gzip_json(&v))?;
    println!("wrote acis/{name}");
    Ok(())
}

fn synth_all(cli: &Cli) -> Result<()> {
    let mut spec = StationSpec::simple(newest_fixed(), 62, 5, 20261);
    spec.base_temp_f = 50.0;
    spec.anomalies = Anomalies {
        reset_at_day: Some(20.0),
        glitch_at_day: Some(35.0),
        spike_at_day: Some(50.0),
    };
    let base = synth::generate(&spec);
    write(cli, "AA:BB:CC:00:00:01", &base)?;
    derive_cmd(cli, "AA:BB:CC:00:00:01", "AA:BB:CC:00:00:02", -3.0, 1.15)?;
    derive_cmd(cli, "AA:BB:CC:00:00:01", "AA:BB:CC:00:00:03", 1.5, 0.85)?;
    for (i, id) in ["USW00099001", "USW00099002", "USW00099003"]
        .iter()
        .enumerate()
    {
        acis_synth(cli, id, 910 + i as u64)?;
    }
    Ok(())
}

fn refuse_production() -> Result<()> {
    if std::env::var("APP_ENV").is_ok_and(|v| v.trim() == "production") {
        bail!("refusing to record with APP_ENV=production");
    }
    Ok(())
}

async fn record_awn(cli: &Cli, mac: &str, as_mac: &str, days: u32) -> Result<()> {
    refuse_production()?;
    let real = Mac::parse(mac).map_err(|e| anyhow!("{e}"))?;
    let app_key = std::env::var("AWN_APPLICATION_KEY")
        .context("AWN_APPLICATION_KEY (your own dev application key) is not set")?;
    eprintln!("AWN user API key (input hidden): ");
    let user_key = rpassword::read_password()?;
    let awn: Arc<dyn AwnApi> = Arc::new(
        LiveAwn::new(SecretString::from(app_key), Duration::from_secs(20))
            .map_err(|e| anyhow!("{e}"))?,
    );
    let key = ApiKey::new(SecretString::from(user_key));
    let mut rows: Vec<Value> = Vec::new();
    let mut end: Option<DateTime<Utc>> = None;
    let want = i64::from(days) * 24 * 60 / 5;
    while (rows.len() as i64) < want {
        tokio::time::sleep(Duration::from_millis(1200)).await; // 1 request/s per key
        let page = awn
            .history(&key, &real, end, 288)
            .await
            .map_err(|e| anyhow!("{e}"))?;
        let Some(oldest) = page.iter().map(|o| o.dateutc).min() else {
            break;
        };
        for o in &page {
            let mut v = serde_json::to_value(o)?;
            if let Some(m) = v.as_object_mut() {
                m.remove("macAddress");
            }
            rows.push(v);
        }
        end = Utc.timestamp_millis_opt(oldest - 1).single();
        eprintln!("{} rows", rows.len());
    }
    if rows.is_empty() {
        bail!("no rows returned");
    }
    write(cli, as_mac, &rows)
}

async fn record_acis(cli: &Cli, sid: &str, as_sid: &str) -> Result<()> {
    refuse_production()?;
    let (name, lat, lon, elev) = seed_reference(&cli.seed, as_sid)?;
    let start = NaiveDate::from_ymd_opt(1991, 1, 1).expect("valid");
    let end = (Utc::now() - chrono::Duration::days(1)).date_naive();
    let body = json!({"sid": sid, "sdate": start.to_string(), "edate": end.to_string(),
        "elems": [{"name":"maxt","add":"f"},{"name":"mint","add":"f"},{"name":"pcpn","add":"f"}]});
    let resp: Value = reqwest::Client::new()
        .post("https://data.rcc-acis.org/StnData")
        .json(&body)
        .timeout(Duration::from_secs(60))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let data = resp
        .get("data")
        .cloned()
        .ok_or_else(|| anyhow!("no data member in the answer"))?;
    // Keep the raw rows; replace the meta so the real station's id and exact coordinates do not appear.
    let out = json!({"meta": {"name": name, "ll": [lon, lat], "sids": [format!("{as_sid} 6")], "elev": elev, "uid": 0}, "data": data});
    let dir = cli.root.join("acis");
    std::fs::create_dir_all(&dir)?;
    let file = format!("{as_sid}__{start}__{end}.json.gz");
    std::fs::write(dir.join(&file), synth::gzip_json(&out))?;
    println!("wrote acis/{file}");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match &cli.cmd {
        Cmd::SynthAll => synth_all(&cli),
        Cmd::Derive {
            from,
            as_mac,
            temp_offset_f,
            rain_scale,
        } => derive_cmd(&cli, from, as_mac, *temp_offset_f, *rain_scale),
        Cmd::Awn { mac, as_mac, days } => record_awn(&cli, mac, as_mac, *days).await,
        Cmd::Acis { sid, as_sid } => record_acis(&cli, sid, as_sid).await,
    }
}
