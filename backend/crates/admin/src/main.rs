//! `admin`: the operator CLI (04-database section 10). Runs as `app_migrator`; reads its configuration through
//! `famweather_core::config::load(Binary::Admin(..))`, which applies every shared refusal rule (production guards,
//! demo-value fingerprints, per-subcommand required environment).

mod audit;
mod cli;
mod commands;
mod ctx;
mod gotrue;
mod ops;
mod seed;
mod validate;

use std::io;
use std::process::ExitCode;

use anyhow::{bail, Result};
use clap::Parser;
use famweather_core::config::{self, AdminCommand, AdminEnv, Binary, Config};
use famweather_core::redact::{self, ScrubMakeWriter};
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Extras};
use crate::ctx::Ctx;

/// Logs go to stderr (stdout carries links, ids and tables) through the secret-scrubbing writer.
fn init_tracing() {
    let filter = std::env::var("RUST_LOG")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .and_then(|v| EnvFilter::try_new(v).ok())
        .unwrap_or_else(|| EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(ScrubMakeWriter(io::stderr))
        .try_init();
    redact::install_panic_hook();
}

#[tokio::main]
async fn main() -> ExitCode {
    // clap exits with status 2 on usage errors (including a rejected --api-key).
    let cli = Cli::parse();
    init_tracing();
    let (cmd, extras) = cli.command.into_parts();
    match run(cmd, extras).await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("error: {}", redact::scrub(&format!("{e:#}")));
            ExitCode::FAILURE
        }
    }
}

fn load_env(cmd: &AdminCommand) -> Result<AdminEnv> {
    match config::load(Binary::Admin(cmd.clone()))? {
        Config::Admin(env) => Ok(env),
        _ => bail!("internal error: unexpected configuration kind"),
    }
}

/// `Ok(true)` is exit 0, `Ok(false)` exit 1 (the command already said why), `Err` exit 1 with a message.
async fn run(cmd: AdminCommand, extras: Extras) -> Result<bool> {
    // keygen loads no configuration and touches no database or network.
    if let AdminCommand::Keygen { check } = cmd {
        commands::keygen(check)?;
        return Ok(true);
    }
    let env = load_env(&cmd)?;
    match cmd {
        AdminCommand::Keygen { .. } => unreachable!("handled above"),
        AdminCommand::Wait { minutes } => commands::wait(minutes).await?,
        AdminCommand::Migrate => commands::migrate(&env).await?,
        other => {
            let ctx = Ctx::connect(env).await?;
            return dispatch(&ctx, other, extras).await;
        }
    }
    Ok(true)
}

async fn dispatch(ctx: &Ctx, cmd: AdminCommand, extras: Extras) -> Result<bool> {
    match cmd {
        AdminCommand::Invite {
            email,
            display_name,
            follow_all,
            admin,
            adopt,
        } => commands::invite(ctx, email, display_name, follow_all, admin, adopt, extras).await?,
        AdminCommand::ResetLink { email } => commands::reset_link(ctx, email).await?,
        cmd @ AdminCommand::AddReferenceStation { .. } => {
            commands::add_reference_station(ctx, cmd).await?
        }
        cmd @ AdminCommand::AddStation { .. } => commands::add_station(ctx, cmd).await?,
        AdminCommand::DeactivateUser { who, force } => {
            commands::deactivate_user(ctx, who, force).await?
        }
        AdminCommand::DeleteUser { id, confirm_email } => {
            commands::delete_user(ctx, id, confirm_email).await?
        }
        AdminCommand::ListUsers => return commands::list_users(ctx).await,
        AdminCommand::RestoreAuthUsers { file, dry_run } => {
            return commands::restore_auth_users(ctx, &file, dry_run).await
        }
        AdminCommand::SeedDev { no_stations } => seed::run(ctx, no_stations).await?,
        AdminCommand::Migrate | AdminCommand::Keygen { .. } | AdminCommand::Wait { .. } => {
            unreachable!("handled before connecting")
        }
    }
    Ok(true)
}
