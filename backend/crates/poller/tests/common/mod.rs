//! Shared helpers for the poller integration tests. Each `#[sqlx::test]` gets a fresh migrated database; the code
//! under test runs on a pool and a lock connection that both `SET ROLE app_poller`, so the real grants apply.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use famweather_core::acis::{AcisApi, AcisDay, AcisError, AcisStationMeta, BBox, ReplayAcis};
use famweather_core::awn::{ApiKey, AwnApi, AwnDevice, AwnError, AwnObservation, Mac, ReplayAwn};
use famweather_core::clock::{Clock, FixedClock};
use famweather_core::crypto::{encrypt_credential, generate_keypair, CredentialPublicKey};
use famweather_core::testdb::{self, role_hook};
use poller_lib::env::Env;
use poller_lib::keys::KeyRing;
use secrecy::SecretString;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub const POLLER_KEY: &str = "fixture-poller-key";

/// `app_poller` has a connection limit of 15 for the whole cluster; at most two tests hold poller connections at once.
static DB_PERMITS: std::sync::OnceLock<Arc<Semaphore>> = std::sync::OnceLock::new();

pub fn committed_fixtures() -> PathBuf {
    std::env::var_os("FIXTURES_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures"))
}

pub fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 17, 12, 3, 30).unwrap()
}

pub struct World {
    /// Superuser pool of the test database (seeding and assertions).
    pub db: PgPool,
    pub env: Env,
    pub clock: Arc<FixedClock>,
    pub keys: Arc<KeyRing>,
    pub pk: CredentialPublicKey,
    pub key_version: i16,
    _permit: OwnedSemaphorePermit,
}

/// Wraps the replay client of a world (counting, failing, spying).
type Wrap<T> = Box<dyn FnOnce(Arc<T>) -> Arc<T>>;

pub struct Builder {
    db: PgPool,
    fixtures: PathBuf,
    wrap_awn: Option<Wrap<dyn AwnApi>>,
    wrap_acis: Option<Wrap<dyn AcisApi>>,
    clock_at: DateTime<Utc>,
    pings: HashMap<String, SecretString>,
}

impl Builder {
    pub fn new(db: PgPool) -> Self {
        Builder {
            db,
            fixtures: committed_fixtures(),
            wrap_awn: None,
            wrap_acis: None,
            clock_at: now(),
            pings: HashMap::new(),
        }
    }
    pub fn fixtures(mut self, root: PathBuf) -> Self {
        self.fixtures = root;
        self
    }
    pub fn awn(mut self, f: impl FnOnce(Arc<dyn AwnApi>) -> Arc<dyn AwnApi> + 'static) -> Self {
        self.wrap_awn = Some(Box::new(f));
        self
    }
    pub fn acis(mut self, f: impl FnOnce(Arc<dyn AcisApi>) -> Arc<dyn AcisApi> + 'static) -> Self {
        self.wrap_acis = Some(Box::new(f));
        self
    }
    pub fn clock_at(mut self, t: DateTime<Utc>) -> Self {
        self.clock_at = t;
        self
    }
    pub fn ping(mut self, suffix: &str, url: String) -> Self {
        self.pings
            .insert(suffix.to_owned(), SecretString::from(url));
        self
    }

    pub async fn build(self) -> World {
        let permit = DB_PERMITS
            .get_or_init(|| Arc::new(Semaphore::new(2)))
            .clone()
            .acquire_owned()
            .await
            .unwrap();
        testdb::prepare(&self.db)
            .await
            .expect("prepare test database");
        let hook = role_hook("app_poller").unwrap();
        let pool_hook = hook.clone();
        let pool = PgPoolOptions::new()
            .max_connections(3)
            .after_connect(move |conn, _| {
                let h = pool_hook.clone();
                Box::pin(async move { h(conn).await })
            })
            .connect_with((*self.db.connect_options()).clone())
            .await
            .unwrap();
        let clock = Arc::new(FixedClock::new(self.clock_at));
        let dyn_clock: Arc<dyn Clock> = clock.clone();
        let mut awn: Arc<dyn AwnApi> =
            Arc::new(ReplayAwn::new(self.fixtures.clone(), dyn_clock.clone()).unwrap());
        if let Some(w) = self.wrap_awn {
            awn = w(awn);
        }
        let mut acis: Arc<dyn AcisApi> = Arc::new(ReplayAcis::new(self.fixtures.clone()).unwrap());
        if let Some(w) = self.wrap_acis {
            acis = w(acis);
        }
        let (pk, sk) = generate_keypair();
        let keys = Arc::new(KeyRing::new((1, sk), None).unwrap());
        let env = Env::new(
            pool,
            (*self.db.connect_options()).clone(),
            dyn_clock,
            awn,
            acis,
            Some(keys.clone()),
            self.pings,
            CancellationToken::new(),
        )
        .with_fast_timing()
        .with_lock_hook(hook);
        World {
            db: self.db,
            env,
            clock,
            keys,
            pk,
            key_version: 1,
            _permit: permit,
        }
    }
}

impl World {
    pub async fn add_owner(&self, name: &str) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO app.users (id, display_name) VALUES ($1, $2)")
            .bind(id)
            .bind(name)
            .execute(&self.db)
            .await
            .unwrap();
        id
    }

    /// Stores `plain` encrypted to the world's public key (AAD = credential id + owner id).
    pub async fn add_credential_with(
        &self,
        id: Uuid,
        owner: Uuid,
        plain: &str,
        version: i16,
        status: &str,
    ) {
        let enc =
            encrypt_credential(&self.pk, version, id, owner, &SecretString::from(plain)).unwrap();
        sqlx::query(
            "INSERT INTO app.credentials (id, owner_id, api_key_ciphertext, encapsulated_key, key_version, status) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(id)
        .bind(owner)
        .bind(enc.ciphertext)
        .bind(enc.encapsulated_key)
        .bind(version)
        .bind(status)
        .execute(&self.db)
        .await
        .unwrap();
    }

    pub async fn add_credential(&self, owner: Uuid, plain: &str) -> Uuid {
        let id = Uuid::new_v4();
        self.add_credential_with(id, owner, plain, self.key_version, "ok")
            .await;
        id
    }

    pub async fn add_station(&self, owner: Uuid, cred: Uuid, mac: &str) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO app.stations (id, mac, name, owner_id, credential_id, lat, lon, timezone, std_utc_offset_minutes) \
             VALUES ($1, $2, 'Test Station', $3, $4, 40.4, -105.1, 'America/Denver', -420)",
        )
        .bind(id)
        .bind(mac)
        .bind(owner)
        .bind(cred)
        .execute(&self.db)
        .await
        .unwrap();
        id
    }

    pub async fn add_reference(&self, id: &str) {
        sqlx::query(
            "INSERT INTO app.reference_stations (id, name, lat, lon, timezone, std_utc_offset_minutes) \
             VALUES ($1, 'Demo Airport', 40.4, -105.0, 'America/Denver', -420)",
        )
        .bind(id)
        .execute(&self.db)
        .await
        .unwrap();
    }

    /// The seed layout: three owners with one `fixture-poller-key` credential each and one station each.
    pub async fn seeded(&self) -> Vec<(Uuid, Uuid, Uuid)> {
        let mut out = Vec::new();
        for (i, name) in ["Rodney", "Alex", "Sam"].iter().enumerate() {
            let owner = self.add_owner(name).await;
            let cred = self.add_credential(owner, POLLER_KEY).await;
            let st = self
                .add_station(owner, cred, &format!("AA:BB:CC:00:00:0{}", i + 1))
                .await;
            out.push((owner, cred, st));
        }
        out
    }

    pub async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(&self.db).await.unwrap()
    }

    pub async fn close(self) {
        self.env.pool.close().await;
    }
}

// ---- AWN / ACIS wrappers ----

/// Counts calls and records history arguments.
pub struct CountingAwn {
    pub inner: Arc<dyn AwnApi>,
    pub devices: AtomicUsize,
    pub history: AtomicUsize,
    pub history_calls: Mutex<Vec<HistoryCall>>,
}

/// `(mac, end_date, limit)` of one history request.
pub type HistoryCall = (String, Option<DateTime<Utc>>, u16);

impl CountingAwn {
    pub fn new(inner: Arc<dyn AwnApi>) -> Arc<Self> {
        Arc::new(CountingAwn {
            inner,
            devices: AtomicUsize::new(0),
            history: AtomicUsize::new(0),
            history_calls: Mutex::new(Vec::new()),
        })
    }
    pub fn history_count(&self) -> usize {
        self.history.load(Ordering::SeqCst)
    }
    pub fn devices_count(&self) -> usize {
        self.devices.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl AwnApi for CountingAwn {
    async fn list_devices(&self, key: &ApiKey) -> Result<Vec<AwnDevice>, AwnError> {
        self.devices.fetch_add(1, Ordering::SeqCst);
        self.inner.list_devices(key).await
    }
    async fn history(
        &self,
        key: &ApiKey,
        mac: &Mac,
        end: Option<DateTime<Utc>>,
        limit: u16,
    ) -> Result<Vec<AwnObservation>, AwnError> {
        self.history.fetch_add(1, Ordering::SeqCst);
        self.history_calls
            .lock()
            .unwrap()
            .push((mac.to_string(), end, limit));
        self.inner.history(key, mac, end, limit).await
    }
}

/// Fails the first `fail_first` `list_devices` calls with `err`, then delegates. With `usize::MAX` it always fails.
/// `history_fail_first` does the same for `history`.
pub struct FlakyAwn {
    pub inner: Arc<dyn AwnApi>,
    pub err: AwnError,
    pub fail_first: usize,
    pub history_fail_first: usize,
    pub device_calls: AtomicUsize,
    pub history_calls: AtomicUsize,
}

impl FlakyAwn {
    pub fn devices(inner: Arc<dyn AwnApi>, err: AwnError, fail_first: usize) -> Arc<Self> {
        Arc::new(FlakyAwn {
            inner,
            err,
            fail_first,
            history_fail_first: 0,
            device_calls: AtomicUsize::new(0),
            history_calls: AtomicUsize::new(0),
        })
    }
    pub fn history(inner: Arc<dyn AwnApi>, err: AwnError, fail_first: usize) -> Arc<Self> {
        Arc::new(FlakyAwn {
            inner,
            err,
            fail_first: 0,
            history_fail_first: fail_first,
            device_calls: AtomicUsize::new(0),
            history_calls: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl AwnApi for FlakyAwn {
    async fn list_devices(&self, key: &ApiKey) -> Result<Vec<AwnDevice>, AwnError> {
        let n = self.device_calls.fetch_add(1, Ordering::SeqCst);
        if n < self.fail_first {
            return Err(self.err.clone());
        }
        self.inner.list_devices(key).await
    }
    async fn history(
        &self,
        key: &ApiKey,
        mac: &Mac,
        end: Option<DateTime<Utc>>,
        limit: u16,
    ) -> Result<Vec<AwnObservation>, AwnError> {
        let n = self.history_calls.fetch_add(1, Ordering::SeqCst);
        if n < self.history_fail_first {
            return Err(self.err.clone());
        }
        self.inner.history(key, mac, end, limit).await
    }
}

/// Records `station_data` calls and optionally rewrites the answer.
pub struct SpyAcis {
    pub inner: Arc<dyn AcisApi>,
    pub calls: Mutex<Vec<(String, chrono::NaiveDate, chrono::NaiveDate)>>,
    pub fail: Mutex<Option<AcisError>>,
    pub tmax_bump: Mutex<f64>,
    pub attempts: AtomicUsize,
}

impl SpyAcis {
    pub fn new(inner: Arc<dyn AcisApi>) -> Arc<Self> {
        Arc::new(SpyAcis {
            inner,
            calls: Mutex::new(Vec::new()),
            fail: Mutex::new(None),
            tmax_bump: Mutex::new(0.0),
            attempts: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl AcisApi for SpyAcis {
    async fn station_data(
        &self,
        sid: &str,
        sdate: chrono::NaiveDate,
        edate: chrono::NaiveDate,
    ) -> Result<Vec<AcisDay>, AcisError> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        self.calls
            .lock()
            .unwrap()
            .push((sid.to_owned(), sdate, edate));
        if let Some(e) = *self.fail.lock().unwrap() {
            return Err(e);
        }
        let mut days = self.inner.station_data(sid, sdate, edate).await?;
        let bump = *self.tmax_bump.lock().unwrap();
        if bump != 0.0 {
            for d in &mut days {
                d.tmax_f = d.tmax_f.map(|t| t + bump);
            }
        }
        Ok(days)
    }
    async fn nearby_stations(&self, bbox: BBox) -> Result<Vec<AcisStationMeta>, AcisError> {
        self.inner.nearby_stations(bbox).await
    }
}

// ---- log capture ----

#[derive(Clone, Default)]
pub struct LogBuf(pub Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogBuf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuf {
    type Writer = LogBuf;
    fn make_writer(&'a self) -> LogBuf {
        self.clone()
    }
}

impl LogBuf {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

/// Captures every event (TRACE) of the current thread until the guard drops.
pub fn capture_logs() -> (tracing::subscriber::DefaultGuard, LogBuf) {
    let buf = LogBuf::default();
    let sub = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(buf.clone())
        .with_ansi(false)
        .finish();
    (tracing::subscriber::set_default(sub), buf)
}

/// Terminates the backend that holds the advisory lock in this database (the lock connection).
pub async fn kill_lock_connection(db: &PgPool) {
    let pid: i32 = sqlx::query_scalar(
        "SELECT l.pid FROM pg_locks l JOIN pg_database d ON d.oid = l.database \
         WHERE l.locktype = 'advisory' AND l.granted AND d.datname = current_database() LIMIT 1",
    )
    .fetch_one(db)
    .await
    .unwrap();
    let ok: bool = sqlx::query_scalar("SELECT pg_terminate_backend($1)")
        .bind(pid)
        .fetch_one(db)
        .await
        .unwrap();
    assert!(ok);
}

/// A place to hand a wrapper built inside `Builder::awn` back to the test.
pub type Slot<T> = Arc<Mutex<Option<Arc<T>>>>;

pub fn slot<T>() -> Slot<T> {
    Arc::new(Mutex::new(None))
}

pub fn taken<T>(s: &Slot<T>) -> Arc<T> {
    s.lock().unwrap().clone().expect("slot filled by build()")
}

impl Builder {
    pub fn awn_counting(self, s: &Slot<CountingAwn>) -> Self {
        let s = s.clone();
        self.awn(move |inner| {
            let c = CountingAwn::new(inner);
            *s.lock().unwrap() = Some(c.clone());
            c
        })
    }

    pub fn acis_spy(self, s: &Slot<SpyAcis>) -> Self {
        let s = s.clone();
        self.acis(move |inner| {
            let c = SpyAcis::new(inner);
            *s.lock().unwrap() = Some(c.clone());
            c
        })
    }
}

/// A fixtures directory holding the given synthetic stations (`awn/stations/<MAC>/...`).
pub fn synth_root(stations: &[(&str, poller_lib::synth::StationSpec)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (mac, spec) in stations {
        let rows = poller_lib::synth::generate(spec);
        let device = poller_lib::synth::device_json(
            mac,
            "Scenario Station",
            40.4,
            -105.1,
            5000.0,
            "America/Denver",
            &rows[0],
        );
        poller_lib::synth::write_station(dir.path(), mac, &device, &rows, false).unwrap();
    }
    dir
}

pub fn ts(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}
