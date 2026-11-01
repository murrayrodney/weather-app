//! `AdminEnv` and `AdminCommand` exactly as 04 section 10.1 defines them, plus the per-subcommand
//! required-environment table that `load(Binary::Admin(cmd))` enforces (a subcommand never demands variables
//! it does not use).

use std::path::PathBuf;

use secrecy::SecretString;
use url::Url;
use uuid::Uuid;

use super::{AppEnv, AwnMode};
use crate::crypto::CredentialPublicKey;

#[derive(Clone)]
pub struct AdminEnv {
    pub app_env: Option<AppEnv>,
    pub database_url: Option<SecretString>,
    pub supabase_url: Option<Url>,
    pub supabase_service_role_key: Option<SecretString>,
    pub local_demo_password: Option<SecretString>,
    pub site_url: Option<Url>,
    pub credentials_public_key: Option<CredentialPublicKey>,
    pub credentials_key_version: Option<i16>,
    pub awn_application_key: Option<SecretString>,
    pub awn_mode: Option<AwnMode>,
    pub fixtures_dir: Option<PathBuf>,
    pub seed_dir: PathBuf,
}

impl AdminEnv {
    pub(crate) fn empty() -> Self {
        Self {
            app_env: None,
            database_url: None,
            supabase_url: None,
            supabase_service_role_key: None,
            local_demo_password: None,
            site_url: None,
            credentials_public_key: None,
            credentials_key_version: None,
            awn_application_key: None,
            awn_mode: None,
            fixtures_dir: None,
            seed_dir: PathBuf::from("/seed"),
        }
    }
}

impl std::fmt::Debug for AdminEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let red = |present: bool| if present { "[REDACTED]" } else { "None" };
        f.debug_struct("AdminEnv")
            .field("app_env", &self.app_env)
            .field("database_url", &red(self.database_url.is_some()))
            .field(
                "supabase_url",
                &self
                    .supabase_url
                    .as_ref()
                    .map(|u| u.host_str().unwrap_or("").to_owned()),
            )
            .field(
                "supabase_service_role_key",
                &red(self.supabase_service_role_key.is_some()),
            )
            .field(
                "local_demo_password",
                &red(self.local_demo_password.is_some()),
            )
            .field(
                "site_url",
                &self
                    .site_url
                    .as_ref()
                    .map(|u| u.host_str().unwrap_or("").to_owned()),
            )
            .field(
                "credentials_public_key",
                &self.credentials_public_key.is_some(),
            )
            .field("credentials_key_version", &self.credentials_key_version)
            .field(
                "awn_application_key",
                &red(self.awn_application_key.is_some()),
            )
            .field("awn_mode", &self.awn_mode)
            .field("fixtures_dir", &self.fixtures_dir)
            .field("seed_dir", &self.seed_dir)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerRef {
    Id(Uuid),
    DisplayName(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserRef {
    Id(Uuid),
    DisplayName(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialMode {
    New,
    Reuse,
    Replace,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AdminCommand {
    Migrate,
    Keygen {
        check: bool,
    },
    Wait {
        minutes: u32,
    },
    Invite {
        email: String,
        display_name: String,
        follow_all: bool,
        admin: bool,
        adopt: bool,
    },
    ResetLink {
        email: String,
    },
    AddReferenceStation {
        id: String,
        name: String,
        lat: f64,
        lon: f64,
        timezone: String,
        std_offset_minutes: i16,
        elevation_ft: Option<i32>,
    },
    AddStation {
        owner: OwnerRef,
        mac: String,
        name: String,
        lat: f64,
        lon: f64,
        timezone: String,
        std_offset_minutes: i16,
        elevation_ft: Option<i32>,
        reference_station: Option<String>,
        credential: CredentialMode,
    },
    DeactivateUser {
        who: UserRef,
        force: bool,
    },
    DeleteUser {
        id: Uuid,
        confirm_email: String,
    },
    ListUsers,
    RestoreAuthUsers {
        file: PathBuf,
        dry_run: bool,
    },
    SeedDev {
        no_stations: bool,
    },
}

/// Environment variables a subcommand requires / may use (04 section 10.1 table).
pub(crate) struct Needs {
    pub required: &'static [&'static str],
    pub optional: &'static [&'static str],
}

const DB: &[&str] = &["APP_ENV", "DATABASE_URL"];
const DB_SUPABASE: &[&str] = &[
    "APP_ENV",
    "DATABASE_URL",
    "SUPABASE_URL",
    "SUPABASE_SERVICE_ROLE_KEY",
];
const SUPABASE_OPT: &[&str] = &["SUPABASE_URL", "SUPABASE_SERVICE_ROLE_KEY"];

pub(crate) fn needs(cmd: &AdminCommand) -> Needs {
    use AdminCommand::*;
    match cmd {
        Migrate | AddReferenceStation { .. } => Needs {
            required: DB,
            optional: &[],
        },
        Keygen { .. } => Needs {
            required: &[],
            optional: &[],
        },
        Wait { .. } => Needs {
            required: &[],
            optional: &["APP_ENV"],
        },
        Invite { .. } | ResetLink { .. } => Needs {
            required: &[
                "APP_ENV",
                "DATABASE_URL",
                "SUPABASE_URL",
                "SUPABASE_SERVICE_ROLE_KEY",
                "SITE_URL",
            ],
            optional: &[],
        },
        RestoreAuthUsers { .. } | DeleteUser { .. } => Needs {
            required: DB_SUPABASE,
            optional: &[],
        },
        ListUsers | DeactivateUser { .. } => Needs {
            required: DB,
            optional: SUPABASE_OPT,
        },
        AddStation { .. } => Needs {
            required: &[
                "APP_ENV",
                "DATABASE_URL",
                "CREDENTIALS_PUBLIC_KEY",
                "CREDENTIALS_KEY_VERSION",
            ],
            optional: &["AWN_APPLICATION_KEY", "AWN_MODE", "FIXTURES_DIR"],
        },
        SeedDev { .. } => Needs {
            required: &[
                "APP_ENV",
                "DATABASE_URL",
                "SUPABASE_URL",
                "SUPABASE_SERVICE_ROLE_KEY",
                "CREDENTIALS_PUBLIC_KEY",
                "CREDENTIALS_KEY_VERSION",
                "LOCAL_DEMO_PASSWORD",
            ],
            optional: &["SEED_DIR"],
        },
    }
}
