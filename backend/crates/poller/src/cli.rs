//! Command line (03 section 3.1).

use chrono::NaiveDate;
use clap::{Args, Parser, Subcommand};
use uuid::Uuid;

use crate::job::JobFlags;
use crate::jobs::JobName;

#[derive(Debug, Parser)]
#[command(name = "poller", version, about = "Family weather poller")]
pub struct Cli {
    /// Without a subcommand the scheduler runs (same as `run`).
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the scheduler (the default).
    Run,
    /// Exit 0 if the heartbeat file was touched within 120 s, else 1. Loads no config, no network, no database.
    Healthcheck,
    /// Run one job once in the foreground: exit 0 success, 1 failure, 3 lock held.
    RunJob(RunJobArgs),
}

#[derive(Debug, Args)]
pub struct RunJobArgs {
    /// awn_ingest | awn_backfill | daily_rollup | reference_sync | credential_reencrypt
    pub job: String,
    /// Only this station (awn_ingest, awn_backfill, daily_rollup)
    #[arg(long)]
    pub station: Option<Uuid>,
    /// awn_backfill: stop once history reaches this date
    #[arg(long)]
    pub since: Option<NaiveDate>,
    /// awn_backfill: at most this many pages per station
    #[arg(long)]
    pub max_pages: Option<u32>,
    /// awn_backfill: start from now again instead of resuming at the stored floor
    #[arg(long)]
    pub restart: bool,
    /// daily_rollup: force these days (with --to), ignoring dirty detection
    #[arg(long)]
    pub from: Option<NaiveDate>,
    #[arg(long)]
    pub to: Option<NaiveDate>,
}

#[derive(Debug, PartialEq)]
pub struct JobRequest {
    pub job: JobName,
    pub station: Option<Uuid>,
    pub flags: JobFlags,
}

impl RunJobArgs {
    /// Validates flags against the job (usage errors are reported by the caller with exit code 2).
    pub fn into_request(self) -> Result<JobRequest, String> {
        let job = JobName::parse(&self.job).ok_or_else(|| format!("unknown job {:?}", self.job))?;
        if self.station.is_some() && !job.accepts_station() {
            return Err(format!("--station is not accepted by {}", job.as_str()));
        }
        let backfill_flags = self.since.is_some() || self.max_pages.is_some() || self.restart;
        if backfill_flags && job != JobName::AwnBackfill {
            return Err(format!(
                "--since, --max-pages and --restart apply to awn_backfill, not {}",
                job.as_str()
            ));
        }
        let range_flags = self.from.is_some() || self.to.is_some();
        if range_flags && job != JobName::DailyRollup {
            return Err(format!(
                "--from and --to apply to daily_rollup, not {}",
                job.as_str()
            ));
        }
        if self.from.is_some() != self.to.is_some() {
            return Err("--from and --to must be given together".to_owned());
        }
        if let (Some(f), Some(t)) = (self.from, self.to) {
            if f > t {
                return Err("--from must not be after --to".to_owned());
            }
        }
        if self.max_pages == Some(0) {
            return Err("--max-pages must be at least 1".to_owned());
        }
        let flags = JobFlags {
            since: self.since,
            max_pages: self.max_pages,
            restart: self.restart,
            from: self.from,
            to: self.to,
            // An explicit rollup run heals days that have observations but no rollup (e.g. after e2e backfill).
            self_heal: job == JobName::DailyRollup && !range_flags,
            pickup_only: false,
        };
        Ok(JobRequest {
            job,
            station: self.station,
            flags,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<JobRequest, String> {
        let mut v = vec!["poller", "run-job"];
        v.extend(args);
        match Cli::try_parse_from(v).map_err(|e| e.to_string())?.command {
            Some(Command::RunJob(a)) => a.into_request(),
            _ => unreachable!(),
        }
    }

    #[test]
    fn plain_jobs() {
        let r = parse(&["awn_ingest"]).unwrap();
        assert_eq!(r.job, JobName::AwnIngest);
        assert!(r.station.is_none());
        let r = parse(&["daily_rollup"]).unwrap();
        assert!(r.flags.self_heal);
        let r = parse(&["daily_rollup", "--from", "2026-01-01", "--to", "2026-01-31"]).unwrap();
        assert!(!r.flags.self_heal && r.flags.from.is_some());
    }

    #[test]
    fn backfill_flags() {
        let id = "7b7c1c0e-0000-4000-8000-000000000001";
        let r = parse(&[
            "awn_backfill",
            "--station",
            id,
            "--since",
            "2026-01-01",
            "--max-pages",
            "2",
            "--restart",
        ])
        .unwrap();
        assert_eq!(r.job, JobName::AwnBackfill);
        assert_eq!(r.station.unwrap().to_string(), id);
        assert!(r.flags.restart && r.flags.max_pages == Some(2));
    }

    #[test]
    fn bad_combinations_are_errors() {
        let id = "7b7c1c0e-0000-4000-8000-000000000001";
        assert!(parse(&["nope"]).is_err());
        assert!(parse(&["reference_sync", "--station", id]).is_err());
        assert!(parse(&["credential_reencrypt", "--station", id]).is_err());
        assert!(parse(&["awn_ingest", "--restart"]).is_err());
        assert!(parse(&["awn_ingest", "--since", "2026-01-01"]).is_err());
        assert!(parse(&["awn_backfill", "--from", "2026-01-01", "--to", "2026-01-02"]).is_err());
        assert!(parse(&["daily_rollup", "--from", "2026-01-01"]).is_err());
        assert!(parse(&["daily_rollup", "--from", "2026-02-01", "--to", "2026-01-01"]).is_err());
        assert!(parse(&["awn_backfill", "--max-pages", "0"]).is_err());
        assert!(parse(&["awn_backfill", "--since", "not-a-date"]).is_err());
    }

    #[test]
    fn no_subcommand_means_run() {
        let cli = Cli::try_parse_from(["poller"]).unwrap();
        assert!(cli.command.is_none());
        assert!(matches!(
            Cli::try_parse_from(["poller", "healthcheck"])
                .unwrap()
                .command,
            Some(Command::Healthcheck)
        ));
    }
}
