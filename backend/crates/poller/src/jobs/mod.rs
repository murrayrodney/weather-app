//! The jobs. Which of them the scheduler registers is the compile-time list in `scheduler::SCHEDULED`.

pub mod awn_backfill;
pub mod awn_ingest;
pub mod credential_reencrypt;
pub mod daily_rollup;
pub mod reference_sync;

/// Job names (`job_runs.job`, lock names, `run-job` arguments).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobName {
    AwnIngest,
    AwnBackfill,
    DailyRollup,
    ReferenceSync,
    CredentialReencrypt,
}

impl JobName {
    pub const ALL: [JobName; 5] = [
        JobName::AwnIngest,
        JobName::AwnBackfill,
        JobName::DailyRollup,
        JobName::ReferenceSync,
        JobName::CredentialReencrypt,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            JobName::AwnIngest => "awn_ingest",
            JobName::AwnBackfill => "awn_backfill",
            JobName::DailyRollup => "daily_rollup",
            JobName::ReferenceSync => "reference_sync",
            JobName::CredentialReencrypt => "credential_reencrypt",
        }
    }

    pub fn parse(s: &str) -> Option<JobName> {
        Self::ALL.into_iter().find(|j| j.as_str() == s)
    }

    /// `--station` is accepted by these jobs and is an error for the others.
    pub fn accepts_station(self) -> bool {
        matches!(
            self,
            JobName::AwnIngest | JobName::AwnBackfill | JobName::DailyRollup
        )
    }
}
