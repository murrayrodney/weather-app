//! Connect flow (02 4.6, 5.7): validate an AWN key, save credential and stations, credential status.
//! The key is a `SecretString` end to end: no `Debug`, never logged, never returned.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use chrono::TimeZone;
use chrono_tz::{OffsetComponents, Tz};
use famweather_core::awn::{ApiKey, AwnDevice, AwnError, Mac};
use famweather_core::crypto::encrypt_credential;
use famweather_core::ratelimit::reserve;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::audit;
use crate::auth::AuthUser;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, NoQuery};
use crate::repo::stations;
use crate::state::AppState;

pub const PER_MINUTE: usize = 5;
pub const PER_HOUR: usize = 20;
const BUSY_RETRY_SECS: u64 = 2;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectValidateRequest {
    api_key: SecretString,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceRequest {
    mac: String,
    name: Option<String>,
    timezone: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectSaveRequest {
    api_key: SecretString,
    devices: Vec<DeviceRequest>,
}

fn check_key(key: &SecretString) -> ApiResult<()> {
    let k = key.expose_secret();
    let ok = (1..=128).contains(&k.len())
        && k.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(ApiError::Validation("The key is not valid."))
    }
}

/// Reserves the durable per-user attempt (advisory-locked count + insert, one transaction) BEFORE the AWN call.
async fn begin_attempt(db: &PgPool, user: Uuid) -> ApiResult<i64> {
    let mut tx = db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('connect:' || $1::text))")
        .bind(user)
        .execute(&mut *tx)
        .await?;
    let rows = sqlx::query(
        "SELECT (extract(epoch FROM (attempted_at + interval '1 hour' - now())))::float8 AS hour_left, \
         (extract(epoch FROM (attempted_at + interval '1 minute' - now())))::float8 AS minute_left \
         FROM app.connect_attempts WHERE user_id = $1 AND attempted_at > now() - interval '1 hour' \
         ORDER BY attempted_at",
    )
    .bind(user)
    .fetch_all(&mut *tx)
    .await?;
    let hour: Vec<f64> = rows
        .iter()
        .map(|r| r.try_get("hour_left"))
        .collect::<Result<_, _>>()?;
    let minute: Vec<f64> = rows
        .iter()
        .map(|r| r.try_get::<f64, _>("minute_left"))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|s| *s > 0.0)
        .collect();
    let mut retry: Option<f64> = None;
    if minute.len() >= PER_MINUTE {
        retry = Some(minute[minute.len() - PER_MINUTE]);
    }
    if hour.len() >= PER_HOUR {
        let h = hour[hour.len() - PER_HOUR];
        retry = Some(retry.map_or(h, |m| m.max(h)));
    }
    if let Some(secs) = retry {
        // Rolled back on drop: rejections are not recorded.
        return Err(ApiError::RateLimited {
            retry_after: secs.ceil().max(1.0) as u64,
        });
    }
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO app.connect_attempts (user_id, outcome) VALUES ($1, 'started') RETURNING id",
    )
    .bind(user)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    // Opportunistic cleanup; failure is irrelevant.
    let _ = sqlx::query(
        "DELETE FROM app.connect_attempts WHERE user_id = $1 AND attempted_at < now() - interval '7 days'",
    )
    .bind(user)
    .execute(db)
    .await;
    Ok(id)
}

enum CallErr {
    /// AWN said the key is not valid.
    InvalidKey,
    Api(ApiError),
}

fn busy() -> CallErr {
    CallErr::Api(ApiError::UpstreamBusy {
        retry_after: BUSY_RETRY_SECS,
    })
}

async fn gated_call(
    st: &AppState,
    user: &AuthUser,
    key: &SecretString,
) -> Result<(Vec<AwnDevice>, &'static str), (CallErr, &'static str)> {
    // 1. The caller's own stored key first: a user hammering their key must not consume the shared app budget.
    let live: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM app.credentials WHERE owner_id = $1 AND status IN ('ok', 'rate_limited') LIMIT 1",
    )
    .bind(user.id)
    .fetch_optional(&st.db)
    .await
    .map_err(|e| (CallErr::Api(e.into()), "error"))?;
    if let Some(cid) = live {
        let wait = reserve(
            &st.db,
            &format!("awn:key:{cid}"),
            st.gates.awn_key_spacing,
            st.gates.max_wait,
        )
        .await
        .map_err(|_| (busy(), "rate_limited"))?;
        tokio::time::sleep(wait).await;
    }
    let wait = reserve(
        &st.db,
        "awn:app:api",
        st.gates.awn_app_spacing,
        st.gates.max_wait,
    )
    .await
    .map_err(|_| (busy(), "rate_limited"))?;
    tokio::time::sleep(wait).await;
    match st.awn.list_devices(&ApiKey::new(key.clone())).await {
        Ok(d) => Ok((d, "valid")),
        Err(AwnError::Unauthorized) => Err((CallErr::InvalidKey, "invalid")),
        Err(AwnError::RateLimited { retry_after }) => Err((
            CallErr::Api(ApiError::UpstreamBusy {
                retry_after: retry_after.map_or(BUSY_RETRY_SECS, |d| d.as_secs().clamp(1, 30)),
            }),
            "rate_limited",
        )),
        Err(_) => Err((CallErr::Api(ApiError::UpstreamUnavailable), "error")),
    }
}

/// Limits, attempt row, gates, AWN call, outcome row.
async fn list_devices(
    st: &AppState,
    user: &AuthUser,
    key: &SecretString,
) -> Result<Vec<AwnDevice>, CallErr> {
    let attempt = begin_attempt(&st.db, user.id).await.map_err(CallErr::Api)?;
    let (result, outcome) = match gated_call(st, user, key).await {
        Ok((d, o)) => (Ok(d), o),
        Err((e, o)) => (Err(e), o),
    };
    let _ =
        sqlx::query("UPDATE app.connect_attempts SET outcome = $1 WHERE id = $2 AND user_id = $3")
            .bind(outcome)
            .bind(attempt)
            .bind(user.id)
            .execute(&st.db)
            .await;
    result
}

fn device_tz(d: &AwnDevice) -> Option<Tz> {
    d.timezone.as_deref().and_then(|t| t.parse::<Tz>().ok())
}

pub async fn validate(
    State(st): State<AppState>,
    user: AuthUser,
    ApiJson(req): ApiJson<ConnectValidateRequest>,
) -> ApiResult<Json<Value>> {
    check_key(&req.api_key)?;
    match list_devices(&st, &user, &req.api_key).await {
        Ok(devs) => {
            let devices: Vec<Value> = devs
                .iter()
                .map(|d| {
                    json!({
                        "mac": d.mac.as_str(),
                        "name": d.name,
                        "timezone": device_tz(d).map(|t| t.name()),
                    })
                })
                .collect();
            Ok(Json(json!({ "valid": true, "devices": devices })))
        }
        Err(CallErr::InvalidKey) => Ok(Json(json!({ "valid": false }))),
        Err(CallErr::Api(e)) => Err(e),
    }
}

struct Resolved {
    mac: String,
    name: String,
    lat: f64,
    lon: f64,
    elevation_ft: Option<i32>,
    timezone: String,
    offset_minutes: i16,
}

fn clean_name(raw: &str) -> String {
    let s: String = raw.chars().filter(|c| !c.is_control()).collect();
    s.trim()
        .chars()
        .take(80)
        .collect::<String>()
        .trim()
        .to_owned()
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

fn base_offset_minutes(tz: Tz, now: chrono::DateTime<chrono::Utc>) -> i16 {
    let off = tz.offset_from_utc_datetime(&now.naive_utc());
    (off.base_utc_offset().num_minutes()) as i16
}

fn resolve(
    req: &DeviceRequest,
    awn: &AwnDevice,
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<Resolved> {
    let fallback = || format!("Station {}", awn.mac.last4());
    let name = [req.name.as_deref(), awn.name.as_deref()]
        .into_iter()
        .flatten()
        .map(clean_name)
        .find(|n| !n.is_empty())
        .unwrap_or_else(fallback);
    let client_tz = match req.timezone.as_deref() {
        Some(t) => Some(
            t.parse::<Tz>()
                .map_err(|_| ApiError::Validation("The time zone is not valid."))?,
        ),
        None => None,
    };
    let tz = match (device_tz(awn), client_tz) {
        (Some(a), Some(c)) if a != c => {
            return Err(ApiError::Validation(
                "The time zone does not match the station.",
            ))
        }
        (Some(a), _) => a,
        (None, Some(c)) => c,
        (None, None) => return Err(ApiError::TimezoneRequired),
    };
    let (lat, lon) = awn
        .coords
        .ok_or(ApiError::Validation("The station has no location."))?;
    let elevation_ft = awn
        .elevation_ft
        .map(|e| ((e / 10.0).round() * 10.0) as i32)
        .filter(|e| (-500..=20000).contains(e));
    Ok(Resolved {
        mac: awn.mac.as_str().to_owned(),
        name,
        lat: round2(lat),
        lon: round2(lon),
        elevation_ft,
        timezone: tz.name().to_owned(),
        offset_minutes: base_offset_minutes(tz, now),
    })
}

async fn map_unique(db: &PgPool, e: sqlx::Error, macs: &[String], me: Uuid) -> ApiError {
    if let sqlx::Error::Database(d) = &e {
        if d.code().as_deref() == Some("23505") {
            if d.constraint() == Some("stations_mac_key") {
                let owners: Result<Vec<Uuid>, _> =
                    sqlx::query_scalar("SELECT owner_id FROM app.stations WHERE mac = ANY($1)")
                        .bind(macs)
                        .fetch_all(db)
                        .await;
                if let Ok(o) = owners {
                    if o.iter().any(|x| *x != me) {
                        return ApiError::DeviceAlreadyRegistered;
                    }
                }
            }
            return ApiError::Conflict;
        }
    }
    e.into()
}

pub async fn save(
    State(st): State<AppState>,
    user: AuthUser,
    ApiJson(req): ApiJson<ConnectSaveRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    check_key(&req.api_key)?;
    if req.devices.is_empty() || req.devices.len() > 10 {
        return Err(ApiError::Validation("Give between 1 and 10 stations."));
    }
    let mut wanted = Vec::with_capacity(req.devices.len());
    for d in &req.devices {
        let mac =
            Mac::parse(&d.mac).map_err(|_| ApiError::Validation("A station id is not valid."))?;
        if wanted.contains(&mac) {
            return Err(ApiError::Validation("A station is listed twice."));
        }
        wanted.push(mac);
    }
    let devices = match list_devices(&st, &user, &req.api_key).await {
        Ok(d) => d,
        Err(CallErr::InvalidKey) => return Err(ApiError::InvalidKey),
        Err(CallErr::Api(e)) => return Err(e),
    };
    // MACs, names and coordinates come only from AWN's own list.
    let now = st.clock.now();
    let mut resolved = Vec::with_capacity(wanted.len());
    for (mac, dreq) in wanted.iter().zip(&req.devices) {
        let awn = devices
            .iter()
            .find(|d| &d.mac == mac)
            .ok_or(ApiError::UnknownDevice)?;
        resolved.push(resolve(dreq, awn, now)?);
    }
    let covered: Vec<String> = devices.iter().map(|d| d.mac.as_str().to_owned()).collect();
    let mine: Vec<String> = resolved.iter().map(|r| r.mac.clone()).collect();

    let new_id = Uuid::new_v4();
    let enc = encrypt_credential(
        &st.cred_pubkey,
        st.cfg.env.credentials_key_version,
        new_id,
        user.id,
        &req.api_key,
    )
    .map_err(|_| {
        tracing::error!("credential encryption failed");
        ApiError::Internal
    })?;

    let run = async {
        let mut tx = st.db.begin().await?;
        let old: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM app.credentials WHERE owner_id = $1 AND status <> 'replaced'",
        )
        .bind(user.id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(old) = old {
            sqlx::query(
                "UPDATE app.credentials SET status = 'replaced', status_changed_at = now() WHERE id = $1 AND owner_id = $2",
            )
            .bind(old)
            .bind(user.id)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "INSERT INTO app.credentials (id, owner_id, api_key_ciphertext, encapsulated_key, key_version) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(new_id)
        .bind(user.id)
        .bind(&enc.ciphertext)
        .bind(&enc.encapsulated_key)
        .bind(enc.key_version)
        .execute(&mut *tx)
        .await?;
        let existing =
            sqlx::query("SELECT id, owner_id, mac FROM app.stations WHERE mac = ANY($1)")
                .bind(&mine)
                .fetch_all(&mut *tx)
                .await?;
        let mut touched: Vec<Uuid> = Vec::new();
        let mut created: Vec<Uuid> = Vec::new();
        for r in &resolved {
            let found = existing
                .iter()
                .find(|e| e.try_get::<String, _>("mac").is_ok_and(|m| m == r.mac));
            if let Some(e) = found {
                if e.try_get::<Uuid, _>("owner_id")? != user.id {
                    return Ok(Err(ApiError::DeviceAlreadyRegistered));
                }
                let sid: Uuid = e.try_get("id")?;
                sqlx::query("UPDATE app.stations SET name = $1 WHERE id = $2 AND owner_id = $3")
                    .bind(&r.name)
                    .bind(sid)
                    .bind(user.id)
                    .execute(&mut *tx)
                    .await?;
                touched.push(sid);
            } else {
                let sid: Uuid = sqlx::query_scalar(
                    "INSERT INTO app.stations (mac, name, owner_id, credential_id, lat, lon, elevation_ft, timezone, std_utc_offset_minutes) \
                     VALUES ($1, $2, $3, $4, $5::float8, $6::float8, $7, $8, $9) RETURNING id",
                )
                .bind(&r.mac)
                .bind(&r.name)
                .bind(user.id)
                .bind(new_id)
                .bind(r.lat)
                .bind(r.lon)
                .bind(r.elevation_ft)
                .bind(&r.timezone)
                .bind(r.offset_minutes)
                .fetch_one(&mut *tx)
                .await?;
                created.push(sid);
                touched.push(sid);
            }
        }
        // Repoint every covered station of the caller that is not already on the new credential.
        sqlx::query(
            "UPDATE app.stations SET credential_id = $1 WHERE owner_id = $2 AND credential_id <> $1 AND mac = ANY($3)",
        )
        .bind(new_id)
        .bind(user.id)
        .bind(&covered)
        .execute(&mut *tx)
        .await?;
        let not_covered = sqlx::query("SELECT id, name FROM app.stations WHERE owner_id = $1 AND credential_id <> $2 ORDER BY name")
            .bind(user.id)
            .bind(new_id)
            .fetch_all(&mut *tx)
            .await?;
        let not_covered: Vec<Value> = not_covered
            .iter()
            .map(|r| Ok(json!({ "id": r.try_get::<Uuid, _>("id")?, "name": r.try_get::<String, _>("name")? })))
            .collect::<Result<_, sqlx::Error>>()?;
        audit::record(
            &mut tx,
            user.id,
            audit::CREDENTIAL_SAVED,
            "credential",
            new_id,
            json!({}),
        )
        .await?;
        if let Some(old) = old {
            audit::record(
                &mut tx,
                user.id,
                audit::CREDENTIAL_REPLACED,
                "credential",
                old,
                json!({ "new_credential_id": new_id }),
            )
            .await?;
        }
        for sid in &created {
            audit::record(
                &mut tx,
                user.id,
                audit::STATION_CREATED,
                "station",
                *sid,
                json!({}),
            )
            .await?;
        }
        tx.commit().await?;
        Ok::<_, sqlx::Error>(Ok((touched, not_covered)))
    };
    let (touched, not_covered) = match run.await {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return Err(e),
        Err(e) => return Err(map_unique(&st.db, e, &mine, user.id).await),
    };
    let out = stations::by_ids(&st.db, user.id, &touched).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "credential": { "status": "ok" },
            "stations": out,
            "not_covered": not_covered,
        })),
    ))
}

pub async fn status(
    State(st): State<AppState>,
    user: AuthUser,
    _: NoQuery,
) -> ApiResult<Json<Value>> {
    let row = sqlx::query(
        "SELECT status, last_ok_at FROM app.credentials WHERE owner_id = $1 AND status <> 'replaced' \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(user.id)
    .fetch_optional(&st.db)
    .await?;
    let credential = match row {
        None => Value::Null,
        Some(r) => {
            let status: String = r.try_get("status")?;
            let last: Option<chrono::DateTime<chrono::Utc>> = r.try_get("last_ok_at")?;
            json!({ "status": status, "last_ok_at": last, "needs_reconnecting": status == "invalid" })
        }
    };
    Ok(Json(json!({ "credential": credential })))
}
