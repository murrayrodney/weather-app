//! Guard tests: every refusal, with synthetic values only. No database is needed.

use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};

fn b64(byte: u8) -> String {
    STANDARD.encode([byte; 32])
}

fn sha(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

const LIVE_KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// A throwaway CA file so `sslrootcert` points at something that exists.
struct Ca {
    _dir: tempfile::TempDir,
    path: String,
}
impl Ca {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ca.crt");
        std::fs::write(&p, "not a real certificate").unwrap();
        let path = p.to_str().unwrap().to_owned();
        Ca { _dir: dir, path }
    }
    fn path(&self) -> &str {
        &self.path
    }
}

fn prod_url(ca: &Ca, user: &str, host: &str, port: u16, password: &str) -> String {
    format!(
        "postgres://{user}:{password}@{host}:{port}/postgres?sslmode=verify-full&sslrootcert={}",
        ca.path()
    )
}

fn poller_prod(ca: &Ca) -> Vars {
    Vars::from_pairs([
        ("APP_ENV", "production".to_owned()),
        (
            "DATABASE_URL",
            prod_url(
                ca,
                "app_poller.abcdef",
                "db.example.invalid",
                5432,
                "synthetic-pw-1",
            ),
        ),
        ("POLLER_ENABLED", "true".to_owned()),
        ("AWN_MODE", "live".to_owned()),
        ("AWN_APPLICATION_KEY", LIVE_KEY.to_owned()),
        ("CREDENTIALS_PRIVATE_KEY", b64(1)),
        ("CREDENTIALS_KEY_VERSION", "2".to_owned()),
        (
            "HEALTHCHECK_URL_AWN_INGEST",
            "https://hc.example.invalid/ping/synthetic-uuid".to_owned(),
        ),
    ])
}

fn api_prod(ca: &Ca) -> Vars {
    Vars::from_pairs([
        ("APP_ENV", "production".to_owned()),
        (
            "DATABASE_URL",
            prod_url(ca, "app_api", "db.example.invalid", 5432, "synthetic-pw-2"),
        ),
        ("SUPABASE_URL", "https://proj.example.invalid".to_owned()),
        (
            "SUPABASE_JWKS_URL",
            "https://proj.example.invalid/auth/v1/.well-known/jwks.json".to_owned(),
        ),
        ("AWN_APPLICATION_KEY", LIVE_KEY.to_owned()),
        ("CREDENTIALS_PUBLIC_KEY", b64(2)),
        ("CREDENTIALS_KEY_VERSION", "2".to_owned()),
    ])
}

fn with(mut v: Vars, k: &str, val: &str) -> Vars {
    v.0.insert(k.to_owned(), val.to_owned());
    v
}
fn without(mut v: Vars, k: &str) -> Vars {
    v.0.remove(k);
    v
}

fn load_poller_err(v: &Vars) -> ConfigError {
    load_with(Binary::Poller, v, "").unwrap_err()
}
fn load_api_err(v: &Vars) -> ConfigError {
    load_with(Binary::Api, v, "").unwrap_err()
}

fn assert_refused(e: ConfigError, var: &str) {
    match e {
        ConfigError::Refused { var: v, .. } | ConfigError::Invalid { var: v, .. } => {
            assert_eq!(v, var)
        }
        ConfigError::MissingEnv(v) | ConfigError::DemoValue(v) => assert_eq!(v, var),
    }
}

// --- production happy paths --------------------------------------------------------------------------------------

#[test]
fn production_poller_loads() {
    let ca = Ca::new();
    let Config::Poller(c) = load_with(Binary::Poller, &poller_prod(&ca), "").unwrap() else {
        panic!()
    };
    assert!(c.enabled);
    assert_eq!(c.awn_mode, AwnMode::Live);
    assert_eq!(c.current_key.as_ref().map(|k| k.0), Some(2));
    assert!(c.previous_key.is_none());
    assert_eq!(
        c.healthcheck_urls.keys().collect::<Vec<_>>(),
        ["AWN_INGEST"]
    );
    assert_eq!(c.database.get_username(), "app_poller.abcdef");
}

#[test]
fn production_api_loads_and_accepts_a_pooler_host() {
    let ca = Ca::new();
    let Config::Api(c) = load_with(Binary::Api, &api_prod(&ca), "").unwrap() else {
        panic!()
    };
    assert_eq!(c.credentials_key_version, 2);
    assert_eq!(c.bind_addr.to_string(), "0.0.0.0:8080");
    // Pooler hosts are refused only for the poller; the API may use one (port other than 6543).
    let v = with(
        api_prod(&ca),
        "DATABASE_URL",
        &prod_url(
            &ca,
            "app_api.abcdef",
            "aws-0-x.pooler.example.invalid",
            5432,
            "synthetic-pw-2",
        ),
    );
    assert!(load_with(Binary::Api, &v, "").is_ok());
}

// --- production refusals -----------------------------------------------------------------------------------------

#[test]
fn app_env_must_be_set_and_valid() {
    let ca = Ca::new();
    assert_eq!(
        load_poller_err(&without(poller_prod(&ca), "APP_ENV")),
        ConfigError::MissingEnv("APP_ENV".into())
    );
    assert_refused(
        load_poller_err(&with(poller_prod(&ca), "APP_ENV", "staging")),
        "APP_ENV",
    );
}

#[test]
fn production_refuses_replay_mode() {
    let ca = Ca::new();
    assert_refused(
        load_poller_err(&with(poller_prod(&ca), "AWN_MODE", "replay")),
        "AWN_MODE",
    );
    assert_refused(
        load_api_err(&with(api_prod(&ca), "AWN_MODE", "replay")),
        "AWN_MODE",
    );
}

#[test]
fn production_refuses_a_non_verify_full_url_and_a_missing_ca_file() {
    let ca = Ca::new();
    for mode in ["require", "verify-ca", "prefer", "disable"] {
        let url = format!("postgres://app_poller:pw@db.example.invalid:5432/postgres?sslmode={mode}&sslrootcert={}", ca.path());
        assert_refused(
            load_poller_err(&with(poller_prod(&ca), "DATABASE_URL", &url)),
            "DATABASE_URL",
        );
    }
    let no_mode = format!(
        "postgres://app_poller:pw@db.example.invalid:5432/postgres?sslrootcert={}",
        ca.path()
    );
    assert_refused(
        load_poller_err(&with(poller_prod(&ca), "DATABASE_URL", &no_mode)),
        "DATABASE_URL",
    );

    let missing = "postgres://app_poller:pw@db.example.invalid:5432/postgres?sslmode=verify-full&sslrootcert=/nonexistent/ca.crt";
    assert_refused(
        load_poller_err(&with(poller_prod(&ca), "DATABASE_URL", missing)),
        "DATABASE_URL",
    );
    let no_cert = "postgres://app_poller:pw@db.example.invalid:5432/postgres?sslmode=verify-full";
    assert_refused(
        load_poller_err(&with(poller_prod(&ca), "DATABASE_URL", no_cert)),
        "DATABASE_URL",
    );
}

#[test]
fn production_refuses_port_6543_for_every_binary() {
    let ca = Ca::new();
    assert_refused(
        load_poller_err(&with(
            poller_prod(&ca),
            "DATABASE_URL",
            &prod_url(&ca, "app_poller", "db.example.invalid", 6543, "pw"),
        )),
        "DATABASE_URL",
    );
    assert_refused(
        load_api_err(&with(
            api_prod(&ca),
            "DATABASE_URL",
            &prod_url(&ca, "app_api", "db.example.invalid", 6543, "pw"),
        )),
        "DATABASE_URL",
    );
    let admin = Vars::from_pairs([
        ("APP_ENV", "production".to_owned()),
        (
            "DATABASE_URL",
            prod_url(&ca, "app_migrator", "db.example.invalid", 6543, "pw"),
        ),
    ]);
    assert_refused(
        load_with(Binary::Admin(AdminCommand::Migrate), &admin, "").unwrap_err(),
        "DATABASE_URL",
    );
}

#[test]
fn production_refuses_pooler_hosts_for_the_poller_only() {
    let ca = Ca::new();
    let url = prod_url(
        &ca,
        "app_poller.abcdef",
        "aws-0-us-east-1.pooler.example.invalid",
        5432,
        "pw",
    );
    assert_refused(
        load_poller_err(&with(poller_prod(&ca), "DATABASE_URL", &url)),
        "DATABASE_URL",
    );
}

#[test]
fn role_user_is_enforced_and_project_ref_suffix_is_accepted() {
    let ca = Ca::new();
    // `app_api` is the wrong role for the poller.
    assert_refused(
        load_poller_err(&with(
            poller_prod(&ca),
            "DATABASE_URL",
            &prod_url(&ca, "app_api", "db.example.invalid", 5432, "pw"),
        )),
        "DATABASE_URL",
    );
    // Similar-looking names and an empty ref are refused.
    for u in ["app_poller2", "app_pollerx.abc", "app_poller.", "postgres"] {
        assert_refused(
            load_poller_err(&with(
                poller_prod(&ca),
                "DATABASE_URL",
                &prod_url(&ca, u, "db.example.invalid", 5432, "pw"),
            )),
            "DATABASE_URL",
        );
    }
    // `<role>` and `<role>.<project-ref>` are accepted.
    for u in ["app_poller", "app_poller.abcdref"] {
        assert!(
            load_with(
                Binary::Poller,
                &with(
                    poller_prod(&ca),
                    "DATABASE_URL",
                    &prod_url(&ca, u, "db.example.invalid", 5432, "pw")
                ),
                ""
            )
            .is_ok(),
            "{u}"
        );
    }
    // The admin binary connects as app_migrator.
    let admin = Vars::from_pairs([
        ("APP_ENV", "production".to_owned()),
        (
            "DATABASE_URL",
            prod_url(&ca, "app_api", "db.example.invalid", 5432, "pw"),
        ),
    ]);
    assert_refused(
        load_with(Binary::Admin(AdminCommand::Migrate), &admin, "").unwrap_err(),
        "DATABASE_URL",
    );
}

#[test]
fn production_refuses_secrets_that_belong_to_another_process() {
    let ca = Ca::new();
    for (var, val) in [
        ("JWT_HS256_SECRET", "x"),
        ("SUPABASE_SERVICE_ROLE_KEY", "x"),
        ("CREDENTIALS_PUBLIC_KEY", "x"),
        ("BACKUP_S3_BUCKET", "x"),
        ("AWS_ACCESS_KEY_ID", "x"),
        ("APP_API_PASSWORD", "x"),
        ("HEALTHCHECK_URL_BACKUP", "https://hc.example.invalid/x"),
    ] {
        assert_refused(load_poller_err(&with(poller_prod(&ca), var, val)), var);
    }
    for (var, val) in [
        ("JWT_HS256_SECRET", "x"),
        ("CREDENTIALS_PRIVATE_KEY", "x"),
        ("CREDENTIALS_PRIVATE_KEY_PREVIOUS", "x"),
        ("SUPABASE_SERVICE_ROLE_KEY", "x"),
        ("HEALTHCHECK_URL_AWN_INGEST", "https://hc.example.invalid/x"),
        ("BACKUP_DATABASE_URL", "x"),
    ] {
        assert_refused(load_api_err(&with(api_prod(&ca), var, val)), var);
    }
}

#[test]
fn production_refuses_localhost_issuer_and_jwks() {
    let ca = Ca::new();
    for url in [
        "http://localhost:8000",
        "https://127.0.0.1",
        "https://auth.localhost",
        "https://x.test",
    ] {
        assert_refused(
            load_api_err(&with(api_prod(&ca), "SUPABASE_URL", url)),
            "SUPABASE_URL",
        );
        assert_refused(
            load_api_err(&with(api_prod(&ca), "SUPABASE_JWKS_URL", url)),
            "SUPABASE_JWKS_URL",
        );
    }
    assert_eq!(
        load_api_err(&without(api_prod(&ca), "SUPABASE_JWKS_URL")),
        ConfigError::MissingEnv("SUPABASE_JWKS_URL".into())
    );
}

#[test]
fn production_poller_requirements() {
    let ca = Ca::new();
    assert_refused(
        load_poller_err(&with(poller_prod(&ca), "POLLER_ENABLED", "false")),
        "POLLER_ENABLED",
    );
    assert_refused(
        load_poller_err(&without(poller_prod(&ca), "POLLER_ENABLED")),
        "POLLER_ENABLED",
    );
    assert_eq!(
        load_poller_err(&without(poller_prod(&ca), "CREDENTIALS_PRIVATE_KEY")),
        ConfigError::MissingEnv("CREDENTIALS_PRIVATE_KEY".into())
    );
    assert_eq!(
        load_poller_err(&without(
            without(poller_prod(&ca), "CREDENTIALS_PRIVATE_KEY"),
            "CREDENTIALS_KEY_VERSION"
        )),
        ConfigError::MissingEnv("CREDENTIALS_PRIVATE_KEY".into())
    );
    assert_eq!(
        load_poller_err(&without(poller_prod(&ca), "AWN_APPLICATION_KEY")),
        ConfigError::MissingEnv("AWN_APPLICATION_KEY".into())
    );
    assert_refused(
        load_poller_err(&with(
            poller_prod(&ca),
            "AWN_APPLICATION_KEY",
            AWN_KEY_PLACEHOLDER,
        )),
        "AWN_APPLICATION_KEY",
    );
    // A previous private key requires its version, and not the other way round.
    let prev = with(
        poller_prod(&ca),
        "CREDENTIALS_PRIVATE_KEY_PREVIOUS",
        &b64(3),
    );
    assert_eq!(
        load_poller_err(&prev),
        ConfigError::MissingEnv("CREDENTIALS_KEY_VERSION_PREVIOUS".into())
    );
    let prev = with(prev, "CREDENTIALS_KEY_VERSION_PREVIOUS", "1");
    let Config::Poller(c) = load_with(Binary::Poller, &prev, "").unwrap() else {
        panic!()
    };
    assert_eq!(c.previous_key.as_ref().map(|k| k.0), Some(1));
    assert_refused(
        load_poller_err(&with(
            poller_prod(&ca),
            "CREDENTIALS_PRIVATE_KEY",
            "not-a-key",
        )),
        "CREDENTIALS_PRIVATE_KEY",
    );
    assert_refused(
        load_poller_err(&with(poller_prod(&ca), "CREDENTIALS_KEY_VERSION", "0")),
        "CREDENTIALS_KEY_VERSION",
    );
}

// --- demo values ---------------------------------------------------------------------------------------------------

#[test]
fn demo_value_is_refused_with_the_exact_message() {
    let ca = Ca::new();
    let demo = b64(7);
    // The fingerprint line is named for a different variable: the match must still be found, and the message
    // must name the variable the value came from.
    let fp = format!("{}  SOME_OTHER_NAME\n", sha(&demo));
    let v = with(poller_prod(&ca), "CREDENTIALS_PRIVATE_KEY", &demo);
    let e = load_with(Binary::Poller, &v, &fp).unwrap_err();
    assert_eq!(
        e.to_string(),
        "refusing CREDENTIALS_PRIVATE_KEY: demo value"
    );
    assert!(!e.to_string().contains(&demo));

    // Same through the API on a different variable.
    let v = with(api_prod(&ca), "CREDENTIALS_PUBLIC_KEY", &demo);
    assert_eq!(
        load_with(Binary::Api, &v, &fp).unwrap_err().to_string(),
        "refusing CREDENTIALS_PUBLIC_KEY: demo value"
    );

    let v = with(poller_prod(&ca), "AWN_APPLICATION_KEY", LIVE_KEY);
    let fp2 = format!("{}  AWN_APPLICATION_KEY\n", sha(LIVE_KEY));
    assert_eq!(
        load_with(Binary::Poller, &v, &fp2).unwrap_err().to_string(),
        "refusing AWN_APPLICATION_KEY: demo value"
    );

    // A health check URL is a secret too.
    let hc = "https://hc.example.invalid/ping/synthetic-uuid";
    let fp3 = format!("{}  X\n", sha(hc));
    assert_eq!(
        load_with(Binary::Poller, &poller_prod(&ca), &fp3)
            .unwrap_err()
            .to_string(),
        "refusing HEALTHCHECK_URL_AWN_INGEST: demo value"
    );

    // Unrelated fingerprints do not match.
    let fp4 = format!("{}  X\n", sha("something else"));
    assert!(load_with(Binary::Poller, &poller_prod(&ca), &fp4).is_ok());
}

#[test]
fn demo_password_inside_database_url_names_database_url() {
    let ca = Ca::new();
    let fp = format!("{}  APP_POLLER_PASSWORD\n", sha("demo-db-password"));
    let url = prod_url(
        &ca,
        "app_poller",
        "db.example.invalid",
        5432,
        "demo-db-password",
    );
    let e = load_with(
        Binary::Poller,
        &with(poller_prod(&ca), "DATABASE_URL", &url),
        &fp,
    )
    .unwrap_err();
    assert_eq!(e.to_string(), "refusing DATABASE_URL: demo value");
    // Percent-encoded passwords are decoded before hashing.
    let enc = prod_url(
        &ca,
        "app_poller",
        "db.example.invalid",
        5432,
        "demo%2Ddb%2Dpassword",
    );
    let e = load_with(
        Binary::Poller,
        &with(poller_prod(&ca), "DATABASE_URL", &enc),
        &fp,
    )
    .unwrap_err();
    assert_eq!(e.to_string(), "refusing DATABASE_URL: demo value");
}

#[test]
fn demo_values_are_ignored_locally() {
    let fp = format!("{}  X\n", sha("localpw"));
    let v = Vars::from_pairs([
        ("APP_ENV", "local"),
        (
            "DATABASE_URL",
            "postgres://app_poller:localpw@db:5432/postgres",
        ),
        ("AWN_MODE", "replay"),
        ("FIXTURES_DIR", "/fixtures"),
    ]);
    assert!(load_with(Binary::Poller, &v, &fp).is_ok());
}

#[test]
fn committed_fingerprint_file_is_well_formed() {
    let lines: Vec<&str> = DEMO_FINGERPRINTS
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect();
    assert!(lines.len() >= 12);
    for l in &lines {
        let (h, name) = l.split_once("  ").expect("two spaces");
        assert_eq!(h.len(), 64, "{l}");
        assert!(
            h.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "{l}"
        );
        assert!(
            !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'),
            "{l}"
        );
    }
    for needle in [
        "CREDENTIALS_PRIVATE_KEY",
        "CREDENTIALS_PUBLIC_KEY",
        "JWT_HS256_SECRET",
        "LOCAL_DEMO_PASSWORD",
    ] {
        assert!(lines.iter().any(|l| l.ends_with(needle)), "{needle}");
    }
}

// --- errors never print values -----------------------------------------------------------------------------------------

#[test]
fn error_text_never_contains_values() {
    let ca = Ca::new();
    let secret = "sekrit-value-0123456789";
    let cases: Vec<(Binary, Vars)> = vec![
        (
            Binary::Poller,
            with(poller_prod(&ca), "CREDENTIALS_PRIVATE_KEY", secret),
        ),
        (
            Binary::Poller,
            with(
                poller_prod(&ca),
                "DATABASE_URL",
                &format!("postgres://app_api:{secret}@h:5432/x?sslmode=verify-full"),
            ),
        ),
        (
            Binary::Poller,
            with(poller_prod(&ca), "JWT_HS256_SECRET", secret),
        ),
        (Binary::Poller, with(poller_prod(&ca), "AWN_MODE", secret)),
        (
            Binary::Api,
            with(
                api_prod(&ca),
                "SUPABASE_URL",
                &format!("not a url {secret}"),
            ),
        ),
        (
            Binary::Api,
            with(api_prod(&ca), "NETLIFY_PROXY_SECRET", secret),
        ),
    ];
    for (bin, vars) in cases {
        let text = match load_with(bin, &vars, &format!("{}  X\n", sha(secret))) {
            Err(e) => format!("{e} {e:?}"),
            Ok(_) => continue, // accepted values are not errors
        };
        assert!(!text.contains(secret), "{text}");
    }
    // Debug of loaded configs hides secrets.
    let Config::Poller(c) = load_with(Binary::Poller, &poller_prod(&ca), "").unwrap() else {
        panic!()
    };
    let dbg = format!("{c:?}");
    for needle in [
        LIVE_KEY,
        &b64(1),
        "synthetic-pw-1",
        "synthetic-uuid",
        "hc.example.invalid",
    ] {
        assert!(!dbg.contains(needle), "{dbg}");
    }
    let Config::Api(c) = load_with(Binary::Api, &api_prod(&ca), "").unwrap() else {
        panic!()
    };
    let dbg = format!("{c:?}");
    for needle in [LIVE_KEY, "synthetic-pw-2"] {
        assert!(!dbg.contains(needle), "{dbg}");
    }
}

// --- local rules ----------------------------------------------------------------------------------------------------------

fn local_poller() -> Vars {
    Vars::from_pairs([
        ("APP_ENV", "local"),
        (
            "DATABASE_URL",
            "postgres://app_poller:localpw@db:5432/postgres",
        ),
        ("AWN_MODE", "replay"),
        ("FIXTURES_DIR", "/fixtures"),
        ("HEALTHCHECK_URL_AWN_INGEST", "https://hc.example.invalid/x"),
    ])
}

#[test]
fn local_poller_defaults() {
    let Config::Poller(c) = load_with(Binary::Poller, &local_poller(), "").unwrap() else {
        panic!()
    };
    assert!(!c.enabled, "POLLER_ENABLED defaults to false");
    assert!(
        c.healthcheck_urls.is_empty(),
        "HEALTHCHECK_URL_* is ignored locally"
    );
    assert!(c.current_key.is_none());
}

#[test]
fn local_refuses_non_local_database_hosts() {
    for host in ["db.example.invalid", "10.0.0.5", "prod.example.com"] {
        let v = with(
            local_poller(),
            "DATABASE_URL",
            &format!("postgres://app_poller:pw@{host}:5432/postgres"),
        );
        assert_refused(load_poller_err(&v), "DATABASE_URL");
    }
    for host in ["db", "localhost", "127.0.0.1", "[::1]"] {
        let v = with(
            local_poller(),
            "DATABASE_URL",
            &format!("postgres://app_poller:pw@{host}:5432/postgres"),
        );
        assert!(load_with(Binary::Poller, &v, "").is_ok(), "{host}");
    }
}

#[test]
fn local_refuses_live_mode_with_empty_or_placeholder_key() {
    let live = with(local_poller(), "AWN_MODE", "live");
    assert_eq!(
        load_poller_err(&live),
        ConfigError::MissingEnv("AWN_APPLICATION_KEY".into())
    );
    assert_refused(
        load_poller_err(&with(
            live.clone(),
            "AWN_APPLICATION_KEY",
            AWN_KEY_PLACEHOLDER,
        )),
        "AWN_APPLICATION_KEY",
    );
    assert_refused(
        load_poller_err(&with(live.clone(), "AWN_APPLICATION_KEY", "short")),
        "AWN_APPLICATION_KEY",
    );
    assert_eq!(
        load_poller_err(&with(live.clone(), "AWN_APPLICATION_KEY", "   ")),
        ConfigError::MissingEnv("AWN_APPLICATION_KEY".into())
    );
    assert!(load_with(
        Binary::Poller,
        &with(live, "AWN_APPLICATION_KEY", LIVE_KEY),
        ""
    )
    .is_ok());
}

#[test]
fn local_api_checks_supabase_host() {
    let base = Vars::from_pairs([
        ("APP_ENV", "local".to_owned()),
        (
            "DATABASE_URL",
            "postgres://app_api:pw@db:5432/postgres".to_owned(),
        ),
        ("SUPABASE_URL", "http://localhost:8000".to_owned()),
        ("AWN_MODE", "replay".to_owned()),
        ("FIXTURES_DIR", "/fixtures".to_owned()),
        ("CREDENTIALS_PUBLIC_KEY", b64(2)),
        ("CREDENTIALS_KEY_VERSION", "1".to_owned()),
        (
            "JWT_HS256_SECRET",
            "local-only-secret-local-only-secret".to_owned(),
        ),
    ]);
    let Config::Api(c) = load_with(Binary::Api, &base, "").unwrap() else {
        panic!()
    };
    assert!(c.jwt_hs256_secret.is_some());
    for host in ["caddy", "auth", "127.0.0.1"] {
        assert!(
            load_with(
                Binary::Api,
                &with(base.clone(), "SUPABASE_URL", &format!("http://{host}:8000")),
                ""
            )
            .is_ok(),
            "{host}"
        );
    }
    assert_refused(
        load_api_err(&with(base, "SUPABASE_URL", "https://proj.supabase.co")),
        "SUPABASE_URL",
    );
}

// --- admin ----------------------------------------------------------------------------------------------------------------

#[test]
fn admin_migrate_needs_only_app_env_and_database_url() {
    let v = Vars::from_pairs([
        ("APP_ENV", "local"),
        (
            "DATABASE_URL",
            "postgres://app_migrator:pw@db:5432/postgres",
        ),
    ]);
    let Config::Admin(c) = load_with(Binary::Admin(AdminCommand::Migrate), &v, "").unwrap() else {
        panic!()
    };
    assert_eq!(c.app_env, Some(AppEnv::Local));
    assert!(c.supabase_url.is_none() && c.credentials_public_key.is_none());
    assert_eq!(
        load_with(
            Binary::Admin(AdminCommand::Migrate),
            &without(v.clone(), "DATABASE_URL"),
            ""
        )
        .unwrap_err(),
        ConfigError::MissingEnv("DATABASE_URL".into())
    );
    // Variables the command does not use are not parsed, so a bad one cannot block it.
    let noisy = with(v, "CREDENTIALS_PUBLIC_KEY", "garbage");
    assert!(load_with(Binary::Admin(AdminCommand::Migrate), &noisy, "").is_ok());
}

#[test]
fn admin_keygen_loads_nothing() {
    let Config::Admin(c) = load_with(
        Binary::Admin(AdminCommand::Keygen { check: false }),
        &Vars::default(),
        "",
    )
    .unwrap() else {
        panic!()
    };
    assert!(c.app_env.is_none() && c.database_url.is_none());
}

#[test]
fn admin_invite_requires_site_url_and_supabase() {
    let v = Vars::from_pairs([
        ("APP_ENV", "local"),
        (
            "DATABASE_URL",
            "postgres://app_migrator:pw@db:5432/postgres",
        ),
        ("SUPABASE_URL", "http://localhost:8000"),
        ("SUPABASE_SERVICE_ROLE_KEY", "local-service-key"),
    ]);
    let cmd = AdminCommand::Invite {
        email: "a@example.test".into(),
        display_name: "A".into(),
        follow_all: false,
        admin: false,
        adopt: false,
    };
    assert_eq!(
        load_with(Binary::Admin(cmd.clone()), &v, "").unwrap_err(),
        ConfigError::MissingEnv("SITE_URL".into())
    );
    let ok = with(v.clone(), "SITE_URL", "http://localhost:8000");
    assert!(load_with(Binary::Admin(cmd.clone()), &ok, "").is_ok());
    assert_refused(
        load_with(
            Binary::Admin(cmd),
            &with(v, "SITE_URL", "http://localhost:8000/path"),
            "",
        )
        .unwrap_err(),
        "SITE_URL",
    );
}

#[test]
fn admin_production_site_url_must_be_https_and_not_local() {
    let ca = Ca::new();
    let base = Vars::from_pairs([
        ("APP_ENV", "production".to_owned()),
        (
            "DATABASE_URL",
            prod_url(&ca, "app_migrator", "db.example.invalid", 5432, "pw"),
        ),
        ("SUPABASE_URL", "https://proj.example.invalid".to_owned()),
        (
            "SUPABASE_SERVICE_ROLE_KEY",
            "sb_secret_synthetic".to_owned(),
        ),
        ("SITE_URL", "https://app.example.invalid".to_owned()),
    ]);
    let cmd = AdminCommand::ResetLink {
        email: "a@example.test".into(),
    };
    assert!(load_with(Binary::Admin(cmd.clone()), &base, "").is_ok());
    for bad in [
        "http://app.example.invalid",
        "https://localhost",
        "https://app.test",
    ] {
        assert_refused(
            load_with(
                Binary::Admin(cmd.clone()),
                &with(base.clone(), "SITE_URL", bad),
                "",
            )
            .unwrap_err(),
            "SITE_URL",
        );
    }
    // A demo service key is refused with the exact message.
    let fp = format!(
        "{}  SUPABASE_SERVICE_ROLE_KEY\n",
        sha("sb_secret_synthetic")
    );
    assert_eq!(
        load_with(Binary::Admin(cmd), &base, &fp)
            .unwrap_err()
            .to_string(),
        "refusing SUPABASE_SERVICE_ROLE_KEY: demo value"
    );
}

#[test]
fn admin_seed_dev_refuses_production() {
    let ca = Ca::new();
    let v = Vars::from_pairs([
        ("APP_ENV", "production".to_owned()),
        (
            "DATABASE_URL",
            prod_url(&ca, "app_migrator", "db.example.invalid", 5432, "pw"),
        ),
        ("SUPABASE_URL", "https://proj.example.invalid".to_owned()),
        ("SUPABASE_SERVICE_ROLE_KEY", "k".to_owned()),
        ("CREDENTIALS_PUBLIC_KEY", b64(2)),
        ("CREDENTIALS_KEY_VERSION", "1".to_owned()),
        ("LOCAL_DEMO_PASSWORD", "pw".to_owned()),
    ]);
    assert_refused(
        load_with(
            Binary::Admin(AdminCommand::SeedDev { no_stations: false }),
            &v,
            "",
        )
        .unwrap_err(),
        "APP_ENV",
    );
}

#[test]
fn admin_add_station_parses_the_public_key_and_optional_awn_settings() {
    let v = Vars::from_pairs([
        ("APP_ENV", "local".to_owned()),
        (
            "DATABASE_URL",
            "postgres://app_migrator:pw@db:5432/postgres".to_owned(),
        ),
        ("CREDENTIALS_PUBLIC_KEY", b64(2)),
        ("CREDENTIALS_KEY_VERSION", "3".to_owned()),
        ("AWN_MODE", "replay".to_owned()),
        ("FIXTURES_DIR", "/fixtures".to_owned()),
    ]);
    let cmd = AdminCommand::AddStation {
        owner: OwnerRef::DisplayName("A".into()),
        mac: "AA:BB:CC:00:00:01".into(),
        name: "S".into(),
        lat: 40.0,
        lon: -105.0,
        timezone: "America/Denver".into(),
        std_offset_minutes: -420,
        elevation_ft: None,
        reference_station: None,
        credential: CredentialMode::New,
    };
    let Config::Admin(c) = load_with(Binary::Admin(cmd.clone()), &v, "").unwrap() else {
        panic!()
    };
    assert_eq!(c.credentials_key_version, Some(3));
    assert_eq!(c.awn_mode, Some(AwnMode::Replay));
    assert!(c.credentials_public_key.is_some());
    assert_refused(
        load_with(
            Binary::Admin(cmd),
            &with(v, "CREDENTIALS_PUBLIC_KEY", "nope"),
            "",
        )
        .unwrap_err(),
        "CREDENTIALS_PUBLIC_KEY",
    );
}

#[test]
fn blank_values_count_as_unset() {
    let v = with(local_poller(), "POLLER_ENABLED", "");
    assert!(load_with(Binary::Poller, &v, "").is_ok());
}

// --- DATABASE_URL query-string overrides (sqlx lets them replace the URL's own parts) --------------------------------------

#[test]
fn production_refuses_query_string_overrides_of_validated_parts() {
    let ca = Ca::new();
    let base = prod_url(&ca, "app_poller", "db.example.invalid", 5432, "pw");
    for extra in [
        "&ssl-mode=disable",
        "&port=6543",
        "&host=aws-0-x.pooler.example.invalid",
        "&hostaddr=10.0.0.9",
        "&user=postgres",
        "&password=demo-db-password",
        "&dbname=other",
        "&application_name=x",
        "&options=-c%20role%3Dpostgres",
        "&sslmode=disable",
        "&sslrootcert=/etc/other.crt",
        "&ssl-root-cert=/etc/other.crt",
        "&statement-cache-capacity=0",
    ] {
        let e = load_poller_err(&with(
            poller_prod(&ca),
            "DATABASE_URL",
            &format!("{base}{extra}"),
        ));
        assert_refused(e, "DATABASE_URL");
    }
    // The same through the other binaries.
    let api = prod_url(&ca, "app_api", "db.example.invalid", 5432, "pw");
    assert_refused(
        load_api_err(&with(
            api_prod(&ca),
            "DATABASE_URL",
            &format!("{api}&port=6543"),
        )),
        "DATABASE_URL",
    );
    let adm = Vars::from_pairs([
        ("APP_ENV", "production".to_owned()),
        (
            "DATABASE_URL",
            format!(
                "{}&user=postgres",
                prod_url(&ca, "app_migrator", "db.example.invalid", 5432, "pw")
            ),
        ),
    ]);
    assert_refused(
        load_with(Binary::Admin(AdminCommand::Migrate), &adm, "").unwrap_err(),
        "DATABASE_URL",
    );
}

#[test]
fn query_overrides_cannot_smuggle_a_demo_password_past_the_check() {
    let ca = Ca::new();
    let fp = format!("{}  X\n", sha("demo-db-password"));
    let url = format!(
        "{}&password=demo-db-password",
        prod_url(&ca, "app_poller", "db.example.invalid", 5432, "other")
    );
    // Refused outright (the override itself), so it can never be the password actually used.
    assert_refused(
        load_with(
            Binary::Poller,
            &with(poller_prod(&ca), "DATABASE_URL", &url),
            &fp,
        )
        .unwrap_err(),
        "DATABASE_URL",
    );
}

#[test]
fn production_requires_the_password_in_the_url() {
    let ca = Ca::new();
    let url = format!(
        "postgres://app_poller@db.example.invalid:5432/postgres?sslmode=verify-full&sslrootcert={}",
        ca.path()
    );
    assert_refused(
        load_poller_err(&with(poller_prod(&ca), "DATABASE_URL", &url)),
        "DATABASE_URL",
    );
}

#[test]
fn local_refuses_a_host_override_in_the_query() {
    for q in [
        "?host=prod.example.com",
        "?hostaddr=10.1.2.3",
        "?host=/var/run/postgresql",
    ] {
        let v = with(
            local_poller(),
            "DATABASE_URL",
            &format!("postgres://app_poller:pw@db:5432/postgres{q}"),
        );
        assert_refused(load_poller_err(&v), "DATABASE_URL");
    }
}

// --- whitespace around secrets --------------------------------------------------------------------------------------------

#[test]
fn padded_secret_values_cannot_slip_past_the_demo_check() {
    let ca = Ca::new();
    let demo = b64(7);
    let fp = format!("{}  X\n", sha(&demo));
    for padded in [
        format!("{demo}\n"),
        format!(" {demo}"),
        format!("{demo}\r\n"),
        format!("\t{demo}\t"),
    ] {
        let e = load_with(
            Binary::Poller,
            &with(poller_prod(&ca), "CREDENTIALS_PRIVATE_KEY", &padded),
            &fp,
        )
        .unwrap_err();
        assert_refused(e.clone(), "CREDENTIALS_PRIVATE_KEY");
        assert!(!e.to_string().contains(&demo));
        let e = load_with(
            Binary::Api,
            &with(api_prod(&ca), "CREDENTIALS_PUBLIC_KEY", &padded),
            &fp,
        )
        .unwrap_err();
        assert_refused(e, "CREDENTIALS_PUBLIC_KEY");
    }
    // The key parser itself no longer trims.
    assert!(crypto::parse_public_key(&format!("{}\n", b64(1))).is_err());
    // Other secrets and the URL are covered too.
    assert_refused(
        load_poller_err(&with(
            poller_prod(&ca),
            "AWN_APPLICATION_KEY",
            &format!("{LIVE_KEY}\n"),
        )),
        "AWN_APPLICATION_KEY",
    );
    assert_refused(
        load_poller_err(&with(
            poller_prod(&ca),
            "HEALTHCHECK_URL_AWN_INGEST",
            " https://hc.example.invalid/x",
        )),
        "HEALTHCHECK_URL_AWN_INGEST",
    );
    let url = format!(
        "{} ",
        prod_url(&ca, "app_poller", "db.example.invalid", 5432, "pw")
    );
    assert_refused(
        load_poller_err(&with(poller_prod(&ca), "DATABASE_URL", &url)),
        "DATABASE_URL",
    );
    assert_refused(
        load_api_err(&with(
            api_prod(&ca),
            "NETLIFY_PROXY_SECRET",
            "secret-value \n",
        )),
        "NETLIFY_PROXY_SECRET",
    );
}

// --- admin: production demo coverage, foreign settings --------------------------------------------------------------

fn admin_prod(ca: &Ca) -> Vars {
    Vars::from_pairs([
        ("APP_ENV", "production".to_owned()),
        (
            "DATABASE_URL",
            prod_url(
                ca,
                "app_migrator",
                "db.example.invalid",
                5432,
                "synthetic-pw-3",
            ),
        ),
    ])
}

#[test]
fn admin_production_demo_check_covers_every_subcommand() {
    let ca = Ca::new();
    let demo = b64(5);
    let fp = format!(
        "{}  X\n{}  Y\n{}  Z\n",
        sha(&demo),
        sha("sb_secret_demo"),
        sha("demo-db-pw")
    );
    for cmd in [
        AdminCommand::Migrate,
        AdminCommand::ListUsers,
        AdminCommand::Wait { minutes: 5 },
    ] {
        // `migrate` does not use these variables, but the demo check still covers them (04 section 10.1).
        let v = with(admin_prod(&ca), "CREDENTIALS_PUBLIC_KEY", &demo);
        assert_eq!(
            load_with(Binary::Admin(cmd.clone()), &v, &fp)
                .unwrap_err()
                .to_string(),
            "refusing CREDENTIALS_PUBLIC_KEY: demo value",
            "{cmd:?}"
        );
        let v = with(
            admin_prod(&ca),
            "SUPABASE_SERVICE_ROLE_KEY",
            "sb_secret_demo",
        );
        assert_eq!(
            load_with(Binary::Admin(cmd.clone()), &v, &fp)
                .unwrap_err()
                .to_string(),
            "refusing SUPABASE_SERVICE_ROLE_KEY: demo value",
            "{cmd:?}"
        );
    }
    let v = with(
        admin_prod(&ca),
        "DATABASE_URL",
        &prod_url(
            &ca,
            "app_migrator",
            "db.example.invalid",
            5432,
            "demo-db-pw",
        ),
    );
    assert_eq!(
        load_with(Binary::Admin(AdminCommand::Wait { minutes: 1 }), &v, &fp)
            .unwrap_err()
            .to_string(),
        "refusing DATABASE_URL: demo value"
    );
}

#[test]
fn admin_production_refuses_awn_key_and_fixtures_dir() {
    let ca = Ca::new();
    for (var, val) in [
        ("AWN_APPLICATION_KEY", LIVE_KEY),
        ("FIXTURES_DIR", "/fixtures"),
    ] {
        let v = with(admin_prod(&ca), var, val);
        assert_refused(
            load_with(Binary::Admin(AdminCommand::Migrate), &v, "").unwrap_err(),
            var,
        );
    }
}

// --- FIXTURES_DIR -----------------------------------------------------------------------------------------------------------

#[test]
fn fixtures_dir_rules() {
    let ca = Ca::new();
    // Refused in production for every binary.
    assert_refused(
        load_poller_err(&with(poller_prod(&ca), "FIXTURES_DIR", "/fixtures")),
        "FIXTURES_DIR",
    );
    assert_refused(
        load_api_err(&with(api_prod(&ca), "FIXTURES_DIR", "/fixtures")),
        "FIXTURES_DIR",
    );
    // Locally: required for replay, no default, optional otherwise.
    assert_eq!(
        load_poller_err(&without(local_poller(), "FIXTURES_DIR")),
        ConfigError::MissingEnv("FIXTURES_DIR".into())
    );
    let live = with(without(local_poller(), "FIXTURES_DIR"), "AWN_MODE", "live");
    let live = with(live, "AWN_APPLICATION_KEY", LIVE_KEY);
    let Config::Poller(c) = load_with(Binary::Poller, &live, "").unwrap() else {
        panic!()
    };
    assert!(c.fixtures_dir.as_os_str().is_empty());
    let Config::Poller(c) = load_with(Binary::Poller, &local_poller(), "").unwrap() else {
        panic!()
    };
    assert_eq!(c.fixtures_dir, PathBuf::from("/fixtures"));
}

// --- Debug never prints secrets ---------------------------------------------------------------------------------------------

#[test]
fn vars_debug_lists_names_only() {
    let v = Vars::from_pairs([
        ("DATABASE_URL", "postgres://u:hunter2hunter2@h/db"),
        ("JWT_HS256_SECRET", "sekrit-value-0123456789"),
    ]);
    let d = format!("{v:?}");
    assert!(d.contains("DATABASE_URL") && d.contains("JWT_HS256_SECRET"));
    assert!(!d.contains("hunter2") && !d.contains("sekrit"));
}

#[test]
fn config_debug_never_prints_the_database_password() {
    let ca = Ca::new();
    for (bin, vars) in [
        (Binary::Poller, poller_prod(&ca)),
        (Binary::Api, api_prod(&ca)),
    ] {
        let cfg = load_with(bin, &vars, "").unwrap();
        let d = format!("{cfg:?} {cfg:#?}");
        assert!(!d.contains("synthetic-pw"), "{d}");
    }
    let Config::Admin(a) =
        load_with(Binary::Admin(AdminCommand::Migrate), &admin_prod(&ca), "").unwrap()
    else {
        panic!()
    };
    assert!(!format!("{a:?}").contains("synthetic-pw-3"));
}
