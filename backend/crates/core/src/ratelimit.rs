//! Shared rate gates (slot reservations in `app.rate_gates`) and Postgres session advisory job locks.
//! Conventions "AWN keys and rate limits" and "Job locks"; 03 sections 4.2 and 5.3.
//!
//! DB-backed behaviour (concurrent slots, lock loss) is tested in T3a once `core::testdb` exists.

use std::time::Duration;

use sha2::{Digest, Sha256};
use sqlx::postgres::types::PgInterval;
use sqlx::postgres::PgConnectOptions;
use sqlx::{Connection, PgConnection, PgPool, Row};

use crate::db::{ConnectHook, DbError};

/// The conventions' single reservation statement, verbatim.
/// `$1` name, `$2` spacing (interval), `$3` max wait (interval).
pub const RESERVE_SQL: &str = "\
INSERT INTO app.rate_gates (name, next_allowed_at) VALUES ($1, now() + $2::interval)
ON CONFLICT (name) DO UPDATE
  SET next_allowed_at = greatest(app.rate_gates.next_allowed_at, now()) + $2::interval
  WHERE greatest(app.rate_gates.next_allowed_at, now()) <= now() + $3::interval
RETURNING greatest(next_allowed_at - $2::interval - now(), interval '0') AS wait";

/// Why a slot was not granted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateBusyReason {
    /// The wait would exceed `max_wait` (no row returned): ordinary back-pressure.
    Busy,
    /// The reservation statement failed. Check [`GateBusy::is_permanent`] to tell a missing grant or table
    /// (SQLSTATE 42501, 42P01, ...) from a transient outage.
    Database(DbError),
    /// The durations could not be expressed as an interval (caller bug).
    BadArguments,
}

/// No slot could be granted. Callers map it to a 503 / a retry later whatever the reason; the reason is for
/// logs and metrics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateBusy {
    pub reason: GateBusyReason,
}

impl std::fmt::Display for GateBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.reason {
            GateBusyReason::Busy => f.write_str("rate gate busy"),
            GateBusyReason::BadArguments => f.write_str("rate gate busy (bad arguments)"),
            GateBusyReason::Database(e) => write!(f, "rate gate busy ({e})"),
        }
    }
}

impl std::error::Error for GateBusy {}

impl GateBusy {
    fn new(reason: GateBusyReason) -> Self {
        Self { reason }
    }

    /// True for failures that retrying cannot fix: a missing privilege or table, a bad statement, a rejected
    /// login (SQLSTATE classes 28, 42 and 3F) or a configuration error. These are logged at `error`.
    pub fn is_permanent(&self) -> bool {
        match &self.reason {
            GateBusyReason::Database(DbError::Config) | GateBusyReason::BadArguments => true,
            GateBusyReason::Database(e) => e
                .sqlstate()
                .is_some_and(|s| ["28", "42", "3F"].iter().any(|c| s.starts_with(c))),
            GateBusyReason::Busy => false,
        }
    }
}

fn interval(d: Duration) -> Result<PgInterval, GateBusy> {
    PgInterval::try_from(d).map_err(|_| GateBusy::new(GateBusyReason::BadArguments))
}

fn interval_to_duration(i: &PgInterval) -> Duration {
    // The statement returns a pure time difference (months = 0, days = 0), but handle days defensively.
    let micros = i.microseconds.max(0) as u64 + (i.days.max(0) as u64) * 86_400_000_000;
    Duration::from_micros(micros)
}

/// Reserves the next slot of gate `name`. Returns how long the caller must `tokio::time::sleep` before using
/// the slot (computed by the database, never from the injected `Clock`). No connection is held while waiting.
/// `Err(GateBusy)` when the wait would exceed `max_wait` (no row returned). A database failure is also reported
/// as `GateBusy` (logged at `warn` with the SQLSTATE only) because the signature admits no other error.
pub async fn reserve(
    pool: &PgPool,
    name: &str,
    spacing: Duration,
    max_wait: Duration,
) -> Result<Duration, GateBusy> {
    let spacing = interval(spacing)?;
    let max_wait = interval(max_wait)?;
    let row = sqlx::query(RESERVE_SQL)
        .bind(name)
        .bind(spacing)
        .bind(max_wait)
        .fetch_optional(pool)
        .await
        .map_err(|e| {
            let busy = GateBusy::new(GateBusyReason::Database(DbError::from(e)));
            if busy.is_permanent() {
                tracing::error!(gate = name, error = %busy, "rate gate reservation failed permanently (grant or schema problem)");
            } else {
                tracing::warn!(gate = name, error = %busy, "rate gate reservation failed");
            }
            busy
        })?;
    let Some(row) = row else {
        return Err(GateBusy::new(GateBusyReason::Busy));
    };
    let wait: PgInterval = row
        .try_get("wait")
        .map_err(|_| GateBusy::new(GateBusyReason::Database(DbError::Other)))?;
    Ok(interval_to_duration(&wait))
}

/// The lock connection is gone or the lock is no longer held; the job must abort (`detail.error = "lock_lost"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("job lock lost")]
pub struct LockLost;

/// A held `pg_try_advisory_lock` on a raw, detached connection (never a pooled one).
/// Dropping it closes the socket, which frees the lock server-side.
pub struct JobLock {
    conn: PgConnection,
    key: i64,
    op_timeout: Duration,
}

/// Bound on opening the lock connection (connect, hook, lock query together).
pub const LOCK_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Bound on `check()` and `release()` round trips: a black-holed connection must not hang a job.
pub const LOCK_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// Timeouts for [`try_job_lock_with`] (tests shorten them).
#[derive(Debug, Clone, Copy)]
pub struct LockTimeouts {
    pub connect: Duration,
    pub check: Duration,
}

impl Default for LockTimeouts {
    fn default() -> Self {
        Self {
            connect: LOCK_CONNECT_TIMEOUT,
            check: LOCK_CHECK_TIMEOUT,
        }
    }
}

impl std::fmt::Debug for JobLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobLock")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

/// Advisory lock key for `job:<name>`: first 8 bytes (big-endian) of `sha256("job:<name>")` as `i64`.
pub fn job_lock_key(job: &str) -> i64 {
    let digest = Sha256::digest(format!("job:{job}").as_bytes());
    let mut b = [0u8; 8];
    b.copy_from_slice(&digest[..8]);
    i64::from_be_bytes(b)
}

/// Opens a fresh connection (same role and TLS as the pool), runs `on_connect` (tests: `SET ROLE app_poller`),
/// then `SELECT pg_try_advisory_lock($1)`. `Ok(None)` when another session holds the lock.
pub async fn try_job_lock(
    opts: &PgConnectOptions,
    job: &str,
    on_connect: Option<ConnectHook>,
) -> Result<Option<JobLock>, DbError> {
    try_job_lock_with(opts, job, on_connect, LockTimeouts::default()).await
}

/// As [`try_job_lock`] with explicit timeouts. On a connect timeout the half-open connection is dropped, which
/// closes the socket (and frees the lock if the server had already granted it).
pub async fn try_job_lock_with(
    opts: &PgConnectOptions,
    job: &str,
    on_connect: Option<ConnectHook>,
    timeouts: LockTimeouts,
) -> Result<Option<JobLock>, DbError> {
    let open = async {
        let mut conn = PgConnection::connect_with(opts).await?;
        if let Some(hook) = on_connect {
            hook(&mut conn).await?;
        }
        let key = job_lock_key(job);
        let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
            .bind(key)
            .fetch_one(&mut conn)
            .await?;
        if got {
            Ok(Some(JobLock {
                conn,
                key,
                op_timeout: timeouts.check,
            }))
        } else {
            let _ = conn.close().await;
            Ok(None)
        }
    };
    tokio::time::timeout(timeouts.connect, open)
        .await
        .map_err(|_| DbError::Timeout)?
}

impl JobLock {
    /// `SELECT 1` on the lock connection, bounded by the check timeout; an error or timeout means the session
    /// (and so possibly the lock) is gone.
    pub async fn check(&mut self) -> Result<(), LockLost> {
        match tokio::time::timeout(
            self.op_timeout,
            sqlx::query("SELECT 1").execute(&mut self.conn),
        )
        .await
        {
            Ok(Ok(_)) => Ok(()),
            _ => Err(LockLost),
        }
    }

    /// `pg_advisory_unlock`, then close the connection. Bounded by the check timeout.
    pub async fn release(mut self) -> Result<(), DbError> {
        let unlock = sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock($1)")
            .bind(self.key)
            .fetch_one(&mut self.conn);
        let res = tokio::time::timeout(self.op_timeout, unlock).await;
        // Closing (or dropping) the connection frees the lock even if the unlock statement failed.
        let _ = tokio::time::timeout(self.op_timeout, self.conn.close()).await;
        match res {
            Ok(r) => r.map(|_| ()).map_err(DbError::from),
            Err(_) => Err(DbError::Timeout),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserve_sql_is_the_conventions_statement() {
        let s = RESERVE_SQL;
        assert!(s.starts_with(
            "INSERT INTO app.rate_gates (name, next_allowed_at) VALUES ($1, now() + $2::interval)"
        ));
        assert!(s.contains("ON CONFLICT (name) DO UPDATE"));
        assert!(s.contains(
            "SET next_allowed_at = greatest(app.rate_gates.next_allowed_at, now()) + $2::interval"
        ));
        assert!(s.contains(
            "WHERE greatest(app.rate_gates.next_allowed_at, now()) <= now() + $3::interval"
        ));
        assert!(s.contains(
            "RETURNING greatest(next_allowed_at - $2::interval - now(), interval '0') AS wait"
        ));
        // One statement, no advisory locks.
        assert!(!s.contains(';'));
        assert!(!s.contains("advisory"));
    }

    #[test]
    fn job_lock_keys_are_stable_and_distinct() {
        let names = [
            "awn_ingest",
            "awn_backfill",
            "daily_rollup",
            "reference_sync",
            "credential_reencrypt",
        ];
        let keys: Vec<i64> = names.iter().map(|n| job_lock_key(n)).collect();
        for (i, a) in keys.iter().enumerate() {
            for b in &keys[i + 1..] {
                assert_ne!(a, b);
            }
        }
        // sha256("job:awn_ingest")[..8], big-endian.
        let d = Sha256::digest(b"job:awn_ingest");
        assert_eq!(
            job_lock_key("awn_ingest"),
            i64::from_be_bytes(d[..8].try_into().unwrap())
        );
        assert_eq!(job_lock_key("awn_ingest"), job_lock_key("awn_ingest"));
    }

    #[test]
    fn interval_conversion() {
        let i = PgInterval {
            months: 0,
            days: 0,
            microseconds: 1_500_000,
        };
        assert_eq!(interval_to_duration(&i), Duration::from_millis(1500));
        let neg = PgInterval {
            months: 0,
            days: 0,
            microseconds: -5,
        };
        assert_eq!(interval_to_duration(&neg), Duration::ZERO);
        assert!(interval(Duration::from_millis(500)).is_ok());
    }

    #[tokio::test]
    async fn reserve_without_a_database_is_gate_busy() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(Duration::from_millis(300))
            .connect_lazy("postgres://u:p@127.0.0.1:1/db")
            .unwrap();
        let r = reserve(
            &pool,
            "awn:app:poller",
            Duration::from_millis(500),
            Duration::from_secs(1),
        )
        .await;
        let busy = r.unwrap_err();
        assert!(
            matches!(busy.reason, GateBusyReason::Database(_)),
            "{busy:?}"
        );
        assert!(!busy.is_permanent(), "an outage is transient");
        assert!(!busy.to_string().contains("127.0.0.1"));
    }

    #[test]
    fn permanent_failures_are_distinguished_from_busy_and_outages() {
        let db = |s: &str| GateBusy::new(GateBusyReason::Database(DbError::Sql(s.into())));
        for permanent in ["42501", "42P01", "42601", "28P01", "3F000"] {
            assert!(db(permanent).is_permanent(), "{permanent}");
        }
        for transient in ["40001", "40P01", "57014", "53300", "08006"] {
            assert!(!db(transient).is_permanent(), "{transient}");
        }
        assert!(!GateBusy::new(GateBusyReason::Busy).is_permanent());
        assert!(!GateBusy::new(GateBusyReason::Database(DbError::Unavailable)).is_permanent());
        assert!(GateBusy::new(GateBusyReason::Database(DbError::Config)).is_permanent());
        assert_eq!(
            GateBusy::new(GateBusyReason::Busy).to_string(),
            "rate gate busy"
        );
        assert!(db("42501").to_string().contains("42501"));
    }

    #[tokio::test]
    async fn job_lock_connect_is_bounded_by_a_timeout() {
        use std::str::FromStr;
        // A server that accepts the TCP connection and never speaks the Postgres protocol.
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let _hold = tokio::spawn(async move {
            let _conn = l.accept().await;
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        let opts = PgConnectOptions::from_str(&format!(
            "postgres://u:p@127.0.0.1:{port}/db?sslmode=disable"
        ))
        .unwrap();
        let t = LockTimeouts {
            connect: Duration::from_millis(250),
            check: Duration::from_millis(250),
        };
        let started = std::time::Instant::now();
        let err = try_job_lock_with(&opts, "awn_ingest", None, t)
            .await
            .unwrap_err();
        assert_eq!(err, DbError::Timeout);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn job_lock_connect_failure_is_a_db_error_without_host() {
        use std::str::FromStr;
        let opts = PgConnectOptions::from_str("postgres://u:secretsecret@127.0.0.1:1/db").unwrap();
        let err = try_job_lock(&opts, "awn_ingest", None).await.unwrap_err();
        let t = format!("{err} {err:?}");
        assert!(!t.contains("secretsecret") && !t.contains("127.0.0.1"));
    }
}
