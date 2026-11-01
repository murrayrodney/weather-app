//! `admin` CLI tests against the Compose test project (04 section 11.5). Run by `make test-backend` as
//! `cargo test -p admin --features compose-tests -- --test-threads=1` with the environment described in 00-conventions
//! ("Testing harness"). The tests invoke the real binary and talk to the real GoTrue (`SUPABASE_URL=http://auth:9999`).
//!
//! The stack may already contain seed data and other tests' leftovers, so every test creates its own users (unique
//! emails, display names, MACs), asserts only on its own rows and on deltas, and cleans up what it created.
#![cfg(feature = "compose-tests")]

use std::collections::HashSet;
use std::path::PathBuf;

use assert_cmd::Command;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use famweather_core::crypto::{decrypt_credential, parse_private_key, EncryptedCredential};
use hmac::{Hmac, Mac};
use secrecy::ExposeSecret;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use uuid::Uuid;

// ---------------------------------------------------------------------------------------------------------------
// harness

fn env(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("{name} must be set (see 00-conventions, Testing harness)"))
}

fn service_role_jwt() -> String {
    let b = |v: &Value| URL_SAFE_NO_PAD.encode(serde_json::to_vec(v).unwrap());
    let head = b(&json!({"alg": "HS256", "typ": "JWT"}));
    let body = b(
        &json!({"iss": "supabase-local", "role": "service_role", "iat": 1_790_000_000u64, "exp": 2_105_000_000u64}),
    );
    let signing = format!("{head}.{body}");
    let mut mac = Hmac::<Sha256>::new_from_slice(env("TEST_JWT_HS256_SECRET").as_bytes()).unwrap();
    mac.update(signing.as_bytes());
    format!(
        "{signing}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    )
}

fn uniq() -> String {
    Uuid::new_v4().simple().to_string()[..12].to_owned()
}

/// Output of one CLI run.
struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Run {
    fn ok(&self) -> &Self {
        assert_eq!(
            self.code, 0,
            "expected exit 0\nstdout: {}\nstderr: {}",
            self.stdout, self.stderr
        );
        self
    }
    fn failed(&self) -> &Self {
        assert_eq!(
            self.code, 1,
            "expected exit 1\nstdout: {}\nstderr: {}",
            self.stdout, self.stderr
        );
        self
    }
}

/// A command with a scrubbed environment and the standard test configuration.
fn cli(args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_admin"));
    c.env_clear()
        .env("APP_ENV", "local")
        .env("DATABASE_URL", env("TEST_MIGRATOR_DATABASE_URL"))
        .env("SUPABASE_URL", env("SUPABASE_URL"))
        .env("SUPABASE_SERVICE_ROLE_KEY", service_role_jwt())
        .env("SITE_URL", env("SITE_URL"))
        .env("CREDENTIALS_PUBLIC_KEY", env("CREDENTIALS_PUBLIC_KEY"))
        .env("CREDENTIALS_KEY_VERSION", env("CREDENTIALS_KEY_VERSION"))
        .args(args);
    c
}

fn run(mut c: Command) -> Run {
    let out = c.output().expect("spawn admin");
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

struct World {
    pool: PgPool, // app_migrator
    su: PgPool,   // superuser (auth.* peeks)
    http: reqwest::Client,
    auth: String,
    jwt: String,
}

async fn world() -> World {
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&env("TEST_MIGRATOR_DATABASE_URL"))
        .await
        .unwrap();
    let su = PgPoolOptions::new()
        .max_connections(4)
        .connect(&env("TEST_SUPERUSER_DATABASE_URL"))
        .await
        .unwrap();
    World {
        pool,
        su,
        http: reqwest::Client::new(),
        auth: env("SUPABASE_URL").trim_end_matches('/').to_owned(),
        jwt: service_role_jwt(),
    }
}

impl World {
    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        bearer: Option<&str>,
    ) -> (u16, Value) {
        let mut rb = self.http.request(method, format!("{}{path}", self.auth));
        if let Some(b) = bearer {
            rb = rb.bearer_auth(b);
        }
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        let resp = rb.send().await.expect("auth request");
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn verify(&self, token_hash: &str) -> (u16, Value) {
        self.call(
            reqwest::Method::POST,
            "/verify",
            Some(json!({"type": "recovery", "token_hash": token_hash})),
            None,
        )
        .await
    }

    async fn set_password(&self, access_token: &str, password: &str) -> (u16, Value) {
        self.call(
            reqwest::Method::PUT,
            "/user",
            Some(json!({"password": password})),
            Some(access_token),
        )
        .await
    }

    async fn sign_in(&self, email: &str, password: &str) -> (u16, Value) {
        self.call(
            reqwest::Method::POST,
            "/token?grant_type=password",
            Some(json!({"email": email, "password": password})),
            None,
        )
        .await
    }

    async fn refresh(&self, refresh_token: &str) -> (u16, Value) {
        self.call(
            reqwest::Method::POST,
            "/token?grant_type=refresh_token",
            Some(json!({"refresh_token": refresh_token})),
            None,
        )
        .await
    }

    async fn auth_user_exists(&self, id: Uuid) -> bool {
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM auth.users WHERE id = $1)")
            .bind(id)
            .fetch_one(&self.su)
            .await
            .unwrap()
    }

    async fn auth_email_count(&self, email: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM auth.users WHERE lower(email) = lower($1)")
            .bind(email)
            .fetch_one(&self.su)
            .await
            .unwrap()
    }

    async fn session_count(&self, id: Uuid) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM auth.sessions WHERE user_id = $1")
            .bind(id)
            .fetch_one(&self.su)
            .await
            .unwrap()
    }

    async fn app_user_exists(&self, id: Uuid) -> bool {
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM app.users WHERE id = $1)")
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn audit_count(&self, action: &str, subject: Uuid) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM app.audit_log WHERE action = $1 AND subject_id = $2",
        )
        .bind(action)
        .bind(subject)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }

    /// Removes a user created by a test: app rows (cascade) and the login.
    async fn cleanup(&self, id: Uuid) {
        let _ = sqlx::query("DELETE FROM app.users WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await;
        let _ = self
            .call(
                reqwest::Method::DELETE,
                &format!("/admin/users/{id}"),
                None,
                Some(&self.jwt),
            )
            .await;
    }
}

struct Invited {
    id: Uuid,
    email: String,
    name: String,
    token: String,
    link: String,
    stdout: String,
    stderr: String,
}

/// Appends a minted set-password token to `$CANARY_TOKENS_FILE` (inside the repo mount). `ops/local/check-secret-logs.sh`
/// then fails `make test-backend` if any of them appears in the db, auth or migrate container logs.
fn record_token(token: &str) {
    use std::io::Write;
    let Ok(path) = std::env::var("CANARY_TOKENS_FILE") else {
        return;
    };
    let path = std::path::PathBuf::from(path);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).expect("create canary token dir");
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open canary token file");
    writeln!(f, "{token}").unwrap();
}

fn parse_invite(email: &str, name: &str, r: &Run) -> Invited {
    r.ok();
    let lines: Vec<&str> = r.stdout.lines().collect();
    let link_lines: Vec<&&str> = lines
        .iter()
        .filter(|l| l.contains("/set-password#token_hash="))
        .collect();
    assert_eq!(
        link_lines.len(),
        1,
        "exactly one link in stdout: {}",
        r.stdout
    );
    let link = (*link_lines[0]).to_owned();
    let token = link
        .split("token_hash=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .to_owned();
    record_token(&token);
    Invited {
        id: lines[0]
            .trim()
            .parse()
            .expect("first stdout line is the user id"),
        email: email.to_owned(),
        name: name.to_owned(),
        token,
        link,
        stdout: r.stdout.clone(),
        stderr: r.stderr.clone(),
    }
}

async fn invite(w: &World, follow_all: bool) -> Invited {
    let t = uniq();
    let (email, name) = (format!("t-{t}@example.test"), format!("T {t}"));
    let mut args = vec![
        "invite",
        "--email",
        email.as_str(),
        "--display-name",
        name.as_str(),
    ];
    if follow_all {
        args.push("--follow-all");
    }
    let r = run(cli(&args));
    let inv = parse_invite(&email, &name, &r);
    assert!(w.app_user_exists(inv.id).await);
    inv
}

/// Redeems an invite / reset link and sets a password; returns the password.
async fn set_password_via_link(w: &World, token: &str) -> String {
    let (status, body) = w.verify(token).await;
    assert_eq!(status, 200, "verify: {body}");
    let access = body["access_token"]
        .as_str()
        .expect("access_token")
        .to_owned();
    let password = format!("Pw-{}-xK9", uniq());
    let (status, body) = w.set_password(&access, &password).await;
    assert_eq!(status, 200, "set password: {body}");
    password
}

/// The single admin: returns (id, created_by_this_test).
async fn ensure_admin(w: &World) -> (Uuid, bool) {
    if let Some(id) = sqlx::query_scalar::<_, Uuid>("SELECT id FROM app.users WHERE is_admin")
        .fetch_optional(&w.pool)
        .await
        .unwrap()
    {
        return (id, false);
    }
    let t = uniq();
    let (email, name) = (format!("t-admin-{t}@example.test"), format!("T Admin {t}"));
    let r = run(cli(&[
        "invite",
        "--email",
        &email,
        "--display-name",
        &name,
        "--admin",
    ]));
    (parse_invite(&email, &name, &r).id, true)
}

fn mac() -> String {
    let u = Uuid::new_v4();
    let b = u.as_bytes();
    format!("AA:BB:CC:{:02X}:{:02X}:{:02X}", b[0], b[1], b[2])
}

fn add_station_args<'a>(owner: &'a str, mac: &'a str, extra: &[&'a str]) -> Vec<&'a str> {
    let mut a = vec![
        "add-station",
        "--owner",
        owner,
        "--mac",
        mac,
        "--name",
        "Test Station",
        "--lat",
        "46.58765",
        "--lon",
        "-112.03999",
        "--timezone",
        "America/Denver",
        "--std-offset-minutes",
        "-420",
    ];
    a.extend_from_slice(extra);
    a
}

const SENTINEL: &str = "canary-secret-awnkey-0123";

fn add_station(owner: Uuid, mac: &str, key: &str, extra: &[&str]) -> Run {
    let owner = owner.to_string();
    let mut c = cli(&add_station_args(&owner, mac, extra));
    c.write_stdin(format!("{key}\n"));
    run(c)
}

// ---------------------------------------------------------------------------------------------------------------
// migrate, keygen, production guard

#[tokio::test]
async fn migrate_env_and_idempotence() {
    let w = world().await;
    let only = |url: String| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_admin"));
        c.env_clear()
            .env("APP_ENV", "local")
            .env("DATABASE_URL", url)
            .arg("migrate");
        c
    };
    run(only(env("TEST_MIGRATOR_DATABASE_URL"))).ok();
    let second = run(only(env("TEST_MIGRATOR_DATABASE_URL")));
    second.ok();
    assert!(
        second.stdout.contains("0 newly applied"),
        "{}",
        second.stdout
    );
    // The migrations table lives in app and public stays empty.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM app._sqlx_migrations")
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert!(n >= 9);
    let public: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = 'public'")
        .fetch_one(&w.su)
        .await
        .unwrap();
    assert_eq!(public, 0);
    // A superuser (not app_migrator) is refused.
    run(only(env("TEST_SUPERUSER_DATABASE_URL"))).failed();
}

#[test]
fn keygen_check() {
    let bare = |args: &[&str]| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_admin"));
        c.env_clear().args(args);
        c
    };
    let generated = run(bare(&["keygen"]));
    generated.ok(); // runs with an empty environment
    let get = |prefix: &str| {
        generated
            .stdout
            .lines()
            .find_map(|l| l.strip_prefix(prefix))
            .unwrap()
            .to_owned()
    };
    let (public, private) = (
        get("CREDENTIALS_PUBLIC_KEY="),
        get("CREDENTIALS_PRIVATE_KEY="),
    );
    let mut c = bare(&["keygen", "--check"]);
    c.env("CREDENTIALS_PUBLIC_KEY", &public)
        .env("CREDENTIALS_PRIVATE_KEY", &private);
    run(c).ok();
    // A mismatched pair exits 1.
    let other = run(bare(&["keygen"]));
    let other_public = other
        .stdout
        .lines()
        .find_map(|l| l.strip_prefix("CREDENTIALS_PUBLIC_KEY="))
        .unwrap()
        .to_owned();
    let mut c = bare(&["keygen", "--check"]);
    c.env("CREDENTIALS_PUBLIC_KEY", &other_public)
        .env("CREDENTIALS_PRIVATE_KEY", &private);
    run(c).failed();
    // The committed demo pair also matches (the interim pair of .env.example).
    let mut c = bare(&["keygen", "--check"]);
    c.env("CREDENTIALS_PUBLIC_KEY", env("CREDENTIALS_PUBLIC_KEY"))
        .env("CREDENTIALS_PRIVATE_KEY", env("CREDENTIALS_PRIVATE_KEY"));
    run(c).ok();
    // The private key is never printed by --check.
    let mut c = bare(&["keygen", "--check"]);
    c.env("CREDENTIALS_PUBLIC_KEY", &public)
        .env("CREDENTIALS_PRIVATE_KEY", &private);
    let r = run(c);
    assert!(!r.stdout.contains(&private) && !r.stderr.contains(&private));
}

fn demo_value(name: &str) -> String {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "..", "..", "..", ".env.example"]
        .iter()
        .collect();
    let text =
        std::fs::read_to_string(&path).expect("read .env.example (the repo is mounted at /work)");
    text.lines()
        .find_map(|l| l.strip_prefix(&format!("{name}=")))
        .unwrap_or_else(|| panic!("{name} missing from .env.example"))
        .trim_matches('\'')
        .to_owned()
}

#[test]
fn production_guard() {
    let ca = tempfile::NamedTempFile::new().unwrap();
    let ca_path = ca.path().display().to_string();
    let random_pw = uniq();
    let good_url = format!("postgres://app_migrator:{random_pw}@db.example.invalid:5432/postgres?sslmode=verify-full&sslrootcert={ca_path}");
    let prod = |url: &str, pub_key: &str, service_key: &str, args: &[&str]| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_admin"));
        c.env_clear()
            .env("APP_ENV", "production")
            .env("DATABASE_URL", url)
            .env("SUPABASE_URL", "https://example.invalid")
            .env("SUPABASE_SERVICE_ROLE_KEY", service_key)
            .env("SITE_URL", "https://example.invalid")
            .env("CREDENTIALS_PUBLIC_KEY", pub_key)
            .env("CREDENTIALS_KEY_VERSION", "1")
            .args(args);
        c
    };
    let real_pub = {
        let (pk, _sk) = famweather_core::crypto::generate_keypair();
        pk.to_base64()
    };
    let real_service = "sb_secret_not-a-demo-value-0123456789";
    let subcommands: [&[&str]; 4] = [
        &["migrate"],
        &["list-users"],
        &[
            "add-reference-station",
            "--id",
            "USW00099999",
            "--name",
            "x",
            "--lat",
            "1",
            "--lon",
            "2",
            "--timezone",
            "UTC",
            "--std-offset-minutes",
            "0",
        ],
        &["deactivate-user", "--display-name", "nobody"],
    ];
    for args in subcommands {
        // not verify-full
        let r = run(prod(
            "postgres://app_migrator:x1y2z3@db.example.invalid:5432/postgres?sslmode=require",
            &real_pub,
            real_service,
            args,
        ));
        r.failed();
        assert!(r.stderr.contains("DATABASE_URL"), "{args:?}: {}", r.stderr);
        // demo public key
        let r = run(prod(
            &good_url,
            &demo_value("CREDENTIALS_PUBLIC_KEY"),
            real_service,
            args,
        ));
        r.failed();
        assert!(
            r.stderr
                .contains("refusing CREDENTIALS_PUBLIC_KEY: demo value"),
            "{args:?}: {}",
            r.stderr
        );
        // demo service key
        let r = run(prod(
            &good_url,
            &real_pub,
            &demo_value("SUPABASE_SERVICE_ROLE_KEY"),
            args,
        ));
        r.failed();
        assert!(
            r.stderr
                .contains("refusing SUPABASE_SERVICE_ROLE_KEY: demo value"),
            "{args:?}: {}",
            r.stderr
        );
        // demo database password
        let demo_pw_url = format!("postgres://app_migrator:{}@db.example.invalid:5432/postgres?sslmode=verify-full&sslrootcert={ca_path}", demo_value("APP_MIGRATOR_PASSWORD"));
        let r = run(prod(&demo_pw_url, &real_pub, real_service, args));
        r.failed();
        assert!(
            r.stderr.contains("refusing DATABASE_URL: demo value"),
            "{args:?}: {}",
            r.stderr
        );
        // no value is echoed
        for secret in [
            demo_value("CREDENTIALS_PUBLIC_KEY"),
            demo_value("SUPABASE_SERVICE_ROLE_KEY"),
            demo_value("APP_MIGRATOR_PASSWORD"),
        ] {
            assert!(!r.stderr.contains(&secret) && !r.stdout.contains(&secret));
        }
    }
    // seed-dev refuses production outright.
    let mut c = prod(&good_url, &real_pub, real_service, &["seed-dev"]);
    c.env("LOCAL_DEMO_PASSWORD", "not-a-demo-value-1234");
    run(c).failed();
}

// ---------------------------------------------------------------------------------------------------------------
// invite

#[tokio::test]
async fn invite_creates_user_and_follows() {
    let w = world().await;
    let before: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM app.users WHERE is_active")
        .fetch_all(&w.pool)
        .await
        .unwrap();
    let inv = invite(&w, true).await;

    // The login was created with no password. GoTrue still stores a random bcrypt hash for a passwordless admin-created
    // user (observed on v2.197.0), so "no password" means: nobody knows one, and no sign-in can succeed before the link is used.
    let (status, _) = w.sign_in(&inv.email, "").await;
    assert!((400..500).contains(&status));
    let (status, _) = w.sign_in(&inv.email, &env("LOCAL_DEMO_PASSWORD")).await;
    assert!(
        (400..500).contains(&status),
        "the demo password must not work for an invited user"
    );

    // Both directions to every previously active user, approved with decided_at.
    let follows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM app.follows WHERE status = 'approved' AND decided_at IS NOT NULL AND (follower_id = $1 OR followee_id = $1)",
    )
    .bind(inv.id)
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(follows, 2 * before.len() as i64);
    let other_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM app.follows WHERE follower_id = $1 OR followee_id = $1",
    )
    .bind(inv.id)
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(other_rows, follows, "no follow in any other state");

    // Audit: user.invited once, follow.seeded once per follow row.
    assert_eq!(w.audit_count("user.invited", inv.id).await, 1);
    let seeded: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM app.audit_log WHERE action = 'follow.seeded' AND (subject_id = $1 OR detail->>'followee_id' = $1::text)",
    )
    .bind(inv.id)
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(seeded, follows);
    let seeded_subject_types: i64 = sqlx::query_scalar("SELECT count(*) FROM app.audit_log WHERE action = 'follow.seeded' AND subject_type <> 'follow'").fetch_one(&w.pool).await.unwrap();
    assert_eq!(seeded_subject_types, 0);

    // stdout: the id and exactly one link, rooted at SITE_URL.
    assert!(
        inv.link.starts_with(&format!(
            "{}/set-password#token_hash=",
            env("SITE_URL").trim_end_matches('/')
        )),
        "{}",
        inv.link
    );
    assert!(inv.link.ends_with("&type=recovery"));
    // The token reaches no other output and no audit row.
    assert!(!inv.stderr.contains(&inv.token));
    assert_eq!(inv.stdout.matches(&inv.token).count(), 1);
    let leaked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM app.audit_log WHERE detail::text LIKE '%' || $1 || '%'",
    )
    .bind(&inv.token)
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(leaked, 0);
    w.cleanup(inv.id).await;
}

#[tokio::test]
async fn invite_link_sets_password() {
    let w = world().await;
    let inv = invite(&w, false).await;

    // Before the link is used there is no password to sign in with.
    let (status, _) = w.sign_in(&inv.email, "anything-at-all-123").await;
    assert!((400..500).contains(&status));

    // Redeem the link.
    let (status, body) = w.verify(&inv.token).await;
    assert_eq!(status, 200, "verify: {body}");
    let access = body["access_token"].as_str().unwrap().to_owned();
    println!(
        "link lifetime in force: GOTRUE_MAILER_OTP_EXP=3600 s (expires_in of the session: {})",
        body["expires_in"]
    );

    // A 6-character password is rejected (GOTRUE_PASSWORD_MIN_LENGTH=12).
    let (status, body) = w.set_password(&access, "abc123").await;
    assert!(
        matches!(status, 400 | 422),
        "short password must be refused: {status} {body}"
    );

    // A real password is accepted without a nonce and without the old password (neither exists).
    let password = format!("Pw-{}-xK9", uniq());
    let (status, body) = w.set_password(&access, &password).await;
    assert_eq!(status, 200, "set password: {body}");

    let (status, body) = w.sign_in(&inv.email, &password).await;
    assert_eq!(status, 200, "sign in: {body}");
    assert!(body["access_token"].is_string());

    // Wrong password and unknown email are indistinguishable.
    let (s1, b1) = w.sign_in(&inv.email, "wrong-password-0123456").await;
    let (s2, b2) = w
        .sign_in(
            &format!("nobody-{}@example.test", uniq()),
            "wrong-password-0123456",
        )
        .await;
    assert_eq!(s1, s2);
    assert_eq!(b1["error_code"], b2["error_code"]);
    assert_eq!(b1["msg"], b2["msg"]);
    assert_eq!(b1["error_code"], "invalid_credentials");

    // Single use: a second /verify with the same token_hash is refused.
    let (second_status, second_body) = w.verify(&inv.token).await;
    println!(
        "second verify with the same token_hash: {second_status} {}",
        second_body["error_code"]
    );
    // Pinned observation (GoTrue v2.197.0, local): 403 otp_expired.
    assert_eq!(
        second_status, 403,
        "behaviour changed: a recovery token_hash is no longer single use"
    );
    assert_eq!(second_body["error_code"], "otp_expired");

    // An unknown / expired token is refused with otp_expired (clock-free check of the expiry path).
    let (status, body) = w.verify(&format!("{:0>64}", uniq())).await;
    assert!((400..500).contains(&status), "{status} {body}");
    assert_eq!(body["error_code"], "otp_expired", "{body}");
    w.cleanup(inv.id).await;
}

/// The "require current password" switch is off: a normal password sign-in session may change the password without
/// supplying the current one (the `/account` flow). With the switch on GoTrue v2.197.0 answers 400 current_password_required.
#[tokio::test]
async fn password_change_without_current_password() {
    let w = world().await;
    let inv = invite(&w, false).await;
    let old = set_password_via_link(&w, &inv.token).await;
    let (status, body) = w.sign_in(&inv.email, &old).await;
    assert_eq!(status, 200, "{body}");
    let access = body["access_token"].as_str().unwrap().to_owned();
    let new = format!("Pw-{}-xK9", uniq());
    let (status, body) = w.set_password(&access, &new).await;
    assert_eq!(
        status, 200,
        "password change from a password session must not need the current password: {body}"
    );
    assert_eq!(w.sign_in(&inv.email, &new).await.0, 200);
    assert!((400..500).contains(&w.sign_in(&inv.email, &old).await.0));
    w.cleanup(inv.id).await;
}

#[tokio::test]
async fn invite_creates_user_without_password_and_signup_off() {
    let w = world().await;
    let inv = invite(&w, false).await;
    // Sign-ups are off for strangers (GOTRUE_DISABLE_SIGNUP=true) while admin-created users work.
    let stranger = format!("stranger-{}@example.test", uniq());
    let (status, body) = w
        .call(
            reqwest::Method::POST,
            "/signup",
            Some(json!({"email": stranger, "password": format!("Pw-{}-xK9", uniq())})),
            None,
        )
        .await;
    assert!(
        (400..500).contains(&status),
        "signup must be refused: {status} {body}"
    );
    assert_eq!(w.auth_email_count(&stranger).await, 0);
    let (status, _) = w
        .call(reqwest::Method::POST, "/signup", Some(json!({})), None)
        .await;
    assert!(
        (400..500).contains(&status),
        "anonymous sign-in must be refused"
    );
    let (status, _) = w.sign_in(&inv.email, "no-password-yet-123456").await;
    assert!((400..500).contains(&status));
    w.cleanup(inv.id).await;
}

#[tokio::test]
async fn invite_without_follow_all() {
    let w = world().await;
    let inv = invite(&w, false).await;
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM app.follows WHERE follower_id = $1 OR followee_id = $1",
    )
    .bind(inv.id)
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(n, 0);
    assert_eq!(w.audit_count("user.invited", inv.id).await, 1);
    w.cleanup(inv.id).await;
}

#[tokio::test]
async fn invite_duplicate_display_name() {
    let w = world().await;
    let first = invite(&w, false).await;
    let email2 = format!("t-{}@example.test", uniq());
    let r = run(cli(&[
        "invite",
        "--email",
        &email2,
        "--display-name",
        &format!("  {}  ", first.name.to_uppercase()),
    ]));
    r.failed();
    assert_eq!(
        w.auth_email_count(&email2).await,
        0,
        "no login may be created before the refusal"
    );
    w.cleanup(first.id).await;
}

#[tokio::test]
async fn invite_duplicate_email() {
    let w = world().await;
    let first = invite(&w, false).await;
    let users_before: i64 = sqlx::query_scalar("SELECT count(*) FROM app.users")
        .fetch_one(&w.pool)
        .await
        .unwrap();
    let audit_before: i64 = sqlx::query_scalar("SELECT count(*) FROM app.audit_log")
        .fetch_one(&w.pool)
        .await
        .unwrap();
    let r = run(cli(&[
        "invite",
        "--email",
        &first.email,
        "--display-name",
        &format!("T {}", uniq()),
    ]));
    r.failed();
    assert!(r.stdout.is_empty(), "no link on failure: {}", r.stdout);
    let users_after: i64 = sqlx::query_scalar("SELECT count(*) FROM app.users")
        .fetch_one(&w.pool)
        .await
        .unwrap();
    let audit_after: i64 = sqlx::query_scalar("SELECT count(*) FROM app.audit_log")
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert_eq!((users_before, audit_before), (users_after, audit_after));
    w.cleanup(first.id).await;
}

#[tokio::test]
async fn invite_second_admin_refused() {
    let w = world().await;
    let (admin_id, created) = ensure_admin(&w).await;
    let email = format!("t-{}@example.test", uniq());
    let r = run(cli(&[
        "invite",
        "--email",
        &email,
        "--display-name",
        &format!("T {}", uniq()),
        "--admin",
    ]));
    r.failed();
    assert_eq!(
        w.auth_email_count(&email).await,
        0,
        "refused before any auth call"
    );
    if created {
        w.cleanup(admin_id).await;
    }
}

#[tokio::test]
async fn invite_rolls_back_on_db_failure() {
    let w = world().await;
    let t = uniq();
    let (email, name) = (format!("t-{t}@example.test"), format!("T {t}"));
    let r = run(cli(&[
        "invite",
        "--email",
        &email,
        "--display-name",
        &name,
        "--test-fail-after-auth",
    ]));
    r.failed();
    assert!(r.stdout.is_empty());
    assert_eq!(
        w.auth_email_count(&email).await,
        0,
        "the login must be removed again"
    );
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM app.users WHERE display_name = $1")
        .bind(&name)
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert_eq!(rows, 0);
}

// ---------------------------------------------------------------------------------------------------------------
// reset-link

#[tokio::test]
async fn reset_link_revokes_sessions_and_allows_reset() {
    let w = world().await;
    let inv = invite(&w, false).await;
    let old_password = set_password_via_link(&w, &inv.token).await;
    let (status, body) = w.sign_in(&inv.email, &old_password).await;
    assert_eq!(status, 200);
    let old_refresh = body["refresh_token"].as_str().unwrap().to_owned();
    assert!(w.session_count(inv.id).await >= 1);

    let r = run(cli(&["reset-link", "--email", &inv.email]));
    r.ok();
    let reset = parse_invite(&inv.email, &inv.name, &r);
    assert_eq!(w.session_count(inv.id).await, 0, "all sessions are gone");
    let (status, _) = w.refresh(&old_refresh).await;
    assert!(
        (400..500).contains(&status),
        "the old refresh token must fail"
    );

    // The new link sets a new password without the old one.
    let new_password = set_password_via_link(&w, &reset.token).await;
    assert_ne!(new_password, old_password);
    assert_eq!(w.sign_in(&inv.email, &new_password).await.0, 200);
    assert_eq!(w.audit_count("user.reset_link_issued", inv.id).await, 1);
    let leaked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM app.audit_log WHERE detail::text LIKE '%' || $1 || '%'",
    )
    .bind(&reset.token)
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(leaked, 0);
    assert!(!r.stderr.contains(&reset.token));
    w.cleanup(inv.id).await;
}

#[tokio::test]
async fn reset_link_refuses_inactive_or_unknown() {
    let w = world().await;
    let inv = invite(&w, false).await;
    run(cli(&["deactivate-user", "--id", &inv.id.to_string()])).ok();
    let before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM app.audit_log WHERE action = 'user.reset_link_issued'",
    )
    .fetch_one(&w.pool)
    .await
    .unwrap();
    for email in [inv.email.clone(), format!("nobody-{}@example.test", uniq())] {
        let r = run(cli(&["reset-link", "--email", &email]));
        r.failed();
        assert!(
            !r.stdout.contains("set-password"),
            "no link may be printed: {}",
            r.stdout
        );
    }
    let after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM app.audit_log WHERE action = 'user.reset_link_issued'",
    )
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(before, after);
    w.cleanup(inv.id).await;
}

#[tokio::test]
async fn reset_link_replaces_or_coexists() {
    let w = world().await;
    let inv = invite(&w, false).await;
    let _pw = set_password_via_link(&w, &inv.token).await;
    let first = parse_invite(
        &inv.email,
        &inv.name,
        &run(cli(&["reset-link", "--email", &inv.email])),
    );
    let second = parse_invite(
        &inv.email,
        &inv.name,
        &run(cli(&["reset-link", "--email", &inv.email])),
    );
    // Does the first link still redeem after the second was issued?
    let (first_after_second, _) = w.verify(&first.token).await;
    // And after the second was used?
    let (second_status, _) = w.verify(&second.token).await;
    assert_eq!(second_status, 200, "the newest link must redeem");
    let (second_again, _) = w.verify(&second.token).await;
    println!("older link after a newer one was issued: {first_after_second}; newest link used twice: {second_again}");
    // Pinned observation (GoTrue v2.197.0, local): issuing a newer link REPLACES the older one (403 otp_expired), and a
    // link is single use. The test fails loudly if either changes.
    assert_eq!(
        first_after_second, 403,
        "behaviour changed: an older recovery link still redeems after a newer one was issued"
    );
    assert_eq!(
        second_again, 403,
        "behaviour changed: a recovery link is reusable"
    );
    w.cleanup(inv.id).await;
}

// ---------------------------------------------------------------------------------------------------------------
// add-station

#[tokio::test]
async fn add_station_encrypts_and_roundtrips() {
    let w = world().await;
    let owner = invite(&w, false).await;
    let m = mac();
    let r = add_station(owner.id, &m, SENTINEL, &[]);
    r.ok();
    assert!(
        r.stderr.contains("key not validated"),
        "no AWN call without AWN_APPLICATION_KEY: {}",
        r.stderr
    );

    let (cid, ct, ek, ver): (Uuid, Vec<u8>, Vec<u8>, i16) = sqlx::query_as(
        "SELECT id, api_key_ciphertext, encapsulated_key, key_version FROM app.credentials WHERE owner_id = $1 AND status = 'ok'",
    )
    .bind(owner.id)
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(ver, env("CREDENTIALS_KEY_VERSION").parse::<i16>().unwrap());
    assert_eq!(ek.len(), 32);
    let enc = EncryptedCredential {
        ciphertext: ct,
        encapsulated_key: ek,
        key_version: ver,
    };
    let sk = parse_private_key(&env("CREDENTIALS_PRIVATE_KEY")).unwrap();
    assert_eq!(
        decrypt_credential(&sk, &enc, cid, owner.id)
            .unwrap()
            .expose_secret(),
        SENTINEL
    );
    assert!(
        decrypt_credential(&sk, &enc, cid, Uuid::new_v4()).is_err(),
        "a swapped owner_id must fail"
    );
    assert!(
        decrypt_credential(&sk, &enc, Uuid::new_v4(), owner.id).is_err(),
        "a swapped credential_id must fail"
    );

    // Audit rows and the station.
    assert_eq!(w.audit_count("credential.saved", cid).await, 1);
    let station: Uuid = sqlx::query_scalar("SELECT id FROM app.stations WHERE mac = $1")
        .bind(&m)
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert_eq!(w.audit_count("station.created", station).await, 1);
    w.cleanup(owner.id).await;
}

#[tokio::test]
async fn add_station_secret_not_leaked() {
    let w = world().await;
    let owner = invite(&w, false).await;
    let m = mac();
    let owner_s = owner.id.to_string();
    let mut c = cli(&add_station_args(&owner_s, &m, &[]));
    c.env("RUST_LOG", "trace")
        .write_stdin(format!("{SENTINEL}\n"));
    let r = run(c);
    r.ok();
    let ct: Vec<u8> =
        sqlx::query_scalar("SELECT api_key_ciphertext FROM app.credentials WHERE owner_id = $1")
            .bind(owner.id)
            .fetch_one(&w.pool)
            .await
            .unwrap();
    for forbidden in [
        SENTINEL.to_owned(),
        base64::engine::general_purpose::STANDARD.encode(&ct),
        hex::encode(&ct),
    ] {
        assert!(
            !r.stdout.contains(&forbidden) && !r.stderr.contains(&forbidden),
            "leaked {} bytes of secret",
            forbidden.len()
        );
    }
    w.cleanup(owner.id).await;
}

#[test]
fn add_station_rejects_api_key_flag() {
    let r = run(cli(&[
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
        "x",
    ]));
    assert_eq!(r.code, 2, "{}", r.stderr);
}

#[tokio::test]
async fn add_station_reuse_only_when_ok() {
    let w = world().await;
    let owner = invite(&w, false).await;
    add_station(owner.id, &mac(), SENTINEL, &[]).ok();
    // Neither flag with an existing credential: error.
    add_station(owner.id, &mac(), SENTINEL, &[]).failed();
    // --reuse-credential on an ok credential adds a station and leaves the row unchanged.
    let before: (Uuid, Vec<u8>, String) = sqlx::query_as(
        "SELECT id, api_key_ciphertext, status FROM app.credentials WHERE owner_id = $1",
    )
    .bind(owner.id)
    .fetch_one(&w.pool)
    .await
    .unwrap();
    let m2 = mac();
    let mut c = cli(&add_station_args(
        &owner.id.to_string(),
        &m2,
        &["--reuse-credential"],
    ));
    c.write_stdin("");
    run(c).ok();
    let after: (Uuid, Vec<u8>, String) = sqlx::query_as(
        "SELECT id, api_key_ciphertext, status FROM app.credentials WHERE owner_id = $1",
    )
    .bind(owner.id)
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(before, after);
    let cred_of_station: Uuid =
        sqlx::query_scalar("SELECT credential_id FROM app.stations WHERE mac = $1")
            .bind(&m2)
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_eq!(cred_of_station, before.0);
    // With status invalid it exits 1.
    sqlx::query("UPDATE app.credentials SET status = 'invalid' WHERE owner_id = $1")
        .bind(owner.id)
        .execute(&w.pool)
        .await
        .unwrap();
    let mut c = cli(&add_station_args(
        &owner.id.to_string(),
        &mac(),
        &["--reuse-credential"],
    ));
    c.write_stdin("");
    run(c).failed();
    w.cleanup(owner.id).await;
}

#[tokio::test]
async fn add_station_replace_key() {
    let w = world().await;
    let owner = invite(&w, false).await;
    let (m1, m2) = (mac(), mac());
    add_station(owner.id, &m1, SENTINEL, &[]).ok();
    let mut c = cli(&add_station_args(
        &owner.id.to_string(),
        &m2,
        &["--reuse-credential"],
    ));
    c.write_stdin("");
    run(c).ok();
    let old_id: Uuid = sqlx::query_scalar("SELECT id FROM app.credentials WHERE owner_id = $1")
        .bind(owner.id)
        .fetch_one(&w.pool)
        .await
        .unwrap();
    let old_ct: Vec<u8> =
        sqlx::query_scalar("SELECT api_key_ciphertext FROM app.credentials WHERE id = $1")
            .bind(old_id)
            .fetch_one(&w.pool)
            .await
            .unwrap();

    let new_key = "canary-secret-awnkey-4567";
    let m3 = mac();
    add_station(owner.id, &m3, new_key, &["--replace-key"]).ok();

    let (status, ct_after): (String, Vec<u8>) =
        sqlx::query_as("SELECT status, api_key_ciphertext FROM app.credentials WHERE id = $1")
            .bind(old_id)
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_eq!(status, "replaced");
    assert_eq!(ct_after, old_ct, "the old ciphertext is unchanged");
    let live: Vec<(Uuid, Vec<u8>, Vec<u8>, i16)> = sqlx::query_as("SELECT id, api_key_ciphertext, encapsulated_key, key_version FROM app.credentials WHERE owner_id = $1 AND status = 'ok'")
        .bind(owner.id).fetch_all(&w.pool).await.unwrap();
    assert_eq!(live.len(), 1, "exactly one live credential");
    let (new_id, ct, ek, ver) = live.into_iter().next().unwrap();
    assert_ne!(new_id, old_id);
    let sk = parse_private_key(&env("CREDENTIALS_PRIVATE_KEY")).unwrap();
    let key = decrypt_credential(
        &sk,
        &EncryptedCredential {
            ciphertext: ct,
            encapsulated_key: ek,
            key_version: ver,
        },
        new_id,
        owner.id,
    )
    .unwrap();
    assert_eq!(key.expose_secret(), new_key);
    // Every station of the owner points at the new row.
    let ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT credential_id FROM app.stations WHERE owner_id = $1")
            .bind(owner.id)
            .fetch_all(&w.pool)
            .await
            .unwrap();
    assert_eq!(ids.len(), 3);
    assert!(ids.iter().all(|c| *c == new_id));
    assert_eq!(w.audit_count("credential.saved", new_id).await, 1);
    assert_eq!(w.audit_count("credential.replaced", old_id).await, 1);
    w.cleanup(owner.id).await;
}

#[tokio::test]
async fn add_station_rounds_coordinates() {
    let w = world().await;
    let owner = invite(&w, false).await;
    let m = mac();
    add_station(owner.id, &m, SENTINEL, &[]).ok();
    let (lat, lon): (f64, f64) =
        sqlx::query_as("SELECT lat::float8, lon::float8 FROM app.stations WHERE mac = $1")
            .bind(&m)
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_eq!((lat, lon), (46.59, -112.04));
    w.cleanup(owner.id).await;
}

#[tokio::test]
async fn add_station_atomic() {
    let w = world().await;
    let (a, b) = (invite(&w, false).await, invite(&w, false).await);
    let m = mac();
    add_station(a.id, &m, SENTINEL, &[]).ok();
    // The same MAC for a new owner fails and leaves no credential behind.
    add_station(b.id, &m, SENTINEL, &[]).failed();
    let creds: i64 = sqlx::query_scalar("SELECT count(*) FROM app.credentials WHERE owner_id = $1")
        .bind(b.id)
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert_eq!(creds, 0);
    w.cleanup(a.id).await;
    w.cleanup(b.id).await;
}

#[tokio::test]
async fn add_station_reference_must_exist() {
    let w = world().await;
    let owner = invite(&w, false).await;
    // A reference id that is certainly absent (no seeded row has this id).
    let rid = "USW00088888";
    let _ = sqlx::query("DELETE FROM app.reference_stations WHERE id = $1")
        .bind(rid)
        .execute(&w.pool)
        .await;
    let m = mac();
    add_station(owner.id, &m, SENTINEL, &["--reference-station", rid]).failed();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM app.stations WHERE mac = $1")
        .bind(&m)
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert_eq!(n, 0, "nothing may be written");
    let creds: i64 = sqlx::query_scalar("SELECT count(*) FROM app.credentials WHERE owner_id = $1")
        .bind(owner.id)
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert_eq!(creds, 0);

    run(cli(&[
        "add-reference-station",
        "--id",
        rid,
        "--name",
        "Test ref",
        "--lat",
        "40.45",
        "--lon",
        "-105.01",
        "--timezone",
        "America/Denver",
        "--std-offset-minutes",
        "-420",
        "--elevation-ft",
        "5015",
    ]))
    .ok();
    add_station(owner.id, &m, SENTINEL, &["--reference-station", rid]).ok();
    let station: (Uuid, Option<String>) =
        sqlx::query_as("SELECT id, reference_station_id FROM app.stations WHERE mac = $1")
            .bind(&m)
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_eq!(station.1.as_deref(), Some(rid));
    assert_eq!(w.audit_count("station.baseline_set", station.0).await, 1);
    w.cleanup(owner.id).await;
    let _ = sqlx::query("DELETE FROM app.reference_stations WHERE id = $1")
        .bind(rid)
        .execute(&w.pool)
        .await;
}

// ---------------------------------------------------------------------------------------------------------------
// deactivate / delete / list / restore

#[tokio::test]
async fn deactivate_user() {
    let w = world().await;
    let inv = invite(&w, false).await;
    let password = set_password_via_link(&w, &inv.token).await;
    let m = mac();
    add_station(inv.id, &m, SENTINEL, &[]).ok();
    let station: Uuid = sqlx::query_scalar("SELECT id FROM app.stations WHERE mac = $1")
        .bind(&m)
        .fetch_one(&w.pool)
        .await
        .unwrap();
    let (status, body) = w.sign_in(&inv.email, &password).await;
    assert_eq!(status, 200);
    let refresh = body["refresh_token"].as_str().unwrap().to_owned();
    assert!(w.session_count(inv.id).await >= 1);
    let can_view_before: bool = sqlx::query_scalar("SELECT app.can_view($1, $2)")
        .bind(inv.id)
        .bind(station)
        .fetch_one(&w.su)
        .await
        .unwrap();
    assert!(can_view_before);

    let r = run(cli(&["deactivate-user", "--id", &inv.id.to_string()]));
    r.ok();
    let (active, at): (bool, Option<chrono::DateTime<chrono::Utc>>) =
        sqlx::query_as("SELECT is_active, deactivated_at FROM app.users WHERE id = $1")
            .bind(inv.id)
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert!(!active && at.is_some());
    assert_eq!(w.session_count(inv.id).await, 0);
    assert!(
        (400..500).contains(&w.refresh(&refresh).await.0),
        "the old refresh token must fail"
    );
    assert!(
        (400..500).contains(&w.sign_in(&inv.email, &password).await.0),
        "a banned login cannot sign in"
    );
    let banned: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT banned_until FROM auth.users WHERE id = $1")
            .bind(inv.id)
            .fetch_one(&w.su)
            .await
            .unwrap();
    assert!(banned.is_some(), "ban applied when the service key is set");
    let can_view_after: bool = sqlx::query_scalar("SELECT app.can_view($1, $2)")
        .bind(inv.id)
        .bind(station)
        .fetch_one(&w.su)
        .await
        .unwrap();
    assert!(!can_view_after);
    assert_eq!(w.audit_count("user.deactivated", inv.id).await, 1);

    // Re-running exits 0 and reports it; no second audit row.
    let again = run(cli(&["deactivate-user", "--id", &inv.id.to_string()]));
    again.ok();
    assert!(again.stdout.contains("already inactive"));
    assert_eq!(w.audit_count("user.deactivated", inv.id).await, 1);

    // Without the service key the ban is skipped with a warning only.
    let other = invite(&w, false).await;
    let mut c = Command::new(env!("CARGO_BIN_EXE_admin"));
    c.env_clear()
        .env("APP_ENV", "local")
        .env("DATABASE_URL", env("TEST_MIGRATOR_DATABASE_URL"))
        .args(["deactivate-user", "--display-name", other.name.as_str()]);
    let r = run(c);
    r.ok();
    assert!(r.stderr.contains("warning"), "{}", r.stderr);
    let banned: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT banned_until FROM auth.users WHERE id = $1")
            .bind(other.id)
            .fetch_one(&w.su)
            .await
            .unwrap();
    assert!(banned.is_none());
    w.cleanup(inv.id).await;
    w.cleanup(other.id).await;
}

#[tokio::test]
async fn deactivate_admin_refused() {
    let w = world().await;
    let (admin_id, created) = ensure_admin(&w).await;
    run(cli(&["deactivate-user", "--id", &admin_id.to_string()])).failed();
    let active: bool = sqlx::query_scalar("SELECT is_active FROM app.users WHERE id = $1")
        .bind(admin_id)
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert!(active);
    if created {
        w.cleanup(admin_id).await;
    }
}

#[tokio::test]
async fn delete_user_cascades() {
    let w = world().await;
    let inv = invite(&w, true).await;
    let m = mac();
    add_station(inv.id, &m, SENTINEL, &[]).ok();
    // An active user cannot be deleted.
    run(cli(&[
        "delete-user",
        "--id",
        &inv.id.to_string(),
        "--confirm-email",
        &inv.email,
    ]))
    .failed();
    run(cli(&["deactivate-user", "--id", &inv.id.to_string()])).ok();
    // A wrong confirmation exits 1.
    run(cli(&[
        "delete-user",
        "--id",
        &inv.id.to_string(),
        "--confirm-email",
        "someone-else@example.test",
    ]))
    .failed();
    assert!(w.app_user_exists(inv.id).await);

    let audit_before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM app.audit_log WHERE subject_id = $1 OR actor_id = $1",
    )
    .bind(inv.id)
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert!(audit_before > 0);
    run(cli(&[
        "delete-user",
        "--id",
        &inv.id.to_string(),
        "--confirm-email",
        &inv.email,
    ]))
    .ok();
    assert!(!w.app_user_exists(inv.id).await);
    assert!(
        !w.auth_user_exists(inv.id).await,
        "the login is deleted too"
    );
    for table in ["credentials", "stations", "connect_attempts"] {
        let owner_col = if table == "connect_attempts" {
            "user_id"
        } else {
            "owner_id"
        };
        let n: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM app.{table} WHERE {owner_col} = $1"
        ))
        .bind(inv.id)
        .fetch_one(&w.pool)
        .await
        .unwrap();
        assert_eq!(n, 0, "{table}");
    }
    let follows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM app.follows WHERE follower_id = $1 OR followee_id = $1",
    )
    .bind(inv.id)
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(follows, 0);
    let audit_after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM app.audit_log WHERE subject_id = $1 OR actor_id = $1",
    )
    .bind(inv.id)
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert!(audit_after >= audit_before, "audit_log rows are kept");
    assert_eq!(w.audit_count("user.deleted", inv.id).await, 1);
}

#[tokio::test]
async fn list_users_reports_orphans() {
    let w = world().await;
    let inv = invite(&w, false).await;
    let r = run(cli(&["list-users"]));
    let line = r
        .stdout
        .lines()
        .find(|l| l.starts_with(&inv.id.to_string()))
        .expect("the new user is listed")
        .to_owned();
    let fields: Vec<&str> = line.split('\t').collect();
    assert_eq!(fields.len(), 6, "{line}");
    assert_eq!(
        (fields[1], fields[2], fields[3], fields[4], fields[5]),
        (
            inv.name.as_str(),
            inv.email.as_str(),
            "active",
            "-",
            "auth_ok"
        )
    );

    // An auth user without an app row is reported as auth_only and makes the exit code 1.
    let orphan_email = format!("t-orphan-{}@example.test", uniq());
    let (status, body) = w
        .call(
            reqwest::Method::POST,
            "/admin/users",
            Some(json!({"email": orphan_email, "email_confirm": true})),
            Some(&w.jwt),
        )
        .await;
    assert!((200..300).contains(&status), "{body}");
    let orphan: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    let r = run(cli(&["list-users"]));
    r.failed();
    assert!(
        r.stdout
            .lines()
            .any(|l| l == format!("auth_only\t{orphan}")),
        "{}",
        r.stdout
    );
    let _ = w
        .call(
            reqwest::Method::DELETE,
            &format!("/admin/users/{orphan}"),
            None,
            Some(&w.jwt),
        )
        .await;
    w.cleanup(inv.id).await;
}

#[tokio::test]
async fn restore_auth_users_keeps_ids() {
    let w = world().await;
    let (a, b) = (invite(&w, false).await, invite(&w, false).await);

    // The backup's CSV, exactly as app_auth.backup_users() produces it.
    let rows: Vec<(Uuid, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT id, email, created_at FROM app_auth.backup_users() WHERE id = ANY($1)",
    )
    .bind(vec![a.id, b.id])
    .fetch_all(&w.su)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("auth-users.csv");
    let mut csv = String::from("id,email,created_at\n");
    for (id, email, at) in &rows {
        csv.push_str(&format!("{id},{email},{}\n", at.to_rfc3339()));
    }
    // A row with no app.users row is reported and never created.
    let ghost = Uuid::new_v4();
    let ghost_email = format!("t-ghost-{}@example.test", uniq());
    csv.push_str(&format!("{ghost},{ghost_email},2026-01-01T00:00:00Z\n"));
    std::fs::write(&file, csv).unwrap();

    // Lose both logins; keep the app rows.
    for u in [&a, &b] {
        let (status, _) = w
            .call(
                reqwest::Method::DELETE,
                &format!("/admin/users/{}", u.id),
                None,
                Some(&w.jwt),
            )
            .await;
        assert!((200..300).contains(&status));
        assert!(!w.auth_user_exists(u.id).await);
    }

    // --dry-run changes nothing.
    let dry = run(cli(&[
        "restore-auth-users",
        "--file",
        file.to_str().unwrap(),
        "--dry-run",
    ]));
    dry.ok();
    assert!(!w.auth_user_exists(a.id).await && !w.auth_user_exists(b.id).await);

    let r = run(cli(&[
        "restore-auth-users",
        "--file",
        file.to_str().unwrap(),
    ]));
    r.ok();
    for u in [&a, &b] {
        assert!(
            w.auth_user_exists(u.id).await,
            "login restored with its original id"
        );
        assert_eq!(w.audit_count("user.auth_restored", u.id).await, 1);
        let email: String = sqlx::query_scalar("SELECT email FROM auth.users WHERE id = $1")
            .bind(u.id)
            .fetch_one(&w.su)
            .await
            .unwrap();
        assert_eq!(email, u.email);
    }
    assert_eq!(
        w.auth_email_count(&ghost_email).await,
        0,
        "the ghost row is not created"
    );
    assert!(
        r.stdout.contains(&ghost.to_string()),
        "the ghost id is reported"
    );

    // Emails never appear in the output.
    for text in [&r.stdout, &r.stderr, &dry.stdout, &dry.stderr] {
        assert!(
            !text.contains(&a.email) && !text.contains(&b.email) && !text.contains(&ghost_email)
        );
    }

    // A second run creates nothing and exits 0.
    let again = run(cli(&[
        "restore-auth-users",
        "--file",
        file.to_str().unwrap(),
    ]));
    again.ok();
    assert_eq!(w.audit_count("user.auth_restored", a.id).await, 1);
    assert!(again.stdout.contains("0 created"), "{}", again.stdout);
    w.cleanup(a.id).await;
    w.cleanup(b.id).await;
}

// ---------------------------------------------------------------------------------------------------------------
// seed-dev

async fn seed_counts(w: &World) -> Vec<i64> {
    let mut out = Vec::new();
    for table in [
        "users",
        "follows",
        "credentials",
        "stations",
        "reference_stations",
        "audit_log",
    ] {
        out.push(
            sqlx::query_scalar(&format!("SELECT count(*) FROM app.{table}"))
                .fetch_one(&w.pool)
                .await
                .unwrap(),
        );
    }
    out
}

fn seed_cmd(app_env: &str) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_admin"));
    c.env_clear()
        .env("APP_ENV", app_env)
        .env("DATABASE_URL", env("TEST_MIGRATOR_DATABASE_URL"))
        .env("SUPABASE_URL", env("SUPABASE_URL"))
        .env("SUPABASE_SERVICE_ROLE_KEY", service_role_jwt())
        .env("CREDENTIALS_PUBLIC_KEY", env("CREDENTIALS_PUBLIC_KEY"))
        .env("CREDENTIALS_KEY_VERSION", env("CREDENTIALS_KEY_VERSION"))
        .env("LOCAL_DEMO_PASSWORD", env("LOCAL_DEMO_PASSWORD"))
        .env("SEED_DIR", env("SEED_DIR"))
        .arg("seed-dev");
    c
}

#[tokio::test]
async fn seed_dev_idempotent() {
    let w = world().await;
    // production is refused before anything is touched
    run(seed_cmd("production")).failed();

    let first = run(seed_cmd("local"));
    first.ok();
    let after_first = seed_counts(&w).await;

    // Six seed users, one inactive, with logins; three stations with the fixture key; the documented follow states.
    let names = ["Rodney", "Alex", "Sam", "Pat", "Dana", "Casey"];
    let rows: Vec<(String, bool, bool)> = sqlx::query_as("SELECT display_name, is_active, is_admin FROM app.users WHERE display_name = ANY($1) ORDER BY display_name")
        .bind(names.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        .fetch_all(&w.pool)
        .await
        .unwrap();
    assert_eq!(rows.len(), 6);
    assert_eq!(
        rows.iter()
            .filter(|r| !r.1)
            .map(|r| r.0.as_str())
            .collect::<Vec<_>>(),
        vec!["Casey"]
    );
    assert_eq!(
        rows.iter()
            .filter(|r| r.2)
            .map(|r| r.0.as_str())
            .collect::<Vec<_>>(),
        vec!["Rodney"]
    );
    let stations: Vec<(String, String)> = sqlx::query_as("SELECT s.mac, u.display_name FROM app.stations s JOIN app.users u ON u.id = s.owner_id WHERE s.mac LIKE 'AA:BB:CC:00:00:%' ORDER BY s.mac").fetch_all(&w.pool).await.unwrap();
    assert_eq!(
        stations,
        vec![
            ("AA:BB:CC:00:00:01".to_owned(), "Rodney".to_owned()),
            ("AA:BB:CC:00:00:02".to_owned(), "Alex".to_owned()),
            ("AA:BB:CC:00:00:03".to_owned(), "Sam".to_owned()),
        ]
    );
    let refs: HashSet<String> = sqlx::query_scalar(
        "SELECT reference_station_id FROM app.stations WHERE mac LIKE 'AA:BB:CC:00:00:%'",
    )
    .fetch_all(&w.pool)
    .await
    .unwrap()
    .into_iter()
    .collect();
    assert_eq!(
        refs,
        ["USW00099001", "USW00099002", "USW00099003"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    );
    let states: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT f.follower_id::text, f.status, ee.display_name FROM app.follows f \
         JOIN app.users er ON er.id = f.follower_id JOIN app.users ee ON ee.id = f.followee_id \
         WHERE er.display_name = 'Dana' ORDER BY ee.display_name",
    )
    .fetch_all(&w.pool)
    .await
    .unwrap();
    assert_eq!(
        states
            .iter()
            .map(|s| (s.2.as_str(), s.1.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("Alex", "pending"),
            ("Rodney", "revoked"),
            ("Sam", "declined")
        ]
    );
    let pat_follows: i64 = sqlx::query_scalar("SELECT count(*) FROM app.follows f JOIN app.users u ON u.id IN (f.follower_id, f.followee_id) WHERE u.display_name = 'Pat'").fetch_one(&w.pool).await.unwrap();
    assert_eq!(pat_follows, 0);
    // The family trio is mutually approved: 6 rows.
    let mesh: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM app.follows f JOIN app.users a ON a.id = f.follower_id JOIN app.users b ON b.id = f.followee_id \
         WHERE a.display_name IN ('Rodney','Alex','Sam') AND b.display_name IN ('Rodney','Alex','Sam') AND f.status = 'approved'",
    )
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(mesh, 6);
    // Seed users sign in with the demo password.
    let (status, _) = w
        .sign_in("alex@example.test", &env("LOCAL_DEMO_PASSWORD"))
        .await;
    assert_eq!(status, 200);
    // The stored key is the fixture key.
    let (cid, ct, ek, ver, owner): (Uuid, Vec<u8>, Vec<u8>, i16, Uuid) = sqlx::query_as(
        "SELECT c.id, c.api_key_ciphertext, c.encapsulated_key, c.key_version, c.owner_id FROM app.credentials c JOIN app.users u ON u.id = c.owner_id WHERE u.display_name = 'Alex'",
    )
    .fetch_one(&w.pool)
    .await
    .unwrap();
    let sk = parse_private_key(&env("CREDENTIALS_PRIVATE_KEY")).unwrap();
    assert_eq!(
        decrypt_credential(
            &sk,
            &EncryptedCredential {
                ciphertext: ct,
                encapsulated_key: ek,
                key_version: ver
            },
            cid,
            owner
        )
        .unwrap()
        .expose_secret(),
        "fixture-poller-key"
    );

    // Second run: exit 0, changes nothing.
    let second = run(seed_cmd("local"));
    second.ok();
    assert!(second.stdout.contains("nothing to do"), "{}", second.stdout);
    assert_eq!(seed_counts(&w).await, after_first);
    // The seed never prints a password or a link.
    assert!(
        !first.stdout.contains(&env("LOCAL_DEMO_PASSWORD"))
            && !first.stderr.contains(&env("LOCAL_DEMO_PASSWORD"))
    );
    assert!(!first.stdout.contains("set-password"));
}

#[test]
fn digest_helper_is_sha256() {
    // Guards the fingerprint helpers used elsewhere in this file's conventions: lower-case hex of SHA-256.
    assert_eq!(
        hex::encode(Sha256::digest(b"abc")),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
