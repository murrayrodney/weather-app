//! Connection pool builder (`PoolSpec`) and a database error type that never carries URLs or credentials.

use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{PgConnection, PgPool};

/// Runs on every new connection (tests pass `SET ROLE app_poller`).
pub type ConnectHook =
    Arc<dyn for<'c> Fn(&'c mut PgConnection) -> BoxFuture<'c, sqlx::Result<()>> + Send + Sync>;

#[derive(Clone)]
pub struct PoolSpec {
    pub max: u32,
    pub min: u32,
    pub acquire_timeout: Duration,
    pub idle_timeout: Duration,
    pub max_lifetime: Duration,
    pub test_before_acquire: bool,
    pub after_connect: Option<ConnectHook>,
}

impl Default for PoolSpec {
    fn default() -> Self {
        Self {
            max: 4,
            min: 1,
            acquire_timeout: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(600),
            max_lifetime: Duration::from_secs(1800),
            test_before_acquire: true,
            after_connect: None,
        }
    }
}

impl PoolSpec {
    /// Poller values (03 section 3.6); one-off machines use `max = 2`.
    pub fn poller() -> Self {
        Self {
            acquire_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(300),
            ..Self::default()
        }
    }
}

impl std::fmt::Debug for PoolSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolSpec")
            .field("max", &self.max)
            .field("min", &self.min)
            .field("acquire_timeout", &self.acquire_timeout)
            .field("idle_timeout", &self.idle_timeout)
            .field("max_lifetime", &self.max_lifetime)
            .field("test_before_acquire", &self.test_before_acquire)
            .field("after_connect", &self.after_connect.is_some())
            .finish()
    }
}

/// A database failure reduced to what is safe to log: no host, no URL, no driver message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DbError {
    #[error("database unavailable")]
    Unavailable,
    #[error("database timeout")]
    Timeout,
    #[error("database configuration error")]
    Config,
    #[error("database error (sqlstate {0})")]
    Sql(String),
    #[error("database error")]
    Other,
}

impl DbError {
    /// SQLSTATE of a server-side error, if any (for example `40P01` deadlock, `40001` serialization failure).
    pub fn sqlstate(&self) -> Option<&str> {
        match self {
            DbError::Sql(s) => Some(s),
            _ => None,
        }
    }
}

impl From<sqlx::Error> for DbError {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::PoolTimedOut => DbError::Timeout,
            sqlx::Error::Io(_)
            | sqlx::Error::Tls(_)
            | sqlx::Error::PoolClosed
            | sqlx::Error::WorkerCrashed => DbError::Unavailable,
            sqlx::Error::Configuration(_) => DbError::Config,
            sqlx::Error::Database(d) => match d.code() {
                Some(c) => DbError::Sql(c.into_owned()),
                None => DbError::Other,
            },
            _ => DbError::Other,
        }
    }
}

/// Builds a pool. Connects eagerly so a bad URL or unreachable host fails at startup.
pub async fn pool(opts: &PgConnectOptions, spec: PoolSpec) -> Result<PgPool, DbError> {
    let mut b = PgPoolOptions::new()
        .max_connections(spec.max)
        .min_connections(spec.min)
        .acquire_timeout(spec.acquire_timeout)
        .idle_timeout(spec.idle_timeout)
        .max_lifetime(spec.max_lifetime)
        .test_before_acquire(spec.test_before_acquire);
    if let Some(hook) = spec.after_connect {
        b = b.after_connect(move |conn, _meta| {
            let hook = hook.clone();
            Box::pin(async move { hook(conn).await })
        });
    }
    b.connect_with(opts.clone()).await.map_err(DbError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn defaults_match_the_guide() {
        let d = PoolSpec::default();
        assert_eq!((d.max, d.min), (4, 1));
        assert_eq!(d.acquire_timeout, Duration::from_secs(30));
        assert_eq!(d.idle_timeout, Duration::from_secs(600));
        assert_eq!(d.max_lifetime, Duration::from_secs(1800));
        assert!(d.test_before_acquire);
        assert!(d.after_connect.is_none());
        let p = PoolSpec::poller();
        assert_eq!(
            (p.acquire_timeout, p.idle_timeout),
            (Duration::from_secs(10), Duration::from_secs(300))
        );
    }

    #[tokio::test]
    async fn connect_failure_is_reported_without_host_or_credentials() {
        let opts =
            PgConnectOptions::from_str("postgres://user:hunter2hunter2@127.0.0.1:1/db").unwrap();
        let spec = PoolSpec {
            acquire_timeout: Duration::from_millis(500),
            ..PoolSpec::default()
        };
        let err = pool(&opts, spec).await.unwrap_err();
        let text = format!("{err} {err:?}");
        assert!(!text.contains("hunter2"));
        assert!(!text.contains("127.0.0.1"));
    }
}
