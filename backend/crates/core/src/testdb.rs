//! Test database helpers (04 section 11.6; feature `testdb`, dev-dependencies only, never in a release binary).
//!
//! Usage: `#[sqlx::test(migrations = false)] async fn t(pool: PgPool) { core::testdb::prepare(&pool).await.unwrap(); ... }`.
//! `#[sqlx::test]` reads the superuser URL from `DATABASE_URL`. [`prepare`] loads `auth_stub.sql` and `auth_access.sql`
//! at run time from `$OPS_DIR` (default `/work/ops/db`); nothing outside `backend/` is ever `include_str!`-ed.
//! Grant behaviour is never *asserted* here (that is `ops/db/tests/run.sh`); [`pool_as`] only makes code under test
//! run with the real role's grants.

use std::path::PathBuf;
use std::sync::Arc;

use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

use crate::db::ConnectHook;

/// The migrations embedded at build time (same files `admin migrate` applies).
pub static MIGRATOR: Migrator = sqlx::migrate!("../../migrations");

/// Roles a test pool may assume.
pub const ROLES: [&str; 5] = [
    "app_api",
    "app_poller",
    "app_migrator",
    "app_backup",
    "app_ci_check",
];

#[derive(Debug, thiserror::Error)]
pub enum TestDbError {
    #[error("reading {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("database error: {0}")]
    Sql(#[from] sqlx::Error),
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error(
        "unknown role {0:?} (allowed: app_api, app_poller, app_migrator, app_backup, app_ci_check)"
    )]
    UnknownRole(String),
}

/// `$OPS_DIR`, default `/work/ops/db`.
pub fn ops_dir() -> PathBuf {
    std::env::var_os("OPS_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/work/ops/db"))
}

async fn read(rel: &str) -> Result<String, TestDbError> {
    let path = ops_dir().join(rel);
    tokio::fs::read_to_string(&path)
        .await
        .map_err(|source| TestDbError::Io { path, source })
}

/// Turns an empty `#[sqlx::test(migrations = false)]` database into a migrated one, the way production is laid out:
/// as the superuser the auth stub and `auth_access.sql`, then (on one connection) `SET ROLE app_migrator;
/// SET search_path = app` and the embedded migrations, so ownership, default privileges and the location of
/// `_sqlx_migrations` match production. Roles already exist (created by `roles.sql` at database init).
pub async fn prepare(pool: &PgPool) -> Result<(), TestDbError> {
    let stub = read("tests/auth_stub.sql").await?;
    let access = read("bootstrap/auth_access.sql").await?;
    sqlx::raw_sql(&stub).execute(pool).await?;
    sqlx::raw_sql(&access).execute(pool).await?;

    let mut conn = pool.acquire().await?;
    sqlx::query("SET ROLE app_migrator")
        .execute(&mut *conn)
        .await?;
    sqlx::query("SET search_path = app")
        .execute(&mut *conn)
        .await?;
    let result = MIGRATOR.run(&mut *conn).await;
    // Leave the pooled connection as we found it, whether or not the migrations succeeded.
    sqlx::query("RESET ROLE").execute(&mut *conn).await?;
    sqlx::query("RESET search_path").execute(&mut *conn).await?;
    result?;
    Ok(())
}

fn check_role(role: &str) -> Result<(), TestDbError> {
    if ROLES.contains(&role) {
        Ok(())
    } else {
        Err(TestDbError::UnknownRole(role.to_owned()))
    }
}

/// A connect hook that runs `SET ROLE <role>`; pass it as `PoolSpec::after_connect` or as the `on_connect` hook of
/// `try_job_lock` so the detached lock connection is subject to the same grants.
pub fn role_hook(role: &str) -> Result<ConnectHook, TestDbError> {
    check_role(role)?;
    let stmt = format!("SET ROLE {role}");
    Ok(Arc::new(move |conn| {
        let stmt = stmt.clone();
        Box::pin(async move {
            sqlx::query(&stmt).execute(conn).await?;
            Ok(())
        })
    }))
}

/// A pool on the same database as `db` (the pool `#[sqlx::test]` hands the test) whose every connection runs
/// `SET ROLE <role>`, so repository code is exercised with the real grants.
pub async fn pool_as(db: &PgPool, role: &str) -> Result<PgPool, TestDbError> {
    let hook = role_hook(role)?;
    let opts = (*db.connect_options()).clone();
    Ok(PgPoolOptions::new()
        .max_connections(8)
        .after_connect(move |conn, _meta| {
            let hook = hook.clone();
            Box::pin(async move { hook(conn).await })
        })
        .connect_with(opts)
        .await?)
}
