# Shared Conventions (all guides)

This file is the single source of truth for names, layout, ports and settings that cross component boundaries. Every component guide must use these exactly. Component-local settings (for example poller tuning variables) may be defined only in their own guide, but anything another component reads or writes belongs here.

Audience: AI agents (and humans) implementing and testing the components. Be literal; prefer exact names over descriptions.

Source of decisions: `docs/Family Weather App Design Doc.md` and `docs/Design Review Findings.md` (cite items by ID, e.g. S5, P3). The accepted proposals (owner, Oct 5 and Oct 7, 2026) live in `docs/proposals/` and hold the rationale and rejected options; this file states their outcomes. Two pre-build verifications remain (see the Pre-build verifications bullet in `docs/Guide Review Decisions.md`): proposal 03's scratch Supabase check and proposal 04's throwaway Netlify test. Proposal 05 (email + password login) has **no** pre-build check: its open items are tests in the build against the local GoTrue (04 compose tests, 01 set-password e2e) and two dashboard settings checked during the real Supabase project setup (06 section 7 step 5). Proposal 06 (no stored forecasts) was verified with curl on 2026-10-07 (results in the proposal and 02 section 5.6b).

## Sign-in and forecasts (decided Oct 7)

- **Login (proposals/05, option b2):** email + password. `admin invite` creates a confirmed Supabase user with **no password** and prints a one-time set-password link from the Admin API `generate_link` (type `recovery`); Rodney sends it by hand. The web app redeems it (`verifyOtp` with the `token_hash`, link shape `https://<site>/set-password#token_hash=<hash>&type=recovery`, token in the URL fragment) and the person chooses their own password (`updateUser`). `admin reset-link --email` issues a new link for a forgotten password and signs the person out everywhere (`app_auth.revoke_sessions`); signed-in people change their password on `/account`. Deactivation is unchanged (`is_active=false`, `revoke_sessions`, ban). **No SMTP, no email provider, no magic links, no PKCE callback, no redirect allow-list, no mail catcher.** Supabase settings: sign-ups and anonymous off; the "secure password change"/reauthentication and "require current password" switches off; minimum password length 12 where settable (04 section 8.1). If a switch cannot be turned off on the hosted free plan, fall back to proposal 05 option (b1).
- **Growth path (documented, not built):** beyond a few people, add custom SMTP (Amazon SES is acceptable then) and emailed links. It is a Supabase settings change (custom SMTP, recovery/invite templates pointing at the same `/set-password#token_hash=...` page) plus small code paths (`--send` flags on `invite`/`reset-link`, a "Forgot password?" form calling `resetPasswordForEmail`). Details: 04 section 8.1a and 06 section 7 step 5. Do not add SMTP variables, mail services or email-sending code before then.
- **Forecasts (proposals/06, option A):** nothing about forecasts is stored: no `forecast_*` tables, no poller forecast job, no `FORECAST_SNAPSHOT` healthcheck, no `forecast_models.toml`. The API fetches live Open-Meteo Forecast and Ensemble data for the viewable station's rounded coordinates, computes p10/p50/p90 from the ensemble members itself, caches about 30 minutes in memory, and serves the forecast-change view day by day from the Previous Runs API ("forecast for day X as it looked 0 to 7 days earlier", `gfs_seamless`). The browser never calls Open-Meteo. Percentiles exist only in the live forecast chart. Forecast-vs-gauge (after v1) can use the Previous Runs archive next to `daily_rollups` with nothing stored. Accepted risk: dependence on Open-Meteo's free archives and terms; fallback is a small deterministic snapshot table (proposal option B).

## Repository layout

```
/
├── docker-compose.yml          # local stack (05-local-dev)
├── .env.example                # committed; .env is git-ignored
├── Makefile                    # local dev and test entry points (05-local-dev)
├── .github/                    # workflows, dependabot (06-deploy-ops)
├── web/                        # SvelteKit static SPA (01-web)
│   └── netlify.toml            # Netlify build, redirects, headers (01-web owns headers/CSP; 06 owns placeholders/secrets)
├── backend/                    # Rust Cargo workspace; Docker build context
│   ├── Cargo.toml              # [workspace]
│   ├── (no .sqlx/)             # queries are runtime sqlx::query with bind parameters, no macros, no offline data
│   ├── migrations/             # sqlx migrations (04-database)
│   ├── crates/
│   │   ├── core/               # shared: config, db pool, models, awn client, crypto, ratelimit, can_view helpers
│   │   ├── api/                # bin `api` (02-api)
│   │   ├── poller/             # bin `poller` (03-poller)
│   │   └── admin/              # bin `admin` (04-database)
│   ├── fixtures/               # recorded AWN / Open-Meteo / ACIS responses (tests + replay mode)
│   └── Dockerfile              # one image; binaries at /usr/local/bin/{api,poller,admin}
├── ops/
│   ├── backup/                 # backup image + its fly.toml (06-deploy-ops)
│   ├── caddy/Caddyfile         # local reverse proxy (05-local-dev; headers from 01-web)
│   ├── ci/                     # CI helper scripts (06-deploy-ops)
│   ├── db/
│   │   ├── init/               # Postgres container init: runs bootstrap/roles.sql (05-local-dev)
│   │   ├── bootstrap/          # roles.sql, auth_access.sql (04-database)
│   │   ├── checks/             # exposure.sql, size query (04-database)
│   │   ├── tests/              # grant / can_view SQL tests (04-database)
│   │   └── seed/               # local seed data (04-database)
│   ├── fly/                    # api.toml, poller.toml, run-oneoff.sh, site-url.txt (06-deploy-ops)
│   └── local/                  # init-env.sh, smoke.sh (05-local-dev)
└── docs/guides/                # these guides
```

## Guides

| File | Component |
| --- | --- |
| `01-web.md` | SvelteKit static SPA, charts, connect-station UI, security headers/CSP |
| `02-api.md` | Axum API: auth middleware, `can_view`, endpoints, connect flow, rate limits. **Authority for the HTTP contract.** |
| `03-poller.md` | Poller jobs, scheduling, locks, rate gates, alerting, HPKE decrypt/rotation |
| `04-database.md` | Schema, migrations, roles/grants, encryption format, Supabase project config, admin CLI. **Authority for table/column names.** |
| `05-local-dev.md` | Docker Compose stack, Caddy, local auth, fixtures/replay, seed, smoke test |
| `06-deploy-ops.md` | Fly, Netlify, Supabase prod, CI/CD, secrets, backups to S3, monitoring, runbooks |

## Environments

`APP_ENV` is `local` or `production`; unset is a startup error. Production refuses to start with any local-only setting (HS256 secret, demo keys, `AWN_MODE=replay`, non-`verify-full` database URL, localhost issuer/JWKS, a secret that does not belong to that process). Local refuses a `DATABASE_URL` host other than `db`/`localhost`/`127.0.0.1`/`::1`, a `SUPABASE_URL` host other than `localhost`/`127.0.0.1`/`caddy`/`auth` (`auth` = direct GoTrue inside the test project), and `AWN_MODE=live` with an empty or placeholder application key; `HEALTHCHECK_URL_*` is ignored locally.

## Fly layout (proposals/02, accepted)

Fly secrets are app-wide, so secret isolation (S5) needs separate apps. Four Fly apps are built from the same `backend` image (each app does its own `--remote-only` build by default; per-app deploy tokens `FLY_API_TOKEN_API`, `FLY_API_TOKEN_POLLER`, `FLY_API_TOKEN_BACKUP` exist only for the apps CI deploys; `famweather-admin` has none):

| App | Machines | Secrets |
| --- | --- | --- |
| `famweather-api` | 1 (`api`), auto-stop allowed (decided Oct 7: the machine sleeps when idle, about 1 s cold start; the in-memory forecast cache is simply empty after a wake) | `app_api` DATABASE_URL, JWKS/issuer, `AWN_APPLICATION_KEY`, `CREDENTIALS_PUBLIC_KEY`, `CREDENTIALS_KEY_VERSION`, `NETLIFY_PROXY_SECRET` (the issuer and JWKS URL are non-secret `[env]` in `ops/fly/api.toml`); outbound HTTPS to AWN, ACIS and the three Open-Meteo hosts (no key) |
| `famweather-poller` | 1 (`poller`), always on, `--ha=false` | `app_poller` DATABASE_URL, `AWN_APPLICATION_KEY`, `CREDENTIALS_PRIVATE_KEY`, `CREDENTIALS_KEY_VERSION`, healthcheck URLs |
| `famweather-admin` | 0 at rest; one-off machines only | `app_migrator` DATABASE_URL, `SUPABASE_URL`, `SUPABASE_SERVICE_ROLE_KEY`, `CREDENTIALS_PUBLIC_KEY`, `CREDENTIALS_KEY_VERSION` (non-secret `SITE_URL`, the origin printed in set-password links, is passed by `run-oneoff.sh` with `-e` from the committed `ops/fly/site-url.txt`; the admin app has no config file) |
| `famweather-backup` | scheduled daily | `app_backup` URL, S3 upload-only creds, age recipient |

One-off `awn_backfill` runs use the poller app (it needs the poller's secrets). Migrations run from CI in a protected environment, not as a Fly release command. Process names `api` and `poller` are kept so the layout could collapse later. `ops/fly/run-oneoff.sh` validates its app arguments (a wrong app would run a command with the wrong secrets); one-off machines inherit their app's secrets.

## Compose services and ports (local)

| Service | Image / build | Internal port | Host port | Notes |
| --- | --- | --- | --- | --- |
| `db` | `postgres:17` | 5432 | 127.0.0.1:5432 | init runs `ops/db/bootstrap/roles.sql` |
| `auth` | `supabase/auth` (GoTrue), pinned tag | 9999 | — | issuer `http://localhost:8000/auth/v1`, HS256, signup off, email provider on with password grant, **no SMTP** (nothing is emailed; there is no mail catcher service) |
| `auth-access` | `postgres:17` (has psql) | — | — | one-shot; after `auth` healthy: `psql -v ON_ERROR_STOP=1 -f /bootstrap/auth_access.sql` as `postgres` (same file and role as production) |
| `migrate` | backend image, `admin migrate` | — | — | one-shot; after `auth-access` completed successfully: sqlx migrations as `app_migrator` (needs only `APP_ENV`, `DATABASE_URL`) |
| `api` | backend image `weather-backend:local`, cmd `api` | 8080 | — | |
| `poller` | backend image, cmd `poller` | — | — | |
| `caddy` | `caddy:2` | 80 | 127.0.0.1:8000 | mounts `./web` read-only and serves `/srv/webroot/build` with SPA fallback to `200.html` (incl. `GET /`), `/api/*` → `api:8080`, `handle_path /auth/v1/*` → `auth:9999` (prefix stripped) |
| `admin` | backend image, cmd `admin` | — | — | profile `tools` |

Startup order: `db` → `auth` (creates `auth.*`) → `auth-access` → `migrate` → `api`, `poller` → `caddy`. GoTrue: `API_EXTERNAL_URL=http://localhost:8000`, `GOTRUE_SITE_URL=http://localhost:8000`, an **empty** redirect allow-list (no flow uses a redirect: `signInWithPassword` and `verifyOtp` with `token_hash` are direct calls; production has none either, so Netlify deploy previews sign in without any allow-list entry), password grant on, `GOTRUE_SECURITY_UPDATE_PASSWORD_REQUIRE_REAUTHENTICATION=false`, current-password requirement off, no `GOTRUE_SMTP_*`, no `GOTRUE_MAILER_URLPATHS_*`. The admin CLI's `SITE_URL` locally is `http://localhost:8000`, so printed links open the local stack. Compose sets `APP_ENV=local` and every `DATABASE_URL` host to `db` literally so `.env` cannot point containers at production.

The browser always talks to one origin (`http://localhost:8000` locally; the Netlify site in production). Locally the Supabase JS client URL is `http://localhost:8000`; in production it is the Supabase project URL.

## Binaries and subcommands

- `api` (serve), `api healthcheck`.
- `poller` (run scheduler), `poller healthcheck` (heartbeat file is touched even when `POLLER_ENABLED=false`), `poller run-job <job> [--station <uuid>]` for one-off runs.
- `admin wait --minutes <n>` (idle main process for interactive one-off machines in production; operator then runs commands via `fly ssh console --pty -C "/usr/local/bin/admin …"`; `fly machine run` has no `-it`).
- `admin migrate`, `admin keygen` (loads no config; prints a new HPKE key pair; `admin keygen --check` verifies a private/public pair matches), `admin invite --email --display-name [--follow-all] [--admin] [--adopt]` (creates a confirmed login with no password and the app rows, then prints the one-time set-password link once to stdout; `--admin` refused if an admin already exists; `--adopt` repairs an auth user left without an app row), `admin reset-link --email` (new one-time set-password link for an active user; signs them out everywhere; prints once; for forgotten passwords and for re-issuing an expired invite link), `admin add-station` (API key from hidden prompt/stdin only; optional `--reference-station <GHCN id>` sets the baseline explicitly), `admin add-reference-station`, `admin deactivate-user`, `admin delete-user`, `admin list-users`, `admin restore-auth-users --file <csv> [--dry-run]` (recreates backed-up Supabase logins with their original ids), `admin seed-dev` (refuses unless `APP_ENV=local`).

## HTTP API

- All routes under `/api/v1/…`; `GET /api/health` is unauthenticated, returns no data, and is exempt from the proxy-signature check.
- Auth: `Authorization: Bearer <supabase access token>` on every other route. No cookies. No CORS.
- JSON only. Errors: `{"error": {"code": "<snake_case>", "message": "<safe text>"}}`. Shared codes: `unauthorized` (401), `account_inactive` (403), `forbidden` (403), `not_found` (404, used for both "doesn't exist" and "not viewable"), `rate_limited` (429 with `Retry-After`), `validation_failed` (422). Never echo upstream bodies or URLs.
- Every API response sets `Cache-Control: no-store` and `X-Content-Type-Options: nosniff` (Netlify does not apply its headers to proxied responses).
- IDs are UUIDs everywhere; never sequential.
- **GDD is computed at query time (decided Oct 7), never stored.** `daily_rollups` and `reference_daily` hold only daily `tmax_f`/`tmin_f`; the API computes GDD with the SQL function `app.gdd_day(tmax_f, tmin_f, method, base_f)` (04 5.1) using request parameters `gdd_method` (`simple_avg` default | `clamped_50_86`), `gdd_base` (integer degrees F, default 50) and `gdd_start` (`MM-DD`, default `04-01`), the same values for the station and its baseline; the response echoes them in a top-level `gdd` object. The poller and the web never compute GDD. There is no per-user stored setting: the web keeps the choice in the page URL.
- Follow discovery (decided Oct 7): `GET /api/v1/users/search?q=` (name prefix, minimum 2 characters, at most 10 results, only `id`, `display_name`, `follow_status`, never an email, excludes self and inactive users) and `POST /api/v1/follows {"followee_id"}`. No "people you may know".
- Baseline (decided Oct 7): `connect/save` never calls ACIS; the owner gets nearby airport candidates from `GET /api/v1/stations/{id}/baseline/candidates` (ACIS `StnMeta`, nearest suggested) and confirms with `PUT /api/v1/stations/{id}/baseline` (owner only). A station without a baseline is a normal state ("no baseline yet").
- The exact endpoint list and JSON shapes are defined in `02-api.md`; `01-web.md` must match it.

## Database

- All application objects live in schema `app`. Nothing in `public`; `app_migrator`'s `search_path` is `app` so `_sqlx_migrations` lands in `app`. Supabase's `auth` schema is owned by GoTrue; app roles never touch `auth.*` tables directly.
- Migrations: `sqlx` only, in `backend/migrations/`, applied by `admin migrate` as `app_migrator`. GoTrue must have migrated `auth` first.
- `ops/db/bootstrap/roles.sql` creates roles (passwords via `\getenv`, never argv).
- **Auth access (proposals/03, accepted):** `ops/db/bootstrap/auth_access.sql`, run as `postgres` (locally by the `auth-access` one-shot; in production once by the operator), creates schema `app_auth` owned by `postgres` with three `SECURITY DEFINER` functions: `app_auth.session_active(session_id uuid, user_id uuid) returns boolean`, `app_auth.revoke_sessions(user_id uuid) returns integer` and `app_auth.backup_users() returns table(id uuid, email text, created_at timestamptz)` (only what a restore needs), each with `SET search_path = ''`, fully qualified names, `REVOKE EXECUTE … FROM PUBLIC`, and `GRANT EXECUTE` only to `app_api` (session_active), `app_migrator` (revoke_sessions) and `app_backup` (backup_users). No FK from `app.users` to `auth.users` (app-level integrity). A scratch-project check on hosted Supabase (proposal 03 checklist) is a required pre-build verification; only if it shows definer functions cannot read `auth.sessions` is the fallback the proposal's option (c) (drop the session check, S2 downgraded). The backup captures auth users through `app_auth.backup_users()` (decided Oct 7) and `admin restore-auth-users` restores them; `app_backup` gets no direct grants on `auth.*`.
- Role connection limits: `app_api` 20, `app_poller` 15, `app_migrator` 10, `app_backup` 3, `app_ci_check` 2. Poller process: query pool max 4 plus one dedicated (detached, non-pooled) connection per held job lock; one-off machines: pool max 2.
- `app_ci_check`: login with no table grants, used only by CI/production exposure checks (reads catalogs).
- Audit `subject_type` ∈ `user|follow|credential|station`; for `follow.*` actions `subject_id` is the follower's id and `detail.followee_id` the followee.
- Audit `action` values are dotted and defined by `04-database.md` (e.g. `follow.requested`, `credential.saved`, `credential.invalid`); every writer uses that list verbatim.
- Roles (least privilege, S5/S12):

| Role | Used by | Key rights |
| --- | --- | --- |
| `app_migrator` | migrations, `admin` | owns schema `app`; EXECUTE `app_auth.revoke_sessions` |
| `app_api` | `api` | read app data; INSERT on `credentials` but **no SELECT on ciphertext columns and no UPDATE of ciphertext columns** (UPDATE only `status`); never `ON CONFLICT DO UPDATE` or `RETURNING` ciphertext columns; INSERT of seven named columns on `reference_stations` and UPDATE of `stations.reference_station_id` (baseline picker); EXECUTE `app_auth.session_active` and `app.gdd_day` |
| `app_poller` | `poller` | read/write ingest tables; SELECT and UPDATE `credentials` (status, last_ok_at, ciphertext columns, key_version for rotation); write `job_runs` |
| `app_backup` | backup job | read-only on `app` (no direct grants on `auth.*`); EXECUTE `app_auth.backup_users` |

- Reconnecting a credential inserts a new row; the old row's status becomes `replaced`. Credential status values: `ok`, `invalid`, `rate_limited`, `replaced`.
- Reconnect writes two audit rows: `credential.saved` (subject = new row) and `credential.replaced` (subject = old row). A concurrent-connect 23505 returns 409 `conflict` (no retry). Stations whose MAC is not in the new key's AWN device list stay on the `replaced` credential, are returned to the caller as `not_covered`, and are disabled by the admin (`stations.is_enabled`); the poller skips stations whose credential is not `ok`/`rate_limited`. If the caller has a live credential, the connect flow also reserves `awn:key:<that credential_id>`.
- Poller status writes on credentials are guarded with `AND status IN ('ok','rate_limited')`; the 401 guard marks a credential `invalid` only if at least one other credential succeeded in the same run.
- Dirty days: `app.rollup_dirty_days(station_id uuid, local_date date, primary key (station_id, local_date))` (04 owns DDL). Ingest/backfill insert into it in the same transaction as the observation upsert with `ON CONFLICT (station_id, local_date) DO UPDATE SET local_date = EXCLUDED.local_date` (takes the row lock so a concurrent rollup DELETE cannot lose the day; 04 grants `UPDATE(local_date)`); `daily_rollup` consumes and deletes rows it has processed. No timestamp comparisons for dirtiness.
- Coverage: completeness is interval-aware (observed interval 5 or 30 min, coverage = time covered / 24 h); 04 §5.7 uses 03's rule.
- Supabase Admin API calls with an `sb_secret_` key send it only in the `apikey` header.
- Time: store `timestamptz` in UTC. Daily rollups use local **standard** time days (F5), via `app.lst_date` / chrono-tz base UTC offset.
- Forecasts are not stored (proposals/06, Oct 7): no forecast tables exist, no `precip_6h_in` variable, no 6-hour windows. Forecast routes are `GET /api/v1/stations/{id}/forecast` and `GET /api/v1/stations/{id}/forecast/changes?from&to` (02 sections 5.6, 5.6a); the forecast location is always the viewable station's own rounded coordinates, never client-supplied.

## Credential encryption (S5, S8, G6) (proposals/01, accepted)

- HPKE (RFC 9180), base mode, X25519 + HKDF-SHA256 + AES-256-GCM (Rust crate `hpke`, exact version pinned; RFC 9180 known-answer test vectors in tests; a unit test that moving a ciphertext to another row (different `credential_id` or `owner_id`) fails to decrypt).
- `info` = `b"family-weather/credential/v1"`. AAD = 16 raw bytes of `credential_id` followed by 16 raw bytes of `owner_id`.
- Keys are base64 of the raw 32-byte X25519 keys.
- `api` and `admin` hold `CREDENTIALS_PUBLIC_KEY` (encrypt only). `poller` holds `CREDENTIALS_PRIVATE_KEY` (the only Fly app that does; the poller decrypts once into memory with `secrecy` and never logs keys). The API sees a pasted key in plain text only while testing it against AWN in the connect flow. The public key is not secret; the private key is a Fly secret on `famweather-poller` only, and the committed dev key pair is local only and refused in production. The decrypt code is behind the `core` cargo feature `decrypt`, enabled only by `poller` (and tests); CI fails if `api` enables it.
- Columns: `api_key_ciphertext bytea`, `encapsulated_key bytea`, `key_version smallint`.
- Rotation: `admin keygen` → set new key as current, old as `*_PREVIOUS` on the poller → on-demand job `credential_reencrypt` rewrites rows → remove previous key.

## AWN keys and rate limits (G6, G8)

- `AWN_APPLICATION_KEY`: one for the whole app (separate dev key in `.env`). Used by `api` (connect flow) and `poller`; `admin add-station` works without it (no validation).
- AWN API keys: one per station-owning user, encrypted in `app.credentials`.
- AWN base URL is fixed (`https://rt.ambientweather.net/v1`), never user-supplied; a test-only constructor may override it.
- Shared rate gates are **slot reservations in a table**, not advisory locks, so no connection is held while waiting and a cancelled request cannot leak a lock. Table `app.rate_gates(name text primary key, next_allowed_at timestamptz not null)` (04 owns DDL; `app_api` and `app_poller` get SELECT/INSERT/UPDATE). One atomic statement reserves a slot:
  ```sql
  INSERT INTO app.rate_gates (name, next_allowed_at) VALUES ($1, now() + $2::interval)
  ON CONFLICT (name) DO UPDATE
    SET next_allowed_at = greatest(app.rate_gates.next_allowed_at, now()) + $2::interval
    WHERE greatest(app.rate_gates.next_allowed_at, now()) <= now() + $3::interval   -- $3 = max wait
  RETURNING greatest(next_allowed_at - $2::interval - now(), interval '0') AS wait;
  ```
  No row returned → the wait would exceed the timeout (API returns 503 `upstream_busy`; poller retries later). Otherwise `tokio::time::sleep(wait)` (a duration computed by the database — never from the injected `Clock`), then call AWN. The **caller** reserves (both the app gate and the per-key gate) in every `AWN_MODE`; `core::awn` clients never reserve. `LiveAwn` takes a per-call timeout (API connect flow: 5 s; poller: 20 s). API: `core::ratelimit::reserve(pool, name, spacing, max_wait) -> Result<Duration, GateBusy>`. Gates apply in every `AWN_MODE` (replay included), with spacing overridable only in tests.
  - `awn:app:poller` — 500 ms spacing (2 req/s), shared by the poller and one-off backfill machines.
  - `awn:app:api` — 1000 ms spacing (1 req/s), used by the connect flow, max wait 3 s. Total ≤ 3 req/s.
  - `awn:key:<credential_id>` — 1000 ms spacing, everyone using that stored key.
  - `acis:api` — 1000 ms spacing, max wait 3 s, used by the API's baseline lookup before every uncached ACIS `StnMeta` call (02 4.15); same table and statement, no AWN involved.
  - Both back off on 429. On 401/403 the poller marks a credential `invalid` only if at least one other credential succeeded in the same run; otherwise it marks none and fails the run with `suspected_application_key_failure`.
- The poller (and one-off backfill machines) connect to Supabase over the **direct** connection, never a pooler, so dropping a lock connection reliably frees its advisory lock.
- Job locks remain Postgres session advisory locks keyed `job:<name>`, taken with `pg_try_advisory_lock` on a **dedicated detached connection** (never a pooled one), released by an explicit async `release()`; on any other drop path the raw connection is closed, which frees the lock. Requires a session-level connection (direct or session-mode pooler, never transaction mode).

## Fixtures

`backend/fixtures/<source>/…` with `FIXTURES_DIR` (compose sets `/fixtures`). 03 owns the fixture files and recorder; 04's seed must match these identifiers exactly.

- **Connect-flow keys** (replay): `fixture-valid-key` → `awn/devices_ok.json` (MACs `AA:BB:CC:00:01:01` and `AA:BB:CC:00:01:02`, **not** seeded, with `info.coords` and `lastData.tz`), `fixture-invalid-key` → 401, `fixture-ratelimited-key` → 429.
- **Seeded poller credential key**: `fixture-poller-key` → poller scenarios under `awn/stations/<MAC>/` for the seeded stations `AA:BB:CC:00:00:01`, `…:02`, `…:03`. Any other key in replay → 401.
- **ACIS fixtures** are stored under the seed's reference ids `USW00099001`–`USW00099003` (recorded data renamed), each with ≥ 12 complete years. Recorded `StnMeta` responses for the baseline picker are `acis/stnmeta__<tag>.json` (03 section 10); the connect-flow devices sit near Loveland CO so the suggestion test returns Northern Colorado Regional Airport (`USW00094035`).
- **Demo values**: every committed local-only secret (HS256 secret, anon key, service key, HPKE key pair, the five local role passwords) has a line `<sha256-hex>  <VARIABLE_NAME>` in `backend/demo-fingerprints.txt` (05 owns content; inside the backend build context). `core::config` embeds it with `include_str!` and refuses any matching value in production; the web production build (01) and 06's pre-import check hash candidate values and compare against column 1.
- `.gitignore` must include `.env`, `.local/`, `web/build/`, `web/node_modules/`, `web/tests/.auth/`, `web/test-results/`, `web/playwright-report/`.
- Docker Compose ≥ 2.24 is required (the test overlay uses `!reset`/`!override`).
- **Clock**: `core::clock::Clock` is injected everywhere "now" is used. In replay mode fixture timestamps are shifted so the newest reading equals now, so replay produces current-year data. Tests use a frozen clock.

## Environment variables (cross-component)

| Variable | Used by | Notes |
| --- | --- | --- |
| `APP_ENV` | all | `local` / `production` |
| `DATABASE_URL` | api, poller, admin | role-specific per process; `sslmode=verify-full` + `sslrootcert=/etc/ssl/app-certs/supabase-ca.crt` in production (Dockerfile: `COPY certs/ /etc/ssl/app-certs/` from the `backend/` context; `backend/certs/.keep` is committed so builds work before the CA is added; production config refuses to start if the file named in `sslrootcert` is missing — it checks that path, not a fixed one, so CI on a runner can point at the repo copy) |
| `SUPABASE_URL` | api, admin | issuer is `${SUPABASE_URL}/auth/v1` |
| `SUPABASE_JWKS_URL` | api | production token keys |
| `JWT_HS256_SECRET` | api, auth | **local only** |
| `SUPABASE_SERVICE_ROLE_KEY` | admin only | |
| `AWN_APPLICATION_KEY` | api, poller | |
| `AWN_MODE` | api, poller, admin | `live` / `replay` |
| `FIXTURES_DIR` | api, poller, admin | |
| `CREDENTIALS_PUBLIC_KEY` | api, admin | |
| `CREDENTIALS_PRIVATE_KEY` | poller | |
| `CREDENTIALS_KEY_VERSION` | api, poller, admin | current version |
| `CREDENTIALS_PRIVATE_KEY_PREVIOUS`, `CREDENTIALS_KEY_VERSION_PREVIOUS` | poller | rotation only |
| `API_BIND_ADDR` | api | default `0.0.0.0:8080` |
| `NETLIFY_PROXY_SECRET` | api, Netlify | optional; when set, the API rejects requests without a valid `x-nf-sign` |
| `POLLER_ENABLED` | poller | default `false` locally |
| `HEALTHCHECK_URL_<JOB>` | poller, backup | suffixes `AWN_INGEST`, `DAILY_ROLLUP`, `REFERENCE_SYNC`, `BACKUP`, `DBSIZE` |
| `LOCAL_DEMO_PASSWORD` | admin (`seed-dev` only) | **local only**; the demo password given to seed users; fingerprinted in `backend/demo-fingerprints.txt`; `seed-dev` refuses unless `APP_ENV=local` |
| `SITE_URL` | admin | origin only (scheme + host, no path); required by `admin invite` and `admin reset-link` to print the set-password link; production: https only and never localhost, supplied by `ops/fly/site-url.txt` via `run-oneoff.sh`; local: `http://localhost:8000`. In production `invite` and `reset-link` run only in `shell` mode (`fly ssh console -C`) so the link never reaches `fly logs`; `run-oneoff.sh batch` refuses them |
| `RUST_LOG` | api, poller, admin | |
| `PUBLIC_SUPABASE_URL`, `PUBLIC_SUPABASE_ANON_KEY` | web build | the only values allowed in the browser bundle; passed as process env |
| `APP_MIGRATOR_PASSWORD_VERIFIER`, `APP_API_PASSWORD_VERIFIER`, `APP_POLLER_PASSWORD_VERIFIER`, `APP_BACKUP_PASSWORD_VERIFIER`, `APP_CI_CHECK_PASSWORD_VERIFIER` | bootstrap only (`roles.sql`) | SCRAM-SHA-256 verifiers computed with `ops/db/bootstrap/scram.py`; plaintext passwords only appear inside the role-specific `DATABASE_URL`s |
| `BACKUP_DATABASE_URL`, `BACKUP_S3_BUCKET`, `BACKUP_S3_PREFIX`, `BACKUP_AGE_RECIPIENT` (comma-separated), `BACKUP_INCLUDE_AUTH` (default on: the backup includes `app_auth.backup_users()` output; set to 0 only for a database without that function), `BACKUP_S3_CONDITIONAL`, `AWS_*` | backup | upload-only IAM user |

## Shared Rust code: ownership and build order

**Crate name:** the shared crate lives in `backend/crates/core`, but its package is named `famweather-core` and it is imported as `famweather_core`. A package named `core` would shadow Rust's built-in `core`. Throughout the guides, `core::x` means `famweather_core::x`, and "the `core` feature `f`" means feature `f` of `famweather-core`. Consumers depend on it with `famweather-core = { workspace = true, ... }`, declared once in `[workspace.dependencies]`; never rename it back to `core`.

Each `core` module has one owner guide; others consume it and must not redefine it.

| Module | Owner | Notes |
| --- | --- | --- |
| `core::config`, `core::db`, `core::redact`, `core::clock` | 03-poller | config validation implements the Environments rules for every binary; per-subcommand required env (e.g. `admin migrate` needs only `APP_ENV`, `DATABASE_URL`) |
| `core::awn` (trait `AwnApi`, `LiveAwn`, `ReplayAwn`, one error enum incl. `Unauthorized`, `RateLimited`, `Unavailable`, `BadResponse`) | 03-poller | 02 uses `list_devices` for the connect flow |
| `core::acis` (trait `AcisApi`: `station_data`, `nearby_stations`; `LiveAcis`, `ReplayAcis`) | 03-poller | the poller's `reference_sync` uses `station_data`; 02 uses `nearby_stations` for the baseline picker; callers reserve the `acis:api` gate |
| `core::ratelimit` | 03-poller | slot reservation above |
| `core::crypto` (`encrypt_credential`, feature-gated `decrypt_credential`, signatures per 04 §9) | 04-database | |
| `core::testdb` and the auth stub | 04-database | |

Build order (data starts accumulating first): **03 core milestone (core::config/db/redact/clock/awn/ratelimit) → 04 database + admin → 03 poller (go live in production as soon as ingest works, because AWN station history cannot be rebuilt; forecasts are fetched live by the API and are no reason to go early; the scheduler registers only implemented jobs) → 02 api → 01 web**; 05 grows alongside each, 06 deploys the poller first. `backend/Dockerfile` (one image, binaries present only for crates that exist) is delivered with the 03 core milestone, not by 02. CI and `make test` skip web steps when `web/package.json` is absent and api steps/features when `backend/crates/api` is absent, so CI is green before 01/02 exist; the `web/security-headers.json` project-ref commit happens in Stage B. The first production deploy is **poller-only**: only the `AWN_INGEST` (plus `BACKUP`/`DBSIZE`) healthchecks exist, no backfill step, and the API/web deploy steps are skipped until 02/01 ship. The `core` milestone is done when it compiles with unit tests; DB-backed gate/lock tests run once 04's migrations and `core::testdb` exist.
- Config ownership: `core::config::load(Binary)` returns `enum Config { Api(ApiEnv), Poller(PollerConfig), Admin(AdminEnv) }` and applies every rule in the Environments section (incl. demo-value refusal — every secret value plus the password extracted from `DATABASE_URL` is hashed and compared against column 1 of every line, message `refusing <VAR>: demo value` naming the env var it came from, `DATABASE_URL` for the password; CA-file presence; port 6543 refused for all binaries; pooler hosts refused only for the poller and its one-offs; role-user check accepts `<role>` or `<role>.<project-ref>`; foreign-secret refusal). Struct fields: `ApiEnv` is defined in 02 §4.1, `AdminEnv`/`AdminCommand` in 04 §10.1 (as named Rust structs/enums); 03 implements `load()` against those definitions for every binary. `api::config` adds only API-specific checks (issuer/JWKS, HS256 pairing) on top; it does not re-implement the shared rules. `core` types never reference crate-local types (e.g. `KeyRing` stays in `poller`, built from raw key parts in `PollerConfig`).
- `core::db::PoolSpec { max, min, acquire_timeout, idle_timeout, max_lifetime, test_before_acquire, after_connect }` with defaults.
- Test-only APIs (`ApiConfig::for_test`, extra test routers, `LiveAwn::with_base`) are gated on a `test-support` feature (never `decrypt` for `api`); `make test-backend` enables it with `--features api/test-support` (admin compose tests run separately with `-p admin --features compose-tests`).
- `core::testdb` (incl. `pool_as(db, role)` for role-scoped pools) lives behind a `core` feature `testdb` (dev-dependencies only, never in release binaries) and loads `auth_stub.sql` / `auth_access.sql` at runtime from `$OPS_DIR` (default `/work/ops/db`); nothing outside `backend/` is ever `include_str!`-ed except `backend/demo-fingerprints.txt`. `run.sh` defaults `MIGRATIONS_DIR=/work/backend/migrations` and `OPS_DIR=/work/ops/db`.
- Crates that use `core::testdb` enable `famweather-core = { workspace = true, features = ["test-support", "testdb"] }` in `[dev-dependencies]` (never `decrypt` for `api`).
- Poller per-page and per-day transactions retry a bounded number of times on SQLSTATE 40P01/40001; upsert rows and dirty-day inserts are sorted by key so writers lock in a consistent order. A failed job run merges `error` into its partial `detail` (gaps, counters) rather than replacing it.
- `make test-web` builds into a tmpfs/volume inside the test project (never the host's `web/build`) and runs `check-build.mjs`, `test:tz`, `headers:check` and `test:bundle`. Test containers mask `/work/.env` with `/dev/null` (including `db`).
- Panic hook (`core::redact::install_panic_hook`): logs file:line and a fixed message only, never the panic payload. The URL-stripping reqwest error helper is `core::redact::http_error`.

- Numeric columns: Rust code uses `f64`; every NUMERIC parameter and select is cast explicitly (`$1::float8`, `col::float8`). No `rust_decimal`.

## Testing harness

- **Tests never touch the developer's working stack.** Every `make test-*` target runs in a separate Compose project (`docker compose -p famweather-test -f docker-compose.yml -f docker-compose.test.yml`) whose `db` uses tmpfs and whose stack is torn down afterwards. CI runs exactly these targets (`make test` = `check-local-only test-db test-backend test-web`); each target builds the backend image and starts its own dependencies.
- `make test-db`: runs `ops/db/tests/run.sh` inside the test project's `db` container (repo mounted read-only at `/work`); it may re-run `roles.sql` freely because the cluster is throwaway. Per-role login checks connect over TCP (`-h 127.0.0.1`) so SCRAM is exercised.
- `make test-backend`: `test-backend` service (repo root at `/work`, `working_dir: /work/backend`) after `migrate`; `#[sqlx::test(migrations = false)]` + `core::testdb::prepare` with `TEST_SUPERUSER_DATABASE_URL`; `prepare` applies the auth stub and `auth_access.sql` as superuser, then migrates after `SET ROLE app_migrator; SET search_path = app` so ownership and default privileges match production. Grant checks run queries after `SET ROLE app_api|app_poller|app_migrator`. Tests needing the real GoTrue session (e.g. 02's gotrue round-trip) use the migrated test-project `postgres` database directly with unique users and cleanup, outside `sqlx::test`.
- Env set by `test-backend`: `DATABASE_URL` (superuser URL; `#[sqlx::test]` reads it), `TEST_SUPERUSER_DATABASE_URL`, `TEST_MIGRATOR_DATABASE_URL`, `TEST_JWT_HS256_SECRET`, `SUPABASE_URL=http://auth:9999` (service-role JWT minted from the HS256 secret), `SITE_URL=http://localhost:8000`, `FIXTURES_DIR=/work/backend/fixtures`, `SEED_DIR=/work/ops/db/seed` and `LOCAL_DEMO_PASSWORD` (for admin's `seed_dev_idempotent`; the seed directory arrives through the repo mount at `/work`), demo `CREDENTIALS_*` keys. `cargo test --workspace --exclude admin --features api/test-support` then `cargo test -p admin --features compose-tests -- --test-threads=1`, then the `api` feature-isolation check (`cargo tree`, below). There is no `cargo sqlx prepare --check` step: queries are runtime `sqlx::query` with bind parameters only (no `query!` macros, no SQL built with `format!`/concatenation), so there is no offline data to check; `ops/local/check-secret-logs.sh` greps the test project's container logs for demo secrets and `canary-secret-` sentinels before teardown (tests plant secrets with that prefix). Test targets depend on `make init` (`.env` must exist even though containers mask it). Tests that redeem a set-password link take the `token_hash` from the printed link and call GoTrue directly at `http://auth:9999` (`POST /verify`, then `PUT /user` with the returned token); nothing is emailed.
- Local role passwords: `.env.example` commits demo plaintext passwords for all five roles (incl. `APP_CI_CHECK_PASSWORD`) and their precomputed SCRAM verifiers; `db` receives only the `*_VERIFIER` values. In production the operator computes verifiers with `ops/db/bootstrap/scram.py`.
- `backend/Cargo.toml` sets `resolver = "2"` (or "3") in `[workspace]` so dev-dependency features never reach release builds.
- Feature isolation check (one canonical command): `cargo tree -p api -e normal,build,features` must show none of `decrypt`, `test-support`, `testdb` (CI fails otherwise); the same for `poller` with `test-support`/`testdb`. `api` never has a dev-dependency enabling `decrypt`; decrypt round-trips are tested in `core`.

## Web headers and proxy

- `web/security-headers.json` is the single source of truth for headers. `web/scripts/headers.mjs generate` writes both the `netlify.toml` header block and the committed `ops/caddy/security-headers.caddy` (local variant: no HSTS, no Cache-Control); the Caddyfile only `import`s it. `headers.mjs check` verifies both generated outputs, that the Caddyfile imports the file, and the redirect invariants. Web scripts use Node built-ins only. `/_app/*` is excluded from the SPA fallback (missing chunks 404).
- Netlify deploy previews: on for pull requests (branch deploys off); `netlify.toml` `[build] ignore` skips `dependabot/*` preview builds; previews use production `PUBLIC_*` values and proxy to the production API; there is no Supabase redirect list any more (password sign-in is a direct call), so previews sign in with no allow-list entry. Set-password links always point at the production site (`SITE_URL`).
- Web deploys are gated (proposals/04, accepted): Netlify's production branch is `production`; `main` builds nothing; only `deploy.yml` fast-forwards `production` (plain `git push`, never force, from the separate `publish-web` job using `GITHUB_TOKEN` with job-level `contents: write`; it needs the approved `deploy` job) after the API deploy succeeds. The GitHub ruleset on the Git branch `production` only blocks deletion and force pushes (`GITHUB_TOKEN` cannot be a bypass actor, so no "restrict updates" rule); anyone with repo write access (only Rodney) could still push it. The repo is public (decided Oct 7: a private Free repo has no deployment environments, so the `production` approval and environment secrets would not work), so the ruleset and the environment protections are enforced. Public-repo hygiene (06 11.1): no real station MACs, exact coordinates, personal emails or keys in fixtures, seed, docs or history; secret scanning with push protection and Dependabot alerts on; fork-PR workflows need approval; PR workflows use `pull_request` (never `pull_request_target`) and get no secrets. The `production` GitHub environment (approval) and the `production` Git branch (Netlify) are different things. No Netlify token, build hook or site ID exists in GitHub. The web build writes `/version.txt` containing the full commit SHA (from Netlify's `COMMIT_REF`, or `GITHUB_SHA` in CI/local builds); it gets no immutable caching (only `/_app/immutable/*` does), so the host default revalidates it on every request; the deploy smoke step polls it until it equals the released SHA (about 10 minutes at most). Rollback is a revert plus the same push, or "Publish deploy" in the Netlify UI.
- CSP: `script-src` hash via SvelteKit `kit.csp` meta tag; static header carries the other directives. One sanctioned exception: `style-src-attr 'unsafe-hashes' 'sha256-<svelte-announcer hash>'` for SvelteKit's route announcer; no other `unsafe-*`. No dynamic inline styles (use classes).
- Cache-Control is set only on `/_app/immutable/*` (immutable, 1 year); HTML relies on the host default (Netlify) / `no-cache` set with a matcher that excludes immutable paths (Caddy). Caddy removes the `Server` header.
- Signed proxy: `signed = "NETLIFY_PROXY_SECRET"` in `netlify.toml`, **commented out for the first deploy**; enabled later together with the API secret.

## Backups (local)

No local `backup` compose service. The backup image is tested with `ops/backup/test.sh` against a throwaway Postgres + MinIO (06 owns). `app_backup` gets `SELECT` on `app._sqlx_migrations` explicitly (it is created before migration 0001's default privileges apply).

## Poller jobs (names used in `job_runs.job`, locks and healthchecks)

`awn_ingest` (every 5 min), `awn_backfill` (one-off / on demand), `daily_rollup`, `reference_sync` (ACIS daily, plus an hourly pickup that loads only reference stations never synced yet), `credential_reencrypt` (on demand). Each job takes a Postgres advisory lock keyed `job:<name>`. `job_runs.status`: `running`, `success`, `failed`, `cancelled`, `abandoned`; `detail jsonb`.

## Agents and production

Implementing agents never run commands that change production (deploys, admin CLI, sign-up probes, migrations against production). They prepare them; Rodney runs them.

## Guide format (every guide)

Write for an implementing agent. Each guide must contain:

1. **Purpose and scope** — what this component owns and what it does not.
2. **Decisions applied** — the design/review item IDs it implements.
3. **Cross-component contracts** — exact interfaces it exposes/consumes, consistent with this file.
4. **Design** — enough precision to implement without guessing.
5. **Implementation tasks** — ordered checklist of small, verifiable steps, each with a "done when" condition.
6. **Testing** — unit, integration and end-to-end tests, including the security tests named in the review, and how to run them under Docker Compose.
7. **Security checklist** — concrete items to verify before merge.
8. **Open questions / assumptions** — anything unresolved, flagged clearly rather than invented.

Do not invent product decisions. If something is undecided, list it under Open questions.
