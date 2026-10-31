//! Records Open-Meteo answers for the replay fixtures (02 section 7.9).
//!
//! `cargo run -p api --example record_openmeteo -- --station-index 1 --as helena`
//!
//! Always records all three files for one station (forecast, ensemble and previous runs), so they come from the same day.
//!
//! The location is the committed fake city-centre coordinate of seed station N in `ops/db/seed/seed.toml`; there are no
//! `--lat/--lon` flags, so a family member's home coordinates can never be recorded. No API key is used.

use std::path::PathBuf;

use anyhow::{anyhow, bail, Result};
use api::openmeteo::{FORECAST_DAILY, FORECAST_HOURLY, FORECAST_WIND_SPEED_UNIT};
use chrono::{Duration, NaiveDate, Utc};

fn arg(name: &str) -> Option<String> {
    let a: Vec<String> = std::env::args().collect();
    a.iter()
        .position(|x| x == name)
        .and_then(|i| a.get(i + 1).cloned())
}

async fn get(client: &reqwest::Client, base: &str, params: &[(&str, String)]) -> Result<Vec<u8>> {
    let mut url = url::Url::parse(base)?;
    url.query_pairs_mut()
        .extend_pairs(params.iter().map(|(k, v)| (*k, v.as_str())));
    let r = client
        .get(url)
        .send()
        .await
        .map_err(|e| anyhow!("request failed: {}", e.without_url()))?;
    if !r.status().is_success() {
        bail!("upstream status {}", r.status());
    }
    Ok(r.bytes().await?.to_vec())
}

#[tokio::main]
async fn main() -> Result<()> {
    let index: usize = arg("--station-index")
        .ok_or_else(|| anyhow!("--station-index N (1..3)"))?
        .parse()?;
    let tag = arg("--as").ok_or_else(|| anyhow!("--as <tag>"))?;
    if !tag.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
        bail!("tag must be lowercase letters");
    }
    let seed_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../ops/db/seed/seed.toml");
    let doc: toml::Table = std::fs::read_to_string(seed_path)?.parse()?;
    let st = doc["station"]
        .as_array()
        .and_then(|a| a.get(index.wrapping_sub(1)))
        .ok_or_else(|| anyhow!("no such seed station"))?;
    let lat = st["lat"].as_float().ok_or_else(|| anyhow!("lat"))?;
    let lon = st["lon"].as_float().ok_or_else(|| anyhow!("lon"))?;
    let tz = st["timezone"]
        .as_str()
        .ok_or_else(|| anyhow!("timezone"))?
        .to_owned();

    let client = reqwest::Client::builder()
        .user_agent("family-weather-fixture-recorder")
        .build()?;
    let common = vec![
        ("latitude", lat.to_string()),
        ("longitude", lon.to_string()),
        ("temperature_unit", "fahrenheit".to_owned()),
        ("precipitation_unit", "inch".to_owned()),
        ("timezone", tz),
        ("models", "gfs_seamless".to_owned()),
    ];
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/open-meteo");
    std::fs::create_dir_all(&out)?;

    // The same variable lists the API requests (api::openmeteo), so the fixture always carries what the parser reads.
    let mut p = common.clone();
    p.push(("hourly", FORECAST_HOURLY.into()));
    p.push(("daily", FORECAST_DAILY.into()));
    p.push(("wind_speed_unit", FORECAST_WIND_SPEED_UNIT.into()));
    p.push(("forecast_days", "16".into()));
    let body = get(&client, "https://api.open-meteo.com/v1/forecast", &p).await?;
    std::fs::write(out.join(format!("forecast__{tag}.json")), body)?;

    let mut p = common.clone();
    p.retain(|(k, _)| *k != "models");
    p.push(("models", "ecmwf_ifs025".into()));
    p.push(("hourly", "temperature_2m,precipitation".into()));
    p.push(("forecast_days", "15".into()));
    let body = get(
        &client,
        "https://ensemble-api.open-meteo.com/v1/ensemble",
        &p,
    )
    .await?;
    std::fs::write(out.join(format!("ensemble_ecmwf_ifs025__{tag}.json")), body)?;

    // A 16-day window: 14 days of local dates padded by one day each side.
    let today: NaiveDate = Utc::now().date_naive();
    let (from, to): (NaiveDate, NaiveDate) =
        (today - Duration::days(14), today + Duration::days(1));
    let mut vars = Vec::new();
    for var in [
        "temperature_2m",
        "precipitation",
        "precipitation_probability",
    ] {
        vars.push(var.to_owned());
        for lead in 1..=7 {
            vars.push(format!("{var}_previous_day{lead}"));
        }
    }
    let mut p = common;
    p.push(("hourly", vars.join(",")));
    p.push(("start_date", from.to_string()));
    p.push(("end_date", to.to_string()));
    let body = get(
        &client,
        "https://previous-runs-api.open-meteo.com/v1/forecast",
        &p,
    )
    .await?;
    std::fs::write(
        out.join(format!("previous_runs__{tag}__{from}__{to}.json")),
        body,
    )?;
    println!("recorded forecast, ensemble and previous runs for tag {tag}");
    Ok(())
}
