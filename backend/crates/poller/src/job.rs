//! Job framework (03 sections 4.2 and 4.3): lock, sweep, `job_runs`, pings, result handling.

use std::panic::AssertUnwindSafe;
use std::time::Instant;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use famweather_core::db::DbError;
use famweather_core::heartbeat::{self, PingKind};
use famweather_core::ratelimit::{try_job_lock_with, JobLock, LockLost};
use famweather_core::redact;
use futures::FutureExt;
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use uuid::Uuid;

use crate::env::Env;

/// Longest error text stored in `job_runs.detail.error`.
pub const MAX_ERROR_BYTES: usize = 1000;
/// List fields in `detail` are capped at this many items.
pub const MAX_LIST_ITEMS: usize = 20;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct JobFlags {
    /// `awn_backfill --since`
    pub since: Option<NaiveDate>,
    /// `awn_backfill --max-pages`
    pub max_pages: Option<u32>,
    /// `awn_backfill --restart`
    pub restart: bool,
    /// `daily_rollup --from/--to` (force these days, ignoring dirty detection)
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
    /// `daily_rollup`: enqueue days with observations but no rollup (process start and the daily housekeeping run).
    pub self_heal: bool,
    /// `reference_sync`: only stations never synced (the hourly pickup).
    pub pickup_only: bool,
}

/// Why a job stopped early.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interrupt {
    Cancelled,
    LockLost,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobErrorKind {
    Cancelled,
    LockLost,
    Failed(String),
}

/// `partial` carries the counters and gaps collected so far; the wrapper merges `error` into it.
#[derive(Debug, Clone)]
pub struct JobError {
    pub kind: JobErrorKind,
    pub partial: Value,
}

impl JobError {
    pub fn failed(msg: impl Into<String>, partial: Value) -> Self {
        Self {
            kind: JobErrorKind::Failed(msg.into()),
            partial,
        }
    }

    pub fn interrupted(i: Interrupt, partial: Value) -> Self {
        Self {
            kind: match i {
                Interrupt::Cancelled => JobErrorKind::Cancelled,
                Interrupt::LockLost => JobErrorKind::LockLost,
            },
            partial,
        }
    }
}

pub struct JobContext {
    pub env: Env,
    pub lock: JobLock,
    pub last_success: Option<DateTime<Utc>>,
    pub run_id: Option<Uuid>,
    pub station: Option<Uuid>,
    pub flags: JobFlags,
}

impl JobContext {
    pub fn pool(&self) -> &PgPool {
        &self.env.pool
    }

    /// Between units of work: cancellation, then the lock connection.
    pub async fn checkpoint(&mut self) -> Result<(), Interrupt> {
        if self.env.cancel.is_cancelled() {
            return Err(Interrupt::Cancelled);
        }
        self.lock
            .check()
            .await
            .map_err(|LockLost| Interrupt::LockLost)
    }

    /// Inserts a `running` `job_runs` row (jobs that write one row per subject, such as `awn_backfill`).
    pub async fn begin_run(&self, job: &str, subject: Option<Uuid>) -> Result<Uuid, DbError> {
        insert_run(&self.env, job, subject).await
    }

    pub async fn finish_run(
        &self,
        id: Uuid,
        status: RunStatus,
        detail: Value,
    ) -> Result<(), DbError> {
        let r = finish_run_row(&self.env.pool, id, status, detail).await;
        forget_run(&self.env, id);
        r
    }
}

#[async_trait]
pub trait Job: Send + Sync {
    /// `job_runs.job`, the lock name and the healthcheck identity.
    fn name(&self) -> &'static str;

    /// Suffix of `HEALTHCHECK_URL_<SUFFIX>`; `None` for jobs that never ping.
    fn ping_suffix(&self) -> Option<&'static str> {
        None
    }

    /// `awn_backfill` writes one `job_runs` row per station itself; the wrapper then writes none.
    fn manages_runs(&self) -> bool {
        false
    }

    /// Asked after the lock is taken and before anything is written: `false` skips the run silently (no
    /// `job_runs` row, no ping), as the hourly reference pickup does when there is nothing to load.
    async fn should_run(&self, _ctx: &JobContext) -> bool {
        true
    }

    async fn run(&self, ctx: &mut JobContext) -> Result<Value, JobError>;

    /// For a failed run: send `/fail` (true) or a success ping (false). Default: fail.
    async fn ping_fail_on_failure(&self, _pool: &PgPool, _detail: &Value) -> bool {
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Success,
    Failed,
    Cancelled,
    Abandoned,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RunStatus::Success => "success",
            RunStatus::Failed => "failed",
            RunStatus::Cancelled => "cancelled",
            RunStatus::Abandoned => "abandoned",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    LockHeld,
    NothingToDo,
}

#[derive(Debug, Clone)]
pub enum RunOutcome {
    Skipped(SkipReason),
    Finished {
        run_id: Option<Uuid>,
        status: RunStatus,
        detail: Value,
    },
}

impl RunOutcome {
    /// `run-job` exit code: 0 success (or nothing to do), 1 failure, 3 lock held.
    pub fn exit_code(&self) -> u8 {
        match self {
            RunOutcome::Skipped(SkipReason::LockHeld) => 3,
            RunOutcome::Skipped(SkipReason::NothingToDo) => 0,
            RunOutcome::Finished {
                status: RunStatus::Success,
                ..
            } => 0,
            RunOutcome::Finished { .. } => 1,
        }
    }
}

pub(crate) async fn insert_run(
    env: &Env,
    job: &str,
    subject: Option<Uuid>,
) -> Result<Uuid, DbError> {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO app.job_runs (job, subject_id, status) VALUES ($1, $2, 'running') RETURNING id",
    )
    .bind(job)
    .bind(subject)
    .fetch_one(&env.pool)
    .await
    .map_err(DbError::from)?;
    env.run_ids
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .push(id);
    Ok(id)
}

/// A finished run no longer needs shutdown cleanup: keeps `run_ids` bounded by the runs in flight.
pub(crate) fn forget_run(env: &Env, id: Uuid) {
    env.run_ids
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .retain(|r| *r != id);
}

pub(crate) async fn finish_run_row(
    pool: &PgPool,
    id: Uuid,
    status: RunStatus,
    detail: Value,
) -> Result<(), DbError> {
    const SQL: &str =
        "UPDATE app.job_runs SET status = $2, finished_at = now(), detail = $3 WHERE id = $1";
    let first = sqlx::query(SQL)
        .bind(id)
        .bind(status.as_str())
        .bind(&detail)
        .execute(pool)
        .await;
    match first {
        Ok(_) => Ok(()),
        // The 8 KiB CHECK on `detail`: record that instead of losing the status.
        Err(sqlx::Error::Database(d)) if d.code().as_deref() == Some("23514") => {
            tracing::warn!(run_id = %id, "job detail too large, storing a marker instead");
            sqlx::query(SQL)
                .bind(id)
                .bind(status.as_str())
                .bind(json!({"error": "detail_too_large"}))
                .execute(pool)
                .await
                .map(|_| ())
                .map_err(DbError::from)
        }
        Err(e) => Err(DbError::from(e)),
    }
}

/// Redacted and cut to `max` bytes on a character boundary.
pub fn truncate_error(msg: &str, max: usize) -> String {
    let scrubbed = redact::scrub(msg);
    if scrubbed.len() <= max {
        return scrubbed.into_owned();
    }
    let mut end = max;
    while !scrubbed.is_char_boundary(end) {
        end -= 1;
    }
    scrubbed[..end].to_owned()
}

/// `partial` with `"error": <text>` merged in (counters and gaps are kept).
pub fn merge_error(partial: Value, error: &str) -> Value {
    let mut m: Map<String, Value> = match partial {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    m.insert(
        "error".to_owned(),
        Value::String(truncate_error(error, MAX_ERROR_BYTES)),
    );
    Value::Object(m)
}

/// Keeps at most [`MAX_LIST_ITEMS`] items of a list for `detail`.
pub fn capped<T: Clone>(items: &[T]) -> Vec<T> {
    items.iter().take(MAX_LIST_ITEMS).cloned().collect()
}

async fn ping(env: &Env, suffix: Option<&'static str>, kind: PingKind) {
    let Some(url) = suffix.and_then(|s| env.pings.get(s)) else {
        return;
    };
    heartbeat::ping(kind, url).await;
}

/// Runs `job` once under its advisory lock (03 section 4.3).
pub async fn run_job<J: Job>(
    env: &Env,
    job: &J,
    station: Option<Uuid>,
    flags: JobFlags,
) -> Result<RunOutcome, DbError> {
    let name = job.name();
    let Some(lock) =
        try_job_lock_with(&env.db_opts, name, env.lock_hook.clone(), env.lock_timeouts).await?
    else {
        tracing::info!(job = name, "job skipped, lock held");
        return Ok(RunOutcome::Skipped(SkipReason::LockHeld));
    };
    let mut ctx = JobContext {
        env: env.clone(),
        lock,
        last_success: None,
        run_id: None,
        station,
        flags,
    };
    let result = run_locked(env, job, &mut ctx).await;
    if let Err(e) = ctx.lock.release().await {
        tracing::warn!(job = name, error = %e, "releasing the job lock failed (the connection is closed)");
    }
    result
}

async fn run_locked<J: Job>(
    env: &Env,
    job: &J,
    ctx: &mut JobContext,
) -> Result<RunOutcome, DbError> {
    let name = job.name();
    if !job.should_run(ctx).await {
        return Ok(RunOutcome::Skipped(SkipReason::NothingToDo));
    }
    // Sweep: safe because we hold the lock, so any `running` row of this job is a leftover of a dead run.
    let swept = sqlx::query(
        "UPDATE app.job_runs SET status = 'abandoned', finished_at = now() WHERE job = $1 AND status = 'running'",
    )
    .bind(name)
    .execute(&env.pool)
    .await
    .map_err(DbError::from)?
    .rows_affected();
    if swept > 0 {
        tracing::warn!(job = name, swept, "marked leftover running rows abandoned");
    }
    ctx.last_success = sqlx::query_scalar(
        "SELECT max(finished_at) FROM app.job_runs WHERE job = $1 AND status = 'success'",
    )
    .bind(name)
    .fetch_one(&env.pool)
    .await
    .map_err(DbError::from)?;
    tracing::debug!(job = name, last_success = ?ctx.last_success, "starting job");

    let manages = job.manages_runs();
    if !manages {
        ctx.run_id = Some(insert_run(env, name, ctx.station).await?);
    }
    ping(env, job.ping_suffix(), PingKind::Start).await;

    let started = Instant::now();
    // The wrapper does not race the job against cancellation: jobs observe `ctx.cancel` between units.
    let caught = AssertUnwindSafe(job.run(ctx)).catch_unwind().await;
    let (status, detail) = match caught {
        Ok(Ok(detail)) => (RunStatus::Success, detail),
        Ok(Err(JobError { kind, partial })) => match kind {
            JobErrorKind::Cancelled => (RunStatus::Cancelled, partial),
            JobErrorKind::LockLost => (RunStatus::Failed, merge_error(partial, "lock_lost")),
            JobErrorKind::Failed(msg) => (RunStatus::Failed, merge_error(partial, &msg)),
        },
        Err(_) => (RunStatus::Failed, merge_error(json!({}), "panic")),
    };
    if let Some(id) = ctx.run_id {
        let r = finish_run_row(&env.pool, id, status, detail.clone()).await;
        forget_run(env, id);
        r?;
    }
    tracing::info!(
        job = name,
        run_id = %ctx.run_id.map(|i| i.to_string()).unwrap_or_default(),
        status = status.as_str(),
        duration_ms = started.elapsed().as_millis() as u64,
        detail = %detail,
        "job finished"
    );
    match status {
        RunStatus::Success => ping(env, job.ping_suffix(), PingKind::Success).await,
        RunStatus::Failed => {
            let kind = if job.ping_fail_on_failure(&env.pool, &detail).await {
                PingKind::Fail
            } else {
                PingKind::Success
            };
            ping(env, job.ping_suffix(), kind).await
        }
        RunStatus::Cancelled | RunStatus::Abandoned => {}
    }
    Ok(RunOutcome::Finished {
        run_id: ctx.run_id,
        status,
        detail,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_respects_char_boundaries_and_scrubs() {
        let s = "é".repeat(800);
        let t = truncate_error(&s, MAX_ERROR_BYTES);
        assert!(t.len() <= MAX_ERROR_BYTES && t.chars().all(|c| c == 'é'));
        assert_eq!(truncate_error("short", 1000), "short");
        let t = truncate_error(
            "failed GET https://rt.ambientweather.net/v1/devices?apiKey=abc123",
            1000,
        );
        assert!(!t.contains("https://") && !t.contains("abc123"), "{t}");
    }

    #[test]
    fn error_merges_into_partial() {
        let v = merge_error(json!({"rows_seen": 5, "gaps": []}), "boom");
        assert_eq!(v["rows_seen"], 5);
        assert_eq!(v["error"], "boom");
        assert_eq!(merge_error(Value::Null, "x")["error"], "x");
    }

    #[test]
    fn exit_codes() {
        assert_eq!(RunOutcome::Skipped(SkipReason::LockHeld).exit_code(), 3);
        assert_eq!(RunOutcome::Skipped(SkipReason::NothingToDo).exit_code(), 0);
        let f = |s| RunOutcome::Finished {
            run_id: None,
            status: s,
            detail: Value::Null,
        };
        assert_eq!(f(RunStatus::Success).exit_code(), 0);
        assert_eq!(f(RunStatus::Failed).exit_code(), 1);
        assert_eq!(f(RunStatus::Cancelled).exit_code(), 1);
    }

    #[test]
    fn lists_are_capped() {
        let v: Vec<u32> = (0..100).collect();
        assert_eq!(capped(&v).len(), MAX_LIST_ITEMS);
    }
}
