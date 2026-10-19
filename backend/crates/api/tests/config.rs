//! API-specific configuration checks (02 4.1, 7.1) on top of `core::config`; the shared rules are core's tests.

use api::config::{ApiConfig, KeySource};
use famweather_core::config::{load_from, Binary, Config, Vars};
use jsonwebtoken::Algorithm;

const DEMO_PUB: &str = "RxUrKamgbeYZkmLE3mZtzRHQP3MBBU9kZKkedBhteQk=";
const SECRET: &str = "a-long-enough-local-secret-0123456789-xyz";

fn local(extra: &[(&str, &str)]) -> Vars {
    let mut v = vec![
        ("APP_ENV", "local"),
        (
            "DATABASE_URL",
            "postgres://app_api:local-only-api@db:5432/postgres?sslmode=disable",
        ),
        ("SUPABASE_URL", "http://localhost:8000"),
        ("JWT_HS256_SECRET", SECRET),
        ("AWN_MODE", "replay"),
        ("FIXTURES_DIR", "/fixtures"),
        ("CREDENTIALS_PUBLIC_KEY", DEMO_PUB),
        ("CREDENTIALS_KEY_VERSION", "1"),
    ];
    v.retain(|(k, _)| !extra.iter().any(|(e, _)| e == k));
    v.extend_from_slice(extra);
    Vars::from_pairs(v.into_iter().filter(|(_, val)| !val.is_empty()))
}

fn cfg(vars: &Vars) -> Result<ApiConfig, String> {
    match load_from(Binary::Api, vars) {
        Ok(Config::Api(env)) => ApiConfig::from_api_env(env).map_err(|e| e.to_string()),
        Ok(_) => Err("wrong kind".into()),
        Err(e) => Err(e.to_string()),
    }
}

#[test]
fn local_hs256_pairs_with_the_exact_local_url() {
    let c = cfg(&local(&[])).unwrap();
    assert_eq!(c.allowed_algs, [Algorithm::HS256]);
    assert_eq!(c.issuer, "http://localhost:8000/auth/v1");
    assert!(matches!(c.key_source, KeySource::Hs256(_)));
    assert!(!format!("{c:?}").contains(SECRET));

    for url in ["http://caddy", "http://127.0.0.1:8000", "http://auth:9999"] {
        let e = cfg(&local(&[("SUPABASE_URL", url)])).unwrap_err();
        assert!(e.contains("SUPABASE_URL"), "{url}: {e}");
    }
}

#[test]
fn local_needs_exactly_one_key_source() {
    let both = local(&[(
        "SUPABASE_JWKS_URL",
        "http://localhost:8000/auth/v1/.well-known/jwks.json",
    )]);
    assert!(cfg(&both).unwrap_err().contains("exactly one"));
    let neither = local(&[("JWT_HS256_SECRET", "")]);
    assert!(cfg(&neither).is_err());
    let jwks_only = Vars::from_pairs([
        ("APP_ENV", "local"),
        (
            "DATABASE_URL",
            "postgres://app_api:local-only-api@db:5432/postgres?sslmode=disable",
        ),
        ("SUPABASE_URL", "http://localhost:8000"),
        (
            "SUPABASE_JWKS_URL",
            "http://localhost:8000/auth/v1/.well-known/jwks.json",
        ),
        ("AWN_MODE", "replay"),
        ("FIXTURES_DIR", "/fixtures"),
        ("CREDENTIALS_PUBLIC_KEY", DEMO_PUB),
        ("CREDENTIALS_KEY_VERSION", "1"),
    ]);
    let c = cfg(&jwks_only).unwrap();
    assert_eq!(c.allowed_algs, [Algorithm::ES256, Algorithm::RS256]);
    let short = local(&[("JWT_HS256_SECRET", "too-short-secret")]);
    assert!(cfg(&short).is_err());
}

fn production(extra: &[(&str, &str)], ca: &str) -> Vars {
    let (pk, _) = famweather_core::crypto::generate_keypair();
    let url = format!("postgres://app_api:Zq7-prod-like-password-91@db.example.invalid:5432/postgres?sslmode=verify-full&sslrootcert={ca}");
    let pk = pk.to_base64();
    let mut v: Vec<(&str, String)> = vec![
        ("APP_ENV", "production".into()),
        ("DATABASE_URL", url),
        ("SUPABASE_URL", "https://example.invalid".into()),
        (
            "SUPABASE_JWKS_URL",
            "https://example.invalid/auth/v1/.well-known/jwks.json".into(),
        ),
        ("AWN_MODE", "live".into()),
        (
            "AWN_APPLICATION_KEY",
            "prod-looking-key-0123456789abcdef".into(),
        ),
        ("CREDENTIALS_PUBLIC_KEY", pk),
        ("CREDENTIALS_KEY_VERSION", "1".into()),
    ];
    v.retain(|(k, _)| !extra.iter().any(|(e, _)| e == k));
    v.extend(extra.iter().map(|(k, val)| (*k, (*val).to_owned())));
    Vars::from_pairs(v.into_iter().filter(|(_, val)| !val.is_empty()))
}

#[test]
fn production_rules() {
    let dir = tempfile::tempdir().unwrap();
    let ca = dir.path().join("ca.crt");
    std::fs::write(&ca, "x").unwrap();
    let ca = ca.to_str().unwrap();
    let c = cfg(&production(&[], ca)).unwrap();
    assert_eq!(c.allowed_algs, [Algorithm::ES256, Algorithm::RS256]);
    assert_eq!(c.issuer, "https://example.invalid/auth/v1");
    assert!(c.is_production());

    // JWKS is required; the issuer and JWKS must be https on public hosts; an HS256 secret is refused.
    assert!(cfg(&production(&[("SUPABASE_JWKS_URL", "")], ca))
        .unwrap_err()
        .contains("SUPABASE_JWKS_URL"));
    assert!(cfg(&production(
        &[("SUPABASE_URL", "http://example.invalid")],
        ca
    ))
    .is_err());
    assert!(cfg(&production(
        &[("SUPABASE_JWKS_URL", "http://example.invalid/jwks")],
        ca
    ))
    .is_err());
    for host in [
        "https://localhost",
        "https://127.0.0.1",
        "https://thing.test",
        "https://internal",
    ] {
        assert!(
            cfg(&production(&[("SUPABASE_URL", host)], ca)).is_err(),
            "{host}"
        );
    }
    let e = cfg(&production(&[("JWT_HS256_SECRET", SECRET)], ca)).unwrap_err();
    assert!(e.contains("JWT_HS256_SECRET") && !e.contains(SECRET), "{e}");
    // Shared refusals surface unchanged and name the variable only.
    let e = cfg(&production(&[("CREDENTIALS_PUBLIC_KEY", DEMO_PUB)], ca)).unwrap_err();
    assert_eq!(e, "refusing CREDENTIALS_PUBLIC_KEY: demo value");
    let e = cfg(&production(&[], "/nonexistent/ca.crt")).unwrap_err();
    assert!(e.contains("DATABASE_URL"), "{e}");
    assert!(cfg(&production(&[("AWN_MODE", "replay")], ca))
        .unwrap_err()
        .contains("AWN_MODE"));
}
