//! The subcommands (04 section 10.2). Output goes to stdout only where the guide says so (links, ids, tables);
//! diagnostics go to stderr. Emails of other users are never echoed except by `list-users`, which exists to print them.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, IsTerminal};
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use famweather_core::awn::{ApiKey, AwnApi, LiveAwn, Mac};
use famweather_core::config::{AdminCommand, AdminEnv, AppEnv, AwnMode, OwnerRef, UserRef};
use famweather_core::{crypto, ratelimit, redact};
use secrecy::{ExposeSecret, SecretString};
use serde_json::json;
use sqlx::migrate::Migrator;
use uuid::Uuid;

use crate::audit;
use crate::cli::Extras;
use crate::ctx::{connect_pool, db_err, Ctx};
use crate::ops::{self, Deactivated, InviteSpec, KeyMaterial, ReferenceSpec, StationSpec};
use crate::validate;

static MIGRATOR: Migrator = sqlx::migrate!("../../migrations");

// ---------------------------------------------------------------------------------------------------------------
// migrate

async fn applied_count(pool: &sqlx::PgPool) -> Result<i64> {
    // Two statements: the count cannot be parsed before the table exists.
    let exists: bool = sqlx::query_scalar("SELECT to_regclass('app._sqlx_migrations') IS NOT NULL")
        .fetch_one(pool)
        .await
        .map_err(db_err)?;
    if !exists {
        return Ok(0);
    }
    sqlx::query_scalar("SELECT count(*) FROM app._sqlx_migrations")
        .fetch_one(pool)
        .await
        .map_err(db_err)
}

pub async fn migrate(env: &AdminEnv) -> Result<()> {
    let pool = connect_pool(env).await?;
    let user: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(&pool)
        .await
        .map_err(db_err)?;
    if user != "app_migrator" && !user.starts_with("app_migrator.") {
        bail!("migrations must run as app_migrator (connected as a different role)");
    }
    let before = applied_count(&pool).await?;
    MIGRATOR
        .run(&pool)
        .await
        .map_err(|e| anyhow!("migration failed: {e}"))?;
    let after = applied_count(&pool).await?;
    println!(
        "migrations up to date: {after} applied in total ({} newly applied)",
        after - before
    );
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// keygen

pub fn keygen(check: bool) -> Result<()> {
    if check {
        let sk = std::env::var("CREDENTIALS_PRIVATE_KEY")
            .map_err(|_| anyhow!("CREDENTIALS_PRIVATE_KEY is not set"))?;
        let pk = std::env::var("CREDENTIALS_PUBLIC_KEY")
            .map_err(|_| anyhow!("CREDENTIALS_PUBLIC_KEY is not set"))?;
        let sk = crypto::parse_private_key(sk.trim())
            .map_err(|_| anyhow!("CREDENTIALS_PRIVATE_KEY is not a base64 32-byte key"))?;
        let pk = crypto::parse_public_key(pk.trim())
            .map_err(|_| anyhow!("CREDENTIALS_PUBLIC_KEY is not a base64 32-byte key"))?;
        if !crypto::keypair_matches(&sk, &pk) {
            bail!("the private and public keys do not match");
        }
        println!("OK: CREDENTIALS_PRIVATE_KEY and CREDENTIALS_PUBLIC_KEY are a matching pair");
        return Ok(());
    }
    let (pk, sk) = crypto::generate_keypair();
    println!("CREDENTIALS_PUBLIC_KEY={}", pk.to_base64());
    println!("CREDENTIALS_PRIVATE_KEY={}", sk.to_base64().expose_secret());
    eprintln!(
        "Set CREDENTIALS_KEY_VERSION to the version these keys belong to (the next integer on rotation). \
         The private key goes to the password manager and to the poller's secrets only."
    );
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// wait

pub async fn wait(minutes: u32) -> Result<()> {
    eprintln!("admin: idle for {minutes} minute(s); run commands over `fly ssh console`");
    tokio::time::sleep(Duration::from_secs(u64::from(minutes) * 60)).await;
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// invite / reset-link

/// The line printed once to stdout: `<SITE_URL origin>/set-password#token_hash=<hash>&type=recovery`.
pub fn set_password_link(env: &AdminEnv, token: &SecretString) -> Result<String> {
    let site = env
        .site_url
        .as_ref()
        .ok_or_else(|| anyhow!("SITE_URL is required"))?;
    Ok(format!(
        "{}/set-password#token_hash={}&type=recovery",
        site.origin().ascii_serialization(),
        token.expose_secret()
    ))
}

fn print_link(env: &AdminEnv, id: Uuid, token: &SecretString) -> Result<()> {
    let link = set_password_link(env, token)?;
    println!("{id}");
    println!("{link}");
    println!(
        "Send this privately; it expires in about an hour (the Email OTP expiration) and works for the person who opens it first."
    );
    Ok(())
}

pub async fn invite(
    ctx: &Ctx,
    email: String,
    display_name: String,
    follow_all: bool,
    admin: bool,
    adopt: bool,
    extras: Extras,
) -> Result<()> {
    let spec = InviteSpec {
        email,
        display_name,
        follow_all,
        admin,
        adopt,
        password: None,
        mint_link: true,
        test_fail_after_auth: extras.test_fail_after_auth,
    };
    let invited = ops::invite(ctx, &spec).await?;
    let token = invited.token.expect("invite mints a link");
    print_link(&ctx.env, invited.id, &token)
}

pub async fn reset_link(ctx: &Ctx, email: String) -> Result<()> {
    let email = validate::email(&email)?;
    let gt = ctx.gotrue()?;
    // 1. An active user with an app row; nothing is minted for deactivated or unknown people.
    let login = gt.find_user_by_email(&email).await?;
    let active_id = match login {
        Some(u) => {
            let row = ops::find_user(&ctx.pool, &UserRef::Id(u.id)).await?;
            row.filter(|r| r.is_active).map(|r| r.id)
        }
        None => None,
    };
    let id = active_id.ok_or_else(|| anyhow!("no active user with that email"))?;
    // 2. The link.
    let token = gt.generate_recovery_token(&email).await?;
    // 3. Sign the person out everywhere. If this fails nothing is printed.
    sqlx::query_scalar::<_, i32>("SELECT app_auth.revoke_sessions($1)")
        .bind(id)
        .fetch_one(&ctx.pool)
        .await
        .map_err(db_err)?;
    // 4. Audit (no token), then print.
    let mut conn = ctx.pool.acquire().await.map_err(db_err)?;
    let actor = audit::actor(&mut conn).await?;
    audit::write(
        &mut conn,
        actor,
        "user.reset_link_issued",
        "user",
        Some(id),
        json!({}),
    )
    .await?;
    drop(conn);
    print_link(&ctx.env, id, &token)
}

// ---------------------------------------------------------------------------------------------------------------
// reference stations and stations

pub async fn add_reference_station(ctx: &Ctx, cmd: AdminCommand) -> Result<()> {
    let AdminCommand::AddReferenceStation {
        id,
        name,
        lat,
        lon,
        timezone,
        std_offset_minutes,
        elevation_ft,
    } = cmd
    else {
        unreachable!()
    };
    let created = ops::add_reference_station(
        &ctx.pool,
        &ReferenceSpec {
            id: id.clone(),
            name,
            lat,
            lon,
            timezone,
            std_offset_minutes,
            elevation_ft,
        },
    )
    .await?;
    println!(
        "{id} {}",
        if created {
            "created"
        } else {
            "already exists (unchanged)"
        }
    );
    Ok(())
}

/// Reads the AWN key without echo: a hidden prompt on a terminal, otherwise exactly one line of stdin.
fn read_api_key() -> Result<SecretString> {
    let raw = if std::io::stdin().is_terminal() {
        rpassword::prompt_password("AWN API key: ").context("could not read the key")?
    } else {
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .context("could not read the key from stdin")?;
        line
    };
    let key = raw.trim().to_owned();
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_graphic()) {
        bail!("the AWN API key must be non-empty printable ASCII");
    }
    Ok(SecretString::from(key))
}

pub async fn add_station(ctx: &Ctx, cmd: AdminCommand) -> Result<()> {
    let AdminCommand::AddStation {
        owner,
        mac,
        name,
        lat,
        lon,
        timezone,
        std_offset_minutes,
        elevation_ft,
        reference_station,
        credential,
    } = cmd
    else {
        unreachable!()
    };
    let env = &ctx.env;
    let public_key = env
        .credentials_public_key
        .as_ref()
        .ok_or_else(|| anyhow!("CREDENTIALS_PUBLIC_KEY is required"))?;
    let key_version = env
        .credentials_key_version
        .ok_or_else(|| anyhow!("CREDENTIALS_KEY_VERSION is required"))?;
    let who = match owner {
        OwnerRef::Id(id) => UserRef::Id(id),
        OwnerRef::DisplayName(n) => UserRef::DisplayName(n),
    };
    let owner = ops::resolve_active_owner(&ctx.pool, &who).await?;
    let mac = validate::mac(&mac)?;

    // Decide whether a key is needed before prompting for one.
    let plan = {
        let mut conn = ctx.pool.acquire().await.map_err(db_err)?;
        ops::plan_credential(&mut conn, owner.id, credential).await?
    };
    let api_key = if plan.needs_key() {
        Some(read_api_key()?)
    } else {
        None
    };
    let _scrub = api_key.as_ref().map(redact::register_scoped);

    // Validation happens only locally, in live mode, with an application key; production never validates.
    let validate_live = env.app_env == Some(AppEnv::Local)
        && env.awn_mode == Some(AwnMode::Live)
        && env.awn_application_key.is_some();
    let mut validated = false;
    if let (true, Some(key)) = (validate_live, &api_key) {
        let app_key = env.awn_application_key.clone().expect("checked");
        let client = LiveAwn::new(app_key, Duration::from_secs(10)).map_err(|e| anyhow!("{e}"))?;
        let wait = ratelimit::reserve(
            &ctx.pool,
            "awn:app:api",
            Duration::from_secs(1),
            Duration::from_secs(3),
        )
        .await
        .map_err(|e| anyhow!("{e}"))?;
        tokio::time::sleep(wait).await;
        let devices = client
            .list_devices(&ApiKey::new(key.clone()))
            .await
            .map_err(|e| anyhow!("key validation failed: {e}"))?;
        let want = Mac::parse(&mac).map_err(|_| anyhow!("invalid MAC address"))?;
        if !devices.iter().any(|d| d.mac == want) {
            bail!("the key is valid but does not list a device with that MAC");
        }
        validated = true;
    }
    if !validated && api_key.is_some() {
        eprintln!("key not validated");
    }

    let station_id = ops::add_station(
        &ctx.pool,
        &StationSpec {
            owner: owner.id,
            mac,
            name,
            lat,
            lon,
            elevation_ft,
            timezone,
            std_offset_minutes,
            reference_station,
            mode: credential,
        },
        &KeyMaterial {
            public_key,
            key_version,
            api_key: api_key.as_ref(),
        },
    )
    .await?;
    println!("{station_id}");
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// users

pub async fn deactivate_user(ctx: &Ctx, who: UserRef, force: bool) -> Result<()> {
    let (user, outcome) = ops::deactivate(ctx, &who, force).await?;
    match outcome {
        Deactivated::Now => println!("deactivated {}", user.id),
        Deactivated::Already => println!("already inactive {}", user.id),
    }
    Ok(())
}

pub async fn delete_user(ctx: &Ctx, id: Uuid, confirm_email: String) -> Result<()> {
    let gt = ctx.gotrue()?;
    let user = ops::find_user(&ctx.pool, &UserRef::Id(id))
        .await?
        .ok_or_else(|| anyhow!("no such user"))?;
    if user.is_active {
        bail!("the user is still active; deactivate first");
    }
    let login = gt.get_user(id).await?;
    match login.as_ref().and_then(|u| u.email.as_deref()) {
        Some(email) if email.eq_ignore_ascii_case(confirm_email.trim()) => {}
        _ => bail!("--confirm-email does not match the user's login"),
    }
    let mut tx = ctx.pool.begin().await.map_err(db_err)?;
    let actor = audit::actor(&mut tx).await?;
    audit::write(&mut tx, actor, "user.deleted", "user", Some(id), json!({})).await?;
    sqlx::query("DELETE FROM app.users WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    match gt.delete_user(id).await {
        Ok(()) => {
            println!("deleted {id}");
            Ok(())
        }
        Err(e) if e.is_not_found() => {
            println!("deleted {id} (the login was already gone)");
            Ok(())
        }
        Err(e) => bail!(
            "the app data was deleted but the login could not be removed ({e}); \
             delete user {id} in the Supabase dashboard"
        ),
    }
}

pub async fn list_users(ctx: &Ctx) -> Result<bool> {
    type Row = (Uuid, String, bool, bool);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, display_name, is_active, is_admin FROM app.users ORDER BY created_at, display_name",
    )
    .fetch_all(&ctx.pool)
    .await
    .map_err(db_err)?;
    let logins: Option<HashMap<Uuid, Option<String>>> = match &ctx.gotrue {
        Some(gt) => Some(
            gt.list_users()
                .await?
                .into_iter()
                .map(|u| (u.id, u.email))
                .collect(),
        ),
        None => None,
    };
    let mut clean = true;
    for (id, name, active, admin) in &rows {
        let state = if *active { "active" } else { "inactive" };
        let admin = if *admin { "admin" } else { "-" };
        match &logins {
            Some(map) => {
                let email = map.get(id).cloned().flatten().unwrap_or_default();
                let auth = if map.contains_key(id) {
                    "auth_ok"
                } else {
                    "auth_missing"
                };
                clean &= map.contains_key(id);
                println!("{id}\t{name}\t{email}\t{state}\t{admin}\t{auth}");
            }
            None => println!("{id}\t{name}\t\t{state}\t{admin}"),
        }
    }
    if let Some(map) = &logins {
        let known: HashSet<Uuid> = rows.iter().map(|r| r.0).collect();
        let mut orphans: Vec<Uuid> = map
            .keys()
            .filter(|id| !known.contains(id))
            .copied()
            .collect();
        orphans.sort();
        for id in orphans {
            println!("auth_only\t{id}");
            clean = false;
        }
    }
    Ok(clean)
}

// ---------------------------------------------------------------------------------------------------------------
// restore-auth-users

pub async fn restore_auth_users(ctx: &Ctx, file: &Path, dry_run: bool) -> Result<bool> {
    let gt = ctx.gotrue()?;
    let mut reader = csv::Reader::from_path(file).context("cannot open the CSV file")?;
    let headers = reader
        .headers()
        .context("cannot read the CSV header")?
        .clone();
    let col = |name: &str| {
        headers
            .iter()
            .position(|h| h.trim().eq_ignore_ascii_case(name))
            .ok_or_else(|| anyhow!("the CSV has no {name} column"))
    };
    let (id_col, email_col) = (col("id")?, col("email")?);

    let existing: HashSet<Uuid> = gt.list_users().await?.into_iter().map(|u| u.id).collect();
    let mut in_file = HashSet::new();
    let (mut created, mut skipped, mut no_row, mut failed) = (0usize, 0usize, 0usize, 0usize);
    for (n, rec) in reader.records().enumerate() {
        let line = n + 2;
        let rec = match rec {
            Ok(r) => r,
            Err(_) => {
                eprintln!("line {line}: unreadable row");
                failed += 1;
                continue;
            }
        };
        let (Some(id), Some(email)) = (
            rec.get(id_col).and_then(|s| s.trim().parse::<Uuid>().ok()),
            rec.get(email_col).map(str::trim),
        ) else {
            eprintln!("line {line}: missing or invalid id / email");
            failed += 1;
            continue;
        };
        in_file.insert(id);
        let Some(app_user) = ops::find_user(&ctx.pool, &UserRef::Id(id)).await? else {
            println!("no app user for {id} (not created)");
            no_row += 1;
            continue;
        };
        if existing.contains(&id) {
            println!("skip {id} (login exists)");
            skipped += 1;
            continue;
        }
        if dry_run {
            println!("would create {id}");
            created += 1;
            continue;
        }
        let email = match validate::email(email) {
            Ok(e) => e,
            Err(_) => {
                eprintln!("line {line}: invalid email");
                failed += 1;
                continue;
            }
        };
        match gt
            .create_user(&email, Some(&app_user.display_name), None, Some(id))
            .await
        {
            Ok(got) if got == id => {
                let mut conn = ctx.pool.acquire().await.map_err(db_err)?;
                let actor = audit::actor(&mut conn).await?;
                audit::write(
                    &mut conn,
                    actor,
                    "user.auth_restored",
                    "user",
                    Some(id),
                    json!({}),
                )
                .await?;
                println!("created {id}");
                created += 1;
            }
            Ok(got) => {
                // The auth service ignored the requested id: the restored login would not line up with app.users.
                eprintln!("{id}: the auth service assigned a different id ({got}); delete that login and investigate");
                failed += 1;
            }
            Err(e) => {
                eprintln!("{id}: {e}");
                failed += 1;
            }
        }
    }
    let mut extra: Vec<Uuid> = existing
        .iter()
        .filter(|id| !in_file.contains(id))
        .copied()
        .collect();
    extra.sort();
    for id in &extra {
        println!("auth user {id} is not in the file");
    }
    println!(
        "{}: {created} created, {skipped} already present, {no_row} without an app user, {} auth users not in the file, {failed} failed",
        if dry_run { "dry run" } else { "restore" },
        extra.len()
    );
    Ok(failed == 0)
}
