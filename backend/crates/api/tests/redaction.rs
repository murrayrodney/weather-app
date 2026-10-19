//! Log and response redaction (02 7.4, S7): sentinel secrets go through the whole app; none may appear in logs or bodies.

mod common;

use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use api::app::build_router;
use axum::routing::get;
use axum::Router;
use common::*;
use famweather_core::awn::{AwnApi, LiveAwn};
use secrecy::SecretString;
use serde_json::json;
use sqlx::PgPool;
use tracing_subscriber::fmt::MakeWriter;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const USER_KEY: &str = "SENTINEL-USER-KEY-7f3a9c";
const APP_KEY: &str = "SENTINEL-APP-KEY-1d8e44";

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
    static L: OnceLock<Capture> = OnceLock::new();
    L.get_or_init(|| {
        let c = Capture::default();
        tracing_subscriber::fmt()
            .with_writer(c.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .init();
        famweather_core::redact::install_panic_hook();
        c
    })
}

#[sqlx::test(migrations = false)]
async fn nothing_secret_reaches_logs_or_bodies(db: PgPool) {
    logs();
    let server = MockServer::start().await;
    let devices = json!([{ "macAddress": "AA:BB:CC:00:09:09", "lastData": { "tz": "America/Denver" },
        "info": { "name": "Mock", "coords": { "coords": { "lat": 40.4, "lon": -105.1 }, "elevation": 5000.0 } } }]);
    Mock::given(method("GET"))
        .and(path("/v1/devices"))
        .respond_with(ResponseTemplate::new(200).set_body_json(devices))
        .mount(&server)
        .await;
    let base = url::Url::parse(&format!("{}/v1/", server.uri())).unwrap();
    let live: Arc<dyn AwnApi> = Arc::new(
        LiveAwn::with_base(
            base,
            SecretString::from(APP_KEY.to_owned()),
            Duration::from_secs(1),
        )
        .unwrap(),
    );
    let w = World::with(db, |b| {
        b.awn = Some(live);
        b.gates.awn_app_spacing = Duration::from_millis(5);
    })
    .await;
    let boom = Router::new().route(
        "/boom",
        get(|| async {
            if true {
                panic!("{USER_KEY} {APP_KEY}")
            }
            "x"
        }),
    );
    let router = build_router(w.state.clone(), boom);
    let tok = w.tok("owner");

    let mut bodies = Vec::new();
    // Success, upstream failures, oversize body, bad tokens, a query string, a panic, a DB-level failure.
    bodies.push(
        send(
            &router,
            "POST",
            "/api/v1/connect/validate",
            Some(&tok),
            Some(json!({ "api_key": USER_KEY })),
        )
        .await,
    );
    bodies.push(
        send(
            &router,
            "POST",
            "/api/v1/connect/save",
            Some(&tok),
            Some(json!({ "api_key": USER_KEY, "devices": [{ "mac": "AA:BB:CC:00:09:09" }] })),
        )
        .await,
    );
    server.reset().await;
    for code in [401u16, 429, 500] {
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(code)
                    .set_body_string(format!("{USER_KEY} {APP_KEY} apiKey={USER_KEY}")),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        bodies.push(
            send(
                &router,
                "POST",
                "/api/v1/connect/validate",
                Some(&tok),
                Some(json!({ "api_key": USER_KEY })),
            )
            .await,
        );
    }
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{ not json ".to_owned()))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    bodies.push(
        send(
            &router,
            "POST",
            "/api/v1/connect/validate",
            Some(&tok),
            Some(json!({ "api_key": USER_KEY })),
        )
        .await,
    );
    bodies.push(
        send(
            &router,
            "POST",
            "/api/v1/connect/validate",
            Some(&tok),
            Some(json!({ "api_key": "x".repeat(20_000) })),
        )
        .await,
    );
    bodies.push(
        send(
            &router,
            "GET",
            "/api/v1/users/search?q=canary-secret-query",
            Some(&tok),
            None,
        )
        .await,
    );
    bodies.push(send(&router, "GET", "/api/v1/boom", Some(&tok), None).await);
    let bad = mint(
        claims(uuid::Uuid::new_v4(), uuid::Uuid::new_v4()),
        "another-secret-another-secret-123456",
    );
    bodies.push(send(&router, "GET", "/api/v1/me", Some(&bad), None).await);
    // A database error: the dead-pool state turns every guard query into a failure.
    let dead = build_router(unreachable_state(), Router::new());
    bodies.push(send(&dead, "GET", "/api/v1/me", Some(&tok), None).await);

    let all_logs = String::from_utf8_lossy(&logs().0.lock().unwrap()).into_owned();
    let mut haystacks = vec![all_logs];
    haystacks.extend(bodies.iter().map(|r| r.text()));
    haystacks.extend(bodies.iter().map(|r| format!("{:?}", r.headers)));
    let audit: String =
        sqlx::query_scalar("SELECT coalesce(string_agg(detail::text, ' '), '') FROM app.audit_log")
            .fetch_one(&w.db)
            .await
            .unwrap();
    haystacks.push(audit);
    for h in &haystacks {
        for needle in [
            USER_KEY,
            APP_KEY,
            tok.as_str(),
            HS_SECRET,
            "apiKey=",
            "applicationKey=",
            "Bearer ",
            "canary-secret-query",
        ] {
            assert!(!h.contains(needle), "found {needle:?}");
        }
    }
    assert!(haystacks[0].contains("request"), "the access log exists");
}
