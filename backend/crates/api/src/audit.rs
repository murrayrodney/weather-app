//! Audit rows (02 section 4.13), written in the same transaction as the action.

use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

pub const FOLLOW_REQUESTED: &str = "follow.requested";
pub const FOLLOW_APPROVED: &str = "follow.approved";
pub const FOLLOW_DECLINED: &str = "follow.declined";
pub const FOLLOW_REVOKED: &str = "follow.revoked";
pub const CREDENTIAL_SAVED: &str = "credential.saved";
pub const CREDENTIAL_REPLACED: &str = "credential.replaced";
pub const STATION_CREATED: &str = "station.created";
pub const STATION_BASELINE_SET: &str = "station.baseline_set";

pub async fn record(
    tx: &mut Transaction<'_, Postgres>,
    actor: Uuid,
    action: &str,
    subject_type: &str,
    subject_id: Uuid,
    detail: Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO app.audit_log (actor_id, action, subject_type, subject_id, detail) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(actor)
    .bind(action)
    .bind(subject_type)
    .bind(subject_id)
    .bind(detail)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
