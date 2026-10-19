//! `app.audit_log` writer. Actions are the dotted list of 04 section 5.9; `detail` never carries keys, emails,
//! tokens or ciphertext.

use famweather_core::db::DbError;
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

/// The single admin's user id (`is_admin`), if there is one: the `actor_id` of every audit row `admin` writes.
pub async fn actor(conn: &mut PgConnection) -> Result<Option<Uuid>, DbError> {
    sqlx::query_scalar("SELECT id FROM app.users WHERE is_admin LIMIT 1")
        .fetch_optional(conn)
        .await
        .map_err(DbError::from)
}

pub async fn write(
    conn: &mut PgConnection,
    actor_id: Option<Uuid>,
    action: &str,
    subject_type: &str,
    subject_id: Option<Uuid>,
    detail: Value,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO app.audit_log (actor_id, action, subject_type, subject_id, detail) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(actor_id)
    .bind(action)
    .bind(subject_type)
    .bind(subject_id)
    .bind(detail)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(DbError::from)
}

/// `detail` of `follow.*` rows: the followee (the subject is the follower).
pub fn followee_detail(followee: Uuid) -> Value {
    json!({ "followee_id": followee })
}
