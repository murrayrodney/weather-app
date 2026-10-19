//! The poller (03-poller): scheduled and on-demand jobs that keep the observation, rollup and reference tables
//! filled. Library half of the `poller` binary so integration tests can drive the jobs directly.

pub mod awn_call;
pub mod cli;
pub mod creds;
pub mod env;
pub mod job;
pub mod jobs;
pub mod keys;
pub mod mapping;
pub mod obs_write;
pub mod scheduler;
/// Deterministic synthetic AWN/ACIS data for fixtures and tests.
#[cfg(any(test, feature = "test-support"))]
pub mod synth;

use famweather_core::db::DbError;
use uuid::Uuid;

pub use env::Env;
pub use job::{run_job, JobFlags, RunOutcome};
pub use jobs::JobName;

/// Runs the named job once under its lock (`poller run-job`, the scheduler and the tests share this).
pub async fn run_named(
    env: &Env,
    job: JobName,
    station: Option<Uuid>,
    flags: JobFlags,
) -> Result<RunOutcome, DbError> {
    use jobs::{
        awn_backfill::AwnBackfill, awn_ingest::AwnIngest,
        credential_reencrypt::CredentialReencrypt, daily_rollup::DailyRollup,
        reference_sync::ReferenceSync,
    };
    match job {
        JobName::AwnIngest => run_job(env, &AwnIngest, station, flags).await,
        JobName::AwnBackfill => run_job(env, &AwnBackfill, station, flags).await,
        JobName::DailyRollup => run_job(env, &DailyRollup, station, flags).await,
        JobName::ReferenceSync => run_job(env, &ReferenceSync, station, flags).await,
        JobName::CredentialReencrypt => run_job(env, &CredentialReencrypt, station, flags).await,
    }
}
