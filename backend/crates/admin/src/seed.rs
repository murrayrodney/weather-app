//! `admin seed-dev` (04 section 12.1): local demo data from `SEED_DIR/seed.toml`.
//!
//! Runs the same internal functions as the real commands, over sqlx and the Admin API. It never constructs an AWN
//! client. Every step looks before it writes, so a second run changes nothing.

use std::collections::HashMap;
use std::str::FromStr;

use anyhow::{anyhow, bail, Context, Result};
use famweather_core::config::{AppEnv, CredentialMode, UserRef};
use secrecy::SecretString;
use serde::Deserialize;
use sqlx::postgres::PgConnectOptions;
use uuid::Uuid;

use crate::audit;
use crate::ctx::{db_err, Ctx};
use crate::ops::{self, InviteSpec, KeyMaterial, ReferenceSpec, StationSpec};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Seed {
    /// The AWN key stored (encrypted) for every seeded owner. A fixture key: it only works in replay mode.
    pub poller_key: String,
    #[serde(default)]
    pub reference_station: Vec<SeedReference>,
    #[serde(default)]
    pub user: Vec<SeedUser>,
    #[serde(default)]
    pub station: Vec<SeedStation>,
    #[serde(default)]
    pub follow: Vec<SeedFollow>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedReference {
    pub id: String,
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    pub elevation_ft: Option<i32>,
    pub timezone: String,
    pub std_offset_minutes: i16,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedUser {
    pub display_name: String,
    pub email: String,
    #[serde(default)]
    pub admin: bool,
    /// Approved follows in both directions with every active user that exists at that moment (`invite --follow-all`).
    #[serde(default)]
    pub follow_all: bool,
    /// Invited, then deactivated.
    #[serde(default)]
    pub inactive: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedStation {
    pub mac: String,
    pub name: String,
    /// Owner display name.
    pub owner: String,
    pub lat: f64,
    pub lon: f64,
    pub elevation_ft: Option<i32>,
    pub timezone: String,
    pub std_offset_minutes: i16,
    pub reference_station: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedFollow {
    pub follower: String,
    pub followee: String,
    /// `pending`, `declined` or `revoked` (the history is made realistic: declined and revoked pass through earlier states).
    pub status: String,
}

pub fn parse(text: &str) -> Result<Seed> {
    let seed: Seed = toml::from_str(text).map_err(|e| anyhow!("seed.toml: {}", e.message()))?;
    if seed.poller_key.trim().is_empty() {
        bail!("seed.toml: poller_key is empty");
    }
    for f in &seed.follow {
        if !matches!(
            f.status.as_str(),
            "pending" | "approved" | "declined" | "revoked"
        ) {
            bail!("seed.toml: unknown follow status {:?}", f.status);
        }
    }
    Ok(seed)
}

#[derive(Default)]
struct Counts {
    reference: usize,
    users: usize,
    follows: usize,
    stations: usize,
    deactivated: usize,
}

pub async fn run(ctx: &Ctx, no_stations: bool) -> Result<()> {
    if ctx.env.app_env != Some(AppEnv::Local) {
        bail!("seed-dev runs only with APP_ENV=local");
    }
    let url = ctx
        .env
        .database_url
        .as_ref()
        .ok_or_else(|| anyhow!("DATABASE_URL is required"))?;
    use secrecy::ExposeSecret;
    let host = PgConnectOptions::from_str(url.expose_secret())
        .map_err(|_| anyhow!("DATABASE_URL is not valid"))?
        .get_host()
        .to_ascii_lowercase();
    if host != "db" && host != "localhost" {
        bail!("seed-dev refuses a database host other than db or localhost");
    }
    let demo_password = ctx
        .env
        .local_demo_password
        .clone()
        .ok_or_else(|| anyhow!("LOCAL_DEMO_PASSWORD is required"))?;
    let public_key = ctx
        .env
        .credentials_public_key
        .as_ref()
        .ok_or_else(|| anyhow!("CREDENTIALS_PUBLIC_KEY is required"))?;
    let key_version = ctx
        .env
        .credentials_key_version
        .ok_or_else(|| anyhow!("CREDENTIALS_KEY_VERSION is required"))?;

    let path = ctx.env.seed_dir.join("seed.toml");
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    let seed = parse(&text)?;
    let mut counts = Counts::default();

    // Reference stations first (stations point at them).
    for r in &seed.reference_station {
        let created = ops::add_reference_station(
            &ctx.pool,
            &ReferenceSpec {
                id: r.id.clone(),
                name: r.name.clone(),
                lat: r.lat,
                lon: r.lon,
                timezone: r.timezone.clone(),
                std_offset_minutes: r.std_offset_minutes,
                elevation_ft: r.elevation_ft,
            },
        )
        .await?;
        counts.reference += usize::from(created);
    }

    // Users, in file order, created with the demo password (no link is printed or minted).
    for u in &seed.user {
        let who = UserRef::DisplayName(u.display_name.clone());
        match ops::find_user(&ctx.pool, &who).await? {
            Some(existing) => {
                if u.inactive && existing.is_active {
                    ops::deactivate(ctx, &UserRef::Id(existing.id), true).await?;
                    counts.deactivated += 1;
                }
            }
            None => {
                ops::invite(
                    ctx,
                    &InviteSpec {
                        email: u.email.clone(),
                        display_name: u.display_name.clone(),
                        follow_all: u.follow_all,
                        admin: u.admin,
                        adopt: true,
                        password: Some(demo_password.clone()),
                        mint_link: false,
                        test_fail_after_auth: false,
                    },
                )
                .await?;
                counts.users += 1;
                if u.inactive {
                    ops::deactivate(ctx, &who, true).await?;
                    counts.deactivated += 1;
                }
            }
        }
    }

    // Follow states beyond the family mesh.
    for f in &seed.follow {
        let follower = user_id(ctx, &f.follower).await?;
        let followee = user_id(ctx, &f.followee).await?;
        let present: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM app.follows WHERE follower_id = $1 AND followee_id = $2)",
        )
        .bind(follower)
        .bind(followee)
        .fetch_one(&ctx.pool)
        .await
        .map_err(db_err)?;
        if present {
            continue;
        }
        let mut tx = ctx.pool.begin().await.map_err(db_err)?;
        let actor = audit::actor(&mut tx).await?;
        // declined / revoked are reached through realistic history: pending -> declined, approved -> revoked.
        let initial = if f.status == "revoked" {
            "approved"
        } else {
            "pending"
        };
        ops::insert_follow(&mut tx, actor, follower, followee, initial).await?;
        if f.status == "declined" || f.status == "revoked" {
            sqlx::query(
                "UPDATE app.follows SET status = $3, decided_at = coalesce(decided_at, now()) \
                 WHERE follower_id = $1 AND followee_id = $2",
            )
            .bind(follower)
            .bind(followee)
            .bind(&f.status)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        } else if f.status == "approved" {
            sqlx::query(
                "UPDATE app.follows SET status = 'approved', decided_at = now() WHERE follower_id = $1 AND followee_id = $2",
            )
            .bind(follower)
            .bind(followee)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }
        tx.commit().await.map_err(db_err)?;
        counts.follows += 1;
    }

    // Stations: one credential per owner, the fixture poller key encrypted to the local public key.
    if !no_stations {
        let poller_key = SecretString::from(seed.poller_key.clone());
        let mut owners: HashMap<String, Uuid> = HashMap::new();
        for s in &seed.station {
            let taken: bool =
                sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM app.stations WHERE mac = $1)")
                    .bind(crate::validate::mac(&s.mac)?)
                    .fetch_one(&ctx.pool)
                    .await
                    .map_err(db_err)?;
            if taken {
                continue;
            }
            let owner = match owners.get(&s.owner) {
                Some(id) => *id,
                None => {
                    let id = user_id(ctx, &s.owner).await?;
                    owners.insert(s.owner.clone(), id);
                    id
                }
            };
            let has_live: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM app.credentials WHERE owner_id = $1 AND status <> 'replaced')",
            )
            .bind(owner)
            .fetch_one(&ctx.pool)
            .await
            .map_err(db_err)?;
            ops::add_station(
                &ctx.pool,
                &StationSpec {
                    owner,
                    mac: s.mac.clone(),
                    name: s.name.clone(),
                    lat: s.lat,
                    lon: s.lon,
                    elevation_ft: s.elevation_ft,
                    timezone: s.timezone.clone(),
                    std_offset_minutes: s.std_offset_minutes,
                    reference_station: Some(s.reference_station.clone()),
                    mode: if has_live {
                        CredentialMode::Reuse
                    } else {
                        CredentialMode::New
                    },
                },
                &KeyMaterial {
                    public_key,
                    key_version,
                    api_key: Some(&poller_key),
                },
            )
            .await?;
            counts.stations += 1;
        }
    }

    let Counts {
        reference,
        users,
        follows,
        stations,
        deactivated,
    } = counts;
    if reference + users + follows + stations + deactivated == 0 {
        println!("seed-dev: nothing to do (already seeded)");
    } else {
        println!(
            "seed-dev: {users} user(s), {reference} reference station(s), {follows} extra follow(s), {stations} station(s), {deactivated} deactivation(s)"
        );
    }
    Ok(())
}

async fn user_id(ctx: &Ctx, display_name: &str) -> Result<Uuid> {
    ops::find_user(&ctx.pool, &UserRef::DisplayName(display_name.to_owned()))
        .await?
        .map(|u| u.id)
        .ok_or_else(|| anyhow!("seed.toml refers to an unknown user {display_name:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
poller_key = "fixture-poller-key"

[[reference_station]]
id = "USW00099001"
name = "Demo A"
lat = 46.6
lon = -112.0
timezone = "America/Denver"
std_offset_minutes = -420

[[user]]
display_name = "A"
email = "a@example.test"
admin = true

[[follow]]
follower = "A"
followee = "B"
status = "pending"
"#;

    #[test]
    fn parses_a_seed_file() {
        let s = parse(SAMPLE).unwrap();
        assert_eq!(s.user.len(), 1);
        assert!(s.user[0].admin && !s.user[0].inactive && !s.user[0].follow_all);
        assert_eq!(s.reference_station[0].id, "USW00099001");
        assert!(s.station.is_empty());
    }

    #[test]
    fn rejects_unknown_fields_and_statuses() {
        assert!(parse(&SAMPLE.replace("status = \"pending\"", "status = \"bogus\"")).is_err());
        assert!(parse(&format!(
            "{SAMPLE}\n[[user]]\ndisplay_name = \"X\"\nemail = \"x@example.test\"\nsurprise = 1\n"
        ))
        .is_err());
        assert!(parse("poller_key = \"\"").is_err());
        assert!(parse("[[user]]\ndisplay_name = \"X\"\nemail = \"x@example.test\"").is_err());
        // poller_key missing
    }
}
