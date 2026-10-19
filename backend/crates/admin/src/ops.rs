//! Building blocks shared by the commands and by `seed-dev` (which runs "the same internal functions as the real
//! commands", 04 section 12.1).

use anyhow::{anyhow, bail, Result};
use famweather_core::config::{CredentialMode, UserRef};
use famweather_core::crypto::{self, CredentialPublicKey};
use secrecy::SecretString;
use serde_json::json;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::audit;
use crate::ctx::{db_err, is_unique_violation, Ctx};
use crate::validate;

#[derive(Debug, Clone)]
pub struct UserRow {
    pub id: Uuid,
    pub display_name: String,
    pub is_active: bool,
    pub is_admin: bool,
}

type RawUser = (Uuid, String, bool, bool);

fn user_row(r: RawUser) -> UserRow {
    UserRow {
        id: r.0,
        display_name: r.1,
        is_active: r.2,
        is_admin: r.3,
    }
}

pub async fn find_user(pool: &PgPool, who: &UserRef) -> Result<Option<UserRow>> {
    let row: Option<RawUser> = match who {
        UserRef::Id(id) => {
            sqlx::query_as(
                "SELECT id, display_name, is_active, is_admin FROM app.users WHERE id = $1",
            )
            .bind(id)
            .fetch_optional(pool)
            .await
        }
        UserRef::DisplayName(name) => {
            sqlx::query_as(
                "SELECT id, display_name, is_active, is_admin FROM app.users \
                 WHERE lower(btrim(display_name)) = lower(btrim($1))",
            )
            .bind(name)
            .fetch_optional(pool)
            .await
        }
    }
    .map_err(db_err)?;
    Ok(row.map(user_row))
}

async fn exists(pool: &PgPool, sql: &str, bind: &str) -> Result<bool> {
    sqlx::query_scalar(sql)
        .bind(bind)
        .fetch_one(pool)
        .await
        .map_err(db_err)
}

// ---------------------------------------------------------------------------------------------------------------
// invite (04 section 10.3)

pub struct InviteSpec {
    pub email: String,
    pub display_name: String,
    pub follow_all: bool,
    pub admin: bool,
    pub adopt: bool,
    /// `seed-dev` creates logins with the demo password; `invite` creates them with none.
    pub password: Option<SecretString>,
    /// `invite` mints a set-password token; `seed-dev` does not.
    pub mint_link: bool,
    /// Compose-test fault injection (the single mechanism, 04 section 10.3): fail after the login was created.
    pub test_fail_after_auth: bool,
}

pub struct Invited {
    pub id: Uuid,
    /// The `hashed_token` of the one-time link, when requested. Never logged or audited.
    pub token: Option<SecretString>,
}

/// All-or-nothing: the login and the app rows exist together or not at all.
pub async fn invite(ctx: &Ctx, spec: &InviteSpec) -> Result<Invited> {
    let email = validate::email(&spec.email)?;
    let name = validate::display_name(&spec.display_name)?;
    let gt = ctx.gotrue()?;

    // 1. Everything that can be refused is refused before any Supabase call.
    if exists(
        &ctx.pool,
        "SELECT EXISTS (SELECT 1 FROM app.users WHERE lower(btrim(display_name)) = lower(btrim($1)))",
        &name,
    )
    .await?
    {
        bail!("a user with this display name already exists");
    }
    if spec.admin {
        let admin_exists: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM app.users WHERE is_admin)")
                .fetch_one(&ctx.pool)
                .await
                .map_err(db_err)?;
        if admin_exists {
            bail!("an admin already exists");
        }
    }

    // 2. Follow targets.
    let targets: Vec<Uuid> = if spec.follow_all {
        sqlx::query_scalar("SELECT id FROM app.users WHERE is_active ORDER BY created_at, id")
            .fetch_all(&ctx.pool)
            .await
            .map_err(db_err)?
    } else {
        Vec::new()
    };

    // 3. The login.
    let (id, created) = match gt
        .create_user(&email, Some(&name), spec.password.as_ref(), None)
        .await
    {
        Ok(id) => (id, true),
        Err(e) if e.is_email_exists() && spec.adopt => {
            let user = gt
                .find_user_by_email(&email)
                .await?
                .ok_or_else(|| anyhow!("the login to adopt was not found"))?;
            let has_row: bool =
                sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM app.users WHERE id = $1)")
                    .bind(user.id)
                    .fetch_one(&ctx.pool)
                    .await
                    .map_err(db_err)?;
            if has_row {
                bail!("that login already has an app user; nothing to adopt");
            }
            (user.id, false)
        }
        Err(e) if e.is_email_exists() => {
            bail!("user already exists (use --adopt to repair a login left without app rows)")
        }
        Err(e) => return Err(e.into()),
    };

    // 4. One transaction for the app rows.
    if let Err(e) = insert_app_user(
        ctx,
        id,
        &name,
        spec.admin,
        &targets,
        spec.test_fail_after_auth,
    )
    .await
    {
        // 5. Roll back the login too (unless --adopt found a pre-existing one).
        if created {
            return match gt.delete_user(id).await {
                Ok(()) => Err(e.context("could not create the app rows; the new login was removed")),
                Err(_) => Err(e.context(format!(
                    "could not create the app rows and could not remove the new login; repair with: \
                     admin invite --email {email} --display-name {name:?} --adopt"
                ))),
            };
        }
        return Err(e);
    }

    // 6. The one-time link.
    let token = if spec.mint_link {
        match gt.generate_recovery_token(&email).await {
            Ok(t) => Some(t),
            Err(e) => bail!(
                "the user and app rows exist, but the set-password link could not be created ({e}); \
                 repair with: admin reset-link --email {email}"
            ),
        }
    } else {
        None
    };
    Ok(Invited { id, token })
}

async fn insert_app_user(
    ctx: &Ctx,
    id: Uuid,
    name: &str,
    is_admin: bool,
    targets: &[Uuid],
    fail_for_test: bool,
) -> Result<()> {
    let mut tx = ctx.pool.begin().await.map_err(db_err)?;
    sqlx::query("INSERT INTO app.users (id, display_name, is_admin) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(name)
        .bind(is_admin)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            if is_unique_violation(&e) {
                anyhow!("a user with this display name (or an admin) already exists")
            } else {
                db_err(e)
            }
        })?;
    if fail_for_test {
        bail!("injected failure (--test-fail-after-auth)");
    }
    let actor = audit::actor(&mut tx).await?;
    audit::write(&mut tx, actor, "user.invited", "user", Some(id), json!({})).await?;
    for &target in targets {
        for (follower, followee) in [(id, target), (target, id)] {
            insert_follow(&mut tx, actor, follower, followee, "approved").await?;
        }
    }
    tx.commit().await.map_err(db_err)?;
    Ok(())
}

/// Inserts one follow row in the given final-or-initial state and its `follow.seeded` audit row.
pub async fn insert_follow(
    conn: &mut PgConnection,
    actor: Option<Uuid>,
    follower: Uuid,
    followee: Uuid,
    status: &str,
) -> Result<()> {
    let decided = status != "pending";
    sqlx::query(
        "INSERT INTO app.follows (follower_id, followee_id, status, decided_at) \
         VALUES ($1, $2, $3, CASE WHEN $4 THEN now() END)",
    )
    .bind(follower)
    .bind(followee)
    .bind(status)
    .bind(decided)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    audit::write(
        conn,
        actor,
        "follow.seeded",
        "follow",
        Some(follower),
        audit::followee_detail(followee),
    )
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// deactivate (04 section 10.5)

pub enum Deactivated {
    Now,
    Already,
}

pub async fn deactivate(ctx: &Ctx, who: &UserRef, force: bool) -> Result<(UserRow, Deactivated)> {
    let user = find_user(&ctx.pool, who)
        .await?
        .ok_or_else(|| anyhow!("no such user"))?;
    if user.is_admin && !force {
        bail!("refusing to deactivate the admin (use --force)");
    }
    let outcome = if user.is_active {
        let mut tx = ctx.pool.begin().await.map_err(db_err)?;
        sqlx::query("UPDATE app.users SET is_active = false, deactivated_at = now() WHERE id = $1")
            .bind(user.id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        let actor = audit::actor(&mut tx).await?;
        audit::write(
            &mut tx,
            actor,
            "user.deactivated",
            "user",
            Some(user.id),
            json!({}),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Deactivated::Now
    } else {
        Deactivated::Already
    };
    // The API rejects the user from here on. Now stop refresh-token use and, best effort, ban the login.
    let revoked: i32 = sqlx::query_scalar("SELECT app_auth.revoke_sessions($1)")
        .bind(user.id)
        .fetch_one(&ctx.pool)
        .await
        .map_err(db_err)?;
    tracing::info!(revoked, "sessions revoked");
    match &ctx.gotrue {
        Some(gt) => {
            if let Err(e) = gt.ban_user(user.id).await {
                eprintln!("warning: could not ban the login ({e}); the user is inactive and signed out");
            }
        }
        None => eprintln!(
            "warning: SUPABASE_URL / SUPABASE_SERVICE_ROLE_KEY not set; the login was not banned (sessions were revoked)"
        ),
    }
    Ok((user, outcome))
}

// ---------------------------------------------------------------------------------------------------------------
// reference stations

pub struct ReferenceSpec {
    pub id: String,
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    pub timezone: String,
    pub std_offset_minutes: i16,
    pub elevation_ft: Option<i32>,
}

/// Inserts a reference station; `false` when it already existed (nothing is changed).
pub async fn add_reference_station(pool: &PgPool, spec: &ReferenceSpec) -> Result<bool> {
    let id = validate::ghcn_id(&spec.id)?;
    let name = spec.name.trim();
    if name.is_empty() {
        bail!("name must not be empty");
    }
    if !(-90.0..=90.0).contains(&spec.lat) || !(-180.0..=180.0).contains(&spec.lon) {
        bail!("coordinates out of range");
    }
    let tz = validate::timezone(&spec.timezone)?;
    let offset = validate::std_offset(spec.std_offset_minutes)?;
    let done = sqlx::query(
        "INSERT INTO app.reference_stations (id, name, lat, lon, elevation_ft, timezone, std_utc_offset_minutes) \
         VALUES ($1, $2, $3::float8, $4::float8, $5, $6, $7) ON CONFLICT (id) DO NOTHING",
    )
    .bind(id)
    .bind(name)
    .bind(spec.lat)
    .bind(spec.lon)
    .bind(spec.elevation_ft)
    .bind(tz)
    .bind(offset)
    .execute(pool)
    .await
    .map_err(db_err)?;
    Ok(done.rows_affected() == 1)
}

// ---------------------------------------------------------------------------------------------------------------
// stations (04 section 10.4)

pub struct StationSpec {
    pub owner: Uuid,
    pub mac: String,
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    pub elevation_ft: Option<i32>,
    pub timezone: String,
    pub std_offset_minutes: i16,
    pub reference_station: Option<String>,
    pub mode: CredentialMode,
}

pub struct KeyMaterial<'a> {
    pub public_key: &'a CredentialPublicKey,
    pub key_version: i16,
    /// The owner's AWN key; `None` only when the credential plan reuses an existing row.
    pub api_key: Option<&'a SecretString>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialPlan {
    /// The owner has no live credential: insert one.
    New,
    /// Use the existing live credential as is.
    Reuse(Uuid),
    /// Mark the existing live credential `replaced` and insert a new one.
    Replace(Uuid),
}

impl CredentialPlan {
    pub fn needs_key(self) -> bool {
        !matches!(self, CredentialPlan::Reuse(_))
    }
}

pub async fn plan_credential(
    conn: &mut PgConnection,
    owner: Uuid,
    mode: CredentialMode,
) -> Result<CredentialPlan> {
    let live: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT id, status FROM app.credentials WHERE owner_id = $1 AND status <> 'replaced'",
    )
    .bind(owner)
    .fetch_optional(conn)
    .await
    .map_err(db_err)?;
    Ok(match (live, mode) {
        (None, _) => CredentialPlan::New,
        (Some(_), CredentialMode::New) => {
            bail!("the owner already has a credential; use --reuse-credential or --replace-key")
        }
        (Some((id, status)), CredentialMode::Reuse) => {
            if status != "ok" {
                bail!("the owner's credential has status {status}; reconnect with --replace-key");
            }
            CredentialPlan::Reuse(id)
        }
        (Some((id, _)), CredentialMode::Replace) => CredentialPlan::Replace(id),
    })
}

/// Resolves the owner (an active user) by id or display name.
pub async fn resolve_active_owner(pool: &PgPool, who: &UserRef) -> Result<UserRow> {
    let user = find_user(pool, who)
        .await?
        .ok_or_else(|| anyhow!("no such owner"))?;
    if !user.is_active {
        bail!("the owner is not active");
    }
    Ok(user)
}

/// One transaction: credential (per the plan) and station, with audit rows. Returns the station id.
pub async fn add_station(
    pool: &PgPool,
    spec: &StationSpec,
    keys: &KeyMaterial<'_>,
) -> Result<Uuid> {
    let mac = validate::mac(&spec.mac)?;
    let name = spec.name.trim().to_owned();
    if name.is_empty() || name.chars().count() > 80 {
        bail!("station name must be 1 to 80 characters");
    }
    let lat = validate::coordinate(spec.lat, 90.0, "latitude")?;
    let lon = validate::coordinate(spec.lon, 180.0, "longitude")?;
    let tz = validate::timezone(&spec.timezone)?;
    let offset = validate::std_offset(spec.std_offset_minutes)?;
    let elevation = validate::elevation_ft(spec.elevation_ft)?;
    if let Some(r) = &spec.reference_station {
        validate::ghcn_id(r)?;
        let known: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM app.reference_stations WHERE id = $1)",
        )
        .bind(r)
        .fetch_one(pool)
        .await
        .map_err(db_err)?;
        if !known {
            bail!(
                "reference station {r} does not exist; create it first with add-reference-station"
            );
        }
    }

    let mut tx = pool.begin().await.map_err(db_err)?;
    let plan = plan_credential(&mut tx, spec.owner, spec.mode).await?;
    let actor = audit::actor(&mut tx).await?;

    let credential_id = match plan {
        CredentialPlan::Reuse(id) => id,
        CredentialPlan::New | CredentialPlan::Replace(_) => {
            let key = keys
                .api_key
                .ok_or_else(|| anyhow!("an AWN API key is required for this operation"))?;
            if let CredentialPlan::Replace(old) = plan {
                sqlx::query(
                    "UPDATE app.credentials SET status = 'replaced', status_changed_at = now() \
                     WHERE id = $1 AND owner_id = $2",
                )
                .bind(old)
                .bind(spec.owner)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            }
            let new_id = Uuid::new_v4();
            let enc = crypto::encrypt_credential(
                keys.public_key,
                keys.key_version,
                new_id,
                spec.owner,
                key,
            )
            .map_err(|e| anyhow!("{e}"))?;
            sqlx::query(
                "INSERT INTO app.credentials (id, owner_id, api_key_ciphertext, encapsulated_key, key_version) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(new_id)
            .bind(spec.owner)
            .bind(&enc.ciphertext)
            .bind(&enc.encapsulated_key)
            .bind(enc.key_version)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
            if let CredentialPlan::Replace(old) = plan {
                // Every station of the owner moves to the new row (no validation in production, 04 10.4).
                sqlx::query("UPDATE app.stations SET credential_id = $1 WHERE owner_id = $2")
                    .bind(new_id)
                    .bind(spec.owner)
                    .execute(&mut *tx)
                    .await
                    .map_err(db_err)?;
                audit::write(
                    &mut tx,
                    actor,
                    "credential.saved",
                    "credential",
                    Some(new_id),
                    json!({}),
                )
                .await?;
                audit::write(
                    &mut tx,
                    actor,
                    "credential.replaced",
                    "credential",
                    Some(old),
                    json!({}),
                )
                .await?;
            } else {
                audit::write(
                    &mut tx,
                    actor,
                    "credential.saved",
                    "credential",
                    Some(new_id),
                    json!({}),
                )
                .await?;
            }
            new_id
        }
    };

    let station_id: Uuid = sqlx::query_scalar(
        "INSERT INTO app.stations \
         (mac, name, owner_id, credential_id, lat, lon, elevation_ft, timezone, std_utc_offset_minutes, reference_station_id) \
         VALUES ($1, $2, $3, $4, $5::float8, $6::float8, $7, $8, $9, $10) RETURNING id",
    )
    .bind(&mac)
    .bind(&name)
    .bind(spec.owner)
    .bind(credential_id)
    .bind(lat)
    .bind(lon)
    .bind(elevation)
    .bind(&tz)
    .bind(offset)
    .bind(&spec.reference_station)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        if is_unique_violation(&e) {
            anyhow!("a station with this MAC already exists")
        } else {
            db_err(e)
        }
    })?;
    audit::write(
        &mut tx,
        actor,
        "station.created",
        "station",
        Some(station_id),
        json!({}),
    )
    .await?;
    if let Some(r) = &spec.reference_station {
        audit::write(
            &mut tx,
            actor,
            "station.baseline_set",
            "station",
            Some(station_id),
            json!({ "reference_station_id": r }),
        )
        .await?;
    }
    tx.commit().await.map_err(db_err)?;
    Ok(station_id)
}
