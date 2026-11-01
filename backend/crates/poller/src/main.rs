//! `poller` binary (03 section 3.1): `poller` / `poller run`, `poller healthcheck`, `poller run-job <job>`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use clap::Parser;
use famweather_core::acis::{AcisApi, LiveAcis, ReplayAcis};
use famweather_core::awn::{AwnApi, LiveAwn, ReplayAwn};
use famweather_core::clock::{Clock, SystemClock};
use famweather_core::config::{self, AwnMode, Binary, Config, PollerConfig};
use famweather_core::db::{self, PoolSpec};
use famweather_core::redact;
use poller_lib::cli::{Cli, Command};
use poller_lib::env::Env;
use poller_lib::keys::KeyRing;
use poller_lib::run_named;
use poller_lib::scheduler::{self, HEARTBEAT_PATH};
use tokio_util::sync::CancellationToken;

/// Per-call timeouts (03 sections 5.2 and 7.5).
const AWN_CALL_TIMEOUT: Duration = Duration::from_secs(20);
const ACIS_CALL_TIMEOUT: Duration = Duration::from_secs(60);

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Healthcheck) => {
            // Loads no config; no network, no database.
            if scheduler::heartbeat_fresh(
                &PathBuf::from(HEARTBEAT_PATH),
                scheduler::HEARTBEAT_MAX_AGE,
            ) {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        command => {
            // Plain text in local Compose (readable in `docker compose logs` and Docker Desktop); JSON everywhere else.
            let json = std::env::var("APP_ENV").as_deref() != Ok("local");
            if redact::init_tracing(json).is_err() {
                eprintln!("error: could not initialise logging");
                return ExitCode::FAILURE;
            }
            match run(command).await {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("error: {}", redact::scrub(&format!("{e:#}")));
                    ExitCode::FAILURE
                }
            }
        }
    }
}

fn load_config() -> Result<PollerConfig> {
    match config::load(Binary::Poller)? {
        Config::Poller(c) => Ok(c),
        _ => bail!("internal error: unexpected configuration kind"),
    }
}

async fn build_env(mut cfg: PollerConfig, pool_max: u32, cancel: CancellationToken) -> Result<Env> {
    if let Some(k) = &cfg.awn_application_key {
        redact::register(k);
    }
    for url in cfg.healthcheck_urls.values() {
        redact::register(url);
    }
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let (awn, acis): (Arc<dyn AwnApi>, Arc<dyn AcisApi>) = match cfg.awn_mode {
        AwnMode::Live => {
            let key = cfg
                .awn_application_key
                .clone()
                .ok_or_else(|| anyhow!("AWN_APPLICATION_KEY is required with AWN_MODE=live"))?;
            (
                Arc::new(LiveAwn::new(key, AWN_CALL_TIMEOUT).map_err(|e| anyhow!("{e}"))?),
                Arc::new(LiveAcis::new(ACIS_CALL_TIMEOUT).map_err(|e| anyhow!("{e}"))?),
            )
        }
        AwnMode::Replay => (
            Arc::new(
                ReplayAwn::new(cfg.fixtures_dir.clone(), clock.clone())
                    .map_err(|e| anyhow!("{e}"))?,
            ),
            Arc::new(ReplayAcis::new(cfg.fixtures_dir.clone()).map_err(|e| anyhow!("{e}"))?),
        ),
    };
    let keys = if cfg.current_key.is_some() {
        Some(Arc::new(
            KeyRing::from_config(&cfg).map_err(|e| anyhow!("{e}"))?,
        ))
    } else {
        None
    };
    let spec = PoolSpec {
        max: pool_max,
        ..PoolSpec::poller()
    };
    let pool = db::pool(&cfg.database, spec)
        .await
        .map_err(|e| anyhow!("{e}"))?;
    let user: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(&pool)
        .await?;
    tracing::info!(db_user = %user, "connected to the database");
    let pings: HashMap<_, _> = std::mem::take(&mut cfg.healthcheck_urls);
    Ok(Env::new(
        pool,
        cfg.database.clone(),
        clock,
        awn,
        acis,
        keys,
        pings,
        cancel,
    )
    .with_awn_mode(cfg.awn_mode))
}

async fn run(command: Option<Command>) -> Result<ExitCode> {
    match command {
        None | Some(Command::Run) => {
            redact::install_panic_hook();
            let cfg = load_config()?;
            let heartbeat = PathBuf::from(HEARTBEAT_PATH);
            if !cfg.enabled {
                scheduler::run_disabled(heartbeat, scheduler::signal_shutdown()).await;
                return Ok(ExitCode::SUCCESS);
            }
            let env = build_env(cfg, 4, CancellationToken::new()).await?;
            scheduler::run_enabled(
                env,
                heartbeat,
                scheduler::SHUTDOWN_GRACE,
                scheduler::signal_shutdown(),
            )
            .await;
            Ok(ExitCode::SUCCESS)
        }
        Some(Command::RunJob(args)) => {
            let req = match args.into_request() {
                Ok(r) => r,
                Err(msg) => {
                    eprintln!("error: {msg}");
                    return Ok(ExitCode::from(2));
                }
            };
            // Allowed with POLLER_ENABLED=false: an explicit operator action. One-off runs use pool max 2.
            let cfg = load_config()?;
            let cancel = CancellationToken::new();
            let env = build_env(cfg, 2, cancel.clone()).await?;
            let shutdown = scheduler::signal_shutdown();
            let job = run_named(&env, req.job, req.station, req.flags);
            tokio::pin!(job);
            let outcome = tokio::select! {
                r = &mut job => r,
                () = shutdown => {
                    // Ask the job to stop at its next unit, then let it finish and record `cancelled`.
                    cancel.cancel();
                    job.await
                }
            };
            env.pool.close().await;
            let outcome = outcome.map_err(|e| anyhow!("{e}"))?;
            tracing::info!(
                job = req.job.as_str(),
                exit = outcome.exit_code(),
                "run-job finished"
            );
            Ok(ExitCode::from(outcome.exit_code()))
        }
        Some(Command::Healthcheck) => unreachable!("handled in main"),
    }
}
