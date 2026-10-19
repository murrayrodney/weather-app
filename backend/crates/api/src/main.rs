//! `api` binary: `api` (serve) and `api healthcheck`.

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use api::app::build_router;
use api::auth::jwks::{HttpJwksFetcher, JwksCache};
use api::auth::token::TokenVerifier;
use api::baseline::BaselineCache;
use api::config::{ApiConfig, KeySource};
use api::limits::{spawn_eviction, UserLimiters};
use api::openmeteo::{LiveOpenMeteo, OpenMeteo, OpenMeteoApi, ReplayOpenMeteo};
use api::state::{AppState, GateConfig};
use famweather_core::acis::{AcisApi, LiveAcis, ReplayAcis};
use famweather_core::awn::{AwnApi, LiveAwn, ReplayAwn};
use famweather_core::clock::{Clock, SystemClock};
use famweather_core::config::AwnMode;
use famweather_core::db::{self, ConnectHook, PoolSpec};
use famweather_core::redact;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const CALL_TIMEOUT: Duration = Duration::from_secs(5);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(25);

#[tokio::main]
async fn main() -> ExitCode {
    let arg = std::env::args().nth(1);
    match arg.as_deref() {
        Some("healthcheck") => healthcheck().await,
        None | Some("serve") => match serve().await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {}", redact::scrub(&format!("{e:#}")));
                ExitCode::FAILURE
            }
        },
        Some(_) => {
            eprintln!("usage: api [healthcheck]");
            ExitCode::from(2)
        }
    }
}

/// GET http://127.0.0.1:<port>/api/health; exit 0 on 200. No shell, curl or config load.
async fn healthcheck() -> ExitCode {
    let port = std::env::var("API_BIND_ADDR")
        .ok()
        .and_then(|a| a.parse::<std::net::SocketAddr>().ok())
        .map_or(8080, |a| a.port());
    let probe = async {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .ok()?;
        s.write_all(b"GET /api/health HTTP/1.0\r\nHost: localhost\r\n\r\n")
            .await
            .ok()?;
        let mut buf = Vec::new();
        s.take(512).read_to_end(&mut buf).await.ok()?;
        let head = String::from_utf8_lossy(&buf);
        head.starts_with("HTTP/1.0 200")
            .then_some(())
            .or_else(|| head.starts_with("HTTP/1.1 200").then_some(()))
    };
    match tokio::time::timeout(Duration::from_secs(3), probe).await {
        Ok(Some(())) => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    }
}

async fn session_setup(conn: &mut sqlx::PgConnection) -> sqlx::Result<()> {
    for stmt in [
        "SET statement_timeout = '10s'",
        "SET idle_in_transaction_session_timeout = '15s'",
        "SET TIME ZONE 'UTC'",
    ] {
        sqlx::query(stmt).execute(&mut *conn).await?;
    }
    Ok(())
}

fn connect_hook() -> ConnectHook {
    Arc::new(|conn| Box::pin(session_setup(conn)))
}

async fn serve() -> Result<()> {
    // First, before any configuration is read: a panic logs file:line and a fixed message, never the payload (4.11).
    redact::install_panic_hook();
    let cfg = ApiConfig::from_env()?;
    redact::init_tracing(cfg.is_production()).map_err(|e| anyhow!("{e}"))?;
    for s in [
        &cfg.env.awn_application_key,
        &cfg.env.jwt_hs256_secret,
        &cfg.env.netlify_proxy_secret,
    ]
    .into_iter()
    .flatten()
    {
        redact::register(s);
    }
    if cfg.is_production() && cfg.env.netlify_proxy_secret.is_none() {
        tracing::warn!("NETLIFY_PROXY_SECRET is not set: the signed-proxy check is disabled");
    }
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);

    let spec = PoolSpec {
        max: 5,
        min: 0,
        acquire_timeout: Duration::from_secs(5),
        idle_timeout: Duration::from_secs(300),
        max_lifetime: Duration::from_secs(1800),
        test_before_acquire: true,
        after_connect: Some(connect_hook()),
    };
    let pool = db::pool(&cfg.env.database, spec)
        .await
        .map_err(|e| anyhow!("{e}"))?;
    api::auth::startup_probe(&pool)
        .await
        .map_err(|e| anyhow!("{e}"))?;

    let jwt = match &cfg.key_source {
        KeySource::Hs256(secret) => TokenVerifier::hs256(cfg.issuer.clone(), secret),
        KeySource::Jwks(url) => {
            let fetcher = HttpJwksFetcher::new(url.clone(), cfg.is_production())
                .map_err(|_| anyhow!("could not build the JWKS client"))?;
            let cache = Arc::new(JwksCache::new(Arc::new(fetcher), clock.clone()));
            cache
                .load_initial()
                .await
                .map_err(|_| anyhow!("could not load the JWKS at startup"))?;
            TokenVerifier::jwks(cfg.issuer.clone(), cache)
        }
    };

    let (awn, acis, om): (Arc<dyn AwnApi>, Arc<dyn AcisApi>, Arc<dyn OpenMeteoApi>) =
        match cfg.env.awn_mode {
            AwnMode::Live => {
                let key =
                    cfg.env.awn_application_key.clone().ok_or_else(|| {
                        anyhow!("AWN_APPLICATION_KEY is required with AWN_MODE=live")
                    })?;
                (
                    Arc::new(LiveAwn::new(key, CALL_TIMEOUT).map_err(|e| anyhow!("{e}"))?),
                    Arc::new(LiveAcis::new(CALL_TIMEOUT).map_err(|e| anyhow!("{e}"))?),
                    Arc::new(LiveOpenMeteo::new().map_err(|e| anyhow!("{e}"))?),
                )
            }
            AwnMode::Replay => (
                Arc::new(
                    ReplayAwn::new(cfg.env.fixtures_dir.clone(), clock.clone())
                        .map_err(|e| anyhow!("{e}"))?,
                ),
                Arc::new(
                    ReplayAcis::new(cfg.env.fixtures_dir.clone()).map_err(|e| anyhow!("{e}"))?,
                ),
                Arc::new(ReplayOpenMeteo::new(
                    cfg.env.fixtures_dir.clone(),
                    clock.clone(),
                )),
            ),
        };

    let limiters = Arc::new(UserLimiters::new());
    spawn_eviction(limiters.clone());
    let bind = cfg.env.bind_addr;
    let state = AppState {
        db: pool,
        jwt: Arc::new(jwt),
        awn,
        limiters,
        acis,
        baseline_cache: Arc::new(BaselineCache::new()),
        cred_pubkey: cfg.env.credentials_public_key.clone(),
        openmeteo: Arc::new(OpenMeteo::new(om, clock.clone())),
        clock,
        gates: GateConfig::default(),
        cfg: Arc::new(cfg),
    };
    let app = build_router(state, axum::Router::new());
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!("api listening");
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let server = axum::serve(listener, app).with_graceful_shutdown(async {
        let _ = rx.await;
    });
    let mut server = Box::pin(std::future::IntoFuture::into_future(server));
    tokio::select! {
        r = &mut server => r?,
        () = shutdown_signal() => {
            let _ = tx.send(());
            if tokio::time::timeout(SHUTDOWN_GRACE, &mut server).await.is_err() {
                tracing::warn!("graceful shutdown timed out");
            }
        }
    }
    Ok(())
}

async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
    tokio::select! {
        _ = term.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
}
