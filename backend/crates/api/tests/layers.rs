//! Layers, envelope and logging behaviour (02 task 3, 4, 7).

mod common;

use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use api::app::build_router;
use api::config::{ApiConfig, KeySource};
use api::state::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::Router;
use common::*;
use http_body_util::BodyExt;
use secrecy::SecretString;
use sqlx::PgPool;
use tower::ServiceExt;
use tracing_subscriber::fmt::MakeWriter;
use uuid::Uuid;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Capture;
    fn make_writer(&'a self) -> Capture {
        self.clone()
    }
}

fn logs() -> &'static Capture {
    static LOGS: OnceLock<Capture> = OnceLock::new();
    LOGS.get_or_init(|| {
        let c = Capture::default();
        tracing_subscriber::fmt()
            .with_writer(c.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::DEBUG)
            .init();
        famweather_core::redact::install_panic_hook();
        c
    })
}

fn log_text() -> String {
    String::from_utf8_lossy(&logs().0.lock().unwrap()).into_owned()
}

fn extras() -> Router<AppState> {
    Router::new()
        .route(
            "/boom",
            get(|| async {
                if true {
                    panic!("canary-secret-panic-payload-77");
                }
                "unreachable"
            }),
        )
        .route(
            "/slow",
            get(|| async {
                tokio::time::sleep(Duration::from_secs(3)).await;
                "slow"
            }),
        )
}

fn with_cfg(mut state: AppState, f: impl FnOnce(&mut ApiConfig)) -> AppState {
    let mut cfg = ApiConfig::for_test(
        ISSUER,
        KeySource::Hs256(SecretString::from(HS_SECRET.to_owned())),
    );
    f(&mut cfg);
    state.cfg = Arc::new(cfg);
    state
}

fn assert_envelope(r: &Resp, status: u16, code: &str) {
    assert_eq!(r.status.as_u16(), status, "{}", r.text());
    assert_eq!(r.code(), code);
    assert_eq!(r.headers["cache-control"], "no-store");
    assert_eq!(r.headers["x-content-type-options"], "nosniff");
}

#[sqlx::test(migrations = false)]
async fn panics_404_413_and_request_ids(db: PgPool) {
    logs();
    let w = World::new(db).await;
    let router = build_router(w.state.clone(), extras());
    let tok = w.tok("owner");

    let r = send(&router, "GET", "/api/v1/boom", Some(&tok), None).await;
    assert_envelope(&r, 500, "internal");
    let text = log_text();
    assert!(text.contains("panic"), "the fixed panic message is logged");
    assert!(
        text.contains("src/") || text.contains("tests/"),
        "file:line is logged"
    );
    assert!(
        !text.contains("canary-secret-panic-payload-77"),
        "the payload is never logged"
    );

    assert_envelope(
        &send(&router, "GET", "/api/v1/nope", Some(&tok), None).await,
        404,
        "not_found",
    );
    assert_envelope(
        &send(&router, "GET", "/nope", None, None).await,
        404,
        "not_found",
    );

    // Client-supplied request ids are replaced.
    let req = Request::builder()
        .uri("/api/health")
        .header("x-request-id", "client-chosen")
        .body(Body::empty())
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    let id = resp.headers()["x-request-id"].to_str().unwrap().to_owned();
    assert_ne!(id, "client-chosen");
    assert!(Uuid::parse_str(&id).is_ok());

    // 413 with a Content-Length over the limit.
    let big = vec![b'a'; 17 * 1024];
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/follows")
        .header("content-type", "application/json")
        .header("content-length", big.len())
        .body(Body::from(big))
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    let (parts, body) = resp.into_parts();
    let r = Resp {
        status: parts.status,
        headers: parts.headers,
        body: body.collect().await.unwrap().to_bytes().to_vec(),
    };
    assert_envelope(&r, 413, "payload_too_large");

    // 413 from a CHUNKED body without Content-Length (through the Json extractor).
    let chunks = futures::stream::iter((0..17).map(|_| Ok::<_, std::io::Error>(vec![b' '; 1024])));
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/follows")
        .header("authorization", format!("Bearer {tok}"))
        .header("content-type", "application/json")
        .body(Body::from_stream(chunks))
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    let (parts, body) = resp.into_parts();
    let r = Resp {
        status: parts.status,
        headers: parts.headers,
        body: body.collect().await.unwrap().to_bytes().to_vec(),
    };
    assert_envelope(&r, 413, "payload_too_large");
}

#[sqlx::test(migrations = false)]
async fn wrong_methods_and_unknown_routes_use_the_envelope(db: PgPool) {
    let w = World::new(db).await;
    let tok = w.tok("owner");
    let r = send(
        &w.router,
        "PUT",
        "/api/v1/me",
        Some(&tok),
        Some(serde_json::json!({})),
    )
    .await;
    assert_envelope(&r, 405, "method_not_allowed");
    assert!(r.headers.contains_key("allow"));
    assert_envelope(
        &send(&w.router, "POST", "/api/health", None, None).await,
        405,
        "method_not_allowed",
    );
    // Without a token the guard may answer first; either way the body is the envelope.
    let r = send(&w.router, "DELETE", "/api/v1/stations", None, None).await;
    assert!(matches!(r.status.as_u16(), 401 | 405), "{}", r.status);
    assert_eq!(r.headers["cache-control"], "no-store");
    assert!(!r.code().is_empty());
    for (m, p) in [
        ("GET", "/api/v1/nope"),
        ("GET", "/api/nope"),
        ("POST", "/"),
        ("GET", "/api/v1"),
    ] {
        assert_envelope(
            &send(&w.router, m, p, Some(&tok), None).await,
            404,
            "not_found",
        );
    }
}

#[sqlx::test(migrations = false)]
async fn slow_requests_time_out_with_the_envelope(db: PgPool) {
    let w = World::new(db).await;
    let state = with_cfg(w.state.clone(), |c| {
        c.request_timeout = Duration::from_millis(200)
    });
    let router = build_router(state, extras());
    let r = send(&router, "GET", "/api/v1/slow", Some(&w.tok("owner")), None).await;
    assert_envelope(&r, 408, "request_timeout");
}

#[sqlx::test(migrations = false)]
async fn the_concurrency_cap_is_global(db: PgPool) {
    let w = World::new(db).await;
    let router = build_router(w.state.clone(), extras());
    // 64 users so the per-user limiter is not what answers.
    let mut handles = Vec::new();
    for i in 0..64 {
        let id = w.add_user(&format!("Load {i}"), true).await;
        let s = w.add_session(id).await;
        let tok = mint(claims(id, s), HS_SECRET);
        let r = router.clone();
        handles.push(tokio::spawn(async move {
            send(&r, "GET", "/api/v1/slow", Some(&tok), None).await
        }));
    }
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let r = send(&router, "GET", "/api/health", None, None).await;
    assert_envelope(&r, 503, "overloaded");
    assert_eq!(r.headers["retry-after"], "1");
    for h in handles {
        assert_eq!(h.await.unwrap().status, StatusCode::OK);
    }
    assert_eq!(
        send(&router, "GET", "/api/health", None, None).await.status,
        StatusCode::OK
    );
}

#[sqlx::test(migrations = false)]
async fn health_is_bounded_and_reports_a_dead_database(db: PgPool) {
    let w = World::new(db).await;
    let r = send(&w.router, "GET", "/api/health", None, None).await;
    assert_eq!(r.json(), serde_json::json!({ "status": "ok" }));
    let mut handles = Vec::new();
    for _ in 0..1000 {
        let r = w.router.clone();
        handles.push(tokio::spawn(async move {
            send(&r, "GET", "/api/health", None, None).await.status
        }));
    }
    for h in handles {
        let s = h.await.unwrap();
        assert!(
            s == StatusCode::OK || s == StatusCode::SERVICE_UNAVAILABLE,
            "{s}"
        );
    }
    assert!(w.api.size() <= 4);

    let dead = build_router(unreachable_state(), Router::new());
    let r = send(&dead, "GET", "/api/health", None, None).await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(r.json(), serde_json::json!({ "status": "unavailable" }));
}

#[sqlx::test(migrations = false)]
async fn the_signed_proxy_check_is_optional_and_exempts_health(db: PgPool) {
    use jsonwebtoken::{encode, EncodingKey, Header};
    let w = World::with(db, |b| {
        b.proxy_secret = Some("proxy-secret-0123456789-abcdef")
    })
    .await;
    let sign = |iss: &str, exp_offset: i64, secret: &str| {
        encode(
            &Header::default(),
            &serde_json::json!({
                "iss": iss, "exp": unix_now() + exp_offset,
                "deploy_context": "deploy-preview", "netlify_id": "x", "site_url": "https://example.test"
            }),
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap()
    };
    let call = |path: &'static str, sig: Option<String>| {
        let router = w.router.clone();
        let tok = w.tok("owner");
        async move {
            let mut b = Request::builder()
                .uri(path)
                .header("authorization", format!("Bearer {tok}"));
            if let Some(s) = sig {
                b = b.header("x-nf-sign", s);
            }
            router
                .oneshot(b.body(Body::empty()).unwrap())
                .await
                .unwrap()
                .status()
        }
    };
    let good = sign("netlify", 600, "proxy-secret-0123456789-abcdef");
    assert_eq!(
        call("/api/v1/me", Some(good)).await,
        StatusCode::OK,
        "deploy_context is not checked"
    );
    assert_eq!(call("/api/v1/me", None).await, StatusCode::FORBIDDEN);
    assert_eq!(
        call(
            "/api/v1/me",
            Some(sign("netlify", 600, "wrong-secret-wrong-secret-0000"))
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            "/api/v1/me",
            Some(sign("netlify", -600, "proxy-secret-0123456789-abcdef"))
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            "/api/v1/me",
            Some(sign("someone", 600, "proxy-secret-0123456789-abcdef"))
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(call("/api/health", None).await, StatusCode::OK);

    // Unset secret: no check.
    let w2 = common::Build::new(w.api.clone()).state();
    let open = build_router(w2, Router::new());
    assert_eq!(
        send(&open, "GET", "/api/v1/me", Some(&w.tok("owner")), None)
            .await
            .status,
        StatusCode::OK
    );
}
