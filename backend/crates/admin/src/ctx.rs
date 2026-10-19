//! Per-command context: the `app_migrator` pool and, when configured, the Supabase Admin API client.

use std::str::FromStr;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use famweather_core::config::AdminEnv;
use famweather_core::db::{self, DbError, PoolSpec};
use secrecy::ExposeSecret;
use sqlx::postgres::PgConnectOptions;
use sqlx::PgPool;

use crate::gotrue::GoTrue;

pub struct Ctx {
    pub env: AdminEnv,
    pub pool: PgPool,
    pub gotrue: Option<GoTrue>,
}

/// Every database error shown to the operator is reduced to its SQLSTATE (Postgres messages can quote row data).
pub fn db_err(e: sqlx::Error) -> anyhow::Error {
    anyhow::Error::new(DbError::from(e))
}

pub fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if d.code().as_deref() == Some("23505"))
}

pub async fn connect_pool(env: &AdminEnv) -> Result<PgPool> {
    let url = env
        .database_url
        .as_ref()
        .ok_or_else(|| anyhow!("DATABASE_URL is required"))?;
    let opts = PgConnectOptions::from_str(url.expose_secret())
        .map_err(|_| anyhow!("DATABASE_URL is not a valid connection string"))?;
    db::pool(
        &opts,
        PoolSpec {
            max: 2,
            min: 0,
            acquire_timeout: Duration::from_secs(15),
            ..PoolSpec::default()
        },
    )
    .await
    .context("cannot connect to the database")
}

impl Ctx {
    pub async fn connect(env: AdminEnv) -> Result<Ctx> {
        let pool = connect_pool(&env).await?;
        let gotrue = match (&env.supabase_url, &env.supabase_service_role_key) {
            (Some(url), Some(key)) => Some(GoTrue::new(url, key)?),
            _ => None,
        };
        Ok(Ctx { env, pool, gotrue })
    }

    /// The Admin API client, or an error naming the missing configuration.
    pub fn gotrue(&self) -> Result<&GoTrue> {
        self.gotrue.as_ref().ok_or_else(|| {
            anyhow!("SUPABASE_URL and SUPABASE_SERVICE_ROLE_KEY are required for this command")
        })
    }
}
