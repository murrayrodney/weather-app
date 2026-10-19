//! Shared test harness: a migrated database, personas with sessions and HS256 tokens, the real router.
//! The API under test runs on a pool whose connections `SET ROLE app_api`, so every handler query obeys the real grants.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use api::app::build_router;
use api::auth::token::TokenVerifier;
use api::baseline::BaselineCache;
use api::config::{ApiConfig, KeySource};
use api::limits::UserLimiters;
use api::openmeteo::{OpenMeteo, OpenMeteoApi, ReplayOpenMeteo};
use api::state::{AppState, GateConfig};
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::Router;
use chrono::{DateTime, TimeZone, Utc};
use famweather_core::acis::{AcisApi, ReplayAcis};
use famweather_core::awn::{AwnApi, ReplayAwn};
use famweather_core::clock::{Clock, FixedClock};
use famweather_core::crypto::{generate_keypair, CredentialPrivateKey, CredentialPublicKey};
use famweather_core::testdb::{self, role_hook};
use http_body_util::BodyExt;
use jsonwebtoken::{encode, EncodingKey, Header};
use secrecy::SecretString;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

pub const HS_SECRET: &str = "test-hs256-secret-0123456789-abcdef-xyz";
pub const ISSUER: &str = "http://localhost:8000/auth/v1";

pub fn fixtures() -> PathBuf {
    std::env::var_os("FIXTURES_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures"))
}

pub fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap()
}

pub fn unix_now() -> i64 {
    Utc::now().timestamp()
}

#[derive(Clone)]
pub struct Persona {
    pub id: Uuid,
    pub session: Uuid,
    pub name: &'static str,
    pub token: String,
}

pub fn mint(claims: Value, secret: &str) -> String {
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .unwrap()
}

pub fn claims(user: Uuid, session: Uuid) -> Value {
    json!({
        "sub": user, "iss": ISSUER, "aud": "authenticated", "role": "authenticated",
        "session_id": session, "iat": unix_now(), "exp": unix_now() + 3600,
    })
}

pub struct Resp {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|_| panic!("not JSON: {}", String::from_utf8_lossy(&self.body)))
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    pub fn code(&self) -> String {
        self.json()["error"]["code"]
            .as_str()
            .unwrap_or("")
            .to_owned()
    }
}

pub async fn send(
    router: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> Resp {
    let mut b = Request::builder().method(method).uri(path);
    if let Some(t) = token {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    let body = match body {
        Some(v) => {
            b = b.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&v).unwrap())
        }
        None => Body::empty(),
    };
    let resp = router.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let (parts, body) = resp.into_parts();
    Resp {
        status: parts.status,
        headers: parts.headers,
        body: body.collect().await.unwrap().to_bytes().to_vec(),
    }
}

/// Everything configurable about the app under test.
pub struct Build {
    pub db: PgPool,
    pub clock: Arc<FixedClock>,
    pub awn: Option<Arc<dyn AwnApi>>,
    pub acis: Option<Arc<dyn AcisApi>>,
    pub om: Option<Arc<dyn OpenMeteoApi>>,
    pub limiters: Option<Arc<UserLimiters>>,
    pub gates: GateConfig,
    pub pubkey: Option<CredentialPublicKey>,
    pub proxy_secret: Option<&'static str>,
    /// (concurrent calls, calls per minute) for the Open-Meteo front.
    pub om_limits: Option<(usize, u32)>,
    /// HS256 secret the API verifies with (default: the harness secret).
    pub hs_secret: Option<String>,
}

impl Build {
    pub fn new(db: PgPool) -> Self {
        Self {
            db,
            clock: Arc::new(FixedClock::new(now())),
            awn: None,
            acis: None,
            om: None,
            limiters: None,
            gates: GateConfig::default(),
            pubkey: None,
            proxy_secret: None,
            om_limits: None,
            hs_secret: None,
        }
    }

    pub fn state(self) -> AppState {
        self.state_with_issuer(ISSUER)
    }

    pub fn state_with_issuer(self, issuer: &str) -> AppState {
        let clock: Arc<dyn Clock> = self.clock.clone();
        let secret = SecretString::from(
            self.hs_secret
                .clone()
                .unwrap_or_else(|| HS_SECRET.to_owned()),
        );
        let mut cfg = ApiConfig::for_test(issuer, KeySource::Hs256(secret.clone()));
        cfg.env.netlify_proxy_secret = self.proxy_secret.map(|s| SecretString::from(s.to_owned()));
        let (pk, _) = generate_keypair();
        let awn = self
            .awn
            .unwrap_or_else(|| Arc::new(ReplayAwn::new(fixtures(), clock.clone()).unwrap()));
        let acis = self
            .acis
            .unwrap_or_else(|| Arc::new(ReplayAcis::new(fixtures()).unwrap()));
        let om = self
            .om
            .unwrap_or_else(|| Arc::new(ReplayOpenMeteo::new(fixtures(), clock.clone())));
        AppState {
            db: self.db,
            jwt: Arc::new(TokenVerifier::hs256(issuer.to_owned(), &secret)),
            cfg: Arc::new(cfg),
            awn,
            limiters: self.limiters.unwrap_or_default(),
            acis,
            baseline_cache: Arc::new(BaselineCache::new()),
            cred_pubkey: self.pubkey.unwrap_or(pk),
            openmeteo: Arc::new(match self.om_limits {
                Some((c, p)) => OpenMeteo::with_limits(om, clock.clone(), c, p),
                None => OpenMeteo::new(om, clock.clone()),
            }),
            clock,
            gates: self.gates,
        }
    }
}

/// A pool whose connections run as `app_api`.
pub async fn api_pool(db: &PgPool) -> PgPool {
    let hook = role_hook("app_api").unwrap();
    PgPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(Duration::from_secs(10))
        .after_connect(move |conn, _| {
            let hook = hook.clone();
            Box::pin(async move { hook(conn).await })
        })
        .connect_with((*db.connect_options()).clone())
        .await
        .unwrap()
}

/// A state whose pool cannot connect: any query turns the expected result into 503, proving no query ran.
pub fn unreachable_state() -> AppState {
    let pool = PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(50))
        .connect_lazy("postgres://x@127.0.0.1:1/x")
        .unwrap();
    Build::new(pool).state()
}

pub struct World {
    /// Superuser pool (seeding and assertions).
    pub db: PgPool,
    pub api: PgPool,
    pub state: AppState,
    pub router: Router,
    pub clock: Arc<FixedClock>,
    pub personas: HashMap<&'static str, Persona>,
    pub pubkey: CredentialPublicKey,
    pub privkey: CredentialPrivateKey,
}

pub const PERSONAS: [&str; 9] = [
    "owner",
    "approved_follower",
    "stranger",
    "pending",
    "declined",
    "revoked",
    "inactive",
    "no_app_row",
    "deleted_session",
];

impl World {
    pub async fn new(db: PgPool) -> World {
        Self::with(db, |_| {}).await
    }

    pub async fn with(db: PgPool, tweak: impl FnOnce(&mut Build)) -> World {
        testdb::prepare(&db).await.unwrap();
        let api = api_pool(&db).await;
        let mut b = Build::new(api.clone());
        let (pk, sk) = generate_keypair();
        b.pubkey = Some(pk.clone());
        tweak(&mut b);
        let clock = b.clock.clone();
        let state = b.state();
        let router = build_router(state.clone(), Router::new());
        let mut w = World {
            db,
            api,
            state,
            router,
            clock,
            personas: HashMap::new(),
            pubkey: pk,
            privkey: sk,
        };
        w.seed_personas().await;
        w
    }

    pub fn p(&self, name: &str) -> &Persona {
        &self.personas[name]
    }

    pub fn tok(&self, name: &str) -> String {
        self.p(name).token.clone()
    }

    pub async fn get(&self, who: &str, path: &str) -> Resp {
        send(&self.router, "GET", path, Some(&self.p(who).token), None).await
    }

    pub async fn post(&self, who: &str, path: &str, body: Value) -> Resp {
        send(
            &self.router,
            "POST",
            path,
            Some(&self.p(who).token),
            Some(body),
        )
        .await
    }

    pub async fn put(&self, who: &str, path: &str, body: Value) -> Resp {
        send(
            &self.router,
            "PUT",
            path,
            Some(&self.p(who).token),
            Some(body),
        )
        .await
    }

    pub async fn add_user(&self, name: &str, active: bool) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO app.users (id, display_name, is_active, deactivated_at) \
             VALUES ($1, $2, $3, CASE WHEN $3 THEN NULL ELSE now() END)",
        )
        .bind(id)
        .bind(name)
        .bind(active)
        .execute(&self.db)
        .await
        .unwrap();
        id
    }

    pub async fn add_session(&self, user: Uuid) -> Uuid {
        let s = Uuid::new_v4();
        sqlx::query("INSERT INTO auth.sessions (id, user_id) VALUES ($1, $2)")
            .bind(s)
            .bind(user)
            .execute(&self.db)
            .await
            .unwrap();
        s
    }

    fn persona(id: Uuid, session: Uuid, name: &'static str) -> Persona {
        Persona {
            id,
            session,
            name,
            token: mint(claims(id, session), HS_SECRET),
        }
    }

    async fn seed_personas(&mut self) {
        let display = [
            ("owner", "Olive"),
            ("approved_follower", "Fern"),
            ("stranger", "Stan"),
            ("pending", "Penny"),
            ("declined", "Declan"),
            ("revoked", "Reva"),
            ("inactive", "Ivan"),
        ];
        for (key, name) in display {
            let id = self.add_user(name, key != "inactive").await;
            let session = self.add_session(id).await;
            self.personas.insert(key, Self::persona(id, session, key));
        }
        // A valid session but no app.users row.
        let id = Uuid::new_v4();
        let session = self.add_session(id).await;
        self.personas
            .insert("no_app_row", Self::persona(id, session, "no_app_row"));
        // An app row and a token whose session does not exist.
        let id = self.add_user("Delia", true).await;
        self.personas.insert(
            "deleted_session",
            Self::persona(id, Uuid::new_v4(), "deleted_session"),
        );
        let owner = self.p("owner").id;
        for (who, status) in [
            ("approved_follower", "approved"),
            ("pending", "pending"),
            ("declined", "declined"),
            ("revoked", "revoked"),
        ] {
            self.add_follow(self.p(who).id, owner, status).await;
        }
        assert_eq!(self.personas.len(), PERSONAS.len());
    }

    pub async fn add_follow(&self, follower: Uuid, followee: Uuid, status: &str) {
        sqlx::query(
            "INSERT INTO app.follows (follower_id, followee_id, status, decided_at) \
             VALUES ($1, $2, $3, CASE WHEN $3 = 'pending' THEN NULL ELSE now() END)",
        )
        .bind(follower)
        .bind(followee)
        .bind(status)
        .execute(&self.db)
        .await
        .unwrap();
    }

    /// A live credential (dummy ciphertext) for `owner`; returns its id.
    pub async fn add_credential(&self, owner: Uuid) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO app.credentials (id, owner_id, api_key_ciphertext, encapsulated_key, key_version) \
             VALUES ($1, $2, $3, $4, 1)",
        )
        .bind(id)
        .bind(owner)
        .bind(vec![1u8; 40])
        .bind(vec![2u8; 32])
        .execute(&self.db)
        .await
        .unwrap();
        id
    }

    pub async fn add_reference(&self, id: &str, synced: bool) {
        sqlx::query(
            "INSERT INTO app.reference_stations (id, name, lat, lon, elevation_ft, timezone, std_utc_offset_minutes, last_synced_date) \
             VALUES ($1, 'Test Airport', 46.6, -112.0, 3830, 'America/Denver', -420, CASE WHEN $2 THEN DATE '2026-10-01' END)",
        )
        .bind(id)
        .bind(synced)
        .execute(&self.db)
        .await
        .unwrap();
    }

    pub async fn add_station(
        &self,
        owner: &str,
        mac: &str,
        name: &str,
        lat: f64,
        lon: f64,
        reference: Option<&str>,
    ) -> Uuid {
        let owner_id = self.p(owner).id;
        let cred: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM app.credentials WHERE owner_id = $1 AND status <> 'replaced'",
        )
        .bind(owner_id)
        .fetch_optional(&self.db)
        .await
        .unwrap();
        let cred = match cred {
            Some(c) => c,
            None => self.add_credential(owner_id).await,
        };
        sqlx::query_scalar(
            "INSERT INTO app.stations (mac, name, owner_id, credential_id, lat, lon, elevation_ft, timezone, std_utc_offset_minutes, reference_station_id) \
             VALUES ($1, $2, $3, $4, $5::float8, $6::float8, 5000, 'America/Denver', -420, $7) RETURNING id",
        )
        .bind(mac)
        .bind(name)
        .bind(owner_id)
        .bind(cred)
        .bind(lat)
        .bind(lon)
        .bind(reference)
        .fetch_one(&self.db)
        .await
        .unwrap()
    }

    pub async fn audit_count(&self, action: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM app.audit_log WHERE action = $1")
            .bind(action)
            .fetch_one(&self.db)
            .await
            .unwrap()
    }
}
