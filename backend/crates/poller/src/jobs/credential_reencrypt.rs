//! `credential_reencrypt` (on demand, 03 section 6.1): rewrites every credential row older than the current key
//! version to the current key, one transaction per row, with the three-part compare-and-swap of 04 section 9.5.
//! No healthcheck pings. Idempotent.

use async_trait::async_trait;
use famweather_core::crypto::{encrypt_credential, public_key_from_private, EncryptedCredential};
use secrecy::SecretString;
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::job::{Job, JobContext, JobError};
use crate::jobs::awn_ingest::db_text;
use crate::keys::{CredentialRow, KeyRing};

pub struct CredentialReencrypt;

#[derive(Debug, Default, Serialize)]
pub struct Detail {
    pub rows: u32,
    pub reencrypted: u32,
    pub skipped_cas: u32,
    pub failed: u32,
    pub above_current: u32,
    /// `key_version -> row count` after the run.
    pub by_version: std::collections::BTreeMap<String, i64>,
}

/// A re-encrypted value ready to be swapped in.
pub struct Reencrypted {
    pub new: EncryptedCredential,
    pub old_version: i16,
    pub old_ciphertext: Vec<u8>,
}

/// Decrypts with the row's own key version and encrypts to the current key (same AAD, fresh encapsulation).
pub fn reencrypt(
    keys: &KeyRing,
    row: &CredentialRow,
) -> Result<Reencrypted, famweather_core::crypto::CryptoError> {
    let plain: SecretString = keys.decrypt_row(row)?;
    let pk = public_key_from_private(keys.current_key());
    let new = encrypt_credential(&pk, keys.current_version(), row.id, row.owner_id, &plain)?;
    Ok(Reencrypted {
        new,
        old_version: row.key_version,
        old_ciphertext: row.ciphertext.clone(),
    })
}

/// The compare-and-swap. Zero rows updated means the row changed meanwhile (only a concurrent run of this job can do
/// that, because the API cannot overwrite ciphertext): the row is skipped. `audit_log` is written only on a swap.
pub async fn swap(pool: &PgPool, id: Uuid, r: &Reencrypted) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let n = sqlx::query(
        "UPDATE app.credentials SET api_key_ciphertext = $1, encapsulated_key = $2, key_version = $3 \
         WHERE id = $4 AND key_version = $5 AND api_key_ciphertext = $6",
    )
    .bind(&r.new.ciphertext)
    .bind(&r.new.encapsulated_key)
    .bind(r.new.key_version)
    .bind(id)
    .bind(r.old_version)
    .bind(&r.old_ciphertext)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if n == 1 {
        sqlx::query(
            "INSERT INTO app.audit_log (actor_id, action, subject_type, subject_id, detail) \
             VALUES (NULL, 'credential.reencrypted', 'credential', $1, $2)",
        )
        .bind(id)
        .bind(json!({"from_version": r.old_version, "to_version": r.new.key_version}))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(n == 1)
}

#[async_trait]
impl Job for CredentialReencrypt {
    fn name(&self) -> &'static str {
        "credential_reencrypt"
    }

    async fn run(&self, ctx: &mut JobContext) -> Result<Value, JobError> {
        let mut d = Detail::default();
        let snap = |d: &Detail| serde_json::to_value(d).unwrap_or_else(|_| json!({}));
        let pool = ctx.env.pool.clone();
        let Some(keys) = ctx.env.keys.clone() else {
            return Err(JobError::failed("no_credentials_private_key", snap(&d)));
        };
        let current = keys.current_version();
        // Every row, including `replaced` ones.
        let rows = sqlx::query(
            "SELECT id, owner_id, api_key_ciphertext, encapsulated_key, key_version FROM app.credentials \
             WHERE key_version <> $1 ORDER BY id",
        )
        .bind(current)
        .fetch_all(&pool)
        .await
        .map_err(|e| JobError::failed(db_text(e), snap(&d)))?;
        for r in rows {
            let row = CredentialRow {
                id: r
                    .try_get("id")
                    .map_err(|e| JobError::failed(db_text(e), snap(&d)))?,
                owner_id: r
                    .try_get("owner_id")
                    .map_err(|e| JobError::failed(db_text(e), snap(&d)))?,
                ciphertext: r
                    .try_get("api_key_ciphertext")
                    .map_err(|e| JobError::failed(db_text(e), snap(&d)))?,
                encapsulated_key: r
                    .try_get("encapsulated_key")
                    .map_err(|e| JobError::failed(db_text(e), snap(&d)))?,
                key_version: r
                    .try_get("key_version")
                    .map_err(|e| JobError::failed(db_text(e), snap(&d)))?,
            };
            if let Err(i) = ctx.checkpoint().await {
                return Err(JobError::interrupted(i, snap(&d)));
            }
            if row.key_version > current {
                // The poller runs an older key set than the API: never decrypted, only reported.
                d.above_current += 1;
                tracing::warn!(credential_id = %row.id, "credential key_version above the current version");
                continue;
            }
            d.rows += 1;
            let new = match reencrypt(&keys, &row) {
                Ok(n) => n,
                Err(_) => {
                    tracing::error!(credential_id = %row.id, "credential_decrypt_failed");
                    d.failed += 1;
                    continue;
                }
            };
            match swap(&pool, row.id, &new).await {
                Ok(true) => d.reencrypted += 1,
                Ok(false) => d.skipped_cas += 1,
                Err(e) => return Err(JobError::failed(db_text(e), snap(&d))),
            }
        }
        let counts = sqlx::query(
            "SELECT key_version, count(*)::bigint AS n FROM app.credentials GROUP BY 1 ORDER BY 1",
        )
        .fetch_all(&pool)
        .await
        .map_err(|e| JobError::failed(db_text(e), snap(&d)))?;
        for c in counts {
            let v: i16 = c
                .try_get("key_version")
                .map_err(|e| JobError::failed(db_text(e), snap(&d)))?;
            let n: i64 = c
                .try_get("n")
                .map_err(|e| JobError::failed(db_text(e), snap(&d)))?;
            d.by_version.insert(v.to_string(), n);
        }
        if d.failed > 0 {
            return Err(JobError::failed("decrypt_failed", snap(&d)));
        }
        Ok(snap(&d))
    }
}
