# 02 - API (Rust Axum) Implementation Guide

Component: `backend/crates/api` (bin `api`). It consumes shared code in `backend/crates/core` that other guides own (see 3.1).
Read first: `00-conventions.md` (binding names, ownership, test harness), `04-database.md` (authority for table, column and function names), the Design Doc, and `Design Review Findings.md` (item IDs cited below).
Other guides: `01-web.md` (consumes the HTTP contract defined here), `03-poller.md` (owns `core::config/db/redact/clock/awn/ratelimit`), `04-database.md` (owns `core::crypto`, `core::testdb`, schema, grants), `05-local-dev.md` (Compose, GoTrue, Caddy), `06-deploy-ops.md` (Fly apps, Netlify, secrets, CI).

Version note: crate versions below are intended majors/minors as of Oct 2026 and were NOT verified against crates.io when this guide was written. Task 1 resolves exact versions in `backend/Cargo.toml` `[workspace.dependencies]` and commits `Cargo.lock`. No wildcard versions.

---

## 1. Purpose and scope

The API is the only HTTP surface of the backend. It:

1. Verifies Supabase access tokens strictly (S1, D2).
2. Confirms per request that the user is active in `app.users` and that the token's session is still live via `app_auth.session_active` (S2, S4, S11).
3. Enforces `app.can_view` on every station-scoped read through types that make skipping it impossible (S10, G7).
4. Serves read endpoints (stations, YTD rain/GDD with reference percentile bands, forecast change history; raw observations and multi-station daily rollups are reserved but deferred).
5. Serves follow endpoints (request, approve, decline, revoke, list).
6. Serves the connect flow (validate AWN key, list devices, save encrypted credential, credential status) (S9, G6, G8).
7. Rate-limits per user (S3), redacts secrets (S7), writes audit rows (S16), exposes `GET /api/health`.

**This guide is the authority for the HTTP contract** (section 5); `01-web.md` must match it exactly.

Does NOT own: schema DDL, roles/grants, SQL bodies of `app.can_view` / `app.visible_station_ids` / `app_auth.session_active`, `core::crypto`, `core::testdb` (04); `core::config`, `core::db`, `core::redact`, `core::clock`, `core::awn`, `core::ratelimit` (03); the admin CLI and invites (04); Caddy/Netlify routing and headers (05/01/06); Fly apps and secrets (06). This guide states what the API requires of those modules; it does not define them.

The API never decrypts credentials. It has no `CREDENTIALS_PRIVATE_KEY`, and its binary does not contain decrypt code (4.7).

## 2. Decisions applied

| ID | How it is applied here |
| --- | --- |
| S1 | Algorithm allow-list, iss/aud/exp/sub/role checks, anon/service-role/alg=none tests (4.3, 7.1) |
| S2 | Bearer token only; per-request `app_auth.session_active` check with a startup probe (4.4) |
| S3 | Flat per-user limiter checked before any DB access; `X-Forwarded-For` never used for decisions; optional Netlify `x-nf-sign` check, enabled after first deploy (4.9, 4.10) |
| S4, S11 | `app.users.is_active` checked on every request, no caching (4.4) |
| S5, G2 | No decrypt code in the API binary; production refuses secrets that do not belong to the API process (4.1, 4.7) |
| S7 | `reqwest::Error::without_url()`, `SecretString`, no `Debug` on secret structs, panic hook, path-only request logs, log-redaction test (4.11, 7.4) |
| S8 | `core::crypto::encrypt_credential` per 04 section 9 (4.7) |
| S9, G6, G8 | Hard-coded AWN host, durable per-user limit recorded BEFORE the AWN call, slot gate `awn:app:api`, generic valid/invalid, MACs only from AWN's list (4.6) |
| S10, G7 | `ViewableStation` newtype, extractors, source-scan guard test; forecast access via station (4.5) |
| S12, C3 | Role `app_api`, verify-full TLS, session-mode pooler or direct, no transaction mode (4.12) |
| S16 | `app.audit_log` inserts in the same transaction as the action; logins stay in GoTrue (4.13) |
| D2 | HS256 only when `APP_ENV=local` and `SUPABASE_URL=http://localhost:8000` (4.1, 4.3) |
| P3 | Single API machine that may auto-stop: in-memory limiter resets are acceptable; connect limits and AWN gate are in Postgres (4.9, 4.14) |
| F5, F6 | Day definition and baseline labels in YTD responses; reconcile status exposed (5.5) |
| G4 | No admin endpoints in v1; admin flag read from DB only (5.9) |
| G5 | The API never creates `app.users` rows; a valid token with no app row is rejected (4.4) |

## 3. Cross-component contracts

### 3.1 Consumed

Environment variables (names from `00-conventions.md`):

| Variable | Rule in API |
| --- | --- |
| `APP_ENV` | required: `local` or `production`; unset or anything else = startup error |
| `DATABASE_URL` | role `app_api`; production requires `sslmode=verify-full` and `sslrootcert=/etc/ssl/app-certs/supabase-ca.crt`; production refuses to start if that file is missing |
| `SUPABASE_URL` | issuer is `${SUPABASE_URL}/auth/v1` (trailing slash trimmed). Local with HS256: must equal `http://localhost:8000` exactly |
| `SUPABASE_JWKS_URL` | required in production |
| `JWT_HS256_SECRET` | local only; >= 32 bytes; rejected in production |
| `AWN_APPLICATION_KEY` | required when `AWN_MODE=live` |
| `AWN_MODE` | `live` (default) or `replay`; `replay` rejected in production |
| `FIXTURES_DIR` | replay fixtures |
| `CREDENTIALS_PUBLIC_KEY`, `CREDENTIALS_KEY_VERSION` | encrypt only |
| `API_BIND_ADDR` | default `0.0.0.0:8080` |
| `NETLIFY_PROXY_SECRET` | optional; when set, `x-nf-sign` is required (4.10) |
| `RUST_LOG` | tracing filter |

Shared Rust modules consumed (owners per conventions; the API crate adds none of them to `core`):

| Module | Owner | What the API uses |
| --- | --- | --- |
| `core::config`, `core::db`, `core::redact`, `core::clock` | 03 | env helpers, the Environments refusal rules and the demo-value check (`backend/demo-fingerprints.txt` embedded with `include_str!`); pool builder; `ScrubWriter`, panic hook and error printer; injected `Clock` |
| `core::awn` (`AwnApi`, `LiveAwn`, `ReplayAwn`, `AwnError`) | 03 | `list_devices(&ApiKey)`; `AwnDevice { mac, name: Option<String>, coords: Option<(f64, f64)>, timezone: Option<String> }` (`timezone` from `lastData.tz`); `AwnError::{Unauthorized, RateLimited { retry_after: Option<Duration> }, Unavailable, BadResponse}`; `LiveAwn::new(app_key: SecretString, call_timeout: Duration)` (the API passes 5 s); `ReplayAwn::new(fixtures_root, clock)`; core::awn NEVER reserves gates and never touches the DB |
| `core::ratelimit::reserve(pool, name, spacing, max_wait) -> Result<Duration, GateBusy>` | 03 | the returned `Duration` is the wait computed by the database; the CALLER reserves and then `tokio::time::sleep(wait)`; the connect flow reserves `awn:app:api` (1000 ms, max wait 3 s) and, when the caller has a live credential, `awn:key:<that credential_id>` (1000 ms, max wait 3 s), in every `AWN_MODE` |
| `core::crypto` (`encrypt_credential`, `CredentialPublicKey`, `parse_public_key`, `EncryptedCredential`; feature-gated decrypt) | 04 | encrypt only |
| `core::testdb` (`prepare`, `pool_as(&db_name, role)`), auth stub | 04 | test setup (7) |

Requirements on 03's modules: response bodies are read with `chunk()` and aborted past the cap (256 KiB for AWN; the API applies 64 KiB to its own JWKS fetch); `list_devices` goes through `without_url()` on every error; `ApiKey` is a secret newtype constructible from the request's `SecretString` and prints no secrets in `Debug`; `ReplayAwn` parses `lastData.tz` and `info.coords` from `awn/devices_ok.json`.

Database objects (names per `04-database.md`; 04 wins on conflict and this guide is then corrected):

| Object | API usage |
| --- | --- |
| `app.users(id, display_name, is_active, is_admin, ...)` | SELECT on every request; follow lookup |
| `app_auth.session_active(session_id uuid, user_id uuid) -> boolean` (SECURITY DEFINER, EXECUTE granted to `app_api`) | per-request session check; the API role has no grant on any `auth.*` table (proposals/03, accepted) |
| `app.follows(follower_id, followee_id, status, requested_at, decided_at, updated_at)` | SELECT, INSERT; UPDATE only `(status, decided_at)` |
| `app.credentials(...)` | INSERT `(id, owner_id, api_key_ciphertext, encapsulated_key, key_version)`; UPDATE only `(status, status_changed_at)`; SELECT only the metadata columns `id, owner_id, key_version, status, last_ok_at, status_changed_at, created_at, updated_at` (`core::models::CREDENTIAL_META_COLUMNS`); never `SELECT *`, never `RETURNING` ciphertext columns, never `ON CONFLICT DO UPDATE` |
| `app.stations(id, mac, name, owner_id, credential_id, lat, lon, timezone, std_utc_offset_minutes, reference_station_id, is_enabled, last_observed_at, ...)` | SELECT; INSERT; UPDATE `(name, credential_id)`; `lat::float8` |
| `app.forecast_locations(id, station_id, lat, lon)` | SELECT; INSERT |
| `app.reference_stations(id, name, source, ...)`, `app.reference_daily(reference_station_id, date, tmax_f, tmin_f, precip_in, precip_trace, gdd, gdd_method, ...)` | SELECT |
| `app.daily_rollups(station_id, local_date, rain_in, tmax_f, tmin_f, gdd, gdd_method, is_complete, rain_reconcile_status, rain_reconcile_diff_in, ...)`, `app.observations` | SELECT |
| `app.forecast_runs(location_id, model, run_time, fetched_at, row_count)`, `app.forecast_snapshots(location_id, model, run_time, variable, valid_time, period_hours, p10, p50, p90, prob_precip)` | SELECT |
| `app.audit_log(id, at, actor_id, action, subject_type, subject_id, detail)` | INSERT only |
| `app.connect_attempts(id, user_id, attempted_at, outcome)` | SELECT, INSERT, DELETE, and UPDATE `(outcome)` |
| `app.rate_gates(name, next_allowed_at)` | SELECT/INSERT/UPDATE via `core::ratelimit::reserve` |
| `app.can_view(user_id, station_id)`, `app.visible_station_ids(user_id)` | access check and list filter |

### 3.2 Exposed

- HTTP routes in section 5, JSON only, error envelope from conventions. Every response (including errors, 404 fallback, 408/413/503 from layers, and health) carries `Cache-Control: no-store` and `X-Content-Type-Options: nosniff`.
- Subcommands: `api` (serve) and `api healthcheck` (GET `http://127.0.0.1:<port of API_BIND_ADDR>/api/health`, exit 0 on 200 else 1; no shell or curl in the image).
- The shared `backend/Dockerfile` (created by 03; this guide adds the api build step and checks, 4.14), deployed as Fly app `famweather-api` (running `api`).

### 3.3 Open requests for other guides

03 and 04 already provide the names and patterns this guide uses (including `AwnDevice.name: Option<String>` and the reconnect repoint predicate) (`LiveAwn::new`, `reserve -> Duration`, `ApiKey`, `AwnDevice`, `http_error`, `CredentialPublicKey`/`EncryptedCredential`/`parse_public_key`, `CREDENTIAL_META_COLUMNS`, `core::testdb::pool_as`, the follow audit rule, the reconnect pattern). Genuinely open:

- 05: password grant must be enabled on the local GoTrue (the gotrue round-trip, 7.5, uses it) and the test-backend service must set `SUPABASE_URL=http://auth:9999` and `DATABASE_URL`.
- 06: supply `backend/certs/supabase-ca.crt` (the repo commits `backend/certs/.keep`).

Changes `01-web.md` must pick up (new or changed since 01 was written):

- 401 `unauthorized` always means "sign out and go to login". A 503 `auth_unavailable` means "retry later", NOT sign out.
- 403 `account_inactive` -> show a "no access" screen.
- New error codes: `request_timeout` (408), `payload_too_large` (413), `overloaded` (503, `Retry-After`), `auth_unavailable` (503), `upstream_busy` (503, `Retry-After`), `upstream_unavailable` (502), `invalid_key`, `unknown_device`, `timezone_required` (422), `device_already_registered` and `conflict` (409). 429 always has `Retry-After`. REMOVED: the per-route cost weights from earlier drafts; the per-user rate is flat (120/min, burst 40). Every `/api/v1` request is aborted with 408 `request_timeout` after 25 s, so the web client timeout should be at least 25 s and treat 408 as retryable.
- `GET /stations/{id}/observations` and `GET /rollups/daily` are DEFERRED (do not call them).
- `GET /ytd`: `year` is REQUIRED. MEANING CHANGE: `missing_days` is now an object `{rain, gdd}` (on `current` and on each `prior_years[]` entry) and `bands.years_included` is an object `{rain, gdd}` (earlier drafts had plain numbers). `current` has `first_date`, `last_date`, `last_complete_date`; the open current day appears as a partial value. `prior_years[]` may include years later than `year`. `gdd_overlay` object; any series array may be `null`.
- `GET /stations/{id}/forecast/history`: `valid_from` and `valid_to` required; `variable` is `precip_6h_in` only.
- `POST /connect/validate` devices include `timezone` (string or null). `POST /connect/save` device `timezone` is only a fallback when AWN gave none (`timezone_required` otherwise) and must not disagree with AWN's; the 201 response includes `not_covered` (stations that stay on the replaced credential and need attention).
- `POST /follows` is provisional (depends on the follow-discovery decision); follows are addressed as `/follows/{follower_id}/...` with `follower_id` = the follower's user id.

## 4. Design

### 4.1 Crate layout, dependencies, config

```
backend/crates/api/
  Cargo.toml
  src/main.rs              # subcommands, config, state, serve, graceful shutdown
  src/lib.rs               # crate is lib + bin so tests/ can build the router; feature `test-support`
  src/app.rs               # build_router(state, extra) -> Router (4.2)
  src/state.rs
  src/config.rs            # ApiConfig + API-specific refusal rules on top of core::config
  src/error.rs             # ApiError, envelope, fixup middleware
  src/auth/{mod.rs,jwks.rs,token.rs,user.rs,guard.rs}
  src/access.rs            # ViewableStation(s), ViewableStationPath, ViewableStationsQuery
  src/extract.rs           # FollowerId and other non-station extractors
  src/limits.rs            # in-memory per-user limiter
  src/audit.rs
  src/telemetry.rs
  src/routes/{mod.rs,health.rs,me.rs,stations.rs,ytd.rs,forecast.rs,follows.rs,connect.rs}
  src/repo/{users.rs,stations.rs,ytd.rs,forecast.rs,follows.rs,credentials.rs,connect_attempts.rs}
  tests/
```

Deferred (post-v1) files: `routes/observations.rs`, `routes/rollups.rs`, `repo/observations.rs`, `repo/rollups.rs`.

Dependencies. `api` depends on `core` WITHOUT feature `decrypt`.

| Crate | Pin (verify) | Notes |
| --- | --- | --- |
| `axum` | 0.8 | `macros`, `json`, `query`, `tokio`, `http1`, `http2` |
| `tokio` | 1 | `rt-multi-thread`, `macros`, `signal`, `sync`, `time` |
| `tower` | 0.5 | `limit`, `timeout`, `load-shed` |
| `tower-http` | 0.6 | `trace`, `request-id`, `catch-panic`, `limit`, `timeout`, `sensitive-headers`, `set-header`, `propagate-header` |
| `sqlx` | 0.8 | `runtime-tokio`, rustls feature per `core::db`, `postgres`, `uuid`, `chrono`, `json`, `macros` |
| `jsonwebtoken` | 9 (or 10 with a crypto-backend feature) | |
| `reqwest` | 0.12 | `default-features = false`, `rustls-tls`, `json` (JWKS fetch only; AWN is via `core::awn`) |
| `secrecy` | 0.10 | serde feature |
| `governor` | 0.8 | one keyed in-memory limiter |
| `tracing`, `tracing-subscriber` | 0.1 / 0.3 | `json`, `env-filter` |
| `serde`, `serde_json`, `uuid`, `chrono`, `chrono-tz`, `thiserror`, `anyhow` | current | |
| dev: `wiremock`, `http-body-util`, `tower` (`util`), `pretty_assertions`, `p256` + `rsa` (test keys) | | |

Config split (conventions "Config ownership"): `core::config::load(Binary::Api)` applies EVERY shared Environments rule and returns the API's environment; the API does not re-implement them. Those shared rules, which the API's tests exercise but do not own, are: `APP_ENV` unset is an error; production refusal of `JWT_HS256_SECRET`, `AWN_MODE=replay`, non-`verify-full` `DATABASE_URL`, a missing `sslrootcert` file (the path named in the URL), port 6543, a `DATABASE_URL` user other than `app_api`/`app_api.<ref>`, secrets that belong to other processes (`CREDENTIALS_PRIVATE_KEY(_PREVIOUS)`, `SUPABASE_SERVICE_ROLE_KEY`, `BACKUP_*`, `AWS_*`, `HEALTHCHECK_URL_*`, role passwords), demo values (refused with the message `refusing <VAR>: demo value`, via `backend/demo-fingerprints.txt`), an empty or short `AWN_APPLICATION_KEY` in live mode, an invalid `CREDENTIALS_PUBLIC_KEY` or `CREDENTIALS_KEY_VERSION < 1`; and the local host rules (`DATABASE_URL` host `db`/`localhost`/`127.0.0.1`/`::1`; `SUPABASE_URL` host `localhost`/`127.0.0.1`/`caddy`/`auth`; placeholder application key refused with `AWN_MODE=live`).

`ApiEnv` is defined here (conventions: 03 implements `load()` against this definition; one typed field per row of the 3.1 env table):

```rust
pub struct ApiEnv {
    pub app_env: AppEnv,                              // APP_ENV: Local | Production
    pub database: sqlx::postgres::PgConnectOptions,   // DATABASE_URL (role app_api; TLS/CA already validated by core::config)
    pub supabase_url: url::Url,                       // SUPABASE_URL
    pub supabase_jwks_url: Option<url::Url>,          // SUPABASE_JWKS_URL (required in production)
    pub jwt_hs256_secret: Option<SecretString>,       // JWT_HS256_SECRET (local only)
    pub awn_mode: AwnMode,                            // AWN_MODE: Live (default) | Replay
    pub awn_application_key: Option<SecretString>,    // AWN_APPLICATION_KEY (required when Live)
    pub fixtures_dir: PathBuf,                        // FIXTURES_DIR
    pub credentials_public_key: core::crypto::CredentialPublicKey,  // CREDENTIALS_PUBLIC_KEY via parse_public_key
    pub credentials_key_version: i16,                 // CREDENTIALS_KEY_VERSION (>= 1)
    pub bind_addr: std::net::SocketAddr,              // API_BIND_ADDR (default 0.0.0.0:8080)
    pub netlify_proxy_secret: Option<SecretString>,   // NETLIFY_PROXY_SECRET
    // RUST_LOG is read by core::redact::init_tracing, not stored here.
}
```

`ApiEnv` has a manual `Debug` (`[redacted]` for secrets and URLs with credentials). `ApiConfig { env: ApiEnv, issuer: String, allowed_algs: Vec<Algorithm>, ... }` wraps it. `api::config::ApiConfig::from_env()` calls `core::config::load(Binary::Api)` to get the `ApiEnv` and adds ONLY API-specific checks:

1. Production: `SUPABASE_URL` and `SUPABASE_JWKS_URL` are `https://` with a dotted, non-local host (`localhost`, `127.*`, `::1`, `*.localhost`, `*.test` refused); `SUPABASE_JWKS_URL` is required.
2. Local: exactly one of `JWT_HS256_SECRET` (>= 32 bytes) or `SUPABASE_JWKS_URL`. With `JWT_HS256_SECRET`, `SUPABASE_URL` must equal `http://localhost:8000` exactly (issuer `http://localhost:8000/auth/v1`). The test-project value `SUPABASE_URL=http://auth:9999` is therefore only used by tests that build `ApiConfig::for_test` and give the issuer explicitly.
3. `NETLIFY_PROXY_SECRET` is optional everywhere (warn at startup in production when unset).

`ApiConfig::for_test(..)` and the extra test routers exist only under `cfg(any(test, feature = "test-support"))`. `api/Cargo.toml` declares `[features] test-support = ["core/test-support"]` and dev-dependencies on itself and on core with the features on (`api = { path = ".", features = ["test-support"] }`, `core = { workspace = true, features = ["test-support", "testdb"] }`, never `decrypt`), so plain `cargo test -p api` works. `backend/Cargo.toml` sets `resolver = "2"` in `[workspace]` so dev-dependency features never reach release builds. `make test-backend` runs the conventions' two steps: `cargo test --workspace --exclude admin --features api/test-support`, then `cargo test -p admin --features compose-tests -- --test-threads=1`. Violations print variable NAMES only and exit non-zero. `ApiConfig` and every struct with a secret implement `Debug` manually (`[redacted]`); secrets are `SecretString`.

### 4.2 Application state and router shape

```rust
pub struct AppState {
    pub db: sqlx::PgPool,
    pub cfg: Arc<ApiConfig>,
    pub jwt: Arc<TokenVerifier>,
    pub awn: Arc<dyn core::awn::AwnApi>,
    pub limiter: Arc<UserLimiter>,
    pub cred_pubkey: core::crypto::CredentialPublicKey,   // from core::crypto::parse_public_key(CREDENTIALS_PUBLIC_KEY)
    pub clock: Arc<dyn core::clock::Clock>,
}
```

Axum layer-order rules this guide relies on: a `.layer()`/`.route_layer()` added LATER wraps those added earlier, so the LAST layer added runs FIRST on a request. A `.fallback()` is covered only by layers added after it is registered. Therefore:

- Authentication and rate limiting are ONE middleware, `auth::guard`, so their order cannot be inverted (4.4).
- One overall `TimeoutLayer` (25 s) wraps all of `/api/v1`, OUTSIDE the guard, so a stalled JWKS fetch, database wait or AWN call cannot hang a request. Per-operation bounds are inside: sqlx `statement_timeout` 10 s, AWN call 5 s, gate wait 3 s.
- The fallback is registered BEFORE the outer layers so 404s get headers and envelope.

```rust
pub fn build_router(state: AppState, extra: Router<AppState>) -> Router   // production main passes Router::new()
// `extra` is merged INSIDE /api/v1 (so the guard applies) and is only non-empty in tests (cfg(any(test, feature="test-support"))):
// panic route, slow route, stub station routes. Files under tests/ are exempt from the source scans in 4.5/7.2.

let v1 = Router::new() /* routes registered via register() */ .merge(extra)
    .route_layer(from_fn_with_state(state.clone(), auth::guard))
    .layer(TimeoutLayer::with_status_code(StatusCode::REQUEST_TIMEOUT, 25 s));    // outside the guard

Router::new()
  .route("/api/health", get(health))
  .nest("/api/v1", v1)
  .fallback(not_found_json)                      // registered before the layers below
  .layer(proxy_signature_layer)                  // only if NETLIFY_PROXY_SECRET set; /api/health exempt
  .layer(HandleErrorLayer(overloaded) + LoadShedLayer + GlobalConcurrencyLimitLayer::new(64))   // ONE semaphore for the whole app; overload -> 503 overloaded
  .layer(RequestBodyLimitLayer(16 KiB))
  .layer(CatchPanicLayer)
  .layer(from_fn(envelope_fixup))                // empty-body 408/413/503 from layers -> error envelope
  .layer(SetResponseHeaderLayer no-store + nosniff)
  .layer(PropagateRequestIdLayer x-request-id)
  .layer(TraceLayer)                             // custom span, 4.11
  .layer(SetRequestIdLayer MakeRequestUuid)
  .layer(from_fn(strip_incoming_request_id))     // LAST added = outermost: runs first
```

Notes:

- `strip_incoming_request_id` removes any client `x-request-id` (`SetRequestIdLayer` keeps an existing one). `SetRequestIdLayer` is outside `TraceLayer` so the span sees the id; `PropagateRequestIdLayer` copies it to the response.
- `Router::layer` wraps each route separately, so a plain `ConcurrencyLimitLayer` would create one limit per route. Use tower's `GlobalConcurrencyLimitLayer` (one shared semaphore; alternatively wrap the finished `Router` in a `ServiceBuilder` that carries the limit), paired with `LoadShedLayer` and `HandleErrorLayer` (503 `overloaded`, `Retry-After: 1`) so saturation fails fast. Because `/api/health` is under the same cap, it can return 503 `overloaded` under saturation, which is the intended behavior.
- tower-http `TimeoutLayer` and `RequestBodyLimitLayer` return empty 408/413 responses; `envelope_fixup` replaces any 408/413 (and any 503 lacking `application/json`) with the envelope (`request_timeout`, `payload_too_large`, `overloaded`). A body that exceeds the limit WITHOUT a `Content-Length` (chunked) surfaces as a `JsonRejection` from the `Json` extractor, not from the layer; the custom rejection mapping (4.8) maps a length-limit failure to 413 `payload_too_large`, never 422.
- Routes are registered only through `register(spec: RouteSpec, handler)` in `routes/mod.rs` (7.2).
- Health: `GET /api/health` runs a plain `SELECT 1` with a 2 s timeout under the global concurrency cap (200 `{"status":"ok"}`, else 503 `{"status":"unavailable"}`). The cap, the pool's `acquire_timeout` and the 2 s timeout bound what anonymous callers can consume; no caching.

### 4.3 JWT verification (S1, D2)

Module `auth::token`. `TokenVerifier::verify(&self, bearer: &str) -> Result<VerifiedClaims, AuthError>`. `AuthError::Invalid` -> HTTP 401 `unauthorized` (one generic message; the reason goes to a `debug` log field without the token). `AuthError::Unavailable` -> HTTP 503 `auth_unavailable` (the web must not sign users out on 503).

`VerifiedClaims { user_id: Uuid, session_id: Uuid }`; nothing else escapes.

Steps (a failure is `Invalid` unless stated):

1. `Bearer <token>` (case-insensitive scheme), token <= 8192 bytes, exactly two dots.
2. `decode_header`. `jku`, `jwk`, `x5u`, `x5c`, `crit` are never used.
3. Allow-list fixed at startup: production `[ES256, RS256]`; local with `JWT_HS256_SECRET` `[HS256]` only; local with `SUPABASE_JWKS_URL` `[ES256, RS256]`. Anything else (including `none`, EdDSA, PS*, ES384, and HS256 in production) is rejected before key lookup.
4. Key selection: asymmetric requires `kid`, looked up in the JWKS cache (4.3.1; returns `Unavailable` when the cache is unusable); the JWK `kty` must match the alg (EC P-256 for ES256, RSA for RS256); a JWK `alg` must equal the token alg; a JWK `use` must be `sig`. HS256 (local): `DecodingKey::from_secret`, `kid` ignored. `Validation::new(alg)` with `algorithms = vec![alg]` (one algorithm only).
5. Validation: issuer `format!("{}/auth/v1", SUPABASE_URL.trim_end_matches('/'))`; audience `["authenticated"]`; `validate_exp`, `validate_nbf`; leeway 5 s; required claims `exp, iss, aud, sub`.
6. Post-checks: `role == "authenticated"` (rejects `anon` and `service_role` tokens even when validly signed); `sub` parses as UUID; `session_id` present and a UUID; `is_anonymous` absent or false.

Supabase facts relied on (verify; links in section 10): JWKS at `${SUPABASE_URL}/auth/v1/.well-known/jwks.json`, cached 10 minutes at Supabase's edge; asymmetric keys are ES256 or RS256; `session_id` is a always-present claim ([JWT fields](https://supabase.com/docs/guides/auth/jwt-fields)); access tokens of revoked sessions stay valid until expiry, which is why the per-request session check exists.

#### 4.3.1 JWKS cache (`auth::jwks`)

- State: `RwLock<JwksCache { keys, fetched_at, last_fetch_ok: bool }>` plus a `Mutex<Instant>` for single-flight and cooldown.
- Startup (production, and local with JWKS): fetch once; failure or zero usable keys exits the process. The Fly machine may auto-stop and cold-start, so the 3 s timeout keeps startup fast.
- Fetch: https only in production, 3 s timeout, response read with `chunk()` and aborted past 64 KiB, no redirects, errors via `without_url()`.
- TTL 600 s, lazy: the first caller after expiry refreshes, others use the stale cache. If refresh fails keep stale keys up to 3600 s total age; beyond that every verification returns `AuthError::Unavailable` (503 `auth_unavailable`), never 401, so users are not signed out by an outage.
- Unknown `kid`: one forced refetch, at most one per 60 s globally; concurrent callers wait for it. Outcome: fetch failed (or cooldown after a failed fetch) -> `Unavailable` (503); fetch succeeded and `kid` still unknown (or cooldown after a successful fetch) -> `Invalid` (401). An anonymous caller therefore cannot cause more than one outbound request per minute (S3).
- A refetch replaces the key set atomically. Accept only `kty` `EC` (crv `P-256`) or `RSA`.

### 4.4 Guard: token, limiter, user and session check (S2, S3, S4, S11)

`auth::guard` runs for every `/api/v1` route, in this order:

1. `TokenVerifier::verify` -> `VerifiedClaims` (signature and claims only; no database access).
2. Per-user limiter, keyed by the verified `user_id` (4.9). This happens BEFORE any database work, so a flood from a valid token cannot reach Postgres. Over the limit -> 429 `rate_limited` with `Retry-After`. Requests that fail step 1 never touch the limiter.
3. One query, always (no caching, S11):

```sql
SELECT u.display_name, u.is_admin, u.is_active,
       app_auth.session_active($2, $1) AS session_ok
FROM app.users u WHERE u.id = $1
```

4. Outcomes: no row or `is_active = false` -> 403 `account_inactive` ("This account is not allowed to use the app."); active but `session_ok = false` -> 401 `unauthorized`; otherwise `AuthUser { id, display_name, is_admin, session_id }` into request extensions (`is_admin` only from this row). A database error -> 503 `auth_unavailable` (not 401, not 500).
5. `impl FromRequestParts for AuthUser` reads extensions and returns 401 if absent (defense in depth).
6. Startup probe: before serving, run `SELECT app_auth.session_active(gen_random_uuid(), gen_random_uuid())`; it must return `false` without error. A permission error or missing function exits the process (fail fast), in every environment.
7. Only if proposal 03's required scratch-project check fails (definer functions cannot read `auth.sessions`), the fallback is its option (c) (drop the session check, S2 downgraded). The check lives in `auth::user::session_ok`, so removal is a one-function change; no runtime off-switch in production.

### 4.5 Authorization: `can_view` cannot be skipped (S10, G7)

All reads of station-scoped data go through `ViewableStation`, constructible only inside `api::access`:

```rust
pub struct ViewableStation { pub(crate) id: Uuid, pub(crate) timezone: chrono_tz::Tz, pub(crate) reference_station_id: Option<String>, pub(crate) forecast_location_id: Option<Uuid> }
pub struct ViewableStations(pub(crate) Vec<ViewableStation>);   // 1..=5, deduplicated
```

Rules:

- Every function in `repo::ytd`, `repo::forecast` (and the deferred `repo::observations`, `repo::rollups`) takes `&ViewableStation(s)`, never a bare station `Uuid`.
- Extractors are the only constructors; each requires `AuthUser`:
  - `ViewableStationPath` reads the path segment `station_id`; `ViewableStationsQuery` reads `station_ids` (comma-separated, 1 to 5 UUIDs, deduplicated). Both call one function, `access::load_viewable(db, viewer, &[Uuid])`, which runs a single statement:

    ```sql
    SELECT s.id, s.timezone, s.reference_station_id, fl.id AS forecast_location_id
    FROM app.stations s
    LEFT JOIN app.forecast_locations fl ON fl.station_id = s.id
    WHERE s.id = ANY($1) AND app.can_view($2, s.id)
    ```

    If fewer rows come back than distinct ids requested, the whole request fails with 404 (no partial results). `ViewableStationPath` passes a one-element array.
- Failure (malformed UUID, nonexistent, not viewable) = 404 `not_found`, identical body. Never 403.
- Forecast access (G7): no route accepts a location id; `forecast_location_id` comes from the join above, after `can_view`.
- The station list uses `WHERE s.id IN (SELECT app.visible_station_ids($1))`, never its own join on `follows`.
- Never cache `can_view` across requests (04 section 7).
- Raw-query handling: `ViewableStationsQuery` reads ONLY the `station_ids` key from the raw query string; it percent-decodes the value BEFORE splitting on commas, so both `a,b` and `a%2Cb` work. Routes that also take other parameters (`/ytd`) parse them from the same raw query with an explicit allow-list type (`YtdParams { year, start, end }`) via a helper that rejects any key outside `{station_ids, year, start, end}` with 422 `validation_failed`; `#[serde(deny_unknown_fields)]` is NOT used on query structs because it would conflict with `station_ids` sharing the query. The same pattern applies to any future multi-station route.
- Guard test `no_raw_station_ids` (7.2) scans `crates/api/src/routes/**/*.rs`: the literal `Path<` must not appear at all (path ids come only from extractors in `access.rs`/`extract.rs`: `ViewableStationPath`, `FollowerId`), and no `Query<T>` struct field may be named `station_id` or `station_ids` (those only exist inside `ViewableStationsQuery` in `access.rs`).
- `FollowerId` (in `extract.rs`) parses the `{follower_id}` segment as a UUID (malformed -> 404). Follow routes concern users, not stations; their authorization is in the SQL `WHERE` clauses (5.8).

### 4.6 Connect flow (S9, G6, G8)

#### 4.6.1 AWN access

The API uses `core::awn::AwnApi::list_devices(&ApiKey)` (owned by 03). Requirements the API relies on: fixed base URL `https://rt.ambientweather.net/v1` (no env var, field or header can change it); `GET /devices?apiKey=<user key>&applicationKey=<app key>` with both keys (https://ambientweather.docs.apiary.io/; 03 confirms field names on first recording); per-call timeout passed by the caller (5 s); no redirects; 256 KiB cap via `chunk()`; errors through `without_url()`; upstream bodies never stored or logged.

Device fields used: `mac` (normalized uppercase, `^([0-9A-F]{2}:){5}[0-9A-F]{2}$`), `name`, `coords: Option<(f64, f64)>`, `timezone: Option<String>` (from `lastData.tz`).

#### 4.6.2 Limits and budget sharing (S9, G8)

Durable per-user limit via `app.connect_attempts`, recorded BEFORE the AWN call so a client disconnect or crash cannot bypass it. Steps 1 to 3 run in ONE transaction:

1. `SELECT pg_advisory_xact_lock(hashtext('connect:' || $user_id))` (released at commit; serializes count+insert per user; the API has no session-level locks, so this is also safe behind a session-mode pooler).
2. Count the user's rows with `attempted_at > now() - interval '1 minute'` (limit 5) and `> now() - interval '1 hour'` (limit 20). If either limit is reached, roll back and return 429 `rate_limited`. Rejections here are NOT recorded. `Retry-After` = whole seconds (rounded up, min 1) until the OLDEST counted row of the binding window leaves that window (`attempted_at + window - now()`); if both windows are exceeded use the larger.
3. `INSERT INTO app.connect_attempts (user_id, outcome) VALUES ($1, 'started') RETURNING id`; commit.
4. Reserve gates, sleep, call AWN (below).
5. After the call: `UPDATE app.connect_attempts SET outcome=$1 WHERE id=$2 AND user_id=$3` with `valid`, `invalid`, `rate_limited` (AWN 429 or gate-busy) or `error`. If the process dies mid-call the row stays `started` and still counts.
6. Opportunistically `DELETE FROM app.connect_attempts WHERE user_id=$1 AND attempted_at < now() - interval '7 days'`.

Both `validate` and `save` count (5/min, 20/hour, config constants).

Gates (G8). The handler (the caller; `core::awn` never reserves) reserves in the same order as the poller, in every `AWN_MODE`:

1. If the caller has a live credential (`status IN ('ok','rate_limited')`, found with the metadata-only SELECT): `wait = reserve(&pool, "awn:key:<that credential_id>", 1000 ms, 3 s)?`, then `tokio::time::sleep(wait)`.
2. `wait = reserve(&pool, "awn:app:api", 1000 ms, 3 s)?`, then `tokio::time::sleep(wait)`.
3. `list_devices` (`LiveAwn::new(app_key, 5 s)`).

`Err(GateBusy)` at either step -> 503 `upstream_busy` with `Retry-After: 2` and attempt outcome `rate_limited`. Because the key gate is reserved and slept first, a busy key gate returns before the app gate is touched, so a user hammering their own key cannot consume the shared app budget. (Reservations cannot deadlock; the order matches the poller's.)

A submitted key that is not the caller's stored key has no credential row, so only the app gate applies; the per-user advisory lock and limits bound its use. Budget: AWN allows 3 req/s per application key; the poller's gate `awn:app:poller` takes 2 req/s, this one 1 req/s. No connection is held while waiting and a cancelled request leaks nothing.

AWN 429: no in-request retry; 503 `upstream_busy` with `Retry-After` = min(AWN's `retry_after`, 30 s; default 2 s).

#### 4.6.3 `validate`

1. Body `{ "api_key": "<string>" }`: 1..=128 chars of `[A-Za-z0-9_-]` else 422 `validation_failed`. Deserialize into `SecretString`; request structs have no `Debug`/`Serialize`.
2. Per-user limit and attempt row (4.6.2), then gate.
3. `list_devices`:
   - `Ok` -> 200 `{ "valid": true, "devices": [{ "mac", "name", "timezone": "<IANA>" | null }] }` (an empty list still `valid: true`).
   - `Unauthorized` -> 200 `{ "valid": false }` only.
   - `RateLimited` -> 503 `upstream_busy`; `Unavailable`/`BadResponse` -> 502 `upstream_unavailable`.
4. No station/credential write; no audit row (the attempts table is the record).

#### 4.6.4 `save`

Body: `{ "api_key": "...", "devices": [ { "mac": "AA:BB:CC:DD:EE:FF", "name": "optional", "timezone": "optional fallback" } ] }` (1 to 10 devices; duplicate MACs in one request -> 422 `validation_failed`).

1. Same validation, per-user limit and gates as validate.
2. Call `list_devices` again (stateless; no pending key held). `Unauthorized` -> 422 `invalid_key`; other failures as validate.
3. Every requested MAC (normalized) must be in the AWN list just returned, else 422 `unknown_device` and nothing is written. MAC, default name and coordinates come only from AWN's response. Station name = the request's override if given, else AWN's `name`, else the fallback `Station <last 4 hex characters of the MAC, colons removed>` (for `AA:BB:CC:00:01:01` -> `Station 0101`). The chosen name is trimmed, control characters removed, and truncated to 80 characters before insert; a name empty after trimming also uses the fallback (04 requires 1 to 80 characters).
4. Timezone is server-derived: use AWN's `timezone` when present and parseable by `chrono-tz`. A client `timezone` is only a fallback when AWN gave none; if AWN gave one and the client's differs, 422 `validation_failed`. If neither exists, 422 `timezone_required`. `std_utc_offset_minutes` is the zone's base (non-DST) UTC offset in minutes (`OffsetComponents::base_utc_offset`).
5. Coordinates: AWN `coords` rounded to 2 decimals in Rust before binding; exact coordinates are discarded. AWN without coordinates -> 422 `validation_failed` (Open questions).
6. Existing MAC: owned by another user -> 409 `device_already_registered` (no owner info); owned by the caller -> only `UPDATE app.stations SET name=$1 WHERE id=$2 AND owner_id=$3`.
7. Encrypt (4.7) and write in ONE transaction following 04's reconnect pattern (the partial unique index forbids two live rows; the composite FK on `stations` is deferred, so the repoint comes last):
   1. `SELECT id FROM app.credentials WHERE owner_id=$1 AND status <> 'replaced'` -> `old_id` (metadata columns only).
   2. `new_id = Uuid::new_v4()` in the API (needed for AAD); `encrypt_credential(&pubkey, key_version, new_id, owner_id, &api_key)`.
   3. If `old_id` exists: `UPDATE app.credentials SET status='replaced', status_changed_at=now() WHERE id=$old AND owner_id=$caller` (never overwrites ciphertext).
   4. `INSERT INTO app.credentials (id, owner_id, api_key_ciphertext, encapsulated_key, key_version) VALUES (...) RETURNING id, status` (no ciphertext in `RETURNING`; no `ON CONFLICT`).
   5. Insert new `app.stations` and `app.forecast_locations (station_id, lat, lon)` rows (`credential_id = new_id`; `reference_station_id` NULL).
   6. Repoint covered stations: `UPDATE app.stations SET credential_id=$new WHERE owner_id=$caller AND credential_id <> $new AND mac = ANY($covered)` where `$covered` = the normalized MACs in the new AWN list. The predicate is `<> $new` (not `= $old`) so stations that an earlier reconnect left on a `replaced` credential are recovered when a later key covers them. Then `not_covered` = `SELECT id, name FROM app.stations WHERE owner_id=$caller AND credential_id <> $new`, i.e. all of the caller's stations not on `$new`; they remain on a `replaced` credential (the poller skips them; an admin may disable them). There is no 422 for them. On first connect (no earlier stations) this does nothing.
   7. Audit rows (04 vocabulary): first connect: `credential.saved` (subject = new credential). Reconnect: BOTH `credential.saved` (subject = new credential) and `credential.replaced` (subject = old credential, `detail: {"new_credential_id"}`). Plus `station.created` (subject = the new station id) per new station.
8. One 23505 behaviour: any unique violation rolls the transaction back and returns 409. When the violated constraint is `stations_mac_key`, run a follow-up `SELECT owner_id FROM app.stations WHERE mac=$1`: owner is the caller -> `conflict`; another owner -> `device_already_registered`. A credentials partial-unique violation (concurrent connect by the same user) -> `conflict`. Never a 500, never a retry.
9. Response 201 `{ "credential": { "status": "ok" }, "stations": [ <5.3 station object> ], "not_covered": [ { "id": "uuid", "name": "..." } ] }` (`stations` = created or updated by this call; `not_covered` is empty on first connect). Record the attempt outcome `valid`.
10. The poller picks up new rows on its next run; the API does not trigger polling. `reference_station_id` is assigned later (Open questions).

#### 4.6.5 Replay mode

`AWN_MODE=replay` (local/test only) uses `core::awn::ReplayAwn` reading `$FIXTURES_DIR/awn/` (fixtures reach the container only through the volume mount): `fixture-valid-key` -> `devices_ok.json` (MACs `AA:BB:CC:00:01:01` and `AA:BB:CC:00:01:02`, not seeded, with `info.coords` and `lastData.tz`), `fixture-invalid-key` -> 401, `fixture-ratelimited-key` -> 429, anything else -> 401. The gate and per-user limits still apply.

#### 4.6.6 Credential status

`GET /api/v1/connect/status` reads the caller's current credential (`WHERE owner_id=$1 AND status <> 'replaced' ORDER BY created_at DESC LIMIT 1`, metadata columns only) and returns `status`, `last_ok_at`, `needs_reconnecting = (status == 'invalid')`. Owner only.

### 4.7 Credential encryption (S8, S5)

Use `core::crypto` exactly as specified in `04-database.md` section 9 (owned by 04; do not redefine): `encrypt_credential(&CredentialPublicKey, key_version: i16, credential_id: Uuid, owner_id: Uuid, &SecretString) -> Result<EncryptedCredential, CryptoError>` (public key parsed with `parse_public_key`), HPKE base mode, X25519/HKDF-SHA256/AES-256-GCM, `info` `b"family-weather/credential/v1"`, AAD `credential_id || owner_id` (16 raw bytes each), `key_version = CREDENTIALS_KEY_VERSION`.

`decrypt_credential` is behind `core` feature `decrypt`, enabled only by `poller` and tests. CI check: `cargo tree -p api -e normal,build,features -f '{p} {f}'` must not show `decrypt` for `core` (dev-dependency edges are excluded on purpose). The round-trip decrypt test lives in `core` (04's crypto tests), not in `api`; the `api` crate has no dev-dependency that enables `decrypt`.

### 4.8 Conventions for all routes

JSON only; bodies require `Content-Type: application/json`; JSON request-body structs use `#[serde(deny_unknown_fields)]` (query structs use explicit allow-lists, 4.5); timestamps RFC 3339 UTC; local dates `YYYY-MM-DD`; temperatures in degrees F, rain in inches; UUIDs lowercase hyphenated. JSON/Query rejections map to 422 `validation_failed` with a fixed message (never axum's default text), except: a body-length-limit failure (including chunked bodies without `Content-Length`) -> 413 `payload_too_large`; a malformed UUID in a station path, `station_ids` or `{follower_id}` -> 404 `not_found`.

### 4.9 Rate limiting (S3)

One flat in-memory per-user limiter, `limits.rs`: `governor::DefaultKeyedRateLimiter<Uuid>`, 120 requests per minute per user, burst 40, key = verified user id, applied inside `auth::guard` right after token verification and before any database access (4.4). Every request costs 1. Over the limit -> 429 `rate_limited` with `Retry-After`. Connect routes additionally have the durable limits in 4.6.2. The limiter is in memory because the API is a single machine (P3) and resets if it auto-stops; the sensitive limits (connect, AWN gates) are in Postgres.

Unauthenticated requests are never keyed by IP. Flood protection: `LoadShed` + `GlobalConcurrencyLimit(64)`, the overall 25 s timeout, 16 KiB body limit, token failures cost no DB access, the JWKS refetch cooldown, the health query's 2 s timeout. Evict idle limiter keys every 10 minutes (`retain_recent`, `shrink_to_fit`). Never read `X-Forwarded-For`, `Forwarded`, `X-Real-IP` or `Fly-Client-IP` for any security decision.

### 4.10 Optional Netlify signed-proxy check (S3)

When `NETLIFY_PROXY_SECRET` is set (conventions: `signed = "NETLIFY_PROXY_SECRET"` in `netlify.toml`, commented out for the first deploy and enabled later with the API secret; implement and test it AFTER the first deploy, task 17), middleware verifies header `x-nf-sign` on every request except `GET /api/health`: HS256 JWS, key `NETLIFY_PROXY_SECRET`, `iss == "netlify"`, valid `exp` (leeway 5 s). Failure -> 403 `forbidden` before token processing. Netlify's claims are `iss`, `exp`, `deploy_context`, `netlify_id`, `site_url` (https://docs.netlify.com/manage/routing/redirects/rewrites-proxies/). `deploy_context` is not checked (previews are off, S14). Unset locally. Defense in depth against direct `fly.dev` calls; never a replacement for token and user checks.

### 4.11 Errors, redaction, logging (S7)

- Shared codes (conventions): `unauthorized` 401, `account_inactive` 403, `forbidden` 403, `not_found` 404, `rate_limited` 429, `validation_failed` 422. API-specific: `invalid_key` 422, `unknown_device` 422, `timezone_required` 422, `device_already_registered` 409, `conflict` 409, `request_timeout` 408, `payload_too_large` 413, `overloaded` 503, `auth_unavailable` 503, `upstream_busy` 503, `upstream_unavailable` 502, `internal` 500.
- Body always `{"error":{"code","message"}}` with fixed static messages, never echoing input. `internal` logs the cause server-side and returns "Something went wrong." plus the `x-request-id` header.
- Secret handling:
  - Secrets are `SecretString`; `#[derive(Debug)]` is forbidden on `ConnectValidateRequest`, `ConnectSaveRequest`, `ApiConfig`, `TokenVerifier` (grep test, 7.4).
  - Every `reqwest::Error` goes through `.without_url()` at the call site (`core::redact::http_error`, which calls `without_url()`), including JWKS fetches.
  - Logs never record `request.uri()`. `TraceLayer::make_span_with` records only `request_id`, `method`, `route` (`MatchedPath`), later `status`, `latency_ms`, `user_id`. No headers, no bodies. `SetSensitiveRequestHeadersLayer` marks `authorization`, `x-nf-sign`, `cookie`.
  - Panic hook from `core::redact::install_panic_hook` is installed first in `main` (logs file:line and a fixed message, never the payload). `CatchPanicLayer` returns the 500 envelope. `panic = "unwind"`.
  - sqlx statement logging off, `sqlx=warn`; bind values (which can include ciphertext) are never logged.
- Logging: JSON lines in production, human format locally; default `RUST_LOG=info,tower_http=info,sqlx=warn,hyper=warn`. One access-log event per request. Retention is set at Fly (06). No metrics endpoint in v1.

### 4.12 Database connection (S12, C3)

Use `core::db` with a `PoolSpec { max: 5, min: 0, acquire_timeout: 5 s, idle_timeout: 5 min, max_lifetime: 30 min, test_before_acquire: true, after_connect }` (role limit for `app_api` is 20; `min = 0` because the machine may auto-stop); `after_connect`: `SET statement_timeout = '10s'; SET idle_in_transaction_session_timeout = '15s'; SET TIME ZONE 'UTC'`. Queries use fully qualified names (`app.`, `app_auth.`).

Connect from Fly via the direct IPv6 host (`db.<ref>.supabase.co`) or Supavisor in session mode (port 5432, username `app_api.<project-ref>`), never transaction mode (port 6543 refused at startup), in the same region as Fly (C3). The API holds no session-level locks (the AWN gate is a table reservation), but named prepared statements still require session semantics; keep sqlx's default statement cache (100). Do not set `statement_cache_capacity(0)` to tolerate transaction mode without a design decision.

The API does not run migrations. Queries use `sqlx::query!`/`query_as!` with committed `.sqlx/` (`SQLX_OFFLINE=true` in Docker and CI; CI `cargo sqlx prepare --workspace --check`). NUMERIC columns are cast explicitly (`lat::float8`, `$1::float8`); no `rust_decimal`. Dynamic SQL strings are forbidden.

### 4.13 Audit log (S16)

`audit::record(tx, actor, action, subject_type, subject_id, detail)` inserts into `app.audit_log (actor_id, action, subject_type, subject_id, detail)` in the same transaction as the state change; `detail` <= 4096 bytes, never keys, tokens or emails. The actions are exactly 04's list for the `api` writer, with 04's subject rule: `follow.requested`, `follow.approved`, `follow.declined`, `follow.revoked` (`subject_type` `follow`, `subject_id` = the FOLLOWER's user id, `detail` = `{"followee_id": "<uuid>"}`); `credential.saved` (`credential`, the new row), `credential.replaced` (`credential`, the old row, written together with `credential.saved` on reconnect); `station.created` (`station`). `actor_id` is the caller. The API writes no `credential.validate` rows and no login rows (logins stay in GoTrue's own audit).

### 4.14 Dockerfile (api build step and api checks)

`backend/Dockerfile` is created by 03's core milestone (conventions: one image, binaries present only for crates that exist); this guide does NOT create it. When `crates/api` lands, add to the existing Dockerfile and its CI checks:

1. The `api` build step in the `builder` stage: `cargo build --release --locked -p api`, run separately from `-p poller -p admin` so `decrypt` (enabled by `poller`) cannot leak into `api` through feature unification, and copy `/usr/local/bin/api` into the final stage. `.sqlx/` must include the API's query data (`cargo sqlx prepare --workspace`).
2. Final stage requirements the API depends on (already specified by conventions and 03; verify, do not redefine): distroless `cc-debian12:nonroot` (no shell, uid 65532), `COPY certs/ /etc/ssl/app-certs/` (CA at `/etc/ssl/app-certs/supabase-ca.crt`; production config refuses to start if the file named by `sslrootcert` is missing), default command overridable (`famweather-api` runs `api`), no fixtures in the image (replay fixtures only via the volume at `FIXTURES_DIR`), read-only root filesystem works.
3. API image checks added to CI (no shell in the image): `docker create --name img-check <image>`; `docker cp img-check:/usr/local/bin/api -` exists; when `backend/certs/supabase-ca.crt` exists in the repo, `docker cp img-check:/etc/ssl/app-certs/supabase-ca.crt -` hashes equal to it; `docker image inspect` shows user 65532; the feature-isolation `cargo tree` check from 4.7 (fails on `decrypt`, `test-support`, `testdb` in `api`).
4. `api healthcheck` is the container health command for Compose; Fly uses HTTP `GET /api/health`. `api` listens on `ApiEnv.bind_addr`; SIGTERM -> 25 s graceful shutdown.

---

## 5. HTTP contract (authority for 01-web)

All paths are under `/api/v1` except health. All routes except health require `Authorization: Bearer <access token>`.

### 5.0 Endpoint list

| # | Method and path | Purpose | Success | Status / notes |
| --- | --- | --- | --- | --- |
| 1 | `GET /api/health` | liveness + DB ping | 200 | v1; no auth |
| 2 | `GET /api/v1/me` | caller profile | 200 | v1 |
| 3 | `GET /api/v1/stations` | stations caller can view | 200 | v1 |
| 4 | `GET /api/v1/stations/{station_id}` | one station | 200 | v1; station-scoped |
| 5 | `GET /api/v1/stations/{station_id}/observations?from&to` | raw 5-minute rows | 200 | DEFERRED (not implemented in v1; 01 does not use it) |
| 6 | `GET /api/v1/rollups/daily?station_ids&from&to` | daily rollups, many stations | 200 | DEFERRED (not implemented in v1; 01 does not use it) |
| 7 | `GET /api/v1/ytd?station_ids&year&start&end` | cumulative rain/GDD, prior years, bands | 200 | v1; multi-station (<= 5) |
| 8 | `GET /api/v1/stations/{station_id}/forecast/history?variable&model&valid_from&valid_to&runs` | forecast change across runs | 200 | v1 (last task) |
| 9 | `GET /api/v1/follows` | caller's follows (both directions) | 200 | v1 |
| 10 | `POST /api/v1/follows` | request to follow a user | 201 / 200 | PROVISIONAL (depends on follow-discovery decision) |
| 11 | `POST /api/v1/follows/{follower_id}/approve` | owner approves | 200 | v1 |
| 12 | `POST /api/v1/follows/{follower_id}/decline` | owner declines a pending request | 200 | v1 |
| 13 | `POST /api/v1/follows/{follower_id}/revoke` | owner revokes an approved follow | 200 | v1 |
| 14 | `POST /api/v1/connect/validate` | check an AWN key, list devices | 200 | v1; durable limits |
| 15 | `POST /api/v1/connect/save` | save credential and stations | 201 | v1; durable limits |
| 16 | `GET /api/v1/connect/status` | caller's credential status | 200 | v1 |

Follows are addressed by the other party's user id. `follower_id` in routes 11 to 13 is the follower's `user.id` as returned in `followers[].user.id` by route 9. The caller is always the followee, taken from the token. There is no follow id. A follower cannot cancel a request or unfollow in v1 (Open questions). There are no admin endpoints.

Common error responses for routes 2 to 16 (omitted below): 401 `unauthorized`, 403 `account_inactive`, 403 `forbidden` (proxy signature only), 404 `not_found`, 408 `request_timeout`, 413 `payload_too_large`, 422 `validation_failed`, 429 `rate_limited`, 500 `internal`, 503 `auth_unavailable` or `overloaded`. Station-scoped routes return 404 for stations that do not exist or are not viewable (identical body).

### 5.1 `GET /api/health` (no auth)

200 `{"status":"ok"}` or 503 `{"status":"unavailable"}` from a plain `SELECT 1` with a 2 s timeout (4.2). No version, host or config data.

### 5.2 `GET /api/v1/me`

```json
{ "id": "uuid", "display_name": "Rodney", "is_admin": true,
  "credential": { "status": "ok" } | null }
```

`credential.status` is `ok`, `invalid` or `rate_limited` (current non-`replaced` credential), or `null`.

### 5.3 `GET /api/v1/stations`

Stations the caller can view (own plus approved followees). Order: own first, then owner display name, then station name.

```json
{ "stations": [ {
  "id": "uuid", "name": "Backyard", "is_own": true,
  "owner": { "id": "uuid", "display_name": "Rodney" },
  "lat": 46.59, "lon": -112.03, "coordinates_rounded": true,
  "timezone": "America/Denver",
  "baseline": { "reference_station_id": "USW00024144", "name": "Helena Regional Airport", "source": "ghcnd_acis" } | null,
  "latest_observation_at": "2026-10-02T17:55:00Z" | null
} ] }
```

`lat`/`lon` are `stations.lat::float8` (stored at 2 decimals). `latest_observation_at` = `stations.last_observed_at`. `baseline` joins `app.reference_stations`. Never returned: `mac`, `credential_id`, owner email, `is_enabled`, sync columns. An empty list is valid (G5).

### 5.4 `GET /api/v1/stations/{station_id}`

Same object as one list element.

### 5.4a DEFERRED: observations and daily rollups (reserved contract, not implemented in v1)

`GET /api/v1/stations/{station_id}/observations?from&to` (span <= 7 days, <= 2100 rows) and `GET /api/v1/rollups/daily?station_ids&from&to` (span <= 400 days; per day `local_date, tmax_f, tmin_f, rain_in, gdd, gdd_method, is_complete, rain_reconcile_status, rain_reconcile_diff_in`) are reserved. Reserved shapes: observations return `{"station_id","from","to","observations":[{"ts", <app.observations sensor columns by their 04 names>}]}`; rollups return `{"day_definition","stations":[{"station_id","days":[...]}]}`. When scheduled, specify them fully in this section (task 18) and update 01-web. Until then the routes do not exist (404 from the fallback) and 01-web must not call them.

### 5.5 `GET /api/v1/ytd?station_ids=<uuid,uuid>&year=2026&start=04-01&end=10-31`

`station_ids` 1 to 5 UUIDs; `year` REQUIRED (2000..=current year); `start`/`end` `MM-DD` (defaults `04-01`/`10-31` per F6; `start <= end`).

```json
{ "year": 2026, "start": "04-01", "end": "10-31",
  "day_definition": "local_standard_time_midnight_to_midnight",
  "stations": [ {
    "station_id": "uuid",
    "baseline": { "reference_station_id": "USW00024144", "name": "...", "source": "ghcnd_acis" } | null,
    "gdd_overlay": { "available": true, "station_method": "avg_50", "baseline_method": "avg_50" },
    "current": { "first_date": "2026-04-01" | null,
                 "last_date": "2026-10-02" | null,
                 "last_complete_date": "2026-10-01" | null,
                 "missing_days": { "rain": 0, "gdd": 0 },
                 "rain_cum_in": [ { "md": "04-01", "v": 0.0 } ],
                 "gdd_cum":     [ { "md": "04-01", "v": 2.3 } ] },
    "prior_years": [ { "year": 2025, "missing_days": { "rain": 3, "gdd": 3 },
                       "rain_cum_in": [ ] | null, "gdd_cum": [ ] | null } ],
    "bands": { "percentiles": [10, 25, 50, 75, 90], "years_included": { "rain": 30, "gdd": 29 },
               "rain_cum_in": [ { "md": "04-01", "p": [0.0, 0.0, 0.1, 0.2, 0.4] } ] | null,
               "gdd_cum":     [ { "md": "04-01", "p": [0.0, 0.0, 1.0, 2.0, 3.0] } ] | null } | null } ] }
```

Computation (SQL window functions in `repo::ytd`):

- **Current series** from `app.daily_rollups` for `year` within `[start, end]`. It starts at the station's FIRST rollup date on or after `start` (`current.first_date`); days before it are neither points nor missing (a station installed mid-season shows a partial total, which is why `first_date` is returned). Rain cumulates `rain_in` and GDD cumulates `gdd` whenever the value is non-NULL, regardless of `is_complete` (the open current day therefore appears as a partial value; `last_complete_date` tells the UI which day is partial). Points are emitted through `last_date` (the latest row in the window); no zero-fill into the future.
- **Missing** means a NULL value or an absent row, counted separately per series between `first_date` and `last_date`: `current.missing_days.rain` and `.gdd`. A missing day adds 0 to the running sum. `is_complete = false` is not "missing".
- **Baseline** (prior years and bands) comes from `app.reference_daily` for `stations.reference_station_id` ONLY; the station's own past years are not used in v1 (Open questions). Rain = running sum of `precip_in`; a day with `precip_trace = true` (where `precip_in` is NULL) counts as 0.00 and is NOT missing; only a NULL `precip_in` with `precip_trace = false`, or an absent row, is missing (adds 0, counted). GDD = running sum of the stored `gdd` (NULL adds 0 and counts as missing).
- **Candidate years** = every year from the first year present in `reference_daily` for that reference station through the current year, EXCLUDING the requested `year`. Years later than the requested `year` count (when `year` is historical, `prior_years` can include later years; the key name is kept for web compatibility). A partially synced current year is naturally ineligible through the missing-day rule.
- **Eligibility is per series.** A year is eligible for rain when its rain `missing_days <= 7` over `[start, end]` (absent rows included), and eligible for GDD when its GDD `missing_days <= 7`.
- `prior_years`: the most recent `PRIOR_YEARS = 5` candidate years eligible for at least one series, newest first; for each, a series array is `null` when that year is ineligible for that series.
- `bands`: per series, `percentile_cont` at 10/25/50/75/90 per `md` over the cumulative values of all eligible years; `years_included.rain` and `.gdd` count them separately; a series' band array is `null` when fewer than 10 years are eligible for it; `bands` is `null` when both are.
- Feb 29: its value stays in the running sum, but no point is emitted for `02-29` (series are keyed by `md` and aligned across years without it).
- **GDD method guard:** collect the distinct non-null `gdd_method` values over the station window and over the eligible baseline years. If either set has more than one value, or the two differ, set `gdd_overlay` to `{"available": false, "reason": "gdd_method_mismatch", "station_method": <string|null>, "baseline_method": <string|null>}`, keep `current.gdd_cum`, and return `null` for every `prior_years[].gdd_cum` and `bands.gdd_cum` (rain unaffected). Still 200. The API never computes GDD.
- No baseline: `baseline: null`, `prior_years: []`, `bands: null`, `gdd_overlay` = `{"available": false, "reason": "no_baseline", ...}`.
- `gdd_overlay` and `day_definition` are returned so the UI never hard-codes labels (F5).

### 5.6 `GET /api/v1/stations/{station_id}/forecast/history`

Query: `valid_from` and `valid_to` (both REQUIRED, RFC 3339, span <= 16 days), `variable` (optional, default and only allowed value `precip_6h_in`; anything else 422), `model` (optional; default = the model of the newest `run_time` for the location), `runs` (1..=12, default 6).

Semantics: `runs` = the newest N `app.forecast_runs` rows for `(location, model)` by `run_time`; for each, `points` = its `forecast_snapshots` rows for `variable` with `valid_time` in `[valid_from, valid_to]`. Access via the station's `forecast_locations` row (G7).

```json
{ "station_id": "uuid", "variable": "precip_6h_in", "model": "ecmwf_ifs025", "period_hours": 6,
  "attribution": "Weather data by Open-Meteo.com (CC BY 4.0)",
  "runs": [ { "run_time": "2026-10-02T06:00:00Z", "fetched_at": "...",
              "points": [ { "valid_time": "2026-10-03T00:00:00Z",
                            "p10": 0.0, "p50": 0.1, "p90": 0.4, "prob_precip": 35.0 } ] } ] }
```

`valid_time` is the UTC start of the 6-hour window; `prob_precip` is 0 to 100. Runs are newest first. No data -> `{"runs": []}` (200). Forecast-vs-gauge is not in this route (after v1).

### 5.7 Connect

- `POST /api/v1/connect/validate` body `{"api_key":"..."}` -> 200 `{"valid":true,"devices":[{"mac":"AA:BB:CC:DD:EE:FF","name":"Backyard","timezone":"America/Denver"|null}]}` or 200 `{"valid":false}`. Also 503 `upstream_busy` (`Retry-After`), 502 `upstream_unavailable`, 429 `rate_limited` (`Retry-After` per 4.6.2).
- `POST /api/v1/connect/save` body `{"api_key":"...","devices":[{"mac":"...","name":"optional","timezone":"fallback, optional"}]}` -> 201 `{"credential":{"status":"ok"},"stations":[<5.3 station object>],"not_covered":[{"id":"uuid","name":"..."}]}` (`not_covered` = the caller's stations that stayed on the replaced credential because their MAC is not in the new key's AWN list; empty on first connect). Errors: 422 `invalid_key`, `unknown_device`, `timezone_required`, `validation_failed`; 409 `device_already_registered`, `conflict`; 503, 502, 429.
- `GET /api/v1/connect/status` -> 200 `{"credential":null}` or `{"credential":{"status":"ok","last_ok_at":"2026-10-02T17:55:00Z"|null,"needs_reconnecting":false}}`.

Never returned: the key, ciphertext, `key_version`, `credential_id`, AWN error details.

### 5.8 Follows

States (04 CHECK): `pending`, `approved`, `declined`, `revoked`. A row `(follower_id=A, followee_id=B, approved)` lets A see B's stations. Transitions are enforced here.

- `GET /api/v1/follows` -> 200

```json
{ "following": [ { "user": { "id": "uuid", "display_name": "Dana" }, "status": "approved", "requested_at": "...", "decided_at": "..." | null } ],
  "followers": [ { "user": { "id": "uuid", "display_name": "Sam" }, "status": "pending", "requested_at": "...", "decided_at": null } ] }
```

  `following` = rows where `follower_id = caller` (`user` is the followee). `followers` = rows where `followee_id = caller` (`user` is the follower; its `id` is the `{follower_id}` of routes 11 to 13). Only `id` and `display_name` of the other party; only active users.
- `POST /api/v1/follows` (PROVISIONAL) body `{"followee_display_name":"Dana"}` -> 201 `{"user":{"id","display_name"},"status":"pending"}`. Lookup: active user with `lower(btrim(display_name)) = lower(btrim($1))` (04 has the unique index `users_display_name_key` on that expression, so there is at most one match); no match or inactive -> 404 `not_found`; self -> 422 `validation_failed`; existing `pending` or `approved` -> 200 with the current row; existing `declined`/`revoked` -> `UPDATE app.follows SET status='pending', decided_at=NULL WHERE follower_id=$caller AND followee_id=$target AND status IN ('declined','revoked')` and 200 (grant `UPDATE (status, decided_at)`; `requested_at` is not reset); new row: `INSERT (follower_id, followee_id)` with default `pending`. The exact discovery mechanism is an Open question; this route may change.
- `POST /api/v1/follows/{follower_id}/approve` -> `UPDATE app.follows SET status='approved', decided_at=now() WHERE follower_id=$1 AND followee_id=$caller AND status='pending'`. 200 with `{"user":{...},"status":"approved","requested_at","decided_at"}`; zero rows -> 404 `not_found` (wrong caller or wrong state, same body). Approval shares all of the owner's stations.
- `.../decline` -> same with `status='declined'`, only from `pending`.
- `.../revoke` -> same with `status='revoked'`, only from `approved`. Effective on the next station read.
- Not implemented (Open questions): a follower cancelling a pending request or unfollowing; blocking re-requests after decline.
- Each transition inserts one audit row in the same transaction (4.13).

### 5.9 Admin endpoints

None in v1 (G4): admin actions use the `admin` CLI. `is_admin` in `/me` is display only. A future admin route must use a `RequireAdmin` extractor reading `AuthUser.is_admin` and must first be added to this guide.

---

## 6. Implementation tasks (in order)

Each task also requires `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && make test-backend` to pass. Build order (conventions): 04 and 03 land first; this component consumes their modules and does not define them. If a consumed module is missing, stub it behind the trait and flag it; do not implement it inside `api`.

1. **Skeleton.** Create `crates/api` per 4.1 with pinned versions and committed `Cargo.lock`; confirm `backend/Cargo.toml` has `resolver = "2"` in `[workspace]`. Done when `cargo build --workspace` succeeds and the feature-isolation check `cargo tree -p api -e normal,build,features` shows none of `decrypt`, `test-support`, `testdb` (CI fails on any).
2. **Config.** `ApiEnv` (4.1) and `api::config::ApiConfig::from_env()` calling `core::config::load(Binary::Api)` plus the three API-specific checks of 4.1, manual `Debug`, `for_test`, the `test-support` feature and self dev-dependency. Done when each API-specific check has a failing and a passing unit test, one test confirms the shared refusals surface unchanged (for example a demo `JWT_HS256_SECRET` in production fails with `refusing JWT_HS256_SECRET: demo value` or the HS256 refusal, and a missing CA file fails), and messages contain variable names only. The shared rules themselves are tested in `core`.
3. **Error type, layers, telemetry.** `lib.rs` + `error.rs`, `build_router(state, extra)` in the exact layer order of 4.2 (test-only `extra` routes under `cfg(any(test, feature = "test-support"))`, `ApiConfig::for_test`), `strip_incoming_request_id`, `envelope_fixup`, `LoadShed`, fallback, `api healthcheck`. Done when tests show: a panicking `extra` route returns the 500 envelope, and the log contains 03's fixed panic message (`panic`) with a file:line location but never the panic payload (use a distinctive payload string and assert it is absent); a client-supplied `x-request-id` is replaced; a 404, a 413 with `Content-Length` 17 KiB, a 413 from a CHUNKED 17 KiB body without `Content-Length` (through the `Json` extractor), a 408 (slow `extra` route past a test-shortened timeout) and an `overloaded` 503 (saturate ONE slow `extra` route with 64 in-flight requests, then a request to a DIFFERENT route gets 503 `overloaded`, proving the limit is global) all return the JSON envelope with `Cache-Control: no-store` and `X-Content-Type-Options: nosniff`.
4. **DB pool and health.** Use `core::db`; `/api/health` as a plain `SELECT 1` with a 2 s timeout. Done when health is 200 against the test database, 503 when the pool cannot connect, and 1000 concurrent anonymous health calls never exceed the pool size or the concurrency cap (excess get 503 `overloaded` quickly).
5. **JWKS cache.** 4.3.1 with an injectable fetcher. Done when tests cover startup fail-fast, TTL refresh, unknown-kid single refetch, cooldown behavior (401 after a successful fetch, 503 after a failed one), the 3600 s stale cap giving 503, and key removal after refetch.
6. **Token verification.** 4.3. Done when every case in 7.1 passes.
7. **Guard: limiter, user/session check, route registry.** `auth::guard` (4.4, 4.9), `register`/`RouteSpec` (7.2), startup probe for `app_auth.session_active`. Done when: persona tests for `/me` pass; a valid token whose session is gone gets 401; probe failure exits the process; two users have independent 429 buckets; an unauthenticated request consumes no bucket; and with the user over the limit a request issues ZERO database queries (lazy unreachable pool still returns 429).
8. **Access module.** 4.5. Done when stranger/pending/declined/revoked/mixed-list tests pass on stub station routes and `no_raw_station_ids` is green.
9. **Stations** (5.3, 5.4). Done when responses match the contract, coordinates have <= 2 decimals, and the persona matrix passes.
10. **YTD** (5.5). Done when seeded `daily_rollups`/`reference_daily` verify: series start at the first rollup date; incomplete-day values are summed; NULL/absent days add 0 and count per series; per-series eligibility and separate `years_included`; later-than-requested years included; Feb 29 in the sum without a point; trace days (`precip_trace=true`, NULL `precip_in`) count 0.00 and are not missing while NULL without trace and absent rows are; `/ytd` rejects an unknown query key with 422 and parses `station_ids` through `ViewableStationsQuery` only; `year` required; the GDD method-mismatch and no-baseline cases; bands from reference only; and the persona matrix.
11. **Follows** (5.8; `POST /follows` provisional) with audit rows. Done when every transition from every state, with the right and wrong caller, is tested and each writes exactly one audit row.
12. **Connect flow** (4.6) using `core::awn` (`ApiKey`, `AwnDevice`, 5 s per-call timeout), `core::ratelimit::reserve` (caller-side app and key gates), `core::crypto::encrypt_credential`. Done when the tests in 7.3 pass.
13. **Forecast history** (5.6). Done when it returns seeded data for a visible station within `[valid_from, valid_to]`, `{"runs":[]}` when empty, 404 for hidden stations, 422 for any variable other than `precip_6h_in`, and the persona matrix passes.
14. **Image: api build step and checks** (4.14; the Dockerfile itself exists from 03's core milestone). Done when the existing image builds with `api` included, `api` runs as uid 65532 with `--read-only`, `api healthcheck` works, a start with empty env exits non-zero without printing values, and the CI image checks of 4.14 item 3 pass.
15. **Compose integration** with 05: `api` healthy and reachable at `http://localhost:8000/api/health` through Caddy; 7.5 passes.
16. **CI hooks** (with 06): `cargo sqlx prepare --workspace --check`, the decrypt-feature check, `cargo deny`/`cargo audit`, image check. Done when a PR run is green.
17. **After first deploy: Netlify signature layer** (4.10). Done when tests show: unset secret disables it; set secret -> missing/invalid/expired/wrong-`iss` `x-nf-sign` is 403 on `/api/v1/*` and health stays open.
18. **Deferred (post-v1): observations and daily rollups** (5.4a), when 01-web needs them.

---

## 7. Testing

Follows the conventions Testing harness. Entry point: `make test-backend`, which runs in the separate `famweather-test` Compose project (throwaway `db`, never the developer's stack; CI uses the same `make` targets) via the `test-backend` service after `migrate`. Env provided by that service: `TEST_SUPERUSER_DATABASE_URL`, `TEST_MIGRATOR_DATABASE_URL`, `DATABASE_URL` (superuser URL, read by `#[sqlx::test]`), `TEST_JWT_HS256_SECRET`, `SUPABASE_URL=http://auth:9999` (the service-role JWT is minted in-test from the HS256 secret), `MAILPIT_URL=http://mail:8025`, `FIXTURES_DIR=/work/backend/fixtures`, demo `CREDENTIALS_*` keys. Run as the conventions' two steps: `cargo test --workspace --exclude admin --features api/test-support`, then `cargo test -p admin --features compose-tests -- --test-threads=1` (the second step is 04's admin tests, not the API's).

- Each DB test is `#[sqlx::test(migrations = false)]` calling `core::testdb::prepare` with the superuser URL (04). The API under test uses `core::testdb::pool_as(&db_name, "app_api")` (connections run `SET ROLE app_api`), so every handler query is subject to `app_api` grants. Seeding uses the superuser pool. The auth stub supplies `auth.sessions` and `app_auth.session_active`.
- "Zero DB queries" assertions use an `AppState` whose pool is `PgPoolOptions::connect_lazy` to an unreachable address (`postgres://x@127.0.0.1:1/x`): any query would turn the expected 401 (token rejection) into 503 `auth_unavailable`, so a passing 401 proves no query ran.
- Test-only routes (panic, slow, stub station routes) are passed as `extra` to `build_router` under `cfg(any(test, feature = "test-support"))`; they live under `tests/` and are exempt from the source scans. Tests build config with `ApiConfig::for_test`.
- Grant and exposure assertions live in `ops/db/tests/run.sh` (04), not here; API tests only assert the API-visible effects (e.g. `SELECT api_key_ciphertext` under `SET ROLE app_api` fails).

### 7.1 Token validation (S1, D2)

Tokens built in-test with generated ES256 and RSA keys served through the in-memory JWKS fetcher, and with the local HS256 secret. Each rejection asserts the status, and runs against the lazy unreachable pool (proves no DB query):

- control: valid token accepted (with a real pool).
- anon key (role `anon`, no `sub`, correctly signed with the local HS256 secret; also an asymmetric-signed token with `role: "anon"`); service-role key (`role: "service_role"`) -> 401.
- `alg: none` (empty signature; and a plausible payload) -> 401.
- HS256 in production config, including HS256 signed with the public key bytes as secret (algorithm confusion); ES256 in local-HS256 mode -> 401.
- expired beyond leeway; `nbf` in the future; missing `exp` -> 401.
- wrong `iss`; wrong `aud` (`anon`, `authenticated2`) -> 401.
- unknown `kid`: exactly one refetch then 401; a second unknown `kid` within 60 s -> 401 with zero fetches; a rotated-in key is accepted after the single refetch; unknown `kid` when the refetch fails -> 503 `auth_unavailable`.
- JWKS cache older than the 3600 s stale cap and refetch failing -> 503 `auth_unavailable` (not 401).
- missing `kid`; JWK `alg` mismatch; embedded `jwk`/`jku` header ignored (attacker-signed token rejected) -> 401.
- missing or non-UUID `session_id`; non-UUID `sub`; `is_anonymous: true` -> 401.
- token > 8 KiB; malformed base64; garbage parts -> 401.
- Config: the API-specific checks of 4.1 (production https/non-local issuer and JWKS, JWKS required, local exactly-one-of HS256/JWKS, HS256 with `SUPABASE_URL` other than `http://localhost:8000`), plus a smoke test that `core::config` refusals reach `main` unchanged.

### 7.2 Authorization (S4, S10, S11, S2)

Personas: `owner`, `approved_follower`, `stranger`, `pending`, `declined`, `revoked`, `inactive`, `no_app_row`, `deleted_session`, `anonymous`.

axum cannot enumerate its routes, so the registry is built by the `register` helper: each `RouteSpec { name, method, path_template, kind: Public | User | Station | Stations, sample: fn(&World) -> SampleRequest, expected: &'static [(Persona, u16)] }` carries a sample request and the expected status per persona as DATA. A source-scan test forbids `.route(` outside `register`. The table test iterates `ROUTES` and, for each route and persona, sends the sample request and compares to `expected`. It makes no claim that the registry equals the router's route set beyond that guarantee. The expected statuses are the table below, encoded once as a `const` table in `tests/personas.rs` keyed by route `kind` and reused by every `RouteSpec`:

| Persona | Station-scoped (4, 8) | Multi-station (7), sample = TWO of the OWNER's stations | Other user routes (2, 3, 9) |
| --- | --- | --- | --- |
| anonymous | 401 | 401 | 401 |
| deleted_session | 401 | 401 | 401 |
| inactive / no_app_row | 403 `account_inactive` | 403 | 403 |
| stranger / pending / declined / revoked | 404 identical to nonexistent id | 404 (no partial data) | 200, own data only |
| approved_follower | 200 | 200 (both stations visible) | 200 |
| owner | 200 | 200 | 200 |

A separate test covers the mixed case for route 7: persona `owner` requesting one of their own stations plus a station owned by a stranger gets 404 with no partial data (and the reverse order, and with a nonexistent id).

Connect (14 to 16) and follow-mutation (10 to 13) personas are tested separately (their semantics are not "visible vs hidden station"): anonymous/deleted_session/inactive as above; follow routes per 5.8; connect per 7.3.

Also required:

- Byte-for-byte equal 404 bodies for hidden and nonexistent stations.
- approve -> read 200 -> revoke -> same request 404 immediately; deactivating a user -> next request 403 with the same token (S11); deleting the stub `auth.sessions` row -> next request 401 (S2).
- 429 keyed on the user (the limiter takes a clock parameter; tests use governor's `FakeRelativeClock` under `test-support`, so no real waiting): exhaust user A's budget (120/min, burst 40) -> A gets 429 with `Retry-After`; user B unaffected; invalid-token requests in between consume nobody's budget. With the lazy unreachable pool (built with `acquire_timeout(50 ms)` so each failing request returns 503 `auth_unavailable` in about 50 ms, or fire the requests concurrently), the 41st request from a valid token within the window is 429 (the first 40 are 503), proving the limiter runs after token verification and before any database query.
- Station list never includes hidden stations, has <= 2 decimals, never contains `mac`, `credential_id`, owner email.
- Forecast route for a hidden station -> 404 although a `forecast_locations` row exists (G7); `location_id` query param -> 422.
- Follow routes: approve/decline/revoke by the follower (not the followee) -> 404; approve from non-`pending` -> 404; revoke from non-`approved` -> 404; re-request after decline/revoke -> pending; each transition writes exactly one `audit_log` row.
- Guard tests: `no_raw_station_ids` (4.5), `no .route( outside register`, "no derive Debug on secret structs".

### 7.3 Connect flow (S9, G6, G8)

Replay fixtures (`AWN_MODE=replay`) plus wiremock-backed `LiveAwn` tests (test-only base URL):

- `fixture-valid-key` -> `valid:true`; MACs equal the fixture MACs (`AA:BB:CC:00:01:01`, `AA:BB:CC:00:01:02`); each device has `timezone` from `lastData.tz`.
- `fixture-invalid-key` -> 200 `{"valid":false}` with no other fields.
- `fixture-ratelimited-key` -> 503 `upstream_busy` with `Retry-After`; wiremock 500/timeout -> 502.
- Durable limit: 5 calls in a minute succeed (rows `started` then updated), the 6th -> 429 with `Retry-After` equal to seconds until the oldest counted row leaves its window; another user unaffected; limit survives re-creating `AppState`; a request whose client disconnects mid-call leaves a counted `started` row; rejected requests add no row.
- Gates (caller-side, both in every `AWN_MODE`): concurrent validates from different users reach wiremock at <= 1 request/second via `awn:app:api`; for a user with a live credential the handler reserves `awn:key:<credential_id>` FIRST and then `awn:app:api` (assert both `rate_gates` rows advance, key first); with the key gate pre-loaded so no slot is free within the 3 s max wait, the API returns 503 `upstream_busy` with `Retry-After: 2` PROMPTLY (< 500 ms: `reserve` returns `GateBusy` immediately when the computed wait exceeds the max, it does not sleep first), records outcome `rate_limited`, leaves the `awn:app:api` row unchanged, and holds no pooled connection; the same with the app gate pre-loaded. The per-user advisory lock serializes concurrent calls from one user (two parallel validates yield at most one extra `started` row beyond the limit).
- Requests carry `apiKey` and `applicationKey`; the host is always `rt.ambientweather.net` in live mode; body fields `host`/`url` are rejected.
- Save: MAC not in AWN list -> 422 `unknown_device`, no rows; duplicate MACs in one request -> 422; valid -> station has AWN's MAC, rounded lat/lon, timezone from AWN, `std_utc_offset_minutes` correct (`America/Denver` -> -420), `credential_id` = new credential, a `forecast_locations` row, `credential.saved` and `station.created` audit rows; client timezone differing from AWN's -> 422; AWN without timezone and no client value -> 422 `timezone_required`; MAC owned by another user -> 409 `device_already_registered`; concurrent saves of the same MAC by two different users -> one 201, one 409 `device_already_registered` (via `stations_mac_key`); concurrent saves of the same MAC by the same user: each response is 201 or 409 (`conflict`), never 500, and afterward the user has exactly one live (non-`replaced`) credential and one station for that MAC; atomicity (inject failure on the `forecast_locations` insert -> no credential, station, or `replaced` update persists).
- Reconnect: second save by the same owner inserts a new credential row, marks the old `replaced`, repoints exactly the stations whose MACs are in the new AWN list (`credential_id <> $new AND mac = ANY($covered)`), writes BOTH `credential.saved` (new row) and `credential.replaced` (old row), and `/connect/status` shows the new row. A reconnect whose AWN list lacks one of the owner's stations returns 201 with that station in `not_covered`; it stays on the `replaced` credential and no 422 is returned. A THIRD save whose AWN list covers that earlier `not_covered` station repoints it to the newest credential and `not_covered` is empty. Station naming: AWN device with no name -> `Station 0101` style fallback; a 120-character name is truncated to 80; a whitespace-only override falls back.
- Encryption: read the row as superuser and decrypt with the test private key via `core`'s test helper (04's crypto tests own the decrypt round-trip) -> equals the sentinel key; `key_version` equals config; `encapsulated_key` is 32 bytes.
- Privileges under `SET ROLE app_api`: `SELECT api_key_ciphertext`, `SELECT *`, `RETURNING api_key_ciphertext`, `ON CONFLICT DO UPDATE` on `credentials`, and `UPDATE ... SET api_key_ciphertext` all fail with permission errors.
- No response, log or audit row contains the key.

### 7.4 Log redaction (S7)

`tests/log_redaction.rs`: TRACE subscriber with an in-memory writer; run the full app with sentinel secrets (user AWN key `SENTINEL-USER-KEY-7f3a9c`, app key `SENTINEL-APP-KEY-1d8e44`, a minted Bearer token, `JWT_HS256_SECRET`). Exercise: connect validate/save success; AWN 401, 429, 500, connection reset, malformed JSON, oversize body (wiremock); JWKS fetch failure; invalid tokens; handler panic; DB error. Assert logs and response bodies contain none of: either sentinel, the token, `apiKey=`, `applicationKey=`, `Bearer `, a request URI with a query string. A grep test asserts no `derive(Debug)` on the secret-bearing structs.

### 7.5 Integration with Compose Postgres and GoTrue

- 7.2 and 7.3 run under `make test-backend`.
- `gotrue_roundtrip` is NOT an `sqlx::test`: it runs against the migrated `postgres` database of the test project with unique users and explicit cleanup (conventions), skipped with a message if `SUPABASE_URL` is unset. It talks to GoTrue directly at `SUPABASE_URL=http://auth:9999`, WITHOUT the `/auth/v1` prefix (Caddy strips it; the test bypasses Caddy). Steps: mint a `service_role` JWT from `TEST_JWT_HS256_SECRET` (claims `role=service_role`, `iss`, short `exp`); `POST /admin/users` with `{email, password, email_confirm: true}` to create the user WITH a password; insert the matching `app.users` row; `POST /token?grant_type=password` for the session (password grant is chosen; 05 must enable it on the local GoTrue; if it is ever replaced by a Mailpit magic link, links in the email must be rewritten from `http://localhost:8000/auth/v1/` to `http://auth:9999/`). Build the API with `ApiConfig::for_test` using GoTrue's actual issuer (`http://localhost:8000/auth/v1`, which differs from the in-network `SUPABASE_URL`) and the HS256 secret. Then: `GET /api/v1/me` with the real token -> 200; the token's `session_id` makes `app_auth.session_active` true; `POST /logout` on GoTrue with the user's token -> the same token returns 401 (S2); the local anon key -> 401 (S1); delete the test user and rows at the end.

- Compose smoke through Caddy (`make` smoke target from 05): `GET http://localhost:8000/api/health` -> 200; `GET http://localhost:8000/api/v1/me` without token -> 401 with the error envelope and `Cache-Control: no-store`.

---

## 8. Security checklist (verify before merge)

- [ ] Production refuses: HS256 secret, replay mode, non-`verify-full` URL or missing/absent `sslrootcert` file, port 6543, non-`app_api` user, localhost/non-https issuer or JWKS, secrets from other processes, demo values from `demo-fingerprints.txt` (S5, D2, C3). Local refuses non-local hosts, placeholder app key, and HS256 without `SUPABASE_URL=http://localhost:8000`.
- [ ] Only ES256/RS256 in production; one algorithm per `Validation`; `aud`, `iss`, `exp`, `nbf`, `sub`, `role=authenticated`, `session_id` enforced; S1 tests pass.
- [ ] JWKS refetch single-flight, 60 s cooldown, fail-fast startup; outages return 503 not 401.
- [ ] User check and `app_auth.session_active` on every `/api/v1` request, uncached; startup probe fails fast; `is_admin` from DB (S2, S4, S11).
- [ ] `auth::guard` order: token -> limiter -> DB; one overall timeout outside the guard; layer order matches 4.2; the 429-per-user and zero-query tests pass.
- [ ] No repo function takes a bare station UUID; `no_raw_station_ids` green; multi-station checks every id; uniform 404 (S10, G7).
- [ ] No route returns `mac`, `credential_id`, ciphertext, emails, or non-rounded coordinates.
- [ ] `app_api` never reads ciphertext and never uses `ON CONFLICT DO UPDATE`, `RETURNING` or `UPDATE` on ciphertext columns; reconnect inserts a new row (S5).
- [ ] `api` binary has no decrypt code (`cargo tree -p api -e normal,build,features` check).
- [ ] Connect flow: hard-coded host; no user-supplied URL/host; reconnect leaves uncovered stations on the replaced credential (`not_covered`); MACs only from AWN's list; timezone server-derived; durable attempt row inserted before the AWN call (advisory-xact-locked count+insert); caller-side gates `awn:app:api` (+ `awn:key:<credential_id>` for a live credential) with no held connection; generic valid/invalid (S9, G6, G8).
- [ ] Every `reqwest::Error` uses `.without_url()`; response reads are size-capped with `chunk()`; no `Debug` on secret structs; panic hook installed; log-redaction test green (S7).
- [ ] Flat per-user rate limit before DB access; no decisions from `X-Forwarded-For`; health is a bounded plain query; Netlify signature verified when `NETLIFY_PROXY_SECRET` is set (S3).
- [ ] Fixed error messages; layer-generated 408/413/503 use the envelope; every response has `Cache-Control: no-store` and `nosniff`; client `x-request-id` ignored.
- [ ] Audit rows written in the same transaction with 04's dotted action names; no secrets in `detail` (S16).
- [ ] Image: non-root, no shell, read-only root, CA file at `/etc/ssl/app-certs/supabase-ca.crt` (CI check), no fixtures in the image; `cargo audit`/`cargo deny` clean.
- [ ] No CORS layer.

## 9. Open questions / assumptions

Needs decisions from other guides: see 3.3.

Conflicts and gaps:

- Follow discovery: how a user finds a followee is undecided. `POST /follows` currently assumes an exact case-insensitive display name (provisional). Also undecided: whether declined users may re-request (currently allowed) and whether followers may cancel/unfollow (not implemented).
- Station's own past years as an additional baseline are not used in YTD v1 (bands and prior years come from the reference station only); decide whether to add them.
- Whether AWN's `lastData.tz` is the field carrying the IANA zone must be confirmed against a real recorded response; the guide assumes it per conventions' fixture description.
- AWN devices without coordinates are rejected by `save`; alternative is a user-entered location.
- How `reference_station_id` is assigned for stations created via the connect flow (admin `add-reference-station` plus manual link vs automatic nearest airport) is not decided; until set, the baseline is `null`.
- Forecast-change history: the Design Doc lists it for v1 while the original brief called it post-v1; sequenced last (task 13).
- The `app_auth` definer-function design is accepted (proposals/03); that the function can read `auth.sessions` on hosted Supabase is to be confirmed by the required pre-build scratch-project check. Only if that check fails does option (c) drop the per-request session check and downgrade S2.
- Percentile set (10, 25, 50, 75, 90), `PRIOR_YEARS = 5`, eligibility rule `missing_days <= 7`, >= 10 eligible years for bands, the flat rate limit (120/min, burst 40), connect limits (5/min, 20/hour): configurable assumptions, not product decisions.
- Disconnecting a credential or station is not specified; not implemented.
- Netlify proxy: whether to also require `deploy_context == production`.
- If the API ever runs on more than one machine, the in-memory limiter must move to shared storage (connect limits and the AWN gate are already in Postgres).
- Fly layout is four apps (proposals/02, accepted); the API only assumes `famweather-api` runs `api` with the secrets in conventions and may auto-stop (cold-start JWKS fetch, `min_connections = 0`).

## 10. References

- Supabase JWTs and JWKS: https://supabase.com/docs/guides/auth/jwts
- Supabase JWT fields (`session_id`): https://supabase.com/docs/guides/auth/jwt-fields
- Supabase signing out (access tokens outlive revoked sessions): https://supabase.com/docs/guides/auth/signout
- Supabase connecting to Postgres (direct IPv6, Supavisor session vs transaction mode): https://supabase.com/docs/guides/database/connecting-to-postgres
- Netlify signed proxy redirects (`x-nf-sign`): https://docs.netlify.com/manage/routing/redirects/rewrites-proxies/
- Ambient Weather REST API: https://ambientweather.docs.apiary.io/
- PostgreSQL INSERT (column privileges, RETURNING, ON CONFLICT): https://www.postgresql.org/docs/current/sql-insert.html
- axum https://docs.rs/axum (layer ordering, fallback); tower-http https://docs.rs/tower-http (`TimeoutLayer`, `RequestBodyLimitLayer`, request-id layers); tower https://docs.rs/tower (`LoadShedLayer`); sqlx https://docs.rs/sqlx ; jsonwebtoken https://docs.rs/jsonwebtoken ; governor https://docs.rs/governor ; reqwest `Error::without_url` https://docs.rs/reqwest/latest/reqwest/struct.Error.html#method.without_url
- HPKE RFC 9180: https://www.rfc-editor.org/rfc/rfc9180
