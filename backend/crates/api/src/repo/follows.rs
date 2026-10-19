//! Follows and user search (02 5.8, 5.11). Authorization is in the SQL WHERE clauses.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::audit;
use crate::error::{ApiError, ApiResult};

#[derive(Debug, Serialize, Clone)]
pub struct UserRef {
    pub id: Uuid,
    pub display_name: String,
}

#[derive(Debug, Serialize)]
pub struct FollowEntry {
    pub user: UserRef,
    pub status: String,
    pub requested_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct FollowLists {
    pub following: Vec<FollowEntry>,
    pub followers: Vec<FollowEntry>,
}

fn entry(r: &sqlx::postgres::PgRow) -> Result<FollowEntry, sqlx::Error> {
    Ok(FollowEntry {
        user: UserRef {
            id: r.try_get("other_id")?,
            display_name: r.try_get("display_name")?,
        },
        status: r.try_get("status")?,
        requested_at: r.try_get("requested_at")?,
        decided_at: r.try_get("decided_at")?,
    })
}

pub async fn list(db: &PgPool, me: Uuid) -> Result<FollowLists, sqlx::Error> {
    let following = sqlx::query(
        "SELECT f.followee_id AS other_id, u.display_name, f.status, f.requested_at, f.decided_at \
         FROM app.follows f JOIN app.users u ON u.id = f.followee_id AND u.is_active \
         WHERE f.follower_id = $1 ORDER BY lower(u.display_name), u.id",
    )
    .bind(me)
    .fetch_all(db)
    .await?;
    let followers = sqlx::query(
        "SELECT f.follower_id AS other_id, u.display_name, f.status, f.requested_at, f.decided_at \
         FROM app.follows f JOIN app.users u ON u.id = f.follower_id AND u.is_active \
         WHERE f.followee_id = $1 ORDER BY lower(u.display_name), u.id",
    )
    .bind(me)
    .fetch_all(db)
    .await?;
    Ok(FollowLists {
        following: following.iter().map(entry).collect::<Result<_, _>>()?,
        followers: followers.iter().map(entry).collect::<Result<_, _>>()?,
    })
}

#[derive(Debug, Serialize)]
pub struct RequestOut {
    pub user: UserRef,
    pub status: String,
}

/// `POST /follows`. Returns (created, body).
pub async fn request(db: &PgPool, me: Uuid, target: Uuid) -> ApiResult<(bool, RequestOut)> {
    if target == me {
        return Err(ApiError::Validation("You cannot follow yourself."));
    }
    let mut tx = db.begin().await?;
    let user = sqlx::query("SELECT id, display_name FROM app.users WHERE id = $1 AND is_active")
        .bind(target)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    let user = UserRef {
        id: user.try_get("id")?,
        display_name: user.try_get("display_name")?,
    };
    let detail = serde_json::json!({ "followee_id": target });
    let inserted = sqlx::query(
        "INSERT INTO app.follows (follower_id, followee_id) VALUES ($1, $2) ON CONFLICT DO NOTHING RETURNING status",
    )
    .bind(me)
    .bind(target)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(r) = inserted {
        audit::record(&mut tx, me, audit::FOLLOW_REQUESTED, "follow", me, detail).await?;
        tx.commit().await?;
        return Ok((
            true,
            RequestOut {
                user,
                status: r.try_get("status")?,
            },
        ));
    }
    let revived = sqlx::query(
        "UPDATE app.follows SET status = 'pending', decided_at = NULL \
         WHERE follower_id = $1 AND followee_id = $2 AND status IN ('declined', 'revoked') RETURNING status",
    )
    .bind(me)
    .bind(target)
    .fetch_optional(&mut *tx)
    .await?;
    let status: String = match revived {
        Some(r) => {
            audit::record(&mut tx, me, audit::FOLLOW_REQUESTED, "follow", me, detail).await?;
            r.try_get("status")?
        }
        None => {
            sqlx::query_scalar(
                "SELECT status FROM app.follows WHERE follower_id = $1 AND followee_id = $2",
            )
            .bind(me)
            .bind(target)
            .fetch_one(&mut *tx)
            .await?
        }
    };
    tx.commit().await?;
    Ok((false, RequestOut { user, status }))
}

#[derive(Debug, Serialize)]
pub struct DecisionOut {
    pub user: UserRef,
    pub status: String,
    pub requested_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Copy)]
pub enum Decision {
    Approve,
    Decline,
    Revoke,
}

/// approve/decline from `pending`, revoke from `approved`; the caller is the followee. Zero rows = 404.
pub async fn decide(db: &PgPool, me: Uuid, follower: Uuid, d: Decision) -> ApiResult<DecisionOut> {
    let (to, from, action) = match d {
        Decision::Approve => ("approved", "pending", audit::FOLLOW_APPROVED),
        Decision::Decline => ("declined", "pending", audit::FOLLOW_DECLINED),
        Decision::Revoke => ("revoked", "approved", audit::FOLLOW_REVOKED),
    };
    let mut tx = db.begin().await?;
    let row = sqlx::query(
        "UPDATE app.follows SET status = $3, decided_at = now() \
         WHERE follower_id = $1 AND followee_id = $2 AND status = $4 \
         AND EXISTS (SELECT 1 FROM app.users u WHERE u.id = $1 AND u.is_active) \
         RETURNING requested_at, decided_at",
    )
    .bind(follower)
    .bind(me)
    .bind(to)
    .bind(from)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    let name: String = sqlx::query_scalar("SELECT display_name FROM app.users WHERE id = $1")
        .bind(follower)
        .fetch_one(&mut *tx)
        .await?;
    audit::record(
        &mut tx,
        me,
        action,
        "follow",
        follower,
        serde_json::json!({ "followee_id": me }),
    )
    .await?;
    tx.commit().await?;
    Ok(DecisionOut {
        user: UserRef {
            id: follower,
            display_name: name,
        },
        status: to.to_owned(),
        requested_at: row.try_get("requested_at")?,
        decided_at: row.try_get("decided_at")?,
    })
}

#[derive(Debug, Serialize)]
pub struct SearchUser {
    pub id: Uuid,
    pub display_name: String,
    pub follow_status: String,
}

pub fn escape_like(q: &str) -> String {
    let mut out = String::with_capacity(q.len() + 2);
    for c in q.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

pub async fn search(db: &PgPool, me: Uuid, q: &str) -> Result<Vec<SearchUser>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT u.id, u.display_name, f.status AS follow_status \
         FROM app.users u LEFT JOIN app.follows f ON f.follower_id = $1 AND f.followee_id = u.id \
         WHERE u.is_active AND u.id <> $1 \
         AND lower(btrim(u.display_name)) LIKE lower(btrim($2)) || '%' ESCAPE '\\' \
         ORDER BY lower(btrim(u.display_name)), u.id LIMIT 10",
    )
    .bind(me)
    .bind(escape_like(q))
    .fetch_all(db)
    .await?;
    rows.iter()
        .map(|r| {
            Ok(SearchUser {
                id: r.try_get("id")?,
                display_name: r.try_get("display_name")?,
                follow_status: r
                    .try_get::<Option<String>, _>("follow_status")?
                    .unwrap_or_else(|| "none".to_owned()),
            })
        })
        .collect()
}
