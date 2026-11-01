//! One configuration loader for every binary (conventions "Environments" and "Config ownership").
//!
//! `load(Binary)` reads the process environment, applies EVERY shared refusal rule and returns the typed
//! environment. `api::config` adds only API-specific checks on top. Error messages name variables, never values.

mod admin;

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use percent_encoding::percent_decode_str;
use secrecy::SecretString;
use sha2::{Digest, Sha256};
use sqlx::postgres::{PgConnectOptions, PgSslMode};
use url::{Host, Url};

pub use admin::{AdminCommand, AdminEnv, CredentialMode, OwnerRef, UserRef};

use crate::crypto::{self, CredentialPrivateKey, CredentialPublicKey};
use crate::redact;

/// Local-only secret fingerprints: lines `<sha256-hex>  <VARIABLE_NAME>` (05 owns the content).
pub const DEMO_FINGERPRINTS: &str = include_str!("../../../demo-fingerprints.txt");

/// The placeholder `.env.example` ships for `AWN_APPLICATION_KEY` (05 section 3).
pub const AWN_KEY_PLACEHOLDER: &str = "REPLACE_WITH_DEV_APPLICATION_KEY";
const MIN_AWN_APPLICATION_KEY_LEN: usize = 16;

const LOCAL_DB_HOSTS: &[&str] = &["db", "localhost", "127.0.0.1", "::1"];
const LOCAL_SUPABASE_HOSTS: &[&str] = &["localhost", "127.0.0.1", "caddy", "auth"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppEnv {
    Local,
    Production,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AwnMode {
    Live,
    Replay,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Binary {
    Api,
    Poller,
    Admin(AdminCommand),
}

#[derive(Debug)]
pub enum Config {
    Api(ApiEnv),
    Poller(PollerConfig),
    Admin(AdminEnv),
}

/// Names the variable, never its value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("missing required environment variable {0}")]
    MissingEnv(String),
    #[error("invalid {var}: {reason}")]
    Invalid { var: String, reason: &'static str },
    #[error("refusing {var}: {reason}")]
    Refused { var: String, reason: &'static str },
    /// Display is exactly `refusing <VAR>: demo value`.
    #[error("refusing {0}: demo value")]
    DemoValue(String),
}

fn missing(var: &str) -> ConfigError {
    ConfigError::MissingEnv(var.to_owned())
}
fn invalid(var: &str, reason: &'static str) -> ConfigError {
    ConfigError::Invalid {
        var: var.to_owned(),
        reason,
    }
}
fn refused(var: &str, reason: &'static str) -> ConfigError {
    ConfigError::Refused {
        var: var.to_owned(),
        reason,
    }
}

/// Environment snapshot. Empty values count as unset (Compose passes blank variables).
#[derive(Clone, Default)]
pub struct Vars(HashMap<String, String>);

/// Prints variable names only: the values include secrets.
impl std::fmt::Debug for Vars {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut names: Vec<&String> = self.0.keys().collect();
        names.sort();
        f.debug_struct("Vars").field("names", &names).finish()
    }
}

impl Vars {
    /// Secret values must not carry leading or trailing whitespace: parsers trim, hashes do not, and a mismatch
    /// would let a demo value slip past the fingerprint check.
    fn reject_padded<'a>(
        &self,
        names: impl IntoIterator<Item = &'a str>,
    ) -> Result<(), ConfigError> {
        for n in names {
            if let Some(v) = self.0.get(n) {
                if !v.trim().is_empty() && v.trim() != v {
                    return Err(refused(n, "value has leading or trailing whitespace"));
                }
            }
        }
        Ok(())
    }

    pub fn from_process() -> Self {
        Self(std::env::vars().collect())
    }

    pub fn from_pairs<I, K, V>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self(
            pairs
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        )
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.0
            .get(key)
            .map(String::as_str)
            .filter(|v| !v.trim().is_empty())
    }

    fn require(&self, key: &str) -> Result<&str, ConfigError> {
        self.get(key).ok_or_else(|| missing(key))
    }

    fn secret(&self, key: &str) -> Option<SecretString> {
        self.get(key).map(|v| SecretString::from(v.to_owned()))
    }

    fn with_prefix<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = (&'a str, &'a str)> + 'a {
        self.0
            .iter()
            .filter(move |(k, v)| k.starts_with(prefix) && !v.trim().is_empty())
            .map(|(k, v)| (k.as_str(), v.as_str()))
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Typed environments

/// `ApiEnv` as defined in 02 section 4.1.
pub struct ApiEnv {
    pub app_env: AppEnv,
    pub database: PgConnectOptions,
    pub supabase_url: Url,
    pub supabase_jwks_url: Option<Url>,
    pub jwt_hs256_secret: Option<SecretString>,
    pub awn_mode: AwnMode,
    pub awn_application_key: Option<SecretString>,
    pub fixtures_dir: PathBuf,
    pub credentials_public_key: CredentialPublicKey,
    pub credentials_key_version: i16,
    pub bind_addr: SocketAddr,
    pub netlify_proxy_secret: Option<SecretString>,
}

pub struct PollerConfig {
    pub app_env: AppEnv,
    pub enabled: bool,
    pub awn_mode: AwnMode,
    pub fixtures_dir: PathBuf,
    pub awn_application_key: Option<SecretString>,
    pub current_key: Option<(i16, CredentialPrivateKey)>,
    pub previous_key: Option<(i16, CredentialPrivateKey)>,
    /// Keyed by env suffix: `"AWN_INGEST"`, `"DAILY_ROLLUP"`, `"REFERENCE_SYNC"`. Empty locally.
    pub healthcheck_urls: HashMap<String, SecretString>,
    pub database: PgConnectOptions,
}

fn red<T>(o: &Option<T>) -> &'static str {
    if o.is_some() {
        "[REDACTED]"
    } else {
        "None"
    }
}

fn db_summary(o: &PgConnectOptions) -> String {
    format!(
        "{}@{}:{}/{}",
        o.get_username(),
        o.get_host(),
        o.get_port(),
        o.get_database().unwrap_or("")
    )
}

impl std::fmt::Debug for ApiEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiEnv")
            .field("app_env", &self.app_env)
            .field("database", &db_summary(&self.database))
            .field("supabase_url", &self.supabase_url.host_str())
            .field(
                "supabase_jwks_url",
                &self.supabase_jwks_url.as_ref().and_then(|u| u.host_str()),
            )
            .field("jwt_hs256_secret", &red(&self.jwt_hs256_secret))
            .field("awn_mode", &self.awn_mode)
            .field("awn_application_key", &red(&self.awn_application_key))
            .field("fixtures_dir", &self.fixtures_dir)
            .field("credentials_public_key", &self.credentials_public_key)
            .field("credentials_key_version", &self.credentials_key_version)
            .field("bind_addr", &self.bind_addr)
            .field("netlify_proxy_secret", &red(&self.netlify_proxy_secret))
            .finish()
    }
}

impl std::fmt::Debug for PollerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut names: Vec<&String> = self.healthcheck_urls.keys().collect();
        names.sort();
        f.debug_struct("PollerConfig")
            .field("app_env", &self.app_env)
            .field("enabled", &self.enabled)
            .field("awn_mode", &self.awn_mode)
            .field("fixtures_dir", &self.fixtures_dir)
            .field("awn_application_key", &red(&self.awn_application_key))
            .field(
                "current_key",
                &self.current_key.as_ref().map(|(v, _)| (*v, "[REDACTED]")),
            )
            .field(
                "previous_key",
                &self.previous_key.as_ref().map(|(v, _)| (*v, "[REDACTED]")),
            )
            .field("healthcheck_urls", &names)
            .field("database", &db_summary(&self.database))
            .finish()
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Entry points

/// Loads and validates the configuration for `bin` from the process environment.
pub fn load(bin: Binary) -> Result<Config, ConfigError> {
    load_from(bin, &Vars::from_process())
}

/// As [`load`], from an explicit environment snapshot.
pub fn load_from(bin: Binary, vars: &Vars) -> Result<Config, ConfigError> {
    load_with(bin, vars, DEMO_FINGERPRINTS)
}

/// As [`load_from`] with explicit fingerprints (tests).
pub(crate) fn load_with(
    bin: Binary,
    vars: &Vars,
    fingerprints: &str,
) -> Result<Config, ConfigError> {
    let (config, secrets) = match bin {
        Binary::Api => {
            let (c, s) = load_api(vars)?;
            (Config::Api(c), s)
        }
        Binary::Poller => {
            let (c, s) = load_poller(vars)?;
            (Config::Poller(c), s)
        }
        Binary::Admin(cmd) => {
            let (c, s) = load_admin(&cmd, vars)?;
            (Config::Admin(c), s)
        }
    };
    let production = match &config {
        Config::Api(c) => c.app_env,
        Config::Poller(c) => c.app_env,
        Config::Admin(c) => c.app_env.unwrap_or(AppEnv::Local),
    } == AppEnv::Production;
    if production {
        check_demo(fingerprints, &secrets)?;
    }
    for (_, v) in &secrets {
        redact::register(&SecretString::from(v.clone()));
    }
    Ok(config)
}

/// A loaded secret value together with the variable it came from.
type Secrets = Vec<(String, String)>;

// ---------------------------------------------------------------------------------------------------------------
// Shared rules

fn parse_app_env(vars: &Vars, required: bool) -> Result<Option<AppEnv>, ConfigError> {
    match vars.get("APP_ENV") {
        None if required => Err(missing("APP_ENV")),
        None => Ok(None),
        Some("local") => Ok(Some(AppEnv::Local)),
        Some("production") => Ok(Some(AppEnv::Production)),
        Some(_) => Err(invalid("APP_ENV", "must be local or production")),
    }
}

fn parse_awn_mode(vars: &Vars) -> Result<Option<AwnMode>, ConfigError> {
    match vars.get("AWN_MODE") {
        None => Ok(None),
        Some("live") => Ok(Some(AwnMode::Live)),
        Some("replay") => Ok(Some(AwnMode::Replay)),
        Some(_) => Err(invalid("AWN_MODE", "must be live or replay")),
    }
}

fn parse_key_version(vars: &Vars, var: &str) -> Result<Option<i16>, ConfigError> {
    match vars.get(var) {
        None => Ok(None),
        Some(v) => match v.trim().parse::<i16>() {
            Ok(n) if n >= 1 => Ok(Some(n)),
            _ => Err(invalid(var, "must be an integer >= 1")),
        },
    }
}

fn parse_url(vars: &Vars, var: &str) -> Result<Option<Url>, ConfigError> {
    match vars.get(var) {
        None => Ok(None),
        Some(v) => Url::parse(v.trim())
            .map(Some)
            .map_err(|_| invalid(var, "not a valid URL")),
    }
}

fn host_of(u: &Url) -> Option<String> {
    match u.host()? {
        Host::Domain(d) => Some(d.to_ascii_lowercase()),
        Host::Ipv4(a) => Some(a.to_string()),
        Host::Ipv6(a) => Some(a.to_string()),
    }
}

fn is_local_host(host: &str) -> bool {
    host == "localhost"
        || host == "::1"
        || host.starts_with("127.")
        || host.ends_with(".localhost")
        || host.ends_with(".test")
}

/// Production refuses localhost-style issuer and JWKS hosts; local restricts `SUPABASE_URL` to the allow-list.
fn check_supabase_url(var: &str, url: &Url, env: AppEnv) -> Result<(), ConfigError> {
    let host = host_of(url).ok_or_else(|| invalid(var, "URL has no host"))?;
    match env {
        AppEnv::Production if is_local_host(&host) => {
            Err(refused(var, "localhost URL in production"))
        }
        AppEnv::Local
            if var == "SUPABASE_URL" && !LOCAL_SUPABASE_HOSTS.contains(&host.as_str()) =>
        {
            Err(refused(var, "host is not local"))
        }
        _ => Ok(()),
    }
}

struct Database {
    options: PgConnectOptions,
    /// Decoded password (and its raw form when it differs), for the demo check.
    passwords: Vec<String>,
}

/// The password sqlx will use (query-string passwords are refused, so it is the URL's own), in decoded form and,
/// when different, raw form, for the demo check.
fn url_passwords(url: &Url) -> Vec<String> {
    let mut v = Vec::new();
    if let Some(p) = url.password() {
        let decoded = percent_decode_str(p).decode_utf8_lossy().into_owned();
        if decoded != p {
            v.push(p.to_owned());
        }
        v.push(decoded);
    }
    v
}

/// Role the process must connect as, and whether pooler hosts are refused (poller and its one-offs).
struct DbPolicy {
    role: &'static str,
    refuse_pooler: bool,
}

fn check_database(vars: &Vars, env: AppEnv, policy: DbPolicy) -> Result<Database, ConfigError> {
    const VAR: &str = "DATABASE_URL";
    let raw = vars.require(VAR)?;
    let url = Url::parse(raw).map_err(|_| invalid(VAR, "not a valid URL"))?;
    if url.scheme() != "postgres" && url.scheme() != "postgresql" {
        return Err(invalid(VAR, "scheme must be postgres"));
    }
    // sqlx lets query parameters (`host`, `hostaddr`, `port`, `user`, `password`, `ssl-mode`, ...) override the
    // URL's own parts, so only the two parameters this check understands are accepted, each at most once.
    let mut seen = HashSet::new();
    for (k, _) in url.query_pairs() {
        if k != "sslmode" && k != "sslrootcert" {
            return Err(refused(
                VAR,
                "unsupported query parameter (only sslmode and sslrootcert are allowed)",
            ));
        }
        if !seen.insert(k.into_owned()) {
            return Err(refused(VAR, "duplicate query parameter"));
        }
    }
    // Everything below validates what sqlx will actually use (the parsed options), not the raw URL.
    // That also covers defaults sqlx takes from PG* environment variables.
    let options = PgConnectOptions::from_str(raw)
        .map_err(|_| invalid(VAR, "not a valid connection string"))?;
    if options.get_socket().is_some() {
        return Err(refused(VAR, "unix sockets are not allowed"));
    }
    let host = options
        .get_host()
        .trim_matches(['[', ']'])
        .to_ascii_lowercase();
    match env {
        AppEnv::Local => {
            if !LOCAL_DB_HOSTS.contains(&host.as_str()) {
                return Err(refused(
                    VAR,
                    "host is not local (db, localhost, 127.0.0.1, ::1)",
                ));
            }
        }
        AppEnv::Production => {
            if !matches!(options.get_ssl_mode(), PgSslMode::VerifyFull) {
                return Err(refused(VAR, "sslmode must be verify-full in production"));
            }
            match url.query_pairs().find(|(k, _)| k == "sslrootcert") {
                None => return Err(refused(VAR, "sslrootcert is required in production")),
                Some((_, p)) if !Path::new(p.as_ref()).is_file() => {
                    return Err(refused(VAR, "sslrootcert file is missing"))
                }
                Some(_) => {}
            }
            if options.get_port() == 6543 {
                return Err(refused(
                    VAR,
                    "port 6543 (transaction pooler) is not allowed",
                ));
            }
            if policy.refuse_pooler && host.contains("pooler") {
                return Err(refused(
                    VAR,
                    "pooler host: this process needs the direct connection",
                ));
            }
            let user = options.get_username();
            let ok = user == policy.role
                || user
                    .strip_prefix(policy.role)
                    .and_then(|r| r.strip_prefix('.'))
                    .is_some_and(|r| !r.is_empty());
            if !ok {
                return Err(refused(
                    VAR,
                    "database user is not the role for this process",
                ));
            }
            if url.password().is_none() {
                return Err(refused(
                    VAR,
                    "the URL must carry the password (no PGPASSWORD or passfile)",
                ));
            }
        }
    }
    Ok(Database {
        options,
        passwords: url_passwords(&url),
    })
}

/// Production: secrets that belong to another process must not be present at all.
fn check_foreign(
    vars: &Vars,
    exact: &[&str],
    prefixes: &[&str],
    healthcheck_ok: &[&str],
) -> Result<(), ConfigError> {
    for name in exact {
        if vars.get(name).is_some() {
            return Err(refused(name, "belongs to another process"));
        }
    }
    for prefix in prefixes {
        if let Some((name, _)) = vars.with_prefix(prefix).min_by_key(|(k, _)| *k) {
            return Err(refused(name, "belongs to another process"));
        }
    }
    for (name, _) in vars.with_prefix("HEALTHCHECK_URL_") {
        let suffix = &name["HEALTHCHECK_URL_".len()..];
        if !healthcheck_ok.contains(&suffix) {
            return Err(refused(name, "belongs to another process"));
        }
    }
    Ok(())
}

/// Role passwords, backup and AWS settings, and local-only values belong to no backend process in production.
const NEVER_ON_BACKEND: &[&str] = &[
    "JWT_HS256_SECRET",
    "LOCAL_DEMO_PASSWORD",
    "POSTGRES_PASSWORD",
    "SUPABASE_AUTH_ADMIN_PASSWORD",
    "APP_MIGRATOR_PASSWORD",
    "APP_API_PASSWORD",
    "APP_POLLER_PASSWORD",
    "APP_BACKUP_PASSWORD",
    "APP_CI_CHECK_PASSWORD",
    "APP_MIGRATOR_PASSWORD_VERIFIER",
    "APP_API_PASSWORD_VERIFIER",
    "APP_POLLER_PASSWORD_VERIFIER",
    "APP_BACKUP_PASSWORD_VERIFIER",
    "APP_CI_CHECK_PASSWORD_VERIFIER",
];
const NEVER_PREFIXES: &[&str] = &["BACKUP_", "AWS_"];

/// `NEVER_ON_BACKEND` plus the per-binary extras.
fn exact(extra: &[&'static str]) -> Vec<&'static str> {
    NEVER_ON_BACKEND.iter().chain(extra).copied().collect()
}

/// Hashes every loaded secret and compares it with column 1 of EVERY fingerprint line (not only the line with the
/// matching name). The first match fails with `refusing <VAR>: demo value`.
fn check_demo(fingerprints: &str, secrets: &[(String, String)]) -> Result<(), ConfigError> {
    let hashes: HashSet<&str> = fingerprints
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        .collect();
    for (var, value) in secrets {
        let digest = hex::encode(Sha256::digest(value.as_bytes()));
        if hashes.contains(digest.as_str()) {
            return Err(ConfigError::DemoValue(var.clone()));
        }
    }
    Ok(())
}

/// Live mode needs a real application key: not empty, not the `.env.example` placeholder, not implausibly short.
fn check_live_key(key: Option<&str>) -> Result<(), ConfigError> {
    const VAR: &str = "AWN_APPLICATION_KEY";
    match key {
        None => Err(missing(VAR)),
        Some(k) if k == AWN_KEY_PLACEHOLDER => {
            Err(refused(VAR, "placeholder application key in live mode"))
        }
        Some(k) if k.len() < MIN_AWN_APPLICATION_KEY_LEN => {
            Err(refused(VAR, "application key too short for live mode"))
        }
        Some(_) => Ok(()),
    }
}

fn parse_public_key(vars: &Vars) -> Result<Option<CredentialPublicKey>, ConfigError> {
    match vars.get("CREDENTIALS_PUBLIC_KEY") {
        None => Ok(None),
        Some(v) => crypto::parse_public_key(v)
            .map(Some)
            .map_err(|_| invalid("CREDENTIALS_PUBLIC_KEY", "not a base64 32-byte key")),
    }
}

/// `FIXTURES_DIR` is required with `AWN_MODE=replay`, optional otherwise (empty path when unset, there is no
/// default), and refused in production (06) by the foreign-variable check.
fn fixtures_dir(vars: &Vars, mode: AwnMode) -> Result<PathBuf, ConfigError> {
    match vars.get("FIXTURES_DIR") {
        Some(v) => Ok(PathBuf::from(v)),
        None if mode == AwnMode::Replay => Err(missing("FIXTURES_DIR")),
        None => Ok(PathBuf::new()),
    }
}

fn push_secret(secrets: &mut Secrets, vars: &Vars, var: &str) {
    if let Some(v) = vars.get(var) {
        secrets.push((var.to_owned(), v.to_owned()));
    }
}

fn push_db(secrets: &mut Secrets, db: &Database) {
    for p in &db.passwords {
        secrets.push(("DATABASE_URL".to_owned(), p.clone()));
    }
}

// ---------------------------------------------------------------------------------------------------------------
// api

fn load_api(vars: &Vars) -> Result<(ApiEnv, Secrets), ConfigError> {
    let env = parse_app_env(vars, true)?.expect("required");
    vars.reject_padded([
        "DATABASE_URL",
        "JWT_HS256_SECRET",
        "AWN_APPLICATION_KEY",
        "CREDENTIALS_PUBLIC_KEY",
        "NETLIFY_PROXY_SECRET",
    ])?;
    let db = check_database(
        vars,
        env,
        DbPolicy {
            role: "app_api",
            refuse_pooler: false,
        },
    )?;
    let awn_mode = parse_awn_mode(vars)?.unwrap_or(AwnMode::Live);
    if env == AppEnv::Production {
        if awn_mode == AwnMode::Replay {
            return Err(refused("AWN_MODE", "replay is not allowed in production"));
        }
        check_foreign(
            vars,
            &exact(&[
                "CREDENTIALS_PRIVATE_KEY",
                "CREDENTIALS_PRIVATE_KEY_PREVIOUS",
                "CREDENTIALS_KEY_VERSION_PREVIOUS",
                "SUPABASE_SERVICE_ROLE_KEY",
                "FIXTURES_DIR",
            ]),
            NEVER_PREFIXES,
            &[],
        )?;
    }
    let supabase_url = parse_url(vars, "SUPABASE_URL")?.ok_or_else(|| missing("SUPABASE_URL"))?;
    check_supabase_url("SUPABASE_URL", &supabase_url, env)?;
    let supabase_jwks_url = parse_url(vars, "SUPABASE_JWKS_URL")?;
    match (&supabase_jwks_url, env) {
        (Some(u), _) => check_supabase_url("SUPABASE_JWKS_URL", u, env)?,
        (None, AppEnv::Production) => return Err(missing("SUPABASE_JWKS_URL")),
        (None, AppEnv::Local) => {}
    }
    if awn_mode == AwnMode::Live {
        check_live_key(vars.get("AWN_APPLICATION_KEY"))?;
    }
    let credentials_public_key =
        parse_public_key(vars)?.ok_or_else(|| missing("CREDENTIALS_PUBLIC_KEY"))?;
    let credentials_key_version = parse_key_version(vars, "CREDENTIALS_KEY_VERSION")?
        .ok_or_else(|| missing("CREDENTIALS_KEY_VERSION"))?;
    let bind_addr = match vars.get("API_BIND_ADDR") {
        None => "0.0.0.0:8080".parse().expect("static address"),
        Some(v) => v
            .trim()
            .parse()
            .map_err(|_| invalid("API_BIND_ADDR", "not a socket address"))?,
    };

    let mut secrets = Vec::new();
    for v in [
        "JWT_HS256_SECRET",
        "AWN_APPLICATION_KEY",
        "CREDENTIALS_PUBLIC_KEY",
        "NETLIFY_PROXY_SECRET",
    ] {
        push_secret(&mut secrets, vars, v);
    }
    push_db(&mut secrets, &db);
    Ok((
        ApiEnv {
            app_env: env,
            database: db.options,
            supabase_url,
            supabase_jwks_url,
            jwt_hs256_secret: vars.secret("JWT_HS256_SECRET"),
            awn_mode,
            awn_application_key: vars.secret("AWN_APPLICATION_KEY"),
            fixtures_dir: fixtures_dir(vars, awn_mode)?,
            credentials_public_key,
            credentials_key_version,
            bind_addr,
            netlify_proxy_secret: vars.secret("NETLIFY_PROXY_SECRET"),
        },
        secrets,
    ))
}

// ---------------------------------------------------------------------------------------------------------------
// poller

fn parse_enabled(vars: &Vars) -> Result<bool, ConfigError> {
    match vars
        .get("POLLER_ENABLED")
        .map(|v| v.trim().to_ascii_lowercase())
    {
        None => Ok(false),
        Some(v) if v == "true" || v == "1" => Ok(true),
        Some(v) if v == "false" || v == "0" => Ok(false),
        Some(_) => Err(invalid("POLLER_ENABLED", "must be true or false")),
    }
}

fn parse_private(
    vars: &Vars,
    key_var: &str,
    version_var: &str,
) -> Result<Option<(i16, CredentialPrivateKey)>, ConfigError> {
    let key = vars.get(key_var);
    let version = parse_key_version(vars, version_var)?;
    match (key, version) {
        (None, None) => Ok(None),
        (Some(_), None) => Err(missing(version_var)),
        (None, Some(_)) => Err(missing(key_var)),
        (Some(k), Some(v)) => {
            let sk = crypto::parse_private_key(k)
                .map_err(|_| invalid(key_var, "not a base64 32-byte key"))?;
            Ok(Some((v, sk)))
        }
    }
}

fn load_poller(vars: &Vars) -> Result<(PollerConfig, Secrets), ConfigError> {
    let env = parse_app_env(vars, true)?.expect("required");
    vars.reject_padded([
        "DATABASE_URL",
        "AWN_APPLICATION_KEY",
        "CREDENTIALS_PRIVATE_KEY",
        "CREDENTIALS_PRIVATE_KEY_PREVIOUS",
    ])?;
    vars.reject_padded(vars.with_prefix("HEALTHCHECK_URL_").map(|(k, _)| k))?;
    let db = check_database(
        vars,
        env,
        DbPolicy {
            role: "app_poller",
            refuse_pooler: true,
        },
    )?;
    let awn_mode = parse_awn_mode(vars)?.unwrap_or(AwnMode::Live);
    let enabled = parse_enabled(vars)?;
    if env == AppEnv::Production {
        if awn_mode == AwnMode::Replay {
            return Err(refused("AWN_MODE", "replay is not allowed in production"));
        }
        if !enabled {
            return Err(refused("POLLER_ENABLED", "must be true in production"));
        }
        check_foreign(
            vars,
            &exact(&[
                "CREDENTIALS_PUBLIC_KEY",
                "SUPABASE_SERVICE_ROLE_KEY",
                "NETLIFY_PROXY_SECRET",
                "FIXTURES_DIR",
            ]),
            NEVER_PREFIXES,
            &["AWN_INGEST", "DAILY_ROLLUP", "REFERENCE_SYNC"],
        )?;
    }
    if awn_mode == AwnMode::Live {
        check_live_key(vars.get("AWN_APPLICATION_KEY"))?;
    }
    let current_key = parse_private(vars, "CREDENTIALS_PRIVATE_KEY", "CREDENTIALS_KEY_VERSION")?;
    if env == AppEnv::Production && current_key.is_none() {
        return Err(missing("CREDENTIALS_PRIVATE_KEY"));
    }
    let previous_key = parse_private(
        vars,
        "CREDENTIALS_PRIVATE_KEY_PREVIOUS",
        "CREDENTIALS_KEY_VERSION_PREVIOUS",
    )?;
    if previous_key.is_some() && current_key.is_none() {
        return Err(missing("CREDENTIALS_PRIVATE_KEY"));
    }

    let mut secrets = Vec::new();
    for v in [
        "AWN_APPLICATION_KEY",
        "CREDENTIALS_PRIVATE_KEY",
        "CREDENTIALS_PRIVATE_KEY_PREVIOUS",
    ] {
        push_secret(&mut secrets, vars, v);
    }
    // Healthcheck URLs are ignored locally.
    let mut healthcheck_urls = HashMap::new();
    if env == AppEnv::Production {
        for (name, value) in vars.with_prefix("HEALTHCHECK_URL_") {
            healthcheck_urls.insert(
                name["HEALTHCHECK_URL_".len()..].to_owned(),
                SecretString::from(value.to_owned()),
            );
            secrets.push((name.to_owned(), value.to_owned()));
        }
    }
    push_db(&mut secrets, &db);
    Ok((
        PollerConfig {
            app_env: env,
            enabled,
            awn_mode,
            fixtures_dir: fixtures_dir(vars, awn_mode)?,
            awn_application_key: vars.secret("AWN_APPLICATION_KEY"),
            current_key,
            previous_key,
            healthcheck_urls,
            database: db.options,
        },
        secrets,
    ))
}

// ---------------------------------------------------------------------------------------------------------------
// admin

fn load_admin(cmd: &AdminCommand, vars: &Vars) -> Result<(AdminEnv, Secrets), ConfigError> {
    let needs = admin::needs(cmd);
    let uses = |v: &str| needs.required.contains(&v) || needs.optional.contains(&v);
    let mut out = AdminEnv::empty();
    let mut secrets = Vec::new();
    if matches!(cmd, AdminCommand::Keygen { .. }) {
        return Ok((out, secrets)); // loads nothing
    }
    for v in needs.required {
        vars.require(v)?;
    }
    out.app_env = parse_app_env(vars, false)?;
    let env = out.app_env.unwrap_or(AppEnv::Local);
    if matches!(cmd, AdminCommand::SeedDev { .. }) && env != AppEnv::Local {
        return Err(refused("APP_ENV", "seed-dev runs only with APP_ENV=local"));
    }
    let production = env == AppEnv::Production;
    vars.reject_padded(
        [
            "DATABASE_URL",
            "SUPABASE_SERVICE_ROLE_KEY",
            "LOCAL_DEMO_PASSWORD",
            "CREDENTIALS_PUBLIC_KEY",
            "AWN_APPLICATION_KEY",
        ]
        .into_iter()
        .filter(|n| production || uses(n)),
    )?;
    if production {
        // AWN_APPLICATION_KEY and FIXTURES_DIR are local-only for admin (04 section 10.1, 06).
        check_foreign(
            vars,
            &exact(&[
                "CREDENTIALS_PRIVATE_KEY",
                "CREDENTIALS_PRIVATE_KEY_PREVIOUS",
                "CREDENTIALS_KEY_VERSION_PREVIOUS",
                "NETLIFY_PROXY_SECRET",
                "AWN_APPLICATION_KEY",
                "FIXTURES_DIR",
            ]),
            NEVER_PREFIXES,
            &[],
        )?;
    }
    if uses("DATABASE_URL") {
        if let Some(raw) = vars.get("DATABASE_URL") {
            let db = check_database(
                vars,
                env,
                DbPolicy {
                    role: "app_migrator",
                    refuse_pooler: false,
                },
            )?;
            push_db(&mut secrets, &db);
            out.database_url = Some(SecretString::from(raw.to_owned()));
        }
    }
    if uses("SUPABASE_URL") {
        if let Some(u) = parse_url(vars, "SUPABASE_URL")? {
            check_supabase_url("SUPABASE_URL", &u, env)?;
            out.supabase_url = Some(u);
        }
    }
    if uses("SUPABASE_SERVICE_ROLE_KEY") {
        push_secret(&mut secrets, vars, "SUPABASE_SERVICE_ROLE_KEY");
        out.supabase_service_role_key = vars.secret("SUPABASE_SERVICE_ROLE_KEY");
    }
    if uses("LOCAL_DEMO_PASSWORD") {
        push_secret(&mut secrets, vars, "LOCAL_DEMO_PASSWORD");
        out.local_demo_password = vars.secret("LOCAL_DEMO_PASSWORD");
    }
    if uses("SITE_URL") {
        if let Some(u) = parse_url(vars, "SITE_URL")? {
            let origin_only = (u.path() == "/" || u.path().is_empty())
                && u.query().is_none()
                && u.fragment().is_none();
            if !origin_only || u.cannot_be_a_base() {
                return Err(invalid(
                    "SITE_URL",
                    "must be an origin only (scheme and host)",
                ));
            }
            if env == AppEnv::Production
                && (u.scheme() != "https" || host_of(&u).is_none_or(|h| is_local_host(&h)))
            {
                return Err(refused(
                    "SITE_URL",
                    "production needs an https, non-local origin",
                ));
            }
            out.site_url = Some(u);
        }
    }
    if uses("CREDENTIALS_PUBLIC_KEY") {
        push_secret(&mut secrets, vars, "CREDENTIALS_PUBLIC_KEY");
        out.credentials_public_key = parse_public_key(vars)?;
    }
    if uses("CREDENTIALS_KEY_VERSION") {
        out.credentials_key_version = parse_key_version(vars, "CREDENTIALS_KEY_VERSION")?;
    }
    if uses("AWN_MODE") {
        out.awn_mode = parse_awn_mode(vars)?;
        if env == AppEnv::Production && out.awn_mode == Some(AwnMode::Replay) {
            return Err(refused("AWN_MODE", "replay is not allowed in production"));
        }
    }
    if uses("AWN_APPLICATION_KEY") {
        push_secret(&mut secrets, vars, "AWN_APPLICATION_KEY");
        out.awn_application_key = vars.secret("AWN_APPLICATION_KEY");
        // Optional for admin (no validation without it), but a placeholder in live mode is refused.
        if out.awn_mode == Some(AwnMode::Live) && vars.get("AWN_APPLICATION_KEY").is_some() {
            check_live_key(vars.get("AWN_APPLICATION_KEY"))?;
        }
    }
    if uses("FIXTURES_DIR") {
        out.fixtures_dir = vars.get("FIXTURES_DIR").map(PathBuf::from);
        if out.awn_mode == Some(AwnMode::Replay) && out.fixtures_dir.is_none() {
            return Err(missing("FIXTURES_DIR"));
        }
    }
    if uses("SEED_DIR") {
        out.seed_dir = PathBuf::from(vars.get("SEED_DIR").unwrap_or("/seed"));
    }
    if production {
        // Every subcommand refuses demo values in these three, whether or not it uses them (04 section 10.1).
        for var in ["CREDENTIALS_PUBLIC_KEY", "SUPABASE_SERVICE_ROLE_KEY"] {
            if !secrets.iter().any(|(n, _)| n == var) {
                push_secret(&mut secrets, vars, var);
            }
        }
        if !secrets.iter().any(|(n, _)| n == "DATABASE_URL") {
            if let Some(url) = vars
                .get("DATABASE_URL")
                .and_then(|raw| Url::parse(raw).ok())
            {
                for p in url_passwords(&url) {
                    secrets.push(("DATABASE_URL".to_owned(), p));
                }
            }
        }
    }
    Ok((out, secrets))
}

#[cfg(test)]
mod tests;
