# 03 - Poller (backend/crates/poller + shared core modules it owns)

Audience: AI agents implementing and testing the poller. Names match `00-conventions.md` (shared names, fixtures, rate gates, test harness) and `04-database.md` (authority for table and column names, section 3.6). Anything this guide defines that no other component reads is local to this guide. Requests to other guides are in 3.4.

Source documents: `docs/Family Weather App Design Doc.md`, `docs/Design Review Findings.md`, `docs/guides/00-conventions.md`, `docs/guides/04-database.md`.

## 1. Purpose and scope

Owns:

- Binary `poller` (crate `backend/crates/poller`): one always-on process running scheduled jobs `awn_ingest`, `daily_rollup`, `reference_sync`, plus on-demand jobs `awn_backfill` and `credential_reencrypt`. The poller stores and fetches no forecasts (proposals/06, option A, Oct 7): the API serves live Open-Meteo forecasts and the forecast-change view itself (02 section 5.6). The poller goes live early only because AWN station history cannot be rebuilt.
- Shared `backend/crates/core` modules (conventions "Shared Rust code"): `config`, `db`, `redact`, `clock`, `awn` (trait `AwnApi`, `LiveAwn`, `ReplayAwn`, error enum), `ratelimit`; and poller-only helpers `time`, `rain`, `acis`. (The `forecast` and `openmeteo` modules no longer exist here; the Open-Meteo client and the percentile code live in the `api` crate, 02.) 02-api consumes `config`, `db`, `redact`, `clock`, `awn`, `ratelimit` and the `acis` StnMeta search (`AcisApi::nearby_stations`, 7.5) for its baseline picker; it must not redefine them. The poller computes no GDD: growing degree days are computed at query time by the API from stored daily tmax/tmin (04 `app.gdd_day`).
- Fixture files and the recorder under `backend/fixtures/` (conventions "Fixtures").

Does not own: schema, grants, `core::crypto` and `core::testdb` (04), Fly apps/secrets/one-off commands/healthchecks.io account (06), Compose and seed wiring (05), connect-flow logic and endpoints (02).

## 2. Decisions applied

| ID | Applied as |
| --- | --- |
| P3 | Poller is the only machine of its Fly app, always on. Every job takes an advisory lock `job:<name>` on a dedicated detached connection (4.2). |
| P4 | Catch-up from data on start, `job_runs` for every run, dead-man's-switch ping per job (4.3 to 4.5). |
| S5 | Role `app_poller`; only the poller holds `CREDENTIALS_PRIVATE_KEY`; `decrypt` feature only in `poller`. |
| S7 | No URLs in errors or logs, `SecretString`, scrubbing writer, redaction test (section 8). |
| S8 | HPKE decrypt via 04's `decrypt_credential`; `credential_reencrypt` job (section 6). |
| G6 | One `AWN_APPLICATION_KEY`; per-owner keys decrypted from `app.credentials`. |
| G8 | Gate reservations `awn:app:poller` (500 ms) and `awn:key:<credential_id>` (1000 ms); 429 backoff; 401/403 decided at end of run (5.3). |
| F1, F2 | Closed by proposals/06: no forecast snapshots; nothing forecast-related runs in the poller. |
| F3 | Backfill idempotent and resumable (`stations.backfill_floor_ts`). |
| F4 | Reference data from the airport GHCN-Daily station via ACIS. |
| F5 | Rollup days are local standard time. |
| F6 | Rain counter deltas, resets, glitches, caps, reconciliation. |
| D6 | Poller off and replaying by default locally; backfill local first, then once in production via `poller run-job awn_backfill` on `famweather-poller`. |
| C3 | Session advisory locks need a session-level connection: the poller (and one-off machines) use the **direct** connection only, never a pooler (conventions). |

## 3. Cross-component contracts

### 3.1 Binary and subcommands

| Command | Behaviour |
| --- | --- |
| `poller` / `poller run` | Scheduler (section 4). Touches heartbeat file `/tmp/poller.heartbeat` every 30 s, **even when `POLLER_ENABLED=false`**. |
| `poller healthcheck` | Exit 0 if the heartbeat file was modified within 120 s, else 1. Loads no config, no network, no DB. **Compose healthcheck only**: Fly cannot exec health commands; production liveness is the healthchecks.io pings. |
| `poller run-job <job> [--station <uuid>] [flags]` | Runs one job once in the foreground; exit 0 success, 1 failure, 3 lock held. Takes the job lock, writes `job_runs`, pings like a scheduled run (except `awn_backfill` and `credential_reencrypt`, which never ping). Allowed with `POLLER_ENABLED=false` (explicit operator action). Flags: `awn_backfill`: `--since YYYY-MM-DD`, `--max-pages N`, `--restart`; `daily_rollup`: `--from YYYY-MM-DD --to YYYY-MM-DD` (forces those days, ignoring dirty detection). `--station` is accepted by `awn_ingest`, `awn_backfill`, `daily_rollup` and is an error elsewhere. |

When `POLLER_ENABLED=false`, `poller run` emits exactly one `info` event whose message is the literal text **`poller disabled: POLLER_ENABLED=false`** (05's smoke test greps for this string), then only runs the heartbeat loop. It touches no table, writes no `job_runs`, sends no ping.

Fixture recording is the cargo example `backend/crates/poller/examples/record_fixtures.rs`, not a subcommand.

### 3.2 Environment variables

Used (all from conventions): `APP_ENV`, `DATABASE_URL` (`app_poller`), `AWN_APPLICATION_KEY`, `AWN_MODE`, `FIXTURES_DIR`, `CREDENTIALS_PRIVATE_KEY`, `CREDENTIALS_KEY_VERSION`, `CREDENTIALS_PRIVATE_KEY_PREVIOUS`, `CREDENTIALS_KEY_VERSION_PREVIOUS` (rotation only), `POLLER_ENABLED`, `HEALTHCHECK_URL_<JOB>` (suffixes used by the poller: `AWN_INGEST`, `DAILY_ROLLUP`, `REFERENCE_SYNC`; ignored locally; `awn_backfill` and `credential_reencrypt` send no pings and any `HEALTHCHECK_URL_AWN_BACKFILL` is ignored), `RUST_LOG`.

Poller-local: none. Everything is a named constant in code so there is nothing to mis-set: heartbeat path, shutdown grace (25 s), rain thresholds (`core::rain::consts`), coverage thresholds, `REFERENCE_START_YEAR = 1991`, precipitation probability threshold (0.01 in). In `AWN_MODE=replay` ACIS also replays from fixtures (no separate switch). Test-only overrides (gate spacing, clock) are constructor arguments under `cfg(any(test, feature = "test-support"))`, never env.

### 3.3 Database objects (04 section 3.6; DDL not repeated)

- `credentials`: read all; UPDATE `status, status_changed_at, last_ok_at, api_key_ciphertext, encapsulated_key, key_version`. Poller uses rows reached through `stations.credential_id` with status `ok` or `rate_limited`; skips `invalid` and `replaced`. Rotation touches **all** rows.
- `stations`: reads `is_enabled`, `mac`, `credential_id`, `timezone`, `std_utc_offset_minutes`, `reference_station_id`; UPDATE only `last_observed_at`, `backfill_floor_ts`.
- `observations`, `daily_rollups`, `reference_daily`: SELECT/INSERT/UPDATE. `reference_stations`: SELECT and UPDATE `last_synced_date` only. There are no forecast tables. `job_runs`: SELECT/INSERT/UPDATE/DELETE. `audit_log`: INSERT. `rate_gates`: SELECT/INSERT/UPDATE; `rollup_dirty_days`: SELECT/INSERT/DELETE plus `UPDATE(local_date)` (needed by the `ON CONFLICT DO UPDATE` row-lock enqueue; conventions; 04 owns DDL).
- Credential writes are always guarded and minimal. Status: `UPDATE app.credentials SET status=$1, status_changed_at=now() WHERE id=$2 AND status IN ('ok','rate_limited') AND status IS DISTINCT FROM $1` (so `status_changed_at` moves only on a real change). `last_ok_at` is a separate statement: `UPDATE app.credentials SET last_ok_at=now() WHERE id=$2 AND status IN ('ok','rate_limited')`. An audit row is written only if the status statement changed a row (`rows_affected() = 1`).
- `job_runs.id` is uuid; status `running|success|failed|cancelled|abandoned`; `detail` at most 8192 bytes.
- Audit actions: `credential.invalid`, `credential.reencrypted` (`subject_type` `credential`, `actor_id` NULL, no secrets).
- Numeric handling (conventions): Rust uses `f64`; every NUMERIC parameter is bound as `$n::float8` and selected as `col::float8`; values are rounded to 3 decimals in Rust before binding so `IS DISTINCT FROM` guards are stable. `real` columns bind as `f32`. No `rust_decimal`.
- `job_runs` older than 90 days are deleted by `daily_rollup` once a day (first run after 03:00 UTC; the previous run's `detail.housekeeping_done_on` records the date).

### 3.4 Requests to other guides

1. **04:** (a) `rain_in` is computed whenever a usable counter exists and `is_complete` carries coverage; only `tmax_f`, `tmin_f` are NULL below the coverage threshold (04 section 5.7 wording must say so, and use the interval-aware coverage rule in 7.3). (b) `core::crypto::public_key_from_private` is provided by 04 (no request). (c) `app.rollup_dirty_days` grants to `app_poller`: SELECT, INSERT, DELETE, `UPDATE(local_date)`; `app.rate_gates`: SELECT/INSERT/UPDATE. (d) Seed uses exactly: stations `AA:BB:CC:00:00:01..03`, poller credential key `fixture-poller-key` (encrypted with the dev public key), reference ids `USW00099001..3`.
2. **02:** `AwnDevice { mac, name, coords, elevation_ft, timezone }` (`elevation_ft: Option<f64>` from `info.coords.elevation`; field name unverified until T7's first recording; the replay fixtures include it) and `core::acis::AcisApi::nearby_stations` (7.5a) for the baseline picker; `AwnError` variants `Unauthorized`, `RateLimited { retry_after }`, `Unavailable`, `BadResponse`; the **caller** reserves gates (`awn:app:api`, 1000 ms, max wait 3 s; plus `awn:key:<id>` when the caller has a live credential) and maps `GateBusy` to 503 `upstream_busy`; `LiveAwn` takes a per-call timeout (API: 5 s).
3. **05:** (a) the exact disabled log line in 3.1; (b) `e2e-data` must run `poller run-job awn_backfill` **before** `awn_ingest` (a station with no data gets only one page from ingest, 7.1); (c) replay shifts data once at process start, so no new readings appear later; (d) `backend/demo-fingerprints.txt` must include a line for `CREDENTIALS_PRIVATE_KEY` (and every other local-only secret); production config refuses a match with exactly `refusing <VAR>: demo value` (`<VAR>` = the variable name, never the value), which 05's check greps for; (e) fixtures may be gzipped (`*.json.gz`), repository size budget in section 10.
4. **06:** the poller's graceful-shutdown grace is **25 s**, so set Fly `kill_timeout` to **30 s**; use the gate names `awn:app:poller` and `awn:key:<credential_id>` (no `gate:` prefix) in any runbook or SQL that inspects `app.rate_gates`; no container healthcheck on Fly; the **go-live variant** (conventions) is poller-only: one poller healthcheck (`HEALTHCHECK_URL_AWN_INGEST`; the `BACKUP` and `DBSIZE` pings belong to other apps), no backfill step, and the API/web deploy steps skipped until 02/01 ship (add the other checks when T12 to T14 ship); the post-deploy check should assert "no job has two `running` rows" in `job_runs` (not "the second scheduler writes nothing"); one-off runs are `poller run-job <job> ...` with pool max 2.

### 3.5 Healthcheck pings (contract for 06)

Per scheduled job, when its URL is set (production only): `GET <url>/start` at run start, `GET <url>` on success, `GET <url>/fail` on failure. Timeout 5 s, one retry; failures log `warn` and never fail the job. Periods: `awn_ingest` 5 min (grace 15); `daily_rollup` 1 h (grace 2 h); `reference_sync` 24 h (grace 6 h). `awn_backfill` and `credential_reencrypt` send no pings. `awn_ingest` sends `/fail` only when every station failed in the run or the last 3 completed runs each had a failed unit (partial failures still ping success so one flaky device does not page).

### 3.6 Core public API (owned here; exact names, 02/04/05 consume them)

```rust
// core::clock
pub trait Clock: Send + Sync { fn now(&self) -> DateTime<Utc>; }
pub struct SystemClock; pub struct FixedClock(/* settable */);

// core::config  (one loader for every binary; applies EVERY shared rule in 00-conventions Environments)
pub enum Binary { Api, Poller, Admin(AdminCommand) }             // AdminCommand: defined in 04 section 10.1 (not redefined here)
pub enum Config { Api(ApiEnv), Poller(PollerConfig), Admin(AdminEnv) }   // ApiEnv: 02 section 4.1; AdminEnv: 04 section 10.1; PollerConfig below
pub fn load(bin: Binary) -> Result<Config, ConfigError>;        // implemented against those definitions; Keygen loads nothing
// Refusals, exactly as 00-conventions states them:
//   all binaries, production: AWN_MODE=replay, non-verify-full URL, sslrootcert file named in DATABASE_URL missing,
//     port 6543 (all binaries), DATABASE_URL user not `<role>` or `<role>.<project-ref>` for this binary, secrets that
//     belong to another process present, localhost issuer/JWKS, demo values (below);
//   pooler hosts are refused only for the poller and its one-offs (direct connection only);
//   local: host allow-lists, placeholder application key.
// Demo values: const DEMO_FINGERPRINTS: &str = include_str!("../../../demo-fingerprints.txt"); lines "<sha256-hex>  <VARIABLE_NAME>".
//   In production hash EVERY loaded secret value AND the password extracted from DATABASE_URL, and compare each against
//   column 1 of EVERY line (not only the line with the matching name). A match fails with exactly `refusing <VAR>: demo value`
//   where <VAR> is the env var the value came from (`DATABASE_URL` for the URL password); never the value.
// core never references poller types: keys are carried as raw parts and the poller builds its KeyRing from them.
pub struct PollerConfig { pub app_env: AppEnv, pub enabled: bool, pub awn_mode: AwnMode, pub fixtures_dir: PathBuf,
    pub awn_application_key: Option<SecretString>,
    pub current_key: Option<(i16, CredentialPrivateKey)>, pub previous_key: Option<(i16, CredentialPrivateKey)>,
    pub healthcheck_urls: HashMap<String /* env suffix: "AWN_INGEST", "DAILY_ROLLUP", ... */, SecretString>,
    pub database: PgConnectOptions }

// core::db
pub struct PoolSpec { pub max: u32, pub min: u32, pub acquire_timeout: Duration, pub idle_timeout: Duration,
                      pub max_lifetime: Duration, pub test_before_acquire: bool, pub after_connect: Option<ConnectHook> }
impl Default for PoolSpec { /* max 4, min 1, acquire 30 s, idle 600 s, max_lifetime 1800 s, test_before_acquire true, no hook */ }
pub type ConnectHook = Arc<dyn for<'c> Fn(&'c mut PgConnection) -> BoxFuture<'c, sqlx::Result<()>> + Send + Sync>;
pub async fn pool(opts: &PgConnectOptions, spec: PoolSpec) -> Result<PgPool, DbError>;
// Poller values: max 4, min 1, acquire_timeout 10 s, idle_timeout 300 s, max_lifetime 1800 s, test_before_acquire true,
// after_connect None (tests: SET ROLE). One-off machines: max 2.

// core::redact
pub fn register(secret: &SecretString);                         // adds a literal to the scrub set (len >= 8)
pub fn scrub(s: &str) -> Cow<'_, str>;                          // literals, key=value regex, URLs
pub fn init_tracing(json: bool) -> Result<(), RedactError>;     // JSON subscriber over ScrubWriter + panic hook
pub fn install_panic_hook();                                    // logs file:line and the fixed message "panic" only, NEVER the payload
pub fn http_error(e: reqwest::Error) -> HttpErrorKind;          // calls without_url(); keeps only Timeout | Connect | Body | Decode | Status(u16) | Other, no source chain

// core::awn
pub struct ApiKey(SecretString);                                // ApiKey::new(SecretString); Debug prints [REDACTED]
pub struct Mac(/* upper-case AA:BB:CC:DD:EE:FF */);             // Mac::parse(&str) -> Result<Mac, MacError>; Display; last4()
pub struct AwnDevice { pub mac: Mac, pub name: Option<String>, pub coords: Option<(f64, f64)>, pub elevation_ft: Option<f64> /* info.coords.elevation, unverified */, pub timezone: Option<String> /* lastData.tz */ }
pub trait AwnApi: Send + Sync { /* list_devices, history: 5.2 */ }
impl LiveAwn   { pub fn new(app_key: SecretString, call_timeout: Duration) -> Result<Self, AwnError>;
                 #[cfg(any(test, feature = "test-support"))] pub fn with_base(base: Url, app_key: SecretString, call_timeout: Duration) -> Result<Self, AwnError>; }
impl ReplayAwn { pub fn new(fixtures_root: PathBuf, clock: Arc<dyn Clock>) -> Result<Self, AwnError>; }   // no gates, no DB, no network

// core::acis (delivered by T14, not part of the core milestone; 02's baseline picker uses nearby_stations and may stub the trait until then)
pub trait AcisApi: Send + Sync { /* station_data (7.5), nearby_stations(bbox) -> Result<Vec<AcisStationMeta>, AcisError> (7.5a) */ }
impl LiveAcis   { pub fn new(call_timeout: Duration) -> Result<Self, AcisError>; }     // fixed host data.rcc-acis.org, keyless, never reserves gates
impl ReplayAcis { pub fn new(fixtures_root: PathBuf) -> Result<Self, AcisError>; }

// core::ratelimit
pub struct GateBusy;
pub async fn reserve(pool: &PgPool, name: &str, spacing: Duration, max_wait: Duration) -> Result<Duration /*wait*/, GateBusy>;
//   the conventions' single SQL statement; caller then tokio::time::sleep(wait) (database-computed, never from Clock)
pub struct JobLock;  pub struct LockLost;
pub async fn try_job_lock(opts: &PgConnectOptions, job: &str, on_connect: Option<ConnectHook>) -> Result<Option<JobLock>, DbError>;
//   on_connect runs on the detached connection before the lock query (tests pass `SET ROLE app_poller`)
impl JobLock { pub async fn check(&mut self) -> Result<(), LockLost>; pub async fn release(self) -> Result<(), DbError>; }
```

Tasks T1 to T3 deliver exactly this block (the "03 core milestone" in conventions) before 04's `admin` and 02 start. The milestone **includes `LiveAwn`, `ReplayAwn` and `list_devices`** (04's local `admin add-station` and 02's connect flow need them); it is done when it compiles, `cargo doc` shows every name above, and unit tests pass. The DB-backed gate and lock tests run after 04's migrations and `core::testdb` exist (T3a).

## 4. Design: process model and job framework

### 4.1 Process model

- `poller run`: config (`core::config`), guards, `PgPool` with `max_connections = 4` (conventions: plus one dedicated detached connection per held job lock; role limit 15), `Clock`, HTTP clients, scheduler. One-off machines use pool max 2.
- Startup (enabled): run once, sequentially, every **implemented** job in this order: `awn_ingest`, `daily_rollup`, `reference_sync` (catch-up, 4.3), then start the timers. The scheduler registers only jobs that exist in the build (a compile-time list), so the first production deploy can ship ingest alone.
- Scheduling: one task per job using `tokio::time::interval_at(first_boundary, period)` where `first_boundary` is computed from `clock.now()` wall time and `MissedTickBehavior::Delay`. Boundaries (UTC): `awn_ingest` every 5 min at second 45; `daily_rollup` hourly at minute 7; `reference_sync` daily 11:15, plus an hourly pickup at minute 20 that loads only reference stations with `last_synced_date IS NULL` (a baseline an owner has just chosen; no `job_runs` row and no ping when there is nothing to load; it takes the same `job:reference_sync` lock, so it never overlaps the daily run). `awn_backfill`, `credential_reencrypt` are never scheduled.
- No state kept in memory across ticks.

```
backend/crates/poller/src/
  main.rs  scheduler.rs  job.rs  keys.rs
  jobs/{awn_ingest,awn_backfill,daily_rollup,reference_sync,credential_reencrypt}.rs
backend/crates/poller/examples/record_fixtures.rs
backend/crates/core/src/
  config.rs db.rs redact.rs clock.rs ratelimit.rs
  awn/{mod,live,replay,models}.rs  acis/{mod,live,replay,parse}.rs
  time.rs rain.rs heartbeat.rs
```

`core::clock`: `trait Clock { fn now(&self) -> DateTime<Utc>; }`, `SystemClock`, `FixedClock`. All "now" in jobs, replay shifting and tests goes through it.

### 4.2 Job locks (conventions "Job locks")

```rust
// core::ratelimit
pub struct JobLock { conn: PgConnection }                    // raw, detached, NOT from the pool
pub async fn try_job_lock(opts: &PgConnectOptions, job: &str, on_connect: Option<ConnectHook>) -> Result<Option<JobLock>, DbError>; // on_connect: tests pass SET ROLE app_poller
impl JobLock {
    pub async fn check(&mut self) -> Result<(), LockLost>;   // SELECT 1 on the lock connection
    pub async fn release(self) -> Result<()>;                // pg_advisory_unlock, then close
}
// Drop: the raw connection is dropped, which closes the socket and frees the lock server-side.
```

- Key: `i64` from the first 8 bytes (big-endian) of `sha256("job:<name>")`; locks `job:awn_ingest`, `job:awn_backfill`, `job:daily_rollup`, `job:reference_sync`, `job:credential_reencrypt`.
- `try_job_lock` opens a fresh `PgConnection::connect_with(opts)` (same role and TLS as the pool) and runs `SELECT pg_try_advisory_lock($1)`. Never use a pooled connection (a returned pooled connection would keep the lock for a later borrower).
- If not acquired: `info` "job skipped, lock held", no `job_runs` row, no ping, return (exit code 3 for `run-job`).
- Loss detection: jobs call `ctx.lock.check().await?` between units of work (per station, page, day). `LockLost` aborts the job as `failed` with `detail.error="lock_lost"`; the new holder owns the work.
- Tests: the lock is free after a job panic, after a cancelled job, and after a normal run (a second `try_job_lock` succeeds immediately in each case).

### 4.3 `run_job` wrapper and catch-up

```rust
pub struct JobContext { pub pool: PgPool, pub clock: Arc<dyn Clock>, pub cancel: CancellationToken, pub lock: JobLock,
                        pub last_success: Option<DateTime<Utc>>, pub run_id: Uuid, pub station: Option<Uuid>, pub flags: JobFlags, /* clients, config */ }
#[async_trait] pub trait Job { const NAME: &'static str; async fn run(&self, ctx: &mut JobContext) -> Result<serde_json::Value, JobError>; }
// JobError { kind: Cancelled | Failed(String) | LockLost, partial: serde_json::Value /* counters and gaps collected so far */ }
```

`run_job(job)`:

1. `try_job_lock`.
2. Sweep: `UPDATE app.job_runs SET status='abandoned', finished_at=now() WHERE job=$1 AND status='running'` (safe because we hold the lock; runs on every acquisition, no age rule).
3. `last_success = SELECT max(finished_at) ... status='success'` (logged only).
4. `INSERT INTO app.job_runs (job, subject_id, status) VALUES (..,'running') RETURNING id`; ping `/start` (only for jobs that have pings: not `awn_backfill`, not `credential_reencrypt`). (`awn_backfill`: see 7.2.)
5. `job.run(&mut ctx).await`. **The wrapper does not race the job against `cancel`**; jobs observe `ctx.cancel` between units and return `Err(JobError::Cancelled)`.
6. Result: Ok -> `success`; `Err(Cancelled)` -> `cancelled`, no `/fail` ping; other Err -> `failed`, `detail = JobError.partial` with `"error": <redacted, truncated to 1000 bytes>` **merged into it** (partial counters and `gaps` are kept); ping `/fail` (ingest: rule in 3.5). If the final `UPDATE` violates the 8 KiB CHECK, retry with `detail={"error":"detail_too_large"}` (list fields are capped at 20 items, so this is only a fallback). Panics are caught at the task boundary (`JoinHandle`) and recorded as `failed`.
7. `lock.release().await`.

Catch-up is data-driven (`last_success` is context only):

| Job | Start behaviour |
| --- | --- |
| `awn_ingest` | per station, fetch enough history to cover `now - max(ts_utc)` (7.1). |
| `daily_rollup` | consumes the `app.rollup_dirty_days` queue (7.3) plus a start-of-process self-heal for days that have observations but no rollup; with an empty queue and no rollups this is the whole history, in date order. |
| `reference_sync` | immediately; full load for any reference station with `last_synced_date IS NULL`. |

### 4.4 Dead-man's switch

`core::heartbeat::ping(kind, &SecretString)`; URLs are secrets (redacted); not used locally.

### 4.5 Graceful shutdown

- SIGTERM/SIGINT: cancel the shared token and stop timers; jobs finish their current unit (each unit is one transaction) and return at the next check. Wait up to 25 s.
- After grace only, the scheduler aborts the remaining task handles (this drops job futures, which drops the lock connection and frees the lock), then marks `cancelled` **only the `job_runs` rows whose ids this process created** (it keeps a `Vec<Uuid>` of its run ids; never `WHERE status='running'` alone), closes the pool, exits 0.
- Crash/SIGKILL: the lock is released by the closed socket; stale `running` rows become `abandoned` the next time that job's lock is acquired.
- Old and new machines overlapping during a deploy is safe: locks serialise jobs and writes are idempotent; ingest catch-up covers downtime up to 12 pages (about 12 days).

## 5. Design: AWN client, gates, credential state

### 5.1 AWN API facts

Verified from `https://github.com/ambient-weather/api-docs/wiki/Device-Data-Specs`: field names and units; `dateutc` is epoch ms rounded down to the minute; `lastData` on `/devices`; history from `/devices/:macAddress` in 5 or 30 minute increments, newest readings up to 10 minutes late. `https://ambientweather.docs.apiary.io/` could not be fetched while writing this guide, so the following are from the known API and **must be confirmed on the first recording (T7)**: base `https://rt.ambientweather.net/v1`; `GET /devices?apiKey=&applicationKey=` returns `[{macAddress, lastData{...}, info{name, location, coords{...}}}]`; `GET /devices/{mac}?apiKey=&applicationKey=&limit=<=288&endDate=<epoch ms or ISO>` returns observations newest first (max 288); HTTP 429 over limit (1 req/s per API key, 3 req/s per application key); 401 for bad keys (bad **application** key response unverified, Q6).

### 5.2 `core::awn`

```rust
#[async_trait]
pub trait AwnApi: Send + Sync {
    async fn list_devices(&self, key: &ApiKey) -> Result<Vec<AwnDevice>, AwnError>;
    async fn history(&self, key: &ApiKey, mac: &Mac, end_date: Option<DateTime<Utc>>, limit: u16 /*<=288*/)
        -> Result<Vec<AwnObservation>, AwnError>;
}
pub enum AwnError {
    Unauthorized,                                    // 401 or 403
    RateLimited { retry_after: Option<Duration> },   // 429
    Unavailable,                                     // 5xx, timeout, connection error (no URL, no source chain)
    BadResponse,                                     // undecodable body, oversize body or unexpected status
}
```

Rules (S7): requests via `.query(..)`; every `reqwest::Error` goes through `redact::http_error` (which calls `without_url()`) into `Unavailable`/`BadResponse`; no `#[from] reqwest::Error`, no `source()`; `Display` never prints URLs. `LiveAwn`: per-call timeout from the constructor (poller 20 s, API 5 s), connect timeout 5 s, `redirect(Policy::none())`, `https_only(true)` in production, UA `family-weather/<version>`; response bodies are read in chunks and abort with `BadResponse` above 256 KiB. Base URL is a constant; `with_base` is test-only. **Neither client touches the database or reserves gates**: callers reserve (5.3). `ReplayAwn` has no gates, no DB and no network. `AwnObservation { dateutc: i64, #[serde(flatten)] rest: HashMap<String, Value> }`; unknown keys are counted in `detail.unknown_fields` (names only, max 20).

### 5.3 Gates and backoff (conventions "Shared rate gates")

`core::ratelimit::reserve` runs the conventions' statement (insert stores `now() + spacing`; `RETURNING` a database-computed `wait`) and returns `GateBusy` when no row comes back (the wait would exceed `max_wait`). The caller sleeps `wait` with `tokio::time::sleep` (never derived from `Clock`). No connection is held while waiting and a cancelled request leaks nothing.

Poller call `awn_call(credential, f)`, the **only** place the poller reserves, in every `AWN_MODE` (replay included):

1. `reserve("awn:key:<credential_id>", 1000 ms, 120 s)`, sleep; `reserve("awn:app:poller", 500 ms, 120 s)`, sleep; one request. Reservations cannot deadlock. `GateBusy` counts as a transient unit failure. Spacing is overridable only in tests.
2. `RateLimited`: wait `retry_after` or 2, 4, 8, 16, 32 s (+-20% jitter); up to 5 attempts, each re-reserving. Still limited: guarded `UPDATE ... SET status='rate_limited' ... AND status IN ('ok','rate_limited')`, record, continue with the next credential. A later success sets `status='ok'` with the same guard and `IS DISTINCT FROM` (a no-op when already `ok`) and updates `last_ok_at` in its own guarded statement (3.3).
3. `Unavailable`/`BadResponse`: 3 attempts at 1/3/9 s, then that unit fails (others continue); the job ends `failed` if any unit failed.
4. `Unauthorized`: no retry; stop using that credential for the rest of the run and record it. After all credentials in the run were attempted: for each recorded 401 credential, if **at least one other credential succeeded in this run**, mark it `invalid` with the guarded update (`... WHERE id=$1 AND status IN ('ok','rate_limited')`) and insert `audit_log` `credential.invalid` only if that update changed a row. If no other credential succeeded (including the case of a single credential), mark nothing and fail the job with `detail.error="suspected_application_key_failure"`. Consequence: a lone credential whose key was truly revoked is never marked invalid until a second credential exists (Q6). Invalid and replaced credentials are not selected again; the API re-enables via a reconnect.

## 6. Credentials: decrypt and re-encrypt (S8)

Wire format: conventions and 04 section 9. Signatures are 04 section 9.3, used verbatim:

```rust
pub struct EncryptedCredential { pub ciphertext: Vec<u8>, pub encapsulated_key: Vec<u8>, pub key_version: i16 }
pub fn encrypt_credential(pk: &CredentialPublicKey, key_version: i16, credential_id: Uuid, owner_id: Uuid,
                          api_key: &SecretString) -> Result<EncryptedCredential, CryptoError>;
#[cfg(feature = "decrypt")]
pub fn decrypt_credential(sk: &CredentialPrivateKey, enc: &EncryptedCredential, credential_id: Uuid,
                          owner_id: Uuid) -> Result<SecretString, CryptoError>;
// also: parse_public_key / parse_private_key, generate_keypair, keypair_matches;
// provided by 04 (always compiled): pub fn public_key_from_private(sk: &CredentialPrivateKey) -> CredentialPublicKey;
```

Poller wrapper (poller crate):

```rust
// poller::keys
pub struct KeyRing { current: (i16, CredentialPrivateKey), previous: Option<(i16, CredentialPrivateKey)> }
impl KeyRing {
    pub fn from_config(cfg: &PollerConfig) -> Result<KeyRing, CryptoError>;     // CREDENTIALS_PRIVATE_KEY[_PREVIOUS] + *_VERSION[_PREVIOUS]
    pub fn decrypt_row(&self, row: &CredentialRow) -> Result<SecretString, CryptoError>; // key by row.key_version, then decrypt_credential(sk, &EncryptedCredential{..}, row.id, row.owner_id)
}
```

- Unknown `key_version`: `CryptoError`, credential skipped, job `failed`. AEAD failure: `error` log `credential_decrypt_failed` with `credential_id` only, no status change, job `failed`.
- Decrypt once per run, hold only for the run, never log any part. Threat model: protects against `app_api` compromise, backups and a Supabase operator; not poller-machine compromise (S8).

### 6.1 `credential_reencrypt`

The poller holds no `CREDENTIALS_PUBLIC_KEY` (that lives on the api and admin apps); for re-encryption it derives the public key from its own private key with `public_key_from_private` (proposals/01).

`poller run-job credential_reencrypt` (on demand; rotation step 4 in 04 section 9.5; production: one-off on `famweather-poller`). For **every** row with `key_version < CREDENTIALS_KEY_VERSION` (04 section 9.5), including `replaced` rows, one transaction each: decrypt with the row's version, encrypt to `public_key_from_private(current key)` (same `credential_id`/`owner_id` AAD, fresh encapsulation, `key_version = CREDENTIALS_KEY_VERSION`), then the compare-and-swap exactly as in 04 section 9.5:

```sql
UPDATE app.credentials SET api_key_ciphertext=$new_ct, encapsulated_key=$new_ek, key_version=$new_v
 WHERE id=$id AND key_version=$old_v AND api_key_ciphertext=$old_ct;
```

Zero rows updated means the row changed meanwhile. The API can no longer overwrite ciphertext (04), so the only possible cause is a concurrent run of this job; skip it. Insert `audit_log` `credential.reencrypted` (`detail` only `{from_version, to_version}`). `detail`: `{rows, reencrypted, skipped_cas, failed, by_version}` where `by_version` is `SELECT key_version, count(*)` after the run. The job fails if any row failed to decrypt. Rows with `key_version > CREDENTIALS_KEY_VERSION` are never decrypted: they are only counted in `detail.above_current` (and logged `warn`), because they indicate the poller is running an older key set than the API. Idempotent. No healthcheck URL.

## 7. Jobs

### 7.1 `awn_ingest` (every 5 min)

1. Enabled stations (`is_enabled`) whose credential status is `ok` or `rate_limited`; decrypt each credential once. `--station` narrows.
2. Per credential: `list_devices`; match station MACs. Device at AWN with no `stations` row: not created (stations come from the connect flow or `admin add-station`); last-4 of MAC into `detail.unknown_devices`, `warn` once per run (Q5). Station MAC missing from the list: `detail.missing_devices += 1`, `warn`.
3. Per station `last = SELECT max(ts_utc)` (an `Option`):
   - `last = None` (empty station): fetch exactly **one** page (`limit=288`, about one day), no further paging; history beyond that is `awn_backfill`'s job (05's `e2e-data` runs backfill first). `detail.empty_stations += 1`.
   - `last = Some(t)`: `limit = clamp(ceil((now - t)/5 min) + 2, 12, 288)`. **Fetch phase:** request pages with `endDate` (cursor = `oldest - 1 ms`) and keep them in memory (at most 12 x 288 rows) while the latest page's `oldest > t`; stop when `oldest <= t`, on an empty page, after 12 pages, on an error, or on cancellation. **Write phase:** write the fetched pages **oldest first** (so `max(ts_utc)` only ever advances over a contiguous prefix and an interrupted write leaves no hole). If the fetch stopped for any reason with the oldest fetched row still `> t` (page limit, empty page, error or cancellation) the remaining gap `(t, oldest)` is recorded in `detail.gaps` as `[{station_id, from: t, to: oldest}]`, `detail.gap_beyond_catchup=true`, and a `warn` is logged with the text `gap beyond ingest catch-up; run: poller run-job awn_backfill --station <uuid> --restart --since <gap start date>` (the gap start is `t`'s standard date): identical handling for every early stop. The hole is filled only by that remedy.
4. In **one transaction per station per page** (retried a bounded number of times, 3 attempts with 50/150/450 ms back-off, on SQLSTATE `40P01`/`40001`; rows are **sorted by `ts_utc`** before the upsert and dirty-day inserts are **sorted by `(station_id, local_date)`** so concurrent writers lock in a consistent order): dedupe the page by `dateutc` in Rust (keep the last; a duplicate key in one `INSERT ... ON CONFLICT DO UPDATE` statement is a Postgres error), upsert, and insert the dirty days (below) with `RETURNING` of inserted/changed rows' `ts_utc`. Then `UPDATE stations SET last_observed_at = GREATEST(COALESCE(last_observed_at, $page_max), $page_max)` (never moves backwards); on success the guarded `last_ok_at` update (3.3). `lastData` is not stored.
5. `detail`: `{credentials, stations, empty_stations, rows_seen, rows_inserted, rows_updated, unknown_devices, missing_devices, unknown_fields, bad_values, unauthorized, rate_limited, failed_units}`.

**Upsert**, chunks of at most 288 deduped rows (`QueryBuilder::push_values`):

```sql
INSERT INTO app.observations (station_id, ts_utc, temp_f, ...) VALUES (...)
ON CONFLICT (station_id, ts_utc) DO UPDATE SET
  temp_f = COALESCE(EXCLUDED.temp_f, app.observations.temp_f), ...,   -- every AWN-sourced column
  received_at = now()
WHERE (app.observations.temp_f, ...) IS DISTINCT FROM (COALESCE(EXCLUDED.temp_f, app.observations.temp_f), ...)
RETURNING ts_utc, (xmax = 0) AS inserted;
```

`rain_inc_in` is never written here. A re-run of the same payload changes zero rows (and enqueues nothing). `ts_utc` is `dateutc` verbatim. Reject `ts_utc > now + 10 min` or before 2000-01-01.

**Dirty days** (conventions: `app.rollup_dirty_days`): for each returned (inserted or changed) `ts_utc`, in the same transaction insert `(station_id, standard_date(ts))` and `(station_id, standard_date(ts - 4 h))` with `ON CONFLICT (station_id, local_date) DO UPDATE SET local_date = EXCLUDED.local_date` (conventions: this takes the row lock, so a concurrent rollup `DELETE` of an already-queued day cannot lose it; 04 grants `UPDATE(local_date)`). The 4-hour look-back marks the previous day too when a row lands shortly after a boundary, covering (a) rain glitch look-ahead (up to 6 readings, 3 h at 30-minute data) and (b) the console wall-day window that ends up to 1 h after the standard midnight.

**Field mapping** (exactly 04 section 5.6). The coercion is applied in Rust before binding; out-of-range or non-numeric values become NULL and are counted in `detail.bad_values` (a bad value never fails the batch):

| AWN field | Column | Type / coercion |
| --- | --- | --- |
| `dateutc` (ms) | `ts_utc` | timestamptz |
| `tempf` | `temp_f` | real (f32) |
| `feelsLike` | `feels_like_f` | real |
| `dewPoint` | `dew_point_f` | real |
| `humidity` | `humidity_pct` | smallint: round, 0..=100 |
| `windspeedmph` | `wind_speed_mph` | real, >= 0 |
| `windgustmph` | `wind_gust_mph` | real, >= 0 |
| `maxdailygust` | `max_daily_gust_mph` | real, >= 0 |
| `winddir` | `wind_dir_deg` | smallint: round, 0..=360 |
| `baromrelin` / `baromabsin` | `baro_rel_inhg` / `baro_abs_inhg` | real |
| `hourlyrainin` | `rain_rate_in_hr` | real, >= 0 |
| `eventrainin` / `dailyrainin` / `weeklyrainin` / `monthlyrainin` / `yearlyrainin` / `totalrainin` | `rain_event_in` / `rain_daily_in` / `rain_weekly_in` / `rain_monthly_in` / `rain_yearly_in` / `rain_total_in` | real, >= 0 |
| `lastRain` (ISO string) | `rain_last_at` | timestamptz, unparsable -> NULL |
| `solarradiation` | `solar_wm2` | real, >= 0 |
| `uv` | `uv_index` | smallint: round, 0..=32767 |
| `tempinf` | `temp_indoor_f` | real |
| `humidityin` | `humidity_indoor_pct` | smallint: round, 0..=100 |
| `battout` | `battery_out` | smallint: round, within i16 |
| (rollup) | `rain_inc_in` | numeric(6,3), written only by `daily_rollup` |

Ignored: `date`, `24hourrainin`, `tz`, `macAddress`, extra sensors, `battin`. Adding a sensor needs a 04 migration plus a row here.

### 7.2 `awn_backfill` (on demand)

`poller run-job awn_backfill [--station <uuid>] [--since YYYY-MM-DD] [--max-pages N] [--restart]`. Lock `job:awn_backfill` is taken once for the whole invocation. **No healthcheck pings.** `job_runs`: **one row per station processed** (`subject_id` = station id; status and `detail` per station), each inserted when that station starts; the abandoned sweep (4.3 step 2) runs once at acquisition.

Per station (sequential):

1. `cursor = stations.backfill_floor_ts - 1 ms` if set and not `--restart`, else `now`. (An empty first page means history is already reached, so reruns are cheap and there is no "complete" flag.) The remedy for an ingest gap is `--restart --since <gap start>`.
2. Loop: `ctx.lock.check()`, cancel check, `awn_call(history(.., Some(cursor), 288))`; dedupe, upsert and enqueue dirty days exactly as 7.1 (one transaction per page); `oldest = min(dateutc)`; after the transaction commits `UPDATE stations SET backfill_floor_ts = LEAST(COALESCE(backfill_floor_ts, $oldest), $oldest), last_observed_at = GREATEST(COALESCE(last_observed_at, $newest), $newest)`.
3. Stop on: empty page; `oldest <= --since`; `oldest >= cursor` (no progress); pages >= `--max-pages` (default 5000); cancellation. Else `cursor = oldest - 1 ms`.
4. `detail`: `{pages, rows_seen, rows_inserted, rows_updated, earliest_ts, stopped_because}` (answers history depth, F3).
5. After all stations: enqueue every standard date from each station's earliest written row to today into `rollup_dirty_days`, then run `run_job_wait(DailyRollup, wait = 15 min)`: poll `try_job_lock` every 5 s for up to 15 minutes; if still held, give up with a `warn` (the queue persists, so the scheduled rollup does the same work).

Idempotent, resumable, about 1 page per second per key. Local: replay. Production: once, `poller run-job awn_backfill` on `famweather-poller`; it shares the gate table with the running poller so budgets are shared automatically, and it does not hold `job:awn_ingest`.

### 7.3 `daily_rollup` (hourly, and after backfill)

**Which days (queue).** Consumption of `app.rollup_dirty_days`:

1. Self-heal at process start and at the daily housekeeping run only: enqueue (a) dates with observations but no `daily_rollups` row, (b) rows with `is_complete = false AND local_date < <today standard date>` within the last 60 days (days that were incomplete when last computed but whose data has since stopped changing, for example the day that was still open at the last run). `--from/--to` enqueues that range for the selected stations.
2. `SELECT station_id, local_date FROM app.rollup_dirty_days ORDER BY station_id, local_date`. Expand to **whole months**: any dirty day in a month schedules every day of that month that has observations (a counter-choice change therefore re-dirties the whole month, and the current month is reprocessed hourly, about 30 cheap station-days per station).
3. Per station-day, in date order, **one transaction** (same bounded retry on `40P01`/`40001`; queue and rollup writes are sorted by key): first `DELETE FROM app.rollup_dirty_days WHERE station_id=$1 AND local_date=$2` (this takes the row lock, so an ingest transaction enqueueing the same day waits for our commit and then re-inserts it; nothing is lost), then compute and write. `lock.check()` and a cancel check between days.
With an empty `job_runs` history this is the full history (every date is enqueued by ingest/backfill or by the self-heal).

**Day definition (F5).** `stations.std_utc_offset_minutes` is the constant standard offset; `standard_date(ts) = (ts + off).date()` equals `app.lst_date`; day bounds `[date 00:00 - off, +24 h)` UTC are always 24 h. At startup warn if the stored offset differs from `chrono_tz` `base_utc_offset` of `timezone` (stored value wins). `core::time`: `standard_day_bounds(off, date)`, `standard_date(off, ts)`, `base_offset_minutes(tz, at)`, and (optional, 7.3 reconciliation) `wall_day_bounds(tz, date)`.

**Step 1: `rain_inc_in`.** Per station-month: load rows from `month_start - 1 row` through `month_end + GLITCH_LOOKAHEAD_READINGS rows`, compute `core::rain::increments` over that whole contiguous series, and write `rain_inc_in` **only** for rows whose `ts` lies inside the standard day being processed (`UPDATE ... SET rain_inc_in = round($1::float8::numeric,3) WHERE ... AND rain_inc_in IS DISTINCT FROM ...`). The predecessor row and the look-ahead rows are read, never rewritten.

**Coverage and temperatures.** AWN history is 5 or 30 minute data. Per day: `interval = 5 min` if the median gap between that day's consecutive readings is <= 7.5 min, else `30 min`. `covered = count(temp_f IS NOT NULL) * interval`; `coverage = min(1, covered / 24 h)`. `obs_count = count(temp_f IS NOT NULL)` (288 or 48 when complete). `coverage < 0.5`: `tmax_f`, `tmin_f` NULL (so query-time GDD is NULL) and `is_complete=false`; `0.5 <= coverage < 0.9`: values computed, `is_complete=false`; `>= 0.9`: `is_complete=true`; a day that includes `now` is always `is_complete=false`. `tmax_f/tmin_f` ignore values outside `[-60, 130]` F (counted in `detail.temp_outliers`). Constants in code; proposals (Q3).

**Rain from counters (F6).** `core::rain::increments(rows) -> Vec<Inc{ts, inc: Option<f64>, reset, glitch, capped}>`; constants in `core::rain::consts` (all proposals, Q3):

| Const | Value |
| --- | --- |
| `MAX_INC_PER_5MIN_IN` | 0.5 |
| `MAX_INC_ABS_IN` | 4.0 |
| `GLITCH_MIN_DROP_IN` | 0.05 |
| `GLITCH_LOOKAHEAD_READINGS` | 6 |
| `NOISE_IN` | 0.0005 |
| `RECON_ABS_IN` / `RECON_REL` | 0.05 / 0.10 |

1. **Counter choice per station-month**: among `rain_total_in`, `rain_yearly_in`, `rain_daily_in` choose the first that is non-null in at least 90% of that month's stored rows. None: `rain_in` NULL, `rain_reconcile_status='unchecked'`, rows' `rain_inc_in` NULL. All days in a month use the same counter.
2. Iterate the series with the chosen counter `c`. A pair contributes only if **both** rows have a non-null chosen counter, so the first row of a day uses its predecessor only when that predecessor's chosen counter is non-null (otherwise `inc = NULL`). Round counters to 0.001; `d = c_i - c_{i-1}`; `gap = t_i - t_{i-1}`:
   - `|d| < NOISE_IN`: `inc = 0`.
   - `d >= 0`: `inc = d`.
   - `d < 0`: if `-d >= GLITCH_MIN_DROP_IN` and some `j` within `GLITCH_LOOKAHEAD_READINGS` readings has `c_j >= c_{i-1} - NOISE_IN` it is a **glitch**: rows `i..j-1` get `inc = 0` (`glitch=true`) and row `j` gets `inc = max(0, c_j - c_{i-1})`. Otherwise it is a **reset**: `inc = c_i`, `reset=true`.
   - Cap: `cap = min(MAX_INC_PER_5MIN_IN * ceil(gap/5 min), MAX_INC_ABS_IN)`; `inc > cap` -> `inc = 0`, `capped=true`, `warn` (station id and `ts` only).
3. `daily_rollups.rain_in = sum(rain_inc_in)` over rows with `ts` in `[day_start, day_end)`; `rain_resets` and `rain_capped` are that day's counts; glitches go into `detail.rain_glitches` (no column).

**Reconciliation (post-go-live, optional: implement last, T16).** Until it exists, write `rain_reconcile_status='unchecked'`, `awn_daily_rain_in` and `rain_reconcile_diff_in` NULL. When implemented: wall-clock window `(start, end]` from `wall_day_bounds(timezone, local_date)` (`start` = console local midnight); rows with `ts > start && ts <= end`, using the increments already computed in the month series (so the next midnight row is included once it exists; the 4-hour dirty look-back in 7.1 re-dirties the day when it arrives). `console_rain_in = sum(inc)`; `awn_daily_rain_in = max(rain_daily_in)`; status `unchecked` when the month's counter is `rain_daily_in`, `no_awn_total` when `rain_daily_in` is NULL in the window, `mismatch` when `|console - awn| > max(RECON_ABS_IN, RECON_REL * awn)`, else `ok`; store the diff. `rain_in` stays the standard-day value. Assumes the console clock zone equals `stations.timezone` (Q4).

**GDD is not computed here.** `daily_rollups` stores only `tmax_f`/`tmin_f`; the API computes growing degree days at query time with 04's `app.gdd_day` (method, base and start date are request parameters, 02 5.5), so a change of method never needs a rollup rewrite or backfill. (Owner decision, Oct 7, 2026; the former `core::gdd`, `GDD_METHOD` and the stored `gdd`/`gdd_method` columns are gone.)

**Write** per station-day: `INSERT ... ON CONFLICT (station_id, local_date) DO UPDATE ... WHERE (...) IS DISTINCT FROM (...)`, with `computed_at = now()` on change.

`detail`: `{stations, days_processed, days_changed, resets, capped, rain_glitches, mismatches, temp_outliers, housekeeping_done_on, job_runs_deleted}`.

### 7.5 `reference_sync` (daily)

ACIS `StnData`, `POST https://data.rcc-acis.org/StnData`. Docs: `https://www.rcc-acis.org/docs_webservices.html` (confirmed: `sid`, `sdate`, `edate`, `elems`, JSON output, flags `M`, `T`, `S`, `A`, `add=f`). Request/response examples are from the documentation as remembered and **must be confirmed against the first recorded fixture**.

Iterates `app.reference_stations` (rows created by `admin add-reference-station` or by the API's baseline picker, 02 5.10; `id` is the GHCN-Daily id; ACIS accepting it as `sid` was confirmed for `USW00094035`, `USW00024144` and `USW00024022` on Oct 7, 2026 with live `StnData` and `StnMeta` calls).

```json
{"sid":"USW00024144","sdate":"1991-01-01","edate":"2026-10-01",
 "elems":[{"name":"maxt","add":"f"},{"name":"mint","add":"f"},{"name":"pcpn","add":"f"}]}
```

Parser (`core::acis::parse`) accepts per element a bare string or `[value, flag]`:

| Raw | `tmax_f`/`tmin_f` or `precip_in` | `precip_trace` | `quality_flags` entry |
| --- | --- | --- | --- |
| number | the number | false | none |
| `M` or empty | NULL | false | `m:M` / `n:M` / `p:M` |
| `T` (precip) | NULL | true | `p:T` |
| `S` (precip) | NULL | false | `p:S` |
| number with `A` | the number | false | `p:A` |
| unparseable | NULL, `detail.unparsed`, `warn` | false | `?` |

`quality_flags` is the comma-joined entries (`m`=max, `n`=min, `p`=precip). Trace counts as 0 in cumulative sums (02's job). Percentile-year eligibility (maximum missing days) is 02's decision (Q2).

Per station: `last_synced_date IS NULL` -> load from `REFERENCE_START_YEAR-01-01` (1991) through yesterday (UTC) in one request; else refresh from `sdate = min(last_synced_date + 1 day, yesterday - 13 days)` (so a station that was last synced long ago catches up fully, and a recent one still re-reads 14 days of possible revisions). Rows are written with `INSERT ... ON CONFLICT (reference_station_id, date) DO UPDATE ... WHERE (...) IS DISTINCT FROM (...)` over `tmax_f, tmin_f, precip_in, precip_trace, quality_flags`, so a revised value is picked up. **No GDD is computed or stored** (04 `app.gdd_day` does it at query time). Finally `UPDATE reference_stations SET last_synced_date = <last date returned>`, but only when the response contained at least one row (a refresh that returns no rows leaves `last_synced_date` unchanged). One station per request, 1 s apart, timeout 60 s, 3 retries on 5xx. F5: airport `USW` days are midnight-to-midnight local standard time (assumption). Baselines: Helena `USW00024144` (record from 1938-04-01), Laramie `USW00024022` (from 1948-01-01) and Loveland: Northern Colorado Regional Airport (FNL), `USW00094035` (from 2011-10-01; only about 14 complete seasons, so its percentile bands are thin). `REFERENCE_START_YEAR = 1991` caps how far back any of them is loaded. Locally the fixtures use ids `USW00099001..3` (conventions).

#### 7.5a ACIS `StnMeta` search (`core::acis`, used by 02's baseline picker; not a poller job)

The same `core::acis` client also exposes `AcisApi::nearby_stations(bbox) -> Result<Vec<AcisStationMeta>, AcisError>`; the API calls it when an owner connects a station (02 5.10). Verified against the live service on Oct 7, 2026 (docs: `https://www.rcc-acis.org/docs_webservices.html`):

```json
POST https://data.rcc-acis.org/StnMeta
{"bbox":"-105.4,40.1,-104.7,40.7","meta":"name,sids,ll,elev,valid_daterange","elems":"pcpn,maxt,mint"}
```
```json
{"meta":[{"ll":[-105.01667,40.45],"sids":["94035 1","FNL 3","KFNL 5","USW00094035 6","USW00094035 32"],"elev":5015.0,
          "valid_daterange":[["2011-10-01","2026-10-05"],["2011-10-01","2026-10-06"],["2011-10-01","2026-10-06"]],
          "name":"FORT COLLINS LOVELAND AP"}]}
```
- `bbox` is `west,south,east,north` in degrees; `ll` is `[lon, lat]`; `elev` is feet; `valid_daterange` has one `[start, end]` per element **in the order of `elems`** (here pcpn, maxt, mint) and is `[]` for an element the station never reported (rainfall-only volunteer stations show `[[...], [], []]`).
- Each `sids` entry is `"<id> <type>"`: type 1 WBAN, 2 COOP, 3 FAA, 4 WMO, 5 ICAO, 6 GHCN-Daily, 7 NWSLI, 10 CoCoRaHS; other types (for example the duplicate GHCN entry with type 32) are ignored. `AcisStationMeta { ghcn_id: Option<String> (type 6), icao: Option<String> (type 5), faa: Option<String> (type 3), name, lat, lon, elevation_ft: Option<f64>, pcpn/maxt/mint: Option<(NaiveDate, NaiveDate)> }`.
- Request: POST, JSON, no key, no redirects, timeout passed by the caller (the API passes 5 s), response read with `chunk()` and aborted past 512 KiB (a 0.7 by 0.6 degree box returns well under that; the API's box has a half-width of 0.9 degrees of latitude, i.e. 1.8 degrees tall, and the same distance in longitude (`0.9 / cos(lat)` degrees, 02 4.15)), errors through `without_url()`. The caller reserves the rate gate `acis:api` (02); `core::acis` never reserves. `AcisError::{Unavailable, BadResponse}`; an empty `meta` array is `Ok(vec![])`.
- `ReplayAcis::nearby_stations` serves `acis/stnmeta__<tag>.json` (10): the file whose recorded centre is nearest to the requested box centre (within 1 degree), otherwise `stnmeta__empty.json`; the tag `unavailable` (a request whose centre is within 0.05 degrees of a marker coordinate documented in the fixtures README) returns `AcisError::Unavailable`, which is how the API's ACIS-down tests run without a mock server.

### 7.6 (removed) no forecast job

Decided Oct 7 (proposals/06, option A): the poller has no `forecast_snapshot` job, no `forecast_models.toml`, no Open-Meteo client, no model-run metadata polling (about 144 calls a day gone), no `FORECAST_SNAPSHOT` healthcheck, no startup catch-up step for forecasts and no forecast tables or grants. Forecasts, ensemble percentiles and the forecast-change view are produced by the API on request (02 section 5.6) and held only in its in-memory cache. Percentiles are used only in live forecast charts; there is no percentile history. The ensemble aggregation tests, `core::forecast` and the Open-Meteo fixtures moved to the `api` crate (02). Section number kept so references to 7.5 and 7.7 stay valid.

## 8. Logging, redaction, metrics (S7)

- `tracing` JSON to stdout; fields `job`, `run_id`, `station_id`, `credential_id`; MAC only as last 4 characters.
- Never log URLs, query strings, bodies, headers, keys, ciphertext, the application key, or healthcheck URLs.
- Layers: `SecretString` everywhere (no `derive(Debug)` on bare-secret structs); error types without URLs (`without_url()`); `core::redact::ScrubWriter` wraps stdout (registered secret literals of length >= 8 -> `[REDACTED]`; regex `(?i)(apikey|applicationkey|api_key|token|key)=[^&\s"']+` -> `$1=[REDACTED]`; `https?://[^\s"']+` -> `[URL]`), also used by the panic hook and `main`'s error printer; default `RUST_LOG=info,hyper=warn,reqwest=warn,sqlx=warn,h2=warn`.
- No HTTP port. Observability: `job_runs.detail`, logs, pings. One `job finished` log line per run with `duration_ms`, `status`, counters.

## 9. Configuration and guards (`core::config`, owned here)

`core::config::load(Binary)` returns `Config::{Api(ApiEnv), Poller(PollerConfig), Admin(AdminEnv)}` and applies every refusal in 3.6 exactly as the conventions' Environments section states it. `ApiEnv` is defined in 02 section 4.1 and `AdminEnv`/`AdminCommand` in 04 section 10.1; this guide implements `load()` against those definitions and does not redefine their fields. Per-subcommand required env: `admin migrate` needs only `APP_ENV` and `DATABASE_URL`; `admin keygen` loads nothing. Poller-specific: production requires `POLLER_ENABLED=true`, `AWN_MODE=live`, `CREDENTIALS_PRIVATE_KEY`, `CREDENTIALS_KEY_VERSION`, `AWN_APPLICATION_KEY`; a `*_PREVIOUS` private key requires its version; the database user is `app_poller` or `app_poller.<project-ref>`; port 6543 and pooler hosts are refused. `PollerConfig` carries raw key parts and healthcheck URLs keyed by env-suffix string; `poller::keys::KeyRing` is built in the poller crate. `api::config` adds only API-specific checks. Unit tests: every refusal, demo match by a differently named line, a demo password inside `DATABASE_URL` (message names `DATABASE_URL`), role-user `app_poller.abcdref` accepted and `app_api` refused for the poller, errors never print values.

## 10. Fixtures and replay

Root `$FIXTURES_DIR` (`/fixtures` in Compose; `backend/fixtures` otherwise). Committed, sanitised, no real keys/MACs/precise coordinates.

```
awn/devices_ok.json                 # fixture-valid-key: MACs AA:BB:CC:00:01:01, AA:BB:CC:00:01:02 (not seeded), with info.coords and lastData.tz
awn/devices_unauthorized.json       # fixture-invalid-key -> Unauthorized (holds the recorded 401 body, never returned to callers)
awn/devices_rate_limited.json       # fixture-ratelimited-key -> RateLimited
awn/stations/<MAC>/device.json      # fixture-poller-key: one /devices element (macAddress, lastData, info) per seeded station
awn/stations/<MAC>/history/page-0001.json[.gz] ...   # newest first, raw AWN arrays, <=288 each
acis/<ID>__<sdate>__<edate>.json    # ID in USW00099001..USW00099003, >= 12 complete years each, recorded data renamed
acis/stnmeta__loveland.json         # recorded StnMeta response (7.5a) for a box around 40.4N 105.0W: contains FNL USW00094035, volunteer COOP/CoCoRaHS stations and a rainfall-only station
acis/stnmeta__helena.json, stnmeta__laramie.json   # same for the other two seeded areas (nearest-airport suggestion tests)
acis/stnmeta__empty.json            # {"meta":[]}: no station in range
acis/stnmeta__nocandidates.json     # only COOP/CoCoRaHS/closed stations: nothing passes the airport filter (02 5.10)
test/<case>/...                      # synthetic scenarios for unit/integration tests, same layout (T6 uses these, not real recordings)
```

`<MAC>` is the upper-case colon form (`AA:BB:CC:00:00:01`, `...:02`, `...:03`); `awn/stations/` has exactly those three directories. Any other API key in replay -> `Unauthorized`. Pages may be gzipped (`.json.gz`); replay reads both. Depth: **at least 60 days** of history per MAC (about 17 280 rows), so backfill, a rolling month and a month boundary can be exercised; with gzip this is a few MB per station. Total fixture size budget: 25 MB (lint fails above it).

`ReplayAwn` (`ReplayAwn::new(fixtures_root, clock)`; no gates, no DB, no network; if `awn/stations/` is missing or empty the shift is 0 and construction never fails, so the connect-flow fixtures work before any station history exists):

- `list_devices`: `fixture-poller-key` returns the array of the three `device.json`; `fixture-valid-key` returns `devices_ok.json`; invalid/rate-limited keys map as above.
- **Time shifting**: at construction `shift = floor_5min(clock.now()) - max(dateutc over every history page of all stations)`; add it to every `dateutc` (and `lastRain`, `date`, `lastData.dateutc`) returned and subtract it from `endDate`. The shift is fixed for the process life, so paging is stable and **replay yields current-year observations and rollups**; no readings newer than process start appear afterwards (Q12).
- `history(mac, end_date, limit)`: **merge all pages of the station into one series**, sorted newest first and deduplicated by `dateutc`; return the `limit` newest rows with `dateutc <= E` (after unshifting; `None` means no upper bound). Paging therefore terminates with an empty page after the oldest row.
- The caller (the job) still reserves gates in replay; `ReplayAwn` itself does not.

`ReplayAcis`: serves the file for the requested `sid`, slicing rows by `sdate..edate`; no shifting. **A `sid` with no fixture file returns an empty result** (`Ok` with zero rows, not an error), logged once per `sid` per process at `info` ("no ACIS fixture for sid, replaying empty"); the job therefore succeeds, writes no rows, leaves `last_synced_date` NULL or unchanged, and does not retry (no retry storm; the hourly pickup of never-synced stations simply tries again next hour, cheaply). This is what a baseline chosen from the `stnmeta__*.json` candidates (for example `USW00094035` before a `StnData` fixture exists for it) gets in local and CI replay. Open-Meteo fixtures and their replay client belong to the API (02 section 5.6 and its fixtures), not the poller.

Recording and deriving: `cargo run -p poller --example record_fixtures -- awn --mac <real MAC> --as AA:BB:CC:00:00:01 --days 60` records one real station (AWN key from an interactive prompt or stdin only, S7; dev application key; refuses in production) and writes sanitised files (placeholder MAC, `info.coords` fuzzed to 1 decimal, no address/name). One recording becomes three stations offline with `-- derive --from AA:BB:CC:00:00:01 --as AA:BB:CC:00:00:02 --temp-offset-f -3.0 --rain-scale 1.15` (deterministic: shifts temperatures, scales rain counters, keeps timestamps), and again for `...:03`; the derived stations keep their own counters so month-long counter-choice and reset cases stay realistic. `-- acis --sid <real id> --as <USW0009900N>` saves raw responses with ids renamed. (Open-Meteo recording moved to the API crate's own example, 02.)

Fixture lint test fails on 32+ hex strings, MACs outside `AA:BB:CC:00:..`, coordinates with more than 1 decimal in `awn/`, any `latitude`/`longitude`/`lat`/`lon` value in `open-meteo/` farther than 0.25 degrees from every seed city-centre coordinate in `ops/db/seed/seed.toml` (Open-Meteo echoes grid-snapped values), any coordinate in `acis/` that is not the public coordinate of the ACIS station named in that file's name or a seed city centre (public station coordinates are allowed; family homes never), ACIS fixtures with fewer than 12 complete years, fewer than 60 days per MAC, or total size over budget.

## 11. Implementation tasks

Order matters: station history cannot be rebuilt (AWN keeps only a limited history, F3), so ingest goes live first; backfill, rollups and reference sync can be rebuilt from stored data afterwards. Every day of delay is a permanent gap in station data. Forecasts are not a reason to go live early: the API fetches them live (proposals/06). T1 to T3 are the **03 core milestone** (conventions): the block in 3.6, delivered before 04's `admin` and 02 build on it.

- [ ] **T1 Config, db, clock.** `core::config` (`load(Binary)`, `Config` enum, every shared refusal, demo-fingerprint refusal with the exact message), `core::db::pool` with `PoolSpec` defaults, `core::clock`. Done when it compiles and the guard unit tests pass, including a production run refusing a demo value with `refusing <VAR>: demo value`.
- [ ] **T2 Redaction.** `core::redact` (`register`, `scrub`, `init_tracing`, `install_panic_hook` logging file:line and a fixed message only, `http_error`). Done when it compiles and the log-redaction and panic-hook unit tests (12.1) pass.
- [ ] **T3 Core awn, gates, locks and Dockerfile (compile level).** `core::awn` (`ApiKey`, `Mac`, `AwnDevice`, `AwnError`, `AwnApi`, `LiveAwn::new`, `ReplayAwn::new` with `list_devices`), `core::ratelimit` (`reserve`, `JobLock`, `try_job_lock` with the connect hook), and `backend/Dockerfile` (one image, binaries copied only for crates that exist; `COPY certs/`; `backend/Cargo.toml` uses `resolver = "2"`). T3 also **commits hand-written synthetic connect-flow fixtures at their final paths** (`awn/devices_ok.json`, `awn/devices_unauthorized.json`, `awn/devices_rate_limited.json`, with the conventions' MACs `AA:BB:CC:00:01:01/02`, `info.coords`, `lastData.tz`); T7 only verifies them against a real recording or replaces them. Done when it compiles, `cargo doc` shows every name in 3.6, the image builds, and the unit tests (replay `list_devices`, error mapping, SQL text of `reserve`) pass. **This ends the 03 core milestone.**
- [ ] **T3a DB-backed gate and lock tests.** After 04's migrations and `core::testdb` exist (04 tasks 1 to 6): gate and lock tests in 12.3 against the migrated test database. Done when they pass under `make test-backend`.
- [ ] **T4 Job framework.** `job.rs`, `run_job`, `job_runs`, heartbeat, `poller healthcheck`, `run-job`, scheduler (compile-time list of implemented jobs, `interval_at`), shutdown, disabled log line. Done when the framework tests (12.3) pass.
- [ ] **T5 Keys.** `poller::keys::KeyRing`. Done when round-trip and negative tests pass and `api` does not enable `decrypt`.
- [ ] **T6 AWN history, backoff, credential rules.** `LiveAwn` behaviour (timeouts, body cap), `ReplayAwn::history` (shifting, merged series), `awn_call` with backoff and the end-of-run 401 rule, guarded credential writes. Uses synthetic fixtures under `backend/fixtures/test/` (not real recordings). Done when wiremock and replay tests pass.
- [ ] **T7 Recorder and real fixtures.** Recorder example with `--as`/`derive`; Rodney records one real station with his own dev key (agents prepare, Rodney runs); the three seeded stations; verify or replace the T3 connect-flow files; lint. Done when lint passes and mismatches with 5.1/7.1 are written to section 14 in the same PR.
- [ ] **T8 `awn_ingest`.** Mapping with coercion, dedupe, upsert, dirty-day enqueue, empty-station behaviour, gap warning. Done when replay integration and idempotency tests pass.
- [ ] **T9 (removed).** No forecast job (proposals/06). The number is kept so T10 to T16 references stay valid.
- [ ] **T10 Replay integration suite.** Compose end to end (12.2) for ingest. Done when `make test-backend` passes and Compose with `AWN_MODE=replay POLLER_ENABLED=true` shows `success` rows in `job_runs` plus observation rows.
- [ ] **T11 Production go-live gate.** Agents prepare a checklist for Rodney (agents never deploy): migrations applied, stations and credentials added via `admin`, Fly secrets set including **only** `HEALTHCHECK_URL_AWN_INGEST` (the `BACKUP` and `DBSIZE` pings belong to other apps), `famweather-poller` deployed with only the implemented jobs registered. Done when Rodney confirms production `success` runs for `awn_ingest` and the first pings arrived. Do not start T12 to T16 before this unless T11 is blocked.
- [ ] **T12 `awn_backfill`.** Done when the backfill replay tests pass.
- [ ] **T13 `daily_rollup`.** `core::rain`, `core::time`, queue consumption, month expansion, coverage; reconciliation stays `unchecked`. Done when unit and rollup integration tests pass.
- [ ] **T14 `reference_sync`.** ACIS client (`station_data` and `nearby_stations`, 7.5a), parser, hourly pickup of `last_synced_date IS NULL`, `USW00099001..3` and `stnmeta__*` fixtures. Done when parser unit and replay tests pass; then add `HEALTHCHECK_URL_REFERENCE_SYNC`/`_DAILY_ROLLUP` in production.
- [ ] **T15 `credential_reencrypt`.** Done when the rotation integration test passes.
- [ ] **T16 Rain reconciliation (optional, post-go-live).** `wall_day_bounds`, window `(start, end]`, statuses. Done when the reconciliation tests pass; skip if it complicates the rollup.

## 12. Testing

Harness (conventions): `make test-backend` runs the `test-backend` Compose service. Integration tests use `#[sqlx::test(migrations = false)]` plus `core::testdb::prepare` with `TEST_SUPERUSER_DATABASE_URL`; the code under test uses a pool whose `after_connect` runs `SET ROLE app_poller` (also applied to the detached lock connection), so grants are exercised. `FIXTURES_DIR=/work/backend/fixtures`. Grant/exposure assertions stay in `ops/db/tests/run.sh`. Tests use a `FixedClock`.

### 12.1 Unit tests

Rain (`core::rain`): monotone counter; reset `[1.20, 1.25, 0.00, 0.04]` gives `0.05, 0, 0.04` and one reset; **glitch `1.20 -> 0.30 -> 1.20` gives increments 0, 0 and no reset**; `1.20 -> 0.30 -> 1.25` gives `0, 0.05`; a drop under `GLITCH_MIN_DROP_IN` is a reset; a drop that never returns within 6 readings is a reset; `0.00 -> 5.00` in 5 min is capped to 0, and so is a 0.6 in jump in 5 min; a 4.0 in jump over a 4 h gap (cap `min(0.5*48, 4.0) = 4.0`) is accepted; null ends give NULL; the first row of a day uses its predecessor only when the predecessor's chosen counter is non-null; the counter is chosen per station-month and is the same for every day of the month; fallback to `rain_daily_in`; noise ignored; reconcile `ok`/`mismatch`/`no_awn_total`/`unchecked` (`unchecked` whenever the counter is `rain_daily_in`); the `(start, end]` window excludes the 00:00 reading's carried increment.

Coverage: 288 readings at 5 min and 48 at 30 min both give `is_complete=true`; 30-minute data with 20 readings (coverage 0.42) gives NULL `tmax_f/tmin_f` and `is_complete=false`; 30-minute data with 40 readings (0.83) computed but incomplete.

Time: `America/Denver` 2026-03-08 and 2026-11-01 standard bounds 07:00Z to 07:00Z, 24 h, while `wall_day_bounds` is 23 h and 25 h; `06:59:59Z` -> previous date, `07:00:00Z` -> next; Phoenix constant; Sydney base offset; `standard_date` equals SQL `app.lst_date`; stored-offset mismatch warning.

ACIS parse: bare and `[value, flag]` forms, `T`, `M`, `S`, `1.20A`, empty, malformed, golden recorded response, `quality_flags` format. StnMeta parse: the recorded `stnmeta__loveland.json` yields FNL with `ghcn_id` `USW00094035`, `icao` `KFNL`, `faa` `FNL`, `elevation_ft` 5015 and three date ranges in `pcpn, maxt, mint` order; a rainfall-only station has `None` for `maxt` and `mint`; unknown sid types are ignored; `ll` is read as `[lon, lat]`; `{"meta":[]}` is `Ok(vec![])`; a body past 512 KiB or invalid JSON is `BadResponse`; a 5xx or timeout is `Unavailable`; errors carry no URL. Replay: `ReplayAcis` asked for an unknown `sid` returns an empty `Ok` result and logs once (a second call for the same `sid` logs nothing); a `reference_sync` run in replay with a reference station that has no fixture succeeds with that station contributing zero rows, `last_synced_date` unchanged, and no retry or error row.

AWN mapping: every row of 7.1 including coercion: `humidity` 100.4 -> 100, 101 -> NULL, `winddir` 361 -> NULL, `uv` -2 -> NULL, a string for a numeric -> NULL, negative rain -> NULL, counted in `detail.bad_values`; `lastRain` parse; `dateutc` ms.

Crypto: round trip via 04's `encrypt_credential`/`decrypt_credential`; `KeyRing::decrypt_row` picks the right key by version; wrong AAD, unknown version and tampering fail.

Config: every guard. Log redaction (S7, required): register `TESTKEY-abcdef123456`; capture at TRACE; trigger AWN 500, connection refused, timeout, decode error, 401, 429 and a `panic!` in a job; assert neither the output nor any `job_runs.detail` contains the key, the application key, `apiKey=` or `http` URLs; `Debug` of `LiveAwn`, config and `ApiKey` print no secrets; a healthcheck URL with an embedded UUID never appears.

### 12.2 Replay-mode integration tests

- `awn_ingest` with `fixture-poller-key`: three stations ingested, timestamps shifted so the newest is `now`, `last_observed_at`, `credentials.last_ok_at`, `job_runs` success. Unknown device reported, missing device counted, disabled station and `invalid`/`replaced` credentials skipped. Catch-up: data ending 3 h ago is filled; a 13-day gap stops at 12 pages, records `detail.gaps` and logs the remedy, and `last_observed_at` never moves backwards; the stop condition `oldest <= last` ends paging.
- 30-minute test scenario: ingest and rollup produce `is_complete=true` days with 48 readings.
- Rollup end to end: current-year days exist after replay ingest and backfill; the reset, glitch and capped-spike scenario gives the expected `rain_in`, `rain_resets`, `rain_capped`; a late-arriving row enqueues its day and (within 4 h of a boundary) the previous day, nothing else; an ingest that commits during a rollup transaction is not lost, including the **row-already-queued interleaving** (the day is queued, the rollup's `DELETE` is open, the ingest's `ON CONFLICT DO UPDATE` blocks on the row lock until the rollup commits and then re-inserts the day: test with two connections and explicit transaction ordering); a month with one dirty day reprocesses every day of that month, so a counter-choice change rewrites the whole month; `rain_inc_in` is written only inside the processed day and the predecessor row is never rewritten; an empty `job_runs` history processes full history in date order; two concurrent rollups cannot both run (lock). Post-backfill rollup waits for a held `job:daily_rollup` lock (bounded) and then runs the backfilled range.
- Backfill: 60 days of replay history, 288 rows per page; `--max-pages 2` stops with `backfill_floor_ts` set; a rerun resumes and ends on an empty page; another run finishes immediately; `--restart` inserts and updates 0; one `job_runs` row per station; no pings are sent; duplicate `dateutc` in a page does not break the upsert; `--restart --since <date>` fills an ingest gap.
- Empty-station ingest fetches exactly one page and logs no gap warning; a gap over 12 pages logs the `awn_backfill --restart --since` remedy text.
- Reference sync with `USW00099001..3` (a refresh with no rows leaves `last_synced_date` unchanged; `sdate = min(last_synced_date + 1, yesterday - 13)` for both a recent and a long-stale station; a revised temperature in a re-fetch updates the row): loads; `T` gives NULL precip and `precip_trace=true`; the second run refreshes only 14 days; `last_synced_date` updated. Pickup: a newly inserted `reference_stations` row (`last_synced_date IS NULL`) is fully loaded by the hourly pickup within the hour, an hourly pickup with nothing to load writes no `job_runs` row, and the pickup never runs while the daily run holds the lock.
- `credential_reencrypt` (a row with `key_version` above current is only reported in `detail.above_current`, never decrypted): rows at versions 1 and 2 (including a `replaced` row), current 2: all become 2, decrypt returns the originals, `audit_log` rows written, a rerun changes nothing; a row whose ciphertext is altered between read and update fails the three-part CAS and is skipped.

### 12.3 Idempotency, locks, gates, errors, shutdown

- Idempotency: each job twice on identical input gives identical tables (hash plus unchanged `xmin`); the second run reports 0 inserted/updated.
- Locks: two `try_job_lock` for one job on separate connections: one wins; different jobs run concurrently; the lock is free after panic, cancellation and normal release; killing the holder connection frees it; `check()` returns `LockLost` after the lock connection is terminated (`pg_terminate_backend`) and the job aborts `failed`.
- Gates (real time, test-overridable spacing, not `tokio::time::pause`; `ReplayAwn` itself never touches the DB, the caller reserves in replay too): with spacing 50 ms, 20 concurrent `reserve` calls across two pools yield sorted `slot_at` values at least 50 ms apart; exceeding `max_wait` returns `GateBusy`; poller reservations never use `awn:app:api`; a task cancelled while sleeping leaves no stuck state (the next `reserve` is granted at the expected slot).
- 429: two 429s then 200 succeeds and status stays `ok`; endless 429 gives up after 5 attempts, status `rate_limited`, other credentials still processed.
- Credential writes: every status `UPDATE` carries `AND status IN ('ok','rate_limited') AND status IS DISTINCT FROM $new`; against a `replaced` or `invalid` row, or when the value is unchanged, it changes nothing (`status_changed_at` stays) and writes no audit row; `last_ok_at` is updated by its own guarded statement.
- 401: credential A gets 401 and credential B succeeds in the same run -> A `invalid` with one `audit_log` `credential.invalid`; A and B both 401 -> none marked and the job fails with `suspected_application_key_failure`; a lone 401 credential -> none marked, job fails; independent of processing order.
- `Unavailable`/`BadResponse`: 3 retries then the unit fails, others continue.
- Framework (dummy job): lock skip writes nothing and sends no ping; success/failed/cancelled rows; no `/fail` for cancelled; ingest `/fail` rule (all failed, or 3 consecutive); `detail.error` truncated to 1000 bytes and the fallback on oversized detail; the sweep marks stale `running` rows `abandoned` on lock acquisition regardless of age; the wrapper never races the job against cancel (cancel mid-unit lets the unit commit, then the job returns `cancelled`); after grace the scheduler aborts and the lock frees.
- Disabled mode: the exact line `poller disabled: POLLER_ENABLED=false` is logged once, no table touched, heartbeat touched; `poller healthcheck` exits 0 shortly after and 1 when the file is stale or missing.
- Two concurrent `run_job(AwnIngest)` calls on **separate pools** against one database: exactly one runs, the other is skipped, and `job_runs` never contains two `running` rows for one job.
- Shutdown marks `cancelled` only this process's run ids (a `running` row inserted by another process stays untouched).

## 13. Security checklist

- [ ] No `#[from] reqwest::Error`; no error `Display` containing URLs; no `{:?}` on request builders, `Url` or bodies.
- [ ] `SecretString` for all keys; `Debug` on config shows `[REDACTED]`; the redaction test passes at TRACE.
- [ ] AWN base URL not runtime-configurable; `with_base` is test-only; no AWN key in env or argv (the recorder prompts).
- [ ] Job locks use detached raw connections only; no pooled advisory locks anywhere.
- [ ] Production guards tested; the poller connects only as `app_poller` (log `current_user` once).
- [ ] `core` feature `decrypt` enabled only by `poller`; CI fails if `api` enables it; decrypted keys never cached across runs.
- [ ] Re-encryption CAS has the three-part predicate.
- [ ] The poller makes no Open-Meteo call (no client, no egress need, no `FORECAST_SNAPSHOT` ping).
- [ ] Fixture lint passes; `job_runs.detail` and `audit_log.detail` contain no secrets or upstream bodies and stay under their CHECKs.
- [ ] The poller connects over the direct connection only; production config refuses port 6543 and any pooler host (C3), so dropping a lock connection frees the lock.
- [ ] `poller healthcheck` loads no secrets and makes no network calls.
- [ ] Agents do not run `run-job` against production; Rodney does.

## 14. Open questions and assumptions

- **Q1 (Closed, Oct 7)** GDD is configurable and computed at query time by the API; the poller computes none.
- **Q2 Baselines.** Loveland is FNL, `USW00094035` (replaces USC00053005; record from 2011-10-01, confirmed through ACIS on Oct 7, 2026); elevation fit for USW00024144 (3,865 ft) and USW00024022 (7,273 ft) still unchecked against the owners' stations; ACIS accepts GHCN ids as `sid` (confirmed); percentile-year eligibility is 02's.
- **Q3 Thresholds are proposals** (all constants in code): 0.5 in per 5 min and 4.0 in absolute caps, the glitch rule (drop of at least 0.05 in returning within 6 readings), reconcile tolerance max(0.05 in, 10%), coverage 0.5/0.9, temperature bounds [-60, 130] F. Rodney to confirm after the first backfill using the reset/glitch/capped/mismatch counts.
- **Q4 Console time zone.** Reconciliation assumes the console clock uses `stations.timezone` with DST; otherwise `mismatch` fires daily.
- **Q5 New devices.** Unknown MACs are reported, not auto-created. Auto-create wanted? The possible Mesa, AZ station on the Laramie owner's account would appear as an unknown device and needs a baseline before enabling.
- **Q6 Bad application key response and lone credentials.** 401 vs 403 for a bad application key is unverified (Apiary page not retrievable); verify at T7. Under the conventions' rule a credential is marked `invalid` only if another credential succeeded in the same run, so a lone revoked key fails the job (and alerts) but is never marked invalid until a second credential exists.
- **Q7, Q9 (closed, Oct 7).** Forecast storage is dropped (proposals/06, option A); the Previous Runs archive was checked with curl on 2026-10-07 and its results are in proposals/06 and 02 section 5.6. Nothing forecast-related is open in the poller.
- **Q10 AWN history depth, `endDate` inclusivity, 288 limit, 5- versus 30-minute interval.** Confirm at T7; backfill records `earliest_ts`.
- **Q11 `snow` from ACIS** is not stored.
- **Q12 Replay data is shifted once at process start**; no new readings appear later (acceptable for local dev).
- **Assumptions.** Airport GHCN-Daily days are midnight-to-midnight local standard time (F5). ACIS free access stays keyless at this volume.
