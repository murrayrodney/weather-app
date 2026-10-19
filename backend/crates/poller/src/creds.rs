//! Stored credentials and their guarded status writes (03 sections 3.3 and 5.3).

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::keys::CredentialRow;

pub struct StoredCredential {
    pub row: CredentialRow,
    pub status: String,
}

/// A station the poller may work on.
#[derive(Debug, Clone)]
pub struct StationRow {
    pub id: Uuid,
    pub mac: String,
    pub credential_id: Uuid,
    pub timezone: String,
    pub std_utc_offset_minutes: i32,
    pub backfill_floor_ts: Option<DateTime<Utc>>,
}

/// Credentials with status `ok` or `rate_limited` that an enabled station (optionally just `station`) uses.
pub async fn load_credentials(
    pool: &PgPool,
    station: Option<Uuid>,
) -> Result<Vec<StoredCredential>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT c.id, c.owner_id, c.api_key_ciphertext, c.encapsulated_key, c.key_version, c.status \
         FROM app.credentials c \
         WHERE c.status IN ('ok','rate_limited') \
           AND EXISTS (SELECT 1 FROM app.stations s WHERE s.credential_id = c.id AND s.is_enabled \
                       AND ($1::uuid IS NULL OR s.id = $1)) \
         ORDER BY c.id",
    )
    .bind(station)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|r| {
            Ok(StoredCredential {
                row: CredentialRow {
                    id: r.try_get("id")?,
                    owner_id: r.try_get("owner_id")?,
                    ciphertext: r.try_get("api_key_ciphertext")?,
                    encapsulated_key: r.try_get("encapsulated_key")?,
                    key_version: r.try_get("key_version")?,
                },
                status: r.try_get("status")?,
            })
        })
        .collect()
}

pub async fn stations_of(
    pool: &PgPool,
    credential_id: Uuid,
    station: Option<Uuid>,
) -> Result<Vec<StationRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, mac, credential_id, timezone, std_utc_offset_minutes::int AS off, backfill_floor_ts \
         FROM app.stations WHERE credential_id = $1 AND is_enabled AND ($2::uuid IS NULL OR id = $2) \
         ORDER BY mac",
    )
    .bind(credential_id)
    .bind(station)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|r| {
            Ok(StationRow {
                id: r.try_get("id")?,
                mac: r.try_get("mac")?,
                credential_id: r.try_get("credential_id")?,
                timezone: r.try_get("timezone")?,
                std_utc_offset_minutes: r.try_get("off")?,
                backfill_floor_ts: r.try_get("backfill_floor_ts")?,
            })
        })
        .collect()
}

/// Guarded status write: only rows still `ok`/`rate_limited`, only on a real change. True when a row changed.
pub async fn set_status(pool: &PgPool, id: Uuid, status: &str) -> Result<bool, sqlx::Error> {
    let n = sqlx::query(
        "UPDATE app.credentials SET status = $1, status_changed_at = now() \
         WHERE id = $2 AND status IN ('ok','rate_limited') AND status IS DISTINCT FROM $1",
    )
    .bind(status)
    .bind(id)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n == 1)
}

/// Separate guarded statement so `status_changed_at` is untouched.
pub async fn touch_last_ok(pool: &PgPool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE app.credentials SET last_ok_at = now() WHERE id = $1 AND status IN ('ok','rate_limited')",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Marks a credential `invalid` (guarded) and writes `credential.invalid` only if that changed a row.
pub async fn mark_invalid(pool: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let n = sqlx::query(
        "UPDATE app.credentials SET status = 'invalid', status_changed_at = now() \
         WHERE id = $1 AND status IN ('ok','rate_limited') AND status IS DISTINCT FROM 'invalid'",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if n == 1 {
        sqlx::query(
            "INSERT INTO app.audit_log (actor_id, action, subject_type, subject_id, detail) \
             VALUES (NULL, 'credential.invalid', 'credential', $1, '{}'::jsonb)",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(n == 1)
}

/// What happened to the credentials during one run.
#[derive(Debug, Default)]
pub struct CredTracker {
    succeeded: HashSet<Uuid>,
    unauthorized: Vec<Uuid>,
    pub rate_limited: u32,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Resolution {
    pub marked_invalid: u32,
    /// A 401 credential that could not be blamed because no other credential succeeded.
    pub suspected_application_key_failure: bool,
}

impl CredTracker {
    /// First successful call of a credential in this run: `status = ok` (a no-op when already ok) and `last_ok_at`.
    pub async fn success(&mut self, pool: &PgPool, id: Uuid) -> Result<(), sqlx::Error> {
        if self.succeeded.insert(id) {
            set_status(pool, id, "ok").await?;
            touch_last_ok(pool, id).await?;
        }
        Ok(())
    }

    pub async fn rate_limited(&mut self, pool: &PgPool, id: Uuid) -> Result<(), sqlx::Error> {
        self.rate_limited += 1;
        set_status(pool, id, "rate_limited").await?;
        Ok(())
    }

    pub fn unauthorized(&mut self, id: Uuid) {
        if !self.unauthorized.contains(&id) {
            self.unauthorized.push(id);
        }
    }

    pub fn unauthorized_count(&self) -> usize {
        self.unauthorized.len()
    }

    /// End-of-run 401 rule: mark a 401 credential `invalid` only if at least one **other** credential succeeded in
    /// this run; otherwise mark nothing and report a suspected application-key failure. Independent of order.
    pub async fn resolve(&self, pool: &PgPool) -> Result<Resolution, sqlx::Error> {
        let mut out = Resolution::default();
        for id in &self.unauthorized {
            if self.succeeded.iter().any(|s| s != id) {
                if mark_invalid(pool, *id).await? {
                    out.marked_invalid += 1;
                }
            } else {
                out.suspected_application_key_failure = true;
            }
        }
        Ok(out)
    }
}
