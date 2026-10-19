//! Token verification and the JWKS cache (02 7.1, tasks 5 and 6). No test here needs a database: rejections run
//! against a pool that cannot connect, which proves no query was issued.

mod common;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use api::app::build_router;
use api::auth::jwks::{FetchError, JwksCache, JwksFetcher, STALE_CAP, TTL};
use api::auth::token::{AuthError, TokenVerifier};
use async_trait::async_trait;
use axum::http::StatusCode;
use axum::Router;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use chrono::Duration;
use common::*;
use famweather_core::clock::FixedClock;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use p256::ecdsa::SigningKey;
use p256::pkcs8::{EncodePrivateKey, LineEnding};
use rsa::pkcs1::EncodeRsaPrivateKey;
use rsa::traits::PublicKeyParts;
use secrecy::SecretString;
use serde_json::{json, Value};
use uuid::Uuid;

struct EcKey {
    kid: String,
    enc: EncodingKey,
    jwk: Value,
}

fn ec_key(kid: &str) -> EcKey {
    let sk = SigningKey::random(&mut rand_core::OsRng);
    let pem = sk.to_pkcs8_pem(LineEnding::LF).unwrap();
    let pt = sk.verifying_key().to_encoded_point(false);
    EcKey {
        kid: kid.to_owned(),
        enc: EncodingKey::from_ec_pem(pem.as_bytes()).unwrap(),
        jwk: json!({ "kty": "EC", "crv": "P-256", "kid": kid, "use": "sig", "alg": "ES256",
            "x": B64.encode(pt.x().unwrap()), "y": B64.encode(pt.y().unwrap()) }),
    }
}

struct FakeFetcher {
    body: Mutex<Vec<u8>>,
    fail: AtomicBool,
    calls: AtomicUsize,
}

impl FakeFetcher {
    fn new(keys: &[&Value]) -> Arc<Self> {
        let f = Arc::new(Self {
            body: Mutex::new(Vec::new()),
            fail: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
        });
        f.set(keys);
        f
    }
    fn set(&self, keys: &[&Value]) {
        *self.body.lock().unwrap() = serde_json::to_vec(&json!({ "keys": keys })).unwrap();
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl JwksFetcher for FakeFetcher {
    async fn fetch(&self) -> Result<Vec<u8>, FetchError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            return Err(FetchError);
        }
        Ok(self.body.lock().unwrap().clone())
    }
}

struct Rig {
    fetcher: Arc<FakeFetcher>,
    clock: Arc<FixedClock>,
    cache: Arc<JwksCache>,
    verifier: TokenVerifier,
}

async fn rig(keys: &[&Value]) -> Rig {
    let fetcher = FakeFetcher::new(keys);
    let clock = Arc::new(FixedClock::new(now()));
    let cache = Arc::new(JwksCache::new(fetcher.clone(), clock.clone()));
    cache.load_initial().await.unwrap();
    let verifier = TokenVerifier::jwks(ISSUER.to_owned(), cache.clone());
    Rig {
        fetcher,
        clock,
        cache,
        verifier,
    }
}

fn es_token(k: &EcKey, claims: Value) -> String {
    let mut h = Header::new(Algorithm::ES256);
    h.kid = Some(k.kid.clone());
    encode(&h, &claims, &k.enc).unwrap()
}

fn ok_claims() -> Value {
    claims(Uuid::new_v4(), Uuid::new_v4())
}

fn bearer(t: &str) -> String {
    format!("Bearer {t}")
}

fn without(mut v: Value, key: &str) -> Value {
    v.as_object_mut().unwrap().remove(key);
    v
}

fn with(mut v: Value, key: &str, val: Value) -> Value {
    v[key] = val;
    v
}

#[tokio::test]
async fn asymmetric_tokens_are_accepted_and_every_bad_one_is_not() {
    let k = ec_key("k1");
    let r = rig(&[&k.jwk]).await;
    let good = ok_claims();
    let v = r
        .verifier
        .verify(&bearer(&es_token(&k, good.clone())))
        .await
        .unwrap();
    assert_eq!(v.user_id.to_string(), good["sub"].as_str().unwrap());
    // Scheme is case-insensitive.
    assert!(r
        .verifier
        .verify(&format!("bEaReR {}", es_token(&k, good.clone())))
        .await
        .is_ok());

    let now = unix_now();
    let bad: Vec<(&str, Value)> = vec![
        ("anon role", with(good.clone(), "role", json!("anon"))),
        (
            "service role",
            with(good.clone(), "role", json!("service_role")),
        ),
        (
            "anon without sub",
            without(with(good.clone(), "role", json!("anon")), "sub"),
        ),
        ("expired", with(good.clone(), "exp", json!(now - 60))),
        ("not yet valid", with(good.clone(), "nbf", json!(now + 600))),
        ("missing exp", without(good.clone(), "exp")),
        (
            "wrong iss",
            with(good.clone(), "iss", json!("http://evil.test/auth/v1")),
        ),
        ("aud anon", with(good.clone(), "aud", json!("anon"))),
        (
            "aud authenticated2",
            with(good.clone(), "aud", json!("authenticated2")),
        ),
        ("missing session_id", without(good.clone(), "session_id")),
        (
            "non-uuid session_id",
            with(good.clone(), "session_id", json!("nope")),
        ),
        ("non-uuid sub", with(good.clone(), "sub", json!("nope"))),
        (
            "anonymous user",
            with(good.clone(), "is_anonymous", json!(true)),
        ),
    ];
    for (name, c) in bad {
        let e = r
            .verifier
            .verify(&bearer(&es_token(&k, c)))
            .await
            .unwrap_err();
        assert!(matches!(e, AuthError::Invalid(_)), "{name}: {e:?}");
    }
}

#[tokio::test]
async fn header_and_key_attacks_fail_closed() {
    let k = ec_key("k1");
    let r = rig(&[&k.jwk]).await;
    let c = ok_claims();
    let tok = es_token(&k, c.clone());
    let (h, rest) = tok.split_once('.').unwrap();
    let _ = h;
    // alg none, with a plausible payload and an empty signature.
    let none = format!(
        "{}.{}.",
        B64.encode(br#"{"alg":"none","typ":"JWT","kid":"k1"}"#),
        B64.encode(c.to_string())
    );
    assert!(matches!(
        r.verifier.verify(&bearer(&none)).await,
        Err(AuthError::Invalid(_))
    ));
    // HS256 signed with the public key bytes as the secret (algorithm confusion) is refused before key lookup.
    let mut hh = Header::new(Algorithm::HS256);
    hh.kid = Some("k1".into());
    let hs = encode(
        &hh,
        &c,
        &EncodingKey::from_secret(k.jwk["x"].as_str().unwrap().as_bytes()),
    )
    .unwrap();
    assert!(matches!(
        r.verifier.verify(&bearer(&hs)).await,
        Err(AuthError::Invalid(_))
    ));
    // Signed by another key but with the legitimate kid.
    let attacker = ec_key("k1");
    assert!(matches!(
        r.verifier
            .verify(&bearer(&es_token(&attacker, c.clone())))
            .await,
        Err(AuthError::Invalid(_))
    ));
    // No kid.
    let nokid = encode(&Header::new(Algorithm::ES256), &c, &attacker.enc).unwrap();
    assert!(matches!(
        r.verifier.verify(&bearer(&nokid)).await,
        Err(AuthError::Invalid(_))
    ));
    // Garbage, size and shape.
    for t in [
        "Bearer".to_owned(),
        "Basic abc".to_owned(),
        bearer("a.b"),
        bearer("a.b.c.d"),
        bearer("!!!.@@@.###"),
        bearer(&format!("{}.{}.{}", "a".repeat(9000), "b", "c")),
        bearer(&format!("x{rest}")),
    ] {
        assert!(
            matches!(r.verifier.verify(&t).await, Err(AuthError::Invalid(_))),
            "{}",
            &t[..t.len().min(20)]
        );
    }
}

#[tokio::test]
async fn jwk_constraints_are_enforced() {
    let k = ec_key("k1");
    let mut wrong_alg = k.jwk.clone();
    wrong_alg["kid"] = json!("k2");
    wrong_alg["alg"] = json!("RS256");
    let mut wrong_use = k.jwk.clone();
    wrong_use["kid"] = json!("k3");
    wrong_use["use"] = json!("enc");
    let rsa_jwk = json!({ "kty": "RSA", "kid": "k4", "use": "sig", "alg": "RS256", "n": "AQAB", "e": "AQAB" });
    let r = rig(&[&k.jwk, &wrong_alg, &wrong_use, &rsa_jwk]).await;
    let c = ok_claims();
    for kid in ["k2", "k3", "k4"] {
        let mut h = Header::new(Algorithm::ES256);
        h.kid = Some(kid.into());
        let t = encode(&h, &c, &k.enc).unwrap();
        assert!(
            matches!(
                r.verifier.verify(&bearer(&t)).await,
                Err(AuthError::Invalid(_))
            ),
            "{kid}"
        );
    }
    assert!(r.verifier.verify(&bearer(&es_token(&k, c))).await.is_ok());
}

#[tokio::test]
async fn rs256_is_accepted() {
    let sk = rsa::RsaPrivateKey::new(&mut rand_core::OsRng, 2048).unwrap();
    let pem = sk.to_pkcs1_pem(LineEnding::LF).unwrap();
    let jwk = json!({ "kty": "RSA", "kid": "r1", "use": "sig", "alg": "RS256",
        "n": B64.encode(sk.n().to_bytes_be()), "e": B64.encode(sk.e().to_bytes_be()) });
    let r = rig(&[&jwk]).await;
    let mut h = Header::new(Algorithm::RS256);
    h.kid = Some("r1".into());
    let t = encode(
        &h,
        &ok_claims(),
        &EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap(),
    )
    .unwrap();
    assert!(r.verifier.verify(&bearer(&t)).await.is_ok());
}

#[tokio::test]
async fn jwks_startup_ttl_unknown_kid_and_cooldown() {
    let k1 = ec_key("k1");
    // Startup fails fast on an unreachable endpoint and on an empty key set.
    let f = FakeFetcher::new(&[&k1.jwk]);
    f.fail.store(true, Ordering::SeqCst);
    let c = JwksCache::new(f.clone(), Arc::new(FixedClock::new(now())));
    assert!(c.load_initial().await.is_err());
    let empty = FakeFetcher::new(&[]);
    assert!(JwksCache::new(empty, Arc::new(FixedClock::new(now())))
        .load_initial()
        .await
        .is_err());

    let r = rig(&[&k1.jwk]).await;
    assert_eq!(r.fetcher.calls(), 1);
    // Known kid: no fetch. TTL expiry: one lazy refresh.
    assert!(r.cache.get("k1").await.unwrap().is_some());
    assert_eq!(r.fetcher.calls(), 1);
    r.clock.advance(TTL + Duration::seconds(1));
    assert!(r.cache.get("k1").await.unwrap().is_some());
    assert_eq!(r.fetcher.calls(), 2);

    // Unknown kid: exactly one refetch, then none within 60 s, even for another unknown kid.
    assert!(r.cache.get("nope").await.unwrap().is_none());
    assert_eq!(r.fetcher.calls(), 3);
    assert!(r.cache.get("nope2").await.unwrap().is_none());
    assert_eq!(r.fetcher.calls(), 3);

    // A rotated-in key is accepted after the single refetch (cooldown elapsed).
    let k2 = ec_key("k2");
    r.fetcher.set(&[&k1.jwk, &k2.jwk]);
    r.clock.advance(Duration::seconds(61));
    assert!(r.cache.get("k2").await.unwrap().is_some());
    assert_eq!(r.fetcher.calls(), 4);

    // Key removal takes effect on the next refetch.
    r.fetcher.set(&[&k2.jwk]);
    r.clock.advance(Duration::seconds(61));
    assert!(r.cache.get("zzz").await.unwrap().is_none());
    assert!(r.cache.get("k1").await.unwrap().is_none());
}

#[tokio::test]
async fn jwks_outages_are_unavailable_not_invalid() {
    let k1 = ec_key("k1");
    let r = rig(&[&k1.jwk]).await;
    r.fetcher.fail.store(true, Ordering::SeqCst);
    // Unknown kid while the refetch fails -> unavailable; the cooldown keeps saying so.
    assert!(r.cache.get("nope").await.is_err());
    let calls = r.fetcher.calls();
    assert!(r.cache.get("nope").await.is_err());
    assert_eq!(r.fetcher.calls(), calls);
    // Stale keys keep working up to the cap, then the cache is unusable.
    r.clock.advance(TTL + Duration::seconds(5));
    assert!(r.cache.get("k1").await.unwrap().is_some());
    r.clock.advance(STALE_CAP);
    assert!(r.cache.get("k1").await.is_err());
    let tok = es_token(&k1, ok_claims());
    assert_eq!(
        r.verifier.verify(&bearer(&tok)).await,
        Err(AuthError::Unavailable)
    );
}

#[tokio::test]
async fn http_status_mapping_and_no_database_for_rejections() {
    // HS256 (local) mode against a pool that cannot connect.
    let router = build_router(unreachable_state(), Router::new());
    let user = Uuid::new_v4();
    let session = Uuid::new_v4();
    let now = unix_now();
    let cases: Vec<(&str, Option<String>)> = vec![
        ("no header", None),
        (
            "anon key",
            Some(mint(
                json!({ "iss": ISSUER, "aud": "authenticated", "role": "anon", "exp": now + 600 }),
                HS_SECRET,
            )),
        ),
        (
            "service role",
            Some(mint(
                with(claims(user, session), "role", json!("service_role")),
                HS_SECRET,
            )),
        ),
        (
            "wrong secret",
            Some(mint(
                claims(user, session),
                "another-secret-another-secret-123456",
            )),
        ),
        (
            "expired",
            Some(mint(
                with(claims(user, session), "exp", json!(now - 60)),
                HS_SECRET,
            )),
        ),
        (
            "wrong iss",
            Some(mint(
                with(claims(user, session), "iss", json!("http://x/auth/v1")),
                HS_SECRET,
            )),
        ),
        (
            "wrong aud",
            Some(mint(
                with(claims(user, session), "aud", json!("anon")),
                HS_SECRET,
            )),
        ),
        ("es256 in hs256 mode", {
            let k = ec_key("k");
            Some(es_token(&k, claims(user, session)))
        }),
        (
            "alg none",
            Some(format!(
                "{}.{}.",
                B64.encode(br#"{"alg":"none"}"#),
                B64.encode(claims(user, session).to_string())
            )),
        ),
    ];
    for (name, tok) in cases {
        let r = send(&router, "GET", "/api/v1/me", tok.as_deref(), None).await;
        assert_eq!(
            (r.status, r.code().as_str()),
            (StatusCode::UNAUTHORIZED, "unauthorized"),
            "{name}"
        );
    }
    // A valid token passes verification and reaches the (dead) database: 503, never 401.
    let ok = mint(claims(user, session), HS_SECRET);
    let r = send(&router, "GET", "/api/v1/me", Some(&ok), None).await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::SERVICE_UNAVAILABLE, "auth_unavailable")
    );
}

#[tokio::test]
async fn limiter_runs_before_the_database_and_is_per_user() {
    use governor::clock::FakeRelativeClock;
    let clock = FakeRelativeClock::default();
    let mut b = Build::new(
        sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_millis(50))
            .connect_lazy("postgres://x@127.0.0.1:1/x")
            .unwrap(),
    );
    b.limiters = Some(Arc::new(api::limits::UserLimiters::with_clock(
        clock.clone(),
    )));
    let router = build_router(b.state(), Router::new());
    let (a, bb) = (Uuid::new_v4(), Uuid::new_v4());
    let ta = mint(claims(a, Uuid::new_v4()), HS_SECRET);
    let tb = mint(claims(bb, Uuid::new_v4()), HS_SECRET);
    // The first 40 reach the dead database (503); invalid tokens in between consume nobody's budget.
    for i in 0..40 {
        if i == 10 {
            let bad = send(
                &router,
                "GET",
                "/api/v1/me",
                Some("garbage.garbage.garbage"),
                None,
            )
            .await;
            assert_eq!(bad.status, StatusCode::UNAUTHORIZED);
        }
        let r = send(&router, "GET", "/api/v1/me", Some(&ta), None).await;
        assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE, "request {i}");
    }
    let r = send(&router, "GET", "/api/v1/me", Some(&ta), None).await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::TOO_MANY_REQUESTS, "rate_limited")
    );
    assert!(
        r.headers["retry-after"]
            .to_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            >= 1
    );
    // Another user is unaffected.
    assert_eq!(
        send(&router, "GET", "/api/v1/me", Some(&tb), None)
            .await
            .status,
        StatusCode::SERVICE_UNAVAILABLE
    );
    // The bucket refills with time.
    clock.advance(std::time::Duration::from_secs(1));
    assert_eq!(
        send(&router, "GET", "/api/v1/me", Some(&ta), None)
            .await
            .status,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn embedded_jwk_and_jku_headers_are_ignored() {
    let k = ec_key("k1");
    let r = rig(&[&k.jwk]).await;
    let attacker = ec_key("evil");
    let embedded: jsonwebtoken::jwk::Jwk = serde_json::from_value(attacker.jwk.clone()).unwrap();
    for kid in ["k1", "evil"] {
        // Signed by the attacker, announcing the attacker's own key in `jwk` and a key URL in `jku`/`x5u`.
        let mut h = Header::new(Algorithm::ES256);
        h.kid = Some(kid.into());
        h.jwk = Some(embedded.clone());
        h.jku = Some("http://evil.test/jwks.json".into());
        h.x5u = Some("http://evil.test/cert.pem".into());
        let t = encode(&h, &ok_claims(), &attacker.enc).unwrap();
        assert!(
            matches!(
                r.verifier.verify(&bearer(&t)).await,
                Err(AuthError::Invalid(_))
            ),
            "kid {kid}"
        );
    }
    // The embedded key was never trusted, and no URL was ever fetched: only the known key set is used.
    assert!(
        r.fetcher.calls() <= 2,
        "at most the single forced refetch for the unknown kid"
    );
    assert!(r
        .verifier
        .verify(&bearer(&es_token(&k, ok_claims())))
        .await
        .is_ok());
}

#[tokio::test]
async fn the_five_second_leeway_boundary() {
    let k = ec_key("k1");
    let r = rig(&[&k.jwk]).await;
    let now = unix_now();
    let tok = |c: Value| bearer(&es_token(&k, c));
    // exp just inside / outside the leeway; nbf just inside / outside it.
    let inside_exp = with(ok_claims(), "exp", json!(now - 2));
    let outside_exp = with(ok_claims(), "exp", json!(now - 9));
    let inside_nbf = with(ok_claims(), "nbf", json!(now + 2));
    let outside_nbf = with(ok_claims(), "nbf", json!(now + 9));
    assert!(r.verifier.verify(&tok(inside_exp)).await.is_ok());
    assert!(r.verifier.verify(&tok(inside_nbf)).await.is_ok());
    assert!(matches!(
        r.verifier.verify(&tok(outside_exp)).await,
        Err(AuthError::Invalid(_))
    ));
    assert!(matches!(
        r.verifier.verify(&tok(outside_nbf)).await,
        Err(AuthError::Invalid(_))
    ));
}

#[test]
fn hs256_secret_type_is_not_debug_printable() {
    let s = SecretString::from("super-secret-value-0123456789-abcdef".to_owned());
    let v = TokenVerifier::hs256(ISSUER.to_owned(), &s);
    assert!(!format!("{v:?}").contains("super-secret"));
}
