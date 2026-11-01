//! Command line (04 section 10.2). There is deliberately no `--api-key` option anywhere (S7): the AWN key is read from a
//! hidden prompt or from stdin only. clap rejects unknown options with exit status 2.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use famweather_core::config::{AdminCommand, CredentialMode, OwnerRef, UserRef};
use uuid::Uuid;

#[derive(Parser, Debug)]
#[command(
    name = "admin",
    version,
    about = "Family Weather operator CLI (runs as app_migrator; see docs/guides/04-database.md section 10)"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Cmd,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Apply the embedded sqlx migrations forward (needs only APP_ENV and DATABASE_URL; refuses unless current_user is app_migrator).
    Migrate,
    /// Print a new HPKE key pair; with --check, verify that CREDENTIALS_PRIVATE_KEY and CREDENTIALS_PUBLIC_KEY match.
    Keygen {
        #[arg(long)]
        check: bool,
    },
    /// Idle so an operator can run commands in an interactive one-off machine.
    Wait {
        #[arg(long)]
        minutes: u32,
    },
    /// Create a confirmed login with no password and the app rows; print a one-time set-password link.
    Invite {
        #[arg(long)]
        email: String,
        #[arg(long)]
        display_name: String,
        /// Seed approved follows in both directions with every active user.
        #[arg(long)]
        follow_all: bool,
        /// Make this user the single admin (refused when one exists).
        #[arg(long)]
        admin: bool,
        /// Repair an auth user left without an app row by an earlier failed run.
        #[arg(long)]
        adopt: bool,
        /// Fault injection for the compose tests: fail after the auth user was created (compiled in only with `compose-tests`).
        #[cfg(feature = "compose-tests")]
        #[arg(long, hide = true)]
        test_fail_after_auth: bool,
    },
    /// Print a new one-time set-password link for an active user and sign them out everywhere.
    ResetLink {
        #[arg(long)]
        email: String,
    },
    /// Add a shared reference (baseline) weather station.
    AddReferenceStation {
        #[arg(long)]
        id: String,
        #[arg(long)]
        name: String,
        #[arg(long, allow_negative_numbers = true)]
        lat: f64,
        #[arg(long, allow_negative_numbers = true)]
        lon: f64,
        #[arg(long)]
        timezone: String,
        #[arg(long, allow_negative_numbers = true)]
        std_offset_minutes: i16,
        #[arg(long, allow_negative_numbers = true)]
        elevation_ft: Option<i32>,
    },
    /// Add a station (stopgap). The AWN API key is read from a hidden prompt, or one line of stdin when not a terminal.
    AddStation {
        /// Owner: a user id (UUID) or a display name.
        #[arg(long)]
        owner: String,
        #[arg(long)]
        mac: String,
        #[arg(long)]
        name: String,
        #[arg(long, allow_negative_numbers = true)]
        lat: f64,
        #[arg(long, allow_negative_numbers = true)]
        lon: f64,
        #[arg(long)]
        timezone: String,
        #[arg(long, allow_negative_numbers = true)]
        std_offset_minutes: i16,
        #[arg(long, allow_negative_numbers = true)]
        elevation_ft: Option<i32>,
        /// GHCN-Daily id of an existing reference station to use as the baseline.
        #[arg(long)]
        reference_station: Option<String>,
        /// Add the station under the owner's existing credential (only when its status is ok).
        #[arg(long, conflicts_with = "replace_key")]
        reuse_credential: bool,
        /// Replace the owner's credential with a new key (old row becomes `replaced`; all the owner's stations move).
        #[arg(long)]
        replace_key: bool,
    },
    /// Deactivate a user: is_active=false, sessions revoked, login banned (best effort).
    DeactivateUser {
        #[arg(
            long,
            conflicts_with = "display_name",
            required_unless_present = "display_name"
        )]
        id: Option<Uuid>,
        #[arg(long)]
        display_name: Option<String>,
        /// Allow deactivating the admin.
        #[arg(long)]
        force: bool,
    },
    /// Hard-delete an inactive user and all their data (the audit log is kept).
    DeleteUser {
        #[arg(long)]
        id: Uuid,
        /// The user's Supabase email; must match.
        #[arg(long)]
        confirm_email: String,
    },
    /// List users (tab separated); exits 1 when an app user has no login or a login has no app row.
    ListUsers,
    /// Recreate backed-up Supabase logins (with their original ids) from the decrypted auth-users.csv of a backup.
    RestoreAuthUsers {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        dry_run: bool,
    },
    /// Local only: create the demo users, reference stations, stations and follows from SEED_DIR/seed.toml.
    SeedDev {
        #[arg(long)]
        no_stations: bool,
    },
}

/// Things the shared `AdminCommand` does not carry.
#[derive(Debug, Default, Clone, Copy)]
pub struct Extras {
    pub test_fail_after_auth: bool,
}

impl Cmd {
    pub fn into_parts(self) -> (AdminCommand, Extras) {
        #[cfg_attr(not(feature = "compose-tests"), allow(unused_mut))]
        let mut extras = Extras::default();
        let cmd = match self {
            Cmd::Migrate => AdminCommand::Migrate,
            Cmd::Keygen { check } => AdminCommand::Keygen { check },
            Cmd::Wait { minutes } => AdminCommand::Wait { minutes },
            Cmd::Invite {
                email,
                display_name,
                follow_all,
                admin,
                adopt,
                #[cfg(feature = "compose-tests")]
                test_fail_after_auth,
            } => {
                #[cfg(feature = "compose-tests")]
                {
                    extras.test_fail_after_auth = test_fail_after_auth;
                }
                AdminCommand::Invite {
                    email,
                    display_name,
                    follow_all,
                    admin,
                    adopt,
                }
            }
            Cmd::ResetLink { email } => AdminCommand::ResetLink { email },
            Cmd::AddReferenceStation {
                id,
                name,
                lat,
                lon,
                timezone,
                std_offset_minutes,
                elevation_ft,
            } => AdminCommand::AddReferenceStation {
                id,
                name,
                lat,
                lon,
                timezone,
                std_offset_minutes,
                elevation_ft,
            },
            Cmd::AddStation {
                owner,
                mac,
                name,
                lat,
                lon,
                timezone,
                std_offset_minutes,
                elevation_ft,
                reference_station,
                reuse_credential,
                replace_key,
            } => AdminCommand::AddStation {
                owner: match owner.parse::<Uuid>() {
                    Ok(id) => OwnerRef::Id(id),
                    Err(_) => OwnerRef::DisplayName(owner),
                },
                mac,
                name,
                lat,
                lon,
                timezone,
                std_offset_minutes,
                elevation_ft,
                reference_station,
                credential: match (reuse_credential, replace_key) {
                    (true, _) => CredentialMode::Reuse,
                    (_, true) => CredentialMode::Replace,
                    _ => CredentialMode::New,
                },
            },
            Cmd::DeactivateUser {
                id,
                display_name,
                force,
            } => AdminCommand::DeactivateUser {
                who: match (id, display_name) {
                    (Some(id), _) => UserRef::Id(id),
                    (None, Some(name)) => UserRef::DisplayName(name),
                    (None, None) => unreachable!("clap requires --id or --display-name"),
                },
                force,
            },
            Cmd::DeleteUser { id, confirm_email } => AdminCommand::DeleteUser { id, confirm_email },
            Cmd::ListUsers => AdminCommand::ListUsers,
            Cmd::RestoreAuthUsers { file, dry_run } => {
                AdminCommand::RestoreAuthUsers { file, dry_run }
            }
            Cmd::SeedDev { no_stations } => AdminCommand::SeedDev { no_stations },
        };
        (cmd, extras)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn clap_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn no_argument_is_named_api_key() {
        fn walk(cmd: &clap::Command, path: &str, found: &mut Vec<String>) {
            for a in cmd.get_arguments() {
                let id = a.get_id().as_str();
                let long = a.get_long().unwrap_or("");
                if id.contains("api") && id.contains("key") || long.contains("api-key") {
                    found.push(format!("{path} --{long}"));
                }
            }
            for sub in cmd.get_subcommands() {
                walk(sub, &format!("{path} {}", sub.get_name()), found);
            }
        }
        let mut found = Vec::new();
        walk(&Cli::command(), "admin", &mut found);
        assert!(found.is_empty(), "api-key options exist: {found:?}");
    }

    #[test]
    fn api_key_flag_is_rejected_with_usage_error() {
        let err = Cli::try_parse_from([
            "admin",
            "add-station",
            "--owner",
            "x",
            "--mac",
            "AA:BB:CC:00:00:01",
            "--name",
            "n",
            "--lat",
            "1",
            "--lon",
            "2",
            "--timezone",
            "UTC",
            "--std-offset-minutes",
            "0",
            "--api-key",
            "secret",
        ])
        .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn negative_coordinates_and_owner_forms_parse() {
        let cli = Cli::try_parse_from([
            "admin",
            "add-station",
            "--owner",
            "Alex",
            "--mac",
            "AA:BB:CC:00:00:01",
            "--name",
            "n",
            "--lat",
            "40.5",
            "--lon",
            "-105.07",
            "--timezone",
            "America/Denver",
            "--std-offset-minutes",
            "-420",
            "--reuse-credential",
        ])
        .unwrap();
        let (cmd, _) = cli.command.into_parts();
        match cmd {
            AdminCommand::AddStation {
                owner,
                lon,
                std_offset_minutes,
                credential,
                ..
            } => {
                assert_eq!(owner, OwnerRef::DisplayName("Alex".into()));
                assert_eq!(lon, -105.07);
                assert_eq!(std_offset_minutes, -420);
                assert_eq!(credential, CredentialMode::Reuse);
            }
            other => panic!("{other:?}"),
        }
        let id = "7b1c8f64-6a0e-4a0c-9d35-5e4a8f2a0c11";
        let cli = Cli::try_parse_from([
            "admin",
            "add-station",
            "--owner",
            id,
            "--mac",
            "AA:BB:CC:00:00:01",
            "--name",
            "n",
            "--lat",
            "1",
            "--lon",
            "2",
            "--timezone",
            "UTC",
            "--std-offset-minutes",
            "0",
        ])
        .unwrap();
        let (cmd, _) = cli.command.into_parts();
        assert!(matches!(
            cmd,
            AdminCommand::AddStation {
                owner: OwnerRef::Id(_),
                credential: CredentialMode::New,
                ..
            }
        ));
    }

    #[test]
    fn credential_flags_conflict_and_deactivate_needs_a_target() {
        assert!(Cli::try_parse_from([
            "admin",
            "add-station",
            "--owner",
            "x",
            "--mac",
            "m",
            "--name",
            "n",
            "--lat",
            "1",
            "--lon",
            "2",
            "--timezone",
            "UTC",
            "--std-offset-minutes",
            "0",
            "--reuse-credential",
            "--replace-key",
        ])
        .is_err());
        assert!(Cli::try_parse_from(["admin", "deactivate-user"]).is_err());
        assert!(Cli::try_parse_from([
            "admin",
            "deactivate-user",
            "--display-name",
            "A",
            "--id",
            "7b1c8f64-6a0e-4a0c-9d35-5e4a8f2a0c11"
        ])
        .is_err());
    }

    #[test]
    fn every_documented_subcommand_exists() {
        let names: Vec<String> = Cli::command()
            .get_subcommands()
            .map(|c| c.get_name().to_owned())
            .collect();
        for want in [
            "migrate",
            "keygen",
            "wait",
            "invite",
            "reset-link",
            "add-reference-station",
            "add-station",
            "deactivate-user",
            "delete-user",
            "list-users",
            "restore-auth-users",
            "seed-dev",
        ] {
            assert!(names.iter().any(|n| n == want), "missing subcommand {want}");
        }
    }
}
