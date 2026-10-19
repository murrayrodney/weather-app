# 05 - Local Dev (Docker Compose stack)

Read `00-conventions.md` first; names, ports and env vars there are binding. This guide owns: `docker-compose.yml`, `docker-compose.test.yml`, `.env.example`, `Makefile`, `ops/caddy/Caddyfile`, `ops/db/init/*` (wrappers only), `ops/local/*` scripts, `backend/demo-fingerprints.txt` (content only; consumed by 03/01/06). It does not own `ops/db/bootstrap/{roles.sql,auth_access.sql,scram.py}`, `ops/db/seed/`, `ops/db/tests/` (04), `web/security-headers.json`, `web/scripts/headers.mjs` and the committed `ops/caddy/security-headers.caddy` (01), `ops/backup/` (06). Those are referenced, never duplicated. This guide is the source for the GoTrue environment values (4.4); 04 points here.

## 1. Purpose and scope

Owns: one `docker compose` stack that runs the whole app on one machine with no cloud account: plain Postgres, Supabase Auth (GoTrue) standalone with no mail service (nothing is emailed; sign-in is email + password with hand-delivered set-password links, proposals/05), Caddy (static web + proxy), `api`, `poller`, one-shots `auth-access` and `migrate`, and on-demand `admin` and `web` (build) containers; a separate test project (`docker-compose.test.yml`) provides `test-backend`. Also: env template, replay/live AWN wiring, first-run sequence, reset, test entry points (`make` targets that CI also runs), and the smoke test.

Does not own: Rust code, migrations, role/grant SQL, seed content (04); API routes (02); poller logic and fixtures (03); web code, CSP values (01); Fly/Netlify/Supabase production and the backup image (06). Unconfirmed assumptions about them are in section 8.

## 2. Decisions applied

| ID | Applied as |
| --- | --- |
| D1 | Compose runs `postgres:17`, `supabase/auth`, Caddy, api, poller. No Supabase CLI stack, no PostgREST, no Studio, **no mail catcher** (removed Oct 7, proposals/05: nothing sends email). |
| D2 | Local tokens are HS256 signed with `JWT_HS256_SECRET` (shared with GoTrue). Demo values are committed; their SHA-256 fingerprints are a contract for production refusal (section 3.4). |
| D3, D4 | Caddy serves the static `web/build` (same build as Netlify) and proxies `/api/*` and `/auth/v1/*`. |
| D5 | Schema comes only from `sqlx` migrations run by `migrate` (`admin migrate`) as `app_migrator`. GoTrue owns `auth` and migrates itself. |
| D6 | `POLLER_ENABLED=false` and `AWN_MODE=replay` by default; fixtures in `backend/fixtures/`; live AWN only with the developer's own station key and a separate dev application key. |
| S4 | GoTrue `DISABLE_SIGNUP=true`, anonymous sign-ins off, autoconfirm off; users arrive only via `admin invite` (or `seed-dev`, which creates them with the local demo password). |
| S1, S15 | Smoke test sends anon-key, missing, tampered and `alg:none` tokens and checks security headers through Caddy. |
| S5, S12 | Least-privilege role per process; no process uses `postgres` (except the one-shot `auth-access`, same as production, and the test harness); DB URLs are built in the compose file with host `db`. Distroless-safe: no health or smoke step relies on `exec` into backend containers. |
| S16 | Admin invite is the only user-creation path. Local audit behavior is the production code's. |
| P2 | No SSR; Caddy only serves files. |

## 3. Cross-component contracts

### 3.1 Consumed from other guides

| Contract | Owner | Used for |
| --- | --- | --- |
| Image `weather-backend:local` from `backend/Dockerfile` (delivered by 03's core milestone, binaries present only as they exist); binaries `/usr/local/bin/{api,poller,admin}`; no `ENTRYPOINT`; no shell, no `printenv` (treat as distroless); `COPY certs/` from `backend/certs/` (`.keep` committed) | 02/03, conventions | all Rust services |
| `api healthcheck`, `poller healthcheck`, `poller run-job <job>` (works while disabled; exit 0 on success). When disabled the poller logs exactly `poller disabled: POLLER_ENABLED=false` once | conventions, 03 | healthchecks, `make e2e-data`, smoke |
| `admin migrate` (needs only `APP_ENV`, `DATABASE_URL`) | 04 | `migrate` |
| `ops/db/bootstrap/roles.sql` (reads `APP_{MIGRATOR,API,POLLER,BACKUP,CI_CHECK}_PASSWORD_VERIFIER` with `\getenv`; idempotent), `ops/db/bootstrap/scram.py` (verifier from a password on stdin), `ops/db/bootstrap/auth_access.sql` (idempotent, run as `postgres`) | 04 | `10-bootstrap.sh`, `auth-access`, verifier regeneration |
| `admin invite --email --display-name [--follow-all]` (sends **no email**; prints a one-time set-password link; needs `SITE_URL`), `admin reset-link --email`, `admin list-users` (04 section 10.2 columns, tab-separated: `id`, `display_name`, `email`, `active\|inactive`, `admin\|-`, `auth_ok\|auth_missing`; the last three only with the service key; `auth_only<TAB>id` lines for orphans; exit 1 if any orphan), `admin seed-dev` (reads `SEED_DIR`; refuses unless `APP_ENV=local`), `admin keygen` / `admin keygen --check`, `admin add-station` | conventions, 04 | first run, `make seed`, smoke |
| Seed content: **04 section 12.1** (users incl. Dana and Casey, reference ids `USW00099001..03`, stations `AA:BB:CC:00:00:01..03`, follow states). Stored poller key is `fixture-poller-key`; connect-flow keys `fixture-valid-key`, `fixture-invalid-key`, `fixture-ratelimited-key` are never stored | 04, conventions (Fixtures) | `make seed`, e2e |
| Fixtures and replay clock per conventions (Fixtures) | 03 | replay mode, `make e2e-data` |
| `GET /api/health` and `GET /api/v1/me` (02 section 5.2) | 02 | smoke |
| Web npm scripts `build` (output `web/build/` incl. `200.html`), `lint`, `check`, `test`, `test:tz`, `test:bundle`, `headers:check`, `test:e2e`; `web/scripts/headers.mjs` finds `ops/caddy/` at `../ops/caddy` relative to `web/`; Vite dev proxy for `/api` and `/auth/v1` to `http://localhost:8000`; Node 26 | 01 | `web` service, `web-dev`, tests |
| `web/security-headers.json` and committed `ops/caddy/security-headers.caddy` (local variant, no HSTS, no `Cache-Control`), generated and verified by `headers.mjs` | 01 | Caddy headers |
| `ops/db/tests/run.sh` run as `postgres` inside the test project's `db` container at `/work/ops/db/tests/run.sh` | 04 | `make test-db` |
| Test harness per conventions (Testing harness): own Compose project, tmpfs db, torn down | conventions | 5.1b, 5.7 |

### 3.2 Exposed

- Host ports, all bound to `127.0.0.1`: Caddy `8000`, Postgres `5432`. Nothing else is published.
- Browser origin `http://localhost:8000`. GoTrue issuer `http://localhost:8000/auth/v1`, audience `authenticated`. Printed set-password links start `http://localhost:8000/set-password#token_hash=` (`SITE_URL=http://localhost:8000` for the admin container).
- `make` targets in 5.7. CI runs `make test` (= `check-local-only test-db test-backend test-web`; each self-starting in project `famweather-test`) and may run `make smoke SMOKE_ARGS=--clean`.

### 3.3 Compose-only names (not read by other components)

`POSTGRES_PASSWORD`, `SUPABASE_AUTH_ADMIN_PASSWORD`, `SUPABASE_AUTH_ADMIN_PASSWORD_VERIFIER`, `APP_CI_CHECK_PASSWORD`, `WEB_NODE_IMAGE`. The plaintext `APP_*_PASSWORD` values appear only inside the role-specific `DATABASE_URL`s; the `db` service receives only `*_VERIFIER` values (conventions, Testing harness, last bullets). Everything else comes from conventions.

### 3.4 Demo-value fingerprints (D2 contract)

Single source: `backend/demo-fingerprints.txt` (file content in 5.9; format `<sha256-hex>  <VARIABLE_NAME>`, hash of the exact string in `.env.example`). It lists the HS256 secret, anon key, service key, HPKE public and private keys, the five local role passwords, plus `POSTGRES_PASSWORD`, `SUPABASE_AUTH_ADMIN_PASSWORD` and `LOCAL_DEMO_PASSWORD` (the password `seed-dev` gives the seed users). Consumers (each adds a test that fails when the file and `.env.example` drift): `core::config` (03, `include_str!`, refuses matches in production), `admin` (04, same check), the web production build check (01, anon key and localhost URL), 06's pre-import check. The fingerprint file is the only mechanism for demo-value refusal (no separate constants in 02). Smoke step 3 recomputes every line from `.env.example`.

## 4. Design

### 4.1 Topology

```
host :8000 -> caddy:80 --/api/*-------> api:8080 --> db:5432 (app_api)
                      --/auth/v1/*--> auth:9999 --> db:5432 (supabase_auth_admin)   (no SMTP)
                      --else-------> /srv/webroot/build (./web mounted read-only, SPA fallback 200.html)
host :5432 -> db:5432        poller -> db (app_poller); AWN only if AWN_MODE=live        api -> Open-Meteo only if AWN_MODE=live (replay serves recorded files)
```

One default network. No `internal: true` split: the poller (live) and api (connect flow, live forecasts) need egress, and locally there is nothing to isolate from. Only Caddy and Postgres publish ports, on `127.0.0.1` (not the LAN).

### 4.2 Startup order

`db` healthy (init ran `roles.sql`) -> `auth` healthy (GoTrue has created `auth.*`) -> `auth-access` completed successfully (`postgres:17` psql runs `auth_access.sql` as `postgres`, same file and role as production) -> `migrate` completed successfully (sqlx as `app_migrator`) -> `api`, `poller` healthy -> `caddy` healthy. Compose sets `APP_ENV=local` and every `DATABASE_URL` host to `db` literally so `.env` cannot point containers at production.

### 4.3 Web build delivery: mount `./web`, serve `web/build`

Decision: Caddy mounts the whole `./web` directory read-only at `/srv/webroot` and serves `/srv/webroot/build`. Reason: `@sveltejs/adapter-static` deletes and recreates `web/build/` on every build; a bind mount of the `build` directory itself would keep pointing at the deleted inode and serve stale or no files. Mounting the parent survives rebuilds and Caddy sees new files without a restart. The build runs in the one-shot `web` service (profile `tools`) or on the host. `PUBLIC_*` values are baked at build time from process env (not the root `.env` file, which holds secrets); the `web` service passes only the two allowed variables. A multi-stage image was rejected: it forces an image rebuild per frontend change and hides `PUBLIC_*` in build args. Downside: the container sees `web/` including `node_modules`; it is read-only and local.

Frontend iteration (`make web-dev`): needs host Node 26 and a one-time `cd web && npm ci`. It runs `vite dev` at `http://localhost:5173` with `PUBLIC_SUPABASE_URL=http://localhost:5173`; Vite proxies `/api` and `/auth/v1` to Caddy on 8000 (01 owns the proxy config). Password sign-in and the set-password link flow work from the Vite origin without any redirect allow-list (direct calls); printed links point at port 8000 (`SITE_URL`), so open them there. Tokens stay valid because the issuer is pinned. Verify CSP only against `localhost:8000` (dev CSP differs).

### 4.4 Local auth (GoTrue standalone)

This section and 5.1 are the source for the local GoTrue values (04 points here). Image `supabase/auth` (Docker Hub; formerly `supabase/gotrue`), pinned `v2.197.0` (latest release found, 2026-09-09; confirm it pulls, bump deliberately, never `latest`). Multi-arch.

- DB: `GOTRUE_DB_DRIVER=postgres`, `GOTRUE_DB_DATABASE_URL=postgres://supabase_auth_admin:...@db:5432/postgres` (the role's SCRAM verifier is set by `01-auth-prereqs.sh`), `DB_NAMESPACE=auth`. Role owns schema `auth`, `search_path=auth` (init `01-auth-prereqs.sh`). GoTrue migrates at startup.
- JWT: `GOTRUE_JWT_SECRET=${JWT_HS256_SECRET}` (HS256, 32+ chars), `GOTRUE_JWT_AUD=authenticated`, `GOTRUE_JWT_EXP=3600`, `GOTRUE_JWT_ADMIN_ROLES=service_role`, `GOTRUE_JWT_ISSUER=http://localhost:8000/auth/v1` (pinned, so issuer does not depend on request host).
- URLs: `API_EXTERNAL_URL=http://localhost:8000` (this is the bare variable GoTrue reads; `GOTRUE_API_EXTERNAL_URL` is not read). No `GOTRUE_MAILER_URLPATHS_*`: nothing emails a link. `generate_link` still returns an `action_link` built from this URL, which `admin invite` ignores (it prints `SITE_URL/set-password#token_hash=...` from the `hashed_token`).
- Site and redirects: `GOTRUE_SITE_URL=http://localhost:8000`; `GOTRUE_URI_ALLOW_LIST` **empty** (no flow uses a redirect; conventions). The Vite dev origin `http://localhost:5173` needs no entry either because password sign-in and `verifyOtp` are direct calls.
- Invite-only (S4): `GOTRUE_DISABLE_SIGNUP=true` (blocks `/signup` and OTP user creation; creating users through the admin API with the service-role JWT still works), `GOTRUE_EXTERNAL_ANONYMOUS_USERS_ENABLED=false`, `GOTRUE_MAILER_AUTOCONFIRM=false`, `GOTRUE_EXTERNAL_PHONE_ENABLED=false`, `GOTRUE_EXTERNAL_EMAIL_ENABLED=true` (enables the password grant that the web app uses to sign in). Admin-created users are created with `email_confirm: true`.
- Password changes: `GOTRUE_SECURITY_UPDATE_PASSWORD_REQUIRE_REAUTHENTICATION=false` (a changed password must not demand an emailed nonce), the current-password requirement off (exact variable name for the pinned tag is an assumption to confirm in task 3: GoTrue's setting for "require current password"; the compose tests `invite_link_sets_password` and `reset_link_revokes_sessions_and_allows_reset`, 04, fail if it is on), `GOTRUE_PASSWORD_MIN_LENGTH=12`, `GOTRUE_MAILER_OTP_EXP=3600` (link lifetime), `GOTRUE_RATE_LIMIT_HEADER=X-Forwarded-For`. **No `GOTRUE_SMTP_*` at all.** Without SMTP GoTrue's other email endpoints (`/otp`, `/recover`) fail to send, which is the intended local behaviour.
- Note: `admin invite` and `admin reset-link` call the admin API (`generate_link`), send no email, and print the link; seed users (`make seed`) are created with a demo password so e2e tests and humans can sign in without a link.

### 4.5 Role and secret handling

- `APP_ENV: local` is a literal in compose. Each Rust service receives only what conventions allow: `api` has the public key and no private key or service key; `poller` has the private key only; `SUPABASE_SERVICE_ROLE_KEY` only in `admin`; `migrate` only `APP_ENV` + `DATABASE_URL`.
- Passwords must be URL-safe (`[A-Za-z0-9._~-]`).

### 4.6 Replay and live AWN (D6)

- Default `AWN_MODE=replay`, `POLLER_ENABLED=false`: the poller logs once, touches its heartbeat, does nothing else (no DB writes, no `job_runs`, no pings).
- Replay data: `make e2e-data` (4.9) runs jobs once through `poller run-job` (explicit operator action; works while disabled). `make poller-replay` instead enables the scheduler.
- Forecasts (proposals/06): nothing is stored, so there is no forecast seed or `e2e-data` step. With `AWN_MODE=replay` the `api` serves the two forecast routes from recorded Open-Meteo files in `backend/fixtures/open-meteo/` (02 section 5.6 and 7.9), time-shifted so the forecast starts today; with `AWN_MODE=live` the API calls the real Open-Meteo hosts (keyless, cached 30 minutes). Compose tests never reach Open-Meteo.
- Live: `make live` refuses while `AWN_APPLICATION_KEY` is the placeholder, then recreates `api` and `poller` (`--no-deps`) with `AWN_MODE=live POLLER_ENABLED=true`. Procedure: `make reset && make up` (no seed: seeded fake credentials would 401 and burn rate limit), `make invite EMAIL=you@example.com NAME=You`, then add only your own station via the web connect flow or `make add-station`. Never use another person's key or the production application key locally.
- `HEALTHCHECK_URL_*` blank in `.env.example`; core ignores them locally.

### 4.7 Startup refusal rules

Defined once in conventions, Environments. This guide supplies the placeholder `REPLACE_WITH_DEV_APPLICATION_KEY` and tests the refusals (smoke step 12).

### 4.8 Apple Silicon, versions

- Never set `platform:`; all images are multi-arch and run natively. Forcing amd64 is slow and flaky. Rust builds natively as aarch64 (give Docker Desktop 6 GB / 4 CPUs; 02 should use BuildKit cache mounts). The Fly image is amd64 (06).
- Host needs: Docker Compose v2 that supports `up --wait` with completed one-shot dependencies (the implementer records the exact output of `docker compose version` here on first pass; the author's host has Docker Engine 29.6.2), GNU make (3.81 on macOS is fine), `curl`, `jq`, `openssl`, `shasum`; Node 26 only for `make web-dev`.
- Fallback if `--wait` rejects one-shots: `docker compose up -d`, then loop `until [ "$(docker compose ps --format json | jq -s '[.[]|select(.Service|IN("db","auth","api","poller","caddy"))|select(.Health!="healthy")]|length')" = 0 ]; do sleep 2; done` with a 180 s cap.
- Image build: `api` is the only service with `build:`; the others use the same tag (`pull_policy: never`). After backend changes run `make build && make up` (the `up` target builds first); otherwise `poller`, `migrate` and `admin` run a stale image.

### 4.9 `make e2e-data` (data for 01's e2e)

01 needs reference and current-year data for the seeded stations (01 section 6.3); states the seed cannot produce are mocked in the specs. `ops/local/e2e-data.sh` runs, via `docker compose run --rm --no-deps poller poller run-job <job>`: `reference_sync`, then `awn_backfill`, then `awn_ingest`, then `daily_rollup`; then asserts through `psql` as `postgres` (column names per 04): for each of the three seeded stations (MACs `AA:BB:CC:00:00:01..03`) `observations` rows > 0 and `daily_rollups` rows > 0, and for each station's reference id (`USW00099001..03`) at least 12 distinct years in `reference_daily`. Exit non-zero with the failing station. It is idempotent (jobs are repeat-safe). Prerequisite: `make seed`.

## 5. Files

### 5.1 `docker-compose.yml`

```yaml
name: weather

x-backend: &backend
  image: weather-backend:local
  pull_policy: never
  restart: "no"

services:
  db:
    image: postgres:17
    environment:
      POSTGRES_USER: postgres
      POSTGRES_PASSWORD: ${POSTGRES_PASSWORD:?copy .env.example to .env}
      POSTGRES_DB: postgres
      # unix-socket trust for in-container admin (exec -u postgres); every TCP login uses SCRAM
      POSTGRES_INITDB_ARGS: "--auth-local=trust --auth-host=scram-sha-256"
      # Verifiers only: the db container never sees the role plaintext passwords.
      SUPABASE_AUTH_ADMIN_PASSWORD_VERIFIER: ${SUPABASE_AUTH_ADMIN_PASSWORD_VERIFIER:?}
      APP_MIGRATOR_PASSWORD_VERIFIER: ${APP_MIGRATOR_PASSWORD_VERIFIER:?}
      APP_API_PASSWORD_VERIFIER: ${APP_API_PASSWORD_VERIFIER:?}
      APP_POLLER_PASSWORD_VERIFIER: ${APP_POLLER_PASSWORD_VERIFIER:?}
      APP_BACKUP_PASSWORD_VERIFIER: ${APP_BACKUP_PASSWORD_VERIFIER:?}
      APP_CI_CHECK_PASSWORD_VERIFIER: ${APP_CI_CHECK_PASSWORD_VERIFIER:?}
    volumes:
      - db_data:/var/lib/postgresql/data
      - ./ops/db/init:/docker-entrypoint-initdb.d:ro
      - ./ops:/ops:ro
    ports:
      - "127.0.0.1:5432:5432"
    healthcheck:
      # -h 127.0.0.1: during first-run init the temporary server listens on the unix socket only,
      # so a socket-based pg_isready would report ready too early.
      test: ["CMD-SHELL", "pg_isready -h 127.0.0.1 -U postgres -d postgres"]
      interval: 3s
      timeout: 3s
      retries: 30

  auth:
    image: supabase/auth:v2.197.0
    depends_on:
      db:
        condition: service_healthy
    environment:
      GOTRUE_API_HOST: 0.0.0.0
      GOTRUE_API_PORT: "9999"
      API_EXTERNAL_URL: http://localhost:8000
      GOTRUE_DB_DRIVER: postgres
      GOTRUE_DB_DATABASE_URL: postgres://supabase_auth_admin:${SUPABASE_AUTH_ADMIN_PASSWORD}@db:5432/postgres
      DB_NAMESPACE: auth
      GOTRUE_JWT_SECRET: ${JWT_HS256_SECRET:?}
      GOTRUE_JWT_EXP: "3600"
      GOTRUE_JWT_AUD: authenticated
      GOTRUE_JWT_DEFAULT_GROUP_NAME: authenticated
      GOTRUE_JWT_ADMIN_ROLES: service_role
      GOTRUE_JWT_ISSUER: http://localhost:8000/auth/v1
      GOTRUE_SITE_URL: http://localhost:8000
      GOTRUE_URI_ALLOW_LIST: ""        # empty on purpose: no flow uses a redirect (proposals/05)
      GOTRUE_DISABLE_SIGNUP: "true"
      GOTRUE_EXTERNAL_ANONYMOUS_USERS_ENABLED: "false"
      GOTRUE_EXTERNAL_EMAIL_ENABLED: "true"
      GOTRUE_EXTERNAL_PHONE_ENABLED: "false"
      GOTRUE_MAILER_AUTOCONFIRM: "false"
      GOTRUE_MAILER_OTP_EXP: "3600"                                      # lifetime of the set-password link
      GOTRUE_PASSWORD_MIN_LENGTH: "12"
      GOTRUE_SECURITY_UPDATE_PASSWORD_REQUIRE_REAUTHENTICATION: "false"  # no emailed nonce to change a password
      GOTRUE_SECURITY_UPDATE_PASSWORD_REQUIRE_CURRENT_PASSWORD: "false"  # name assumed for this tag: confirm in task 3 (the 04 compose tests fail if the switch is on)
      GOTRUE_RATE_LIMIT_HEADER: X-Forwarded-For
      # No GOTRUE_SMTP_* and no GOTRUE_MAILER_URLPATHS_*: nothing is emailed (proposals/05).
    healthcheck:
      test: ["CMD", "wget", "--quiet", "--spider", "http://127.0.0.1:9999/health"]
      interval: 5s
      timeout: 3s
      retries: 30
      start_period: 10s

  auth-access:
    # One-shot: same file and same role (postgres) as production. Needs GoTrue's tables to exist.
    image: postgres:17
    depends_on:
      auth:
        condition: service_healthy
    environment:
      PGPASSWORD: ${POSTGRES_PASSWORD}
    volumes:
      - ./ops/db/bootstrap:/bootstrap:ro
    command: ["psql", "-X", "-v", "ON_ERROR_STOP=1", "-h", "db", "-U", "postgres", "-d", "postgres", "-f", "/bootstrap/auth_access.sql"]
    restart: "no"

  migrate:
    <<: *backend
    depends_on:
      auth-access:
        condition: service_completed_successfully
    environment:
      APP_ENV: local
      DATABASE_URL: postgres://app_migrator:${APP_MIGRATOR_PASSWORD}@db:5432/postgres?sslmode=disable
      RUST_LOG: ${RUST_LOG:-info}
    command: ["admin", "migrate"]

  api:
    <<: *backend
    build:
      context: ./backend
    depends_on:
      migrate:
        condition: service_completed_successfully
      auth:
        condition: service_healthy
    environment:
      APP_ENV: local
      API_BIND_ADDR: 0.0.0.0:8080
      DATABASE_URL: postgres://app_api:${APP_API_PASSWORD}@db:5432/postgres?sslmode=disable
      SUPABASE_URL: http://localhost:8000
      JWT_HS256_SECRET: ${JWT_HS256_SECRET}
      AWN_APPLICATION_KEY: ${AWN_APPLICATION_KEY:-REPLACE_WITH_DEV_APPLICATION_KEY}
      AWN_MODE: ${AWN_MODE:-replay}
      FIXTURES_DIR: /fixtures
      CREDENTIALS_PUBLIC_KEY: ${CREDENTIALS_PUBLIC_KEY:?}
      CREDENTIALS_KEY_VERSION: ${CREDENTIALS_KEY_VERSION:-1}
      RUST_LOG: ${RUST_LOG:-info}
    volumes:
      - ./backend/fixtures:/fixtures:ro
    command: ["api"]
    healthcheck:
      test: ["CMD", "api", "healthcheck"]
      interval: 5s
      timeout: 3s
      retries: 30
      start_period: 5s

  poller:
    <<: *backend
    depends_on:
      migrate:
        condition: service_completed_successfully
    environment:
      APP_ENV: local
      DATABASE_URL: postgres://app_poller:${APP_POLLER_PASSWORD}@db:5432/postgres?sslmode=disable
      AWN_APPLICATION_KEY: ${AWN_APPLICATION_KEY:-REPLACE_WITH_DEV_APPLICATION_KEY}
      AWN_MODE: ${AWN_MODE:-replay}
      FIXTURES_DIR: /fixtures
      CREDENTIALS_PRIVATE_KEY: ${CREDENTIALS_PRIVATE_KEY:?}
      CREDENTIALS_KEY_VERSION: ${CREDENTIALS_KEY_VERSION:-1}
      POLLER_ENABLED: ${POLLER_ENABLED:-false}
      RUST_LOG: ${RUST_LOG:-info}
    volumes:
      - ./backend/fixtures:/fixtures:ro
    command: ["poller"]
    healthcheck:
      test: ["CMD", "poller", "healthcheck"]
      interval: 10s
      timeout: 3s
      retries: 12
      start_period: 10s

  caddy:
    image: caddy:2
    depends_on:
      api:
        condition: service_healthy
      auth:
        condition: service_healthy
    volumes:
      - ./ops/caddy/Caddyfile:/etc/caddy/Caddyfile:ro
      - ./ops/caddy/security-headers.caddy:/etc/caddy/security-headers.caddy:ro
      - ./web:/srv/webroot:ro
      - caddy_data:/data
    ports:
      - "127.0.0.1:8000:80"
    healthcheck:
      test: ["CMD", "wget", "--quiet", "--spider", "http://127.0.0.1/healthz"]
      interval: 5s
      timeout: 3s
      retries: 20

  # ---- on demand (profiles) ----

  admin:
    <<: *backend
    profiles: ["tools"]
    depends_on:
      migrate:
        condition: service_completed_successfully
      caddy:
        condition: service_healthy
    environment:
      APP_ENV: local
      DATABASE_URL: postgres://app_migrator:${APP_MIGRATOR_PASSWORD}@db:5432/postgres?sslmode=disable
      SUPABASE_URL: http://caddy # in-network route to GoTrue at /auth/v1; admin does not verify tokens
      SUPABASE_SERVICE_ROLE_KEY: ${SUPABASE_SERVICE_ROLE_KEY:?}
      SITE_URL: ${SITE_URL:-http://localhost:8000} # origin printed in set-password links
      LOCAL_DEMO_PASSWORD: ${LOCAL_DEMO_PASSWORD:?} # seed-dev only (04 12.1)
      CREDENTIALS_PUBLIC_KEY: ${CREDENTIALS_PUBLIC_KEY:?}
      CREDENTIALS_KEY_VERSION: ${CREDENTIALS_KEY_VERSION:-1}
      AWN_APPLICATION_KEY: ${AWN_APPLICATION_KEY:-REPLACE_WITH_DEV_APPLICATION_KEY}
      AWN_MODE: ${AWN_MODE:-replay}
      FIXTURES_DIR: /fixtures
      SEED_DIR: /seed
      RUST_LOG: ${RUST_LOG:-info}
    volumes:
      - ./backend/fixtures:/fixtures:ro
      - ./ops/db/seed:/seed:ro
    command: ["admin", "--help"]
    # No tty/stdin_open here: scripted runs use `run -T`; only `make add-station` runs interactively.

  web:
    profiles: ["tools"]
    image: ${WEB_NODE_IMAGE:-node:26-bookworm-slim}
    working_dir: /work/web
    environment:
      PUBLIC_SUPABASE_URL: ${PUBLIC_SUPABASE_URL:-http://localhost:8000}
      PUBLIC_SUPABASE_ANON_KEY: ${PUBLIC_SUPABASE_ANON_KEY:?}
    volumes:
      - ./web:/work/web
      - web_node_modules:/work/web/node_modules
    command: ["sh", "-c", "npm ci && npm run build"]

volumes:
  db_data:
  caddy_data:
  web_node_modules:
```

Compose reads `.env` itself; verifier values are single-quoted in `.env` so `$` is literal (Compose does not interpolate inside single quotes). `.env` is never `source`d or `export`ed by `make`.

### 5.1b `docker-compose.test.yml` (test project only)

Used only as `docker compose -p famweather-test -f docker-compose.yml -f docker-compose.test.yml ...` (Makefile macro `T`). Requires Compose >= 2.24 for `!reset`/`!override`. The test project shares no containers, volumes, networks or host ports with the working stack. Every test container that mounts the repo masks `/work/.env` with `/dev/null` (conventions), so `.env` must exist (the test targets depend on `make init`).

```yaml
services:
  db:
    ports: !reset []
    volumes: !override
      - ./ops/db/init:/docker-entrypoint-initdb.d:ro
      - ./ops:/ops:ro
      - .:/work:ro
      - /dev/null:/work/.env:ro
    tmpfs:
      - /var/lib/postgresql/data
  caddy:
    ports: !reset []

  web:
    # test-web: sources are copied into a tmpfs at /work/web and built there (never the host's web/build,
    # no root-owned files on the host). ops/caddy is visible so headers:check finds ../ops/caddy; the fingerprint file is
    # visible so the production-context bundle check can read ../backend/demo-fingerprints.txt.
    volumes: !override
      - ./web:/src/web:ro
      - ./ops/caddy:/work/ops/caddy:ro
      - ./backend/demo-fingerprints.txt:/work/backend/demo-fingerprints.txt:ro
      - /dev/null:/work/.env:ro
    tmpfs:
      - /work/web:size=3g,exec # tmpfs is noexec by default; npm/vite need exec
    working_dir: /work/web

  test-backend:
    image: rust:1-bookworm
    working_dir: /work/backend
    depends_on:
      migrate:
        condition: service_completed_successfully
      auth:
        condition: service_healthy
    environment:
      DATABASE_URL: postgres://postgres:${POSTGRES_PASSWORD}@db:5432/postgres?sslmode=disable # superuser; #[sqlx::test] reads it
      TEST_SUPERUSER_DATABASE_URL: postgres://postgres:${POSTGRES_PASSWORD}@db:5432/postgres?sslmode=disable
      TEST_MIGRATOR_DATABASE_URL: postgres://app_migrator:${APP_MIGRATOR_PASSWORD}@db:5432/postgres?sslmode=disable
      TEST_JWT_HS256_SECRET: ${JWT_HS256_SECRET}
      SUPABASE_URL: http://auth:9999 # tests mint a service-role JWT from the HS256 secret; set-password tests call GoTrue directly here (/verify, /user, /token)
      SITE_URL: http://localhost:8000 # origin printed in set-password links
      FIXTURES_DIR: /work/backend/fixtures
      SEED_DIR: /work/ops/db/seed # admin seed_dev_idempotent reads seed.toml from here (the repo is mounted at /work below)
      LOCAL_DEMO_PASSWORD: ${LOCAL_DEMO_PASSWORD:?} # seed-dev only (04 12.1)
      CREDENTIALS_PUBLIC_KEY: ${CREDENTIALS_PUBLIC_KEY:?}
      CREDENTIALS_PRIVATE_KEY: ${CREDENTIALS_PRIVATE_KEY:?}
      CREDENTIALS_KEY_VERSION: ${CREDENTIALS_KEY_VERSION:-1}
      CARGO_TARGET_DIR: /cargo-target
    volumes:
      - .:/work
      - /dev/null:/work/.env:ro # mask the developer's .env
      # CI sets CARGO_REGISTRY_SRC / CARGO_TARGET_SRC (06 11.2) so these two become bind mounts of directories that
      # actions/cache saves between runs; unset (local development) they stay named volumes. Compose treats a source
      # containing "/" as a bind mount.
      - ${CARGO_REGISTRY_SRC:-cargo_registry}:/usr/local/cargo/registry
      - ${CARGO_TARGET_SRC:-cargo_target}:/cargo-target
    command:
      - sh
      - -ec
      - |
        FEAT=""; [ -d crates/api ] && FEAT="--features api/test-support"
        EXCL=""; [ -d crates/admin ] && EXCL="--exclude admin"
        cargo test --workspace $EXCL $FEAT
        if [ -d crates/admin ]; then cargo test -p admin --features compose-tests -- --test-threads=1; fi

volumes:
  cargo_registry:
  cargo_target:
```

`test-backend` runs the conventions command in two steps so admin's compose tests are serialized (`--test-threads=1`) while the rest run in parallel. There is no `cargo sqlx prepare --check` step and no sqlx-cli install: queries are runtime `sqlx::query` with bind parameters (02 4.12), so there is no offline data. Feature flags follow what exists: `api/test-support` only when `backend/crates/api` exists, and the admin step only when `backend/crates/admin` exists. The `db` log is grepped for secret sentinels by the Makefile before teardown (5.7), which replaces a separate secret-log target.

Why a separate project: the working stack's data and host ports (8000, 5432) are never touched, and `down -v` after each run leaves nothing behind. The `/dev/null` mounts only hide `.env` from container filesystems; Compose still reads `.env` for interpolation (demo values only).

Notes: there is no `backup` service (conventions, Backups (local)). The backup image is tested by 06's `ops/backup/test.sh` against throwaway Postgres + MinIO. `admin` mirrors 04's contract (migrator URL, `SUPABASE_URL=http://caddy`, service key, public key). `make up` runs without profiles, so only long-running services and one-shots start.

### 5.2 `ops/db/init/` (run once, on an empty `db_data` volume, as `postgres`, filename order)

`ops/db/init/01-auth-prereqs.sh` (GoTrue's own prerequisites; not app roles)

```sh
#!/bin/sh
set -eu
# The verifier is read inside psql with \getenv (not argv) and is a precomputed SCRAM-SHA-256 string,
# so no password text appears in SQL either.
psql -X -v ON_ERROR_STOP=1 -U "$POSTGRES_USER" -d "$POSTGRES_DB" <<'SQL'
\getenv auth_verifier SUPABASE_AUTH_ADMIN_PASSWORD_VERIFIER
-- Roles GoTrue's migrations and Supabase JWT claims refer to. NOLOGIN, no rights.
-- Created before roles.sql so its REVOKEs on anon/authenticated/service_role apply.
-- supabase_admin and dashboard_user are precautionary; delete them if auth boots without (8.1).
CREATE ROLE anon NOLOGIN NOINHERIT;
CREATE ROLE authenticated NOLOGIN NOINHERIT;
CREATE ROLE service_role NOLOGIN NOINHERIT BYPASSRLS;
CREATE ROLE authenticator NOLOGIN NOINHERIT; -- also in 04's test stub; creating it here avoids parallel-test races
CREATE ROLE supabase_admin NOLOGIN;
CREATE ROLE dashboard_user NOLOGIN;

CREATE ROLE supabase_auth_admin LOGIN NOINHERIT CREATEROLE PASSWORD :'auth_verifier';
GRANT CREATE ON DATABASE postgres TO supabase_auth_admin;
CREATE SCHEMA IF NOT EXISTS auth AUTHORIZATION supabase_auth_admin;
ALTER ROLE supabase_auth_admin SET search_path = auth;
SQL
```

`ops/db/init/10-bootstrap.sh` (wrapper required by 04; the SQL is 04's)

```sh
#!/bin/sh
set -eu
# roles.sql reads APP_*_PASSWORD_VERIFIER from the environment with \getenv; nothing on argv.
psql -X -v ON_ERROR_STOP=1 -U "$POSTGRES_USER" -d "$POSTGRES_DB" -f /ops/db/bootstrap/roles.sql
```

Both files must be executable (`chmod +x`). Changing any password or verifier in `.env` after first start requires `make reset` (init scripts do not rerun). To change a demo password, recompute its verifier with `printf '%s' '<password>' | python3 ops/db/bootstrap/scram.py` (04) and update `backend/demo-fingerprints.txt`. `auth_access.sql` is not run here because GoTrue's tables do not exist yet; the `auth-access` one-shot runs it.

### 5.3 `ops/caddy/Caddyfile`

Header values come from 01's `web/security-headers.json` through the generated, committed `ops/caddy/security-headers.caddy`; this file never contains header values of its own. Differences from production, by design: no HSTS and no `upgrade-insecure-requests` (plain http locally), proxy paths, `/healthz`. Cache rules follow conventions (Web headers and proxy): immutable on `/_app/immutable/*`, `no-cache` on everything else via a matcher, `Server` removed.

```caddyfile
{
	admin off
	auto_https off
}

:80 {
	encode zstd gzip
	header -Server

	handle /healthz {
		respond "ok" 200
	}

	handle /api/* {
		request_body {
			max_size 1MB
		}
		reverse_proxy api:8080
	}

	# GoTrue serves at /, clients call /auth/v1/*; handle_path strips the prefix.
	handle_path /auth/v1/* {
		reverse_proxy auth:9999
	}

	# Build assets: never fall back to the SPA page, so a missing chunk is a real 404.
	handle /_app/* {
		root * /srv/webroot/build
		import /etc/caddy/security-headers.caddy
		@immutable path /_app/immutable/*
		header @immutable Cache-Control "public, max-age=31536000, immutable"
		@mutable not path /_app/immutable/*
		header @mutable Cache-Control "no-cache"
		file_server
	}

	handle {
		root * /srv/webroot/build
		import /etc/caddy/security-headers.caddy
		header Cache-Control "no-cache"
		# GET / has no file named "/" to fall back from; rewrite it explicitly.
		@root path /
		rewrite @root /200.html
		try_files {path} /200.html
		file_server
	}
}
```

`ops/caddy/security-headers.caddy` has this shape (values generated, never hand-edited):

```caddyfile
header {
	Content-Security-Policy "<local CSP from web/security-headers.json>"
	X-Content-Type-Options "nosniff"
	# ... every other header in web/security-headers.json except Cache-Control and HSTS
}
```

`/api/*` is never SPA-fallbacked, so unknown API paths return the API's JSON error. API responses carry their own headers (02).

### 5.4 Bootstrap SQL (reference only)

`ops/db/bootstrap/roles.sql` and `auth_access.sql` are 04's. Locally `roles.sql` runs from `10-bootstrap.sh` and `auth_access.sql` from the `auth-access` service (as `postgres`, like production; proposals/03). Do not copy their contents here.

### 5.5 `.env.example`

```dotenv
# ===== LOCAL DEVELOPMENT ONLY. `make init` copies this to .env (git-ignored). =====
# Every secret below is a throwaway demo value, public on purpose. Production refuses them
# (fingerprints in backend/demo-fingerprints.txt). Never put a production URL, key or password here.
# APP_ENV is hard-coded to `local` in docker-compose.yml. The Makefile never sources this file.
# Verifier values contain `$`, so they MUST stay single-quoted.

# --- Postgres: demo plaintext passwords (used inside DATABASE_URLs) and their SCRAM-SHA-256 verifiers
# (the db container receives only the verifiers). Changing any of these needs `make reset`.
POSTGRES_PASSWORD=local-only-postgres
SUPABASE_AUTH_ADMIN_PASSWORD=local-only-auth-admin
SUPABASE_AUTH_ADMIN_PASSWORD_VERIFIER='SCRAM-SHA-256$4096:32eFv3hL6hK3jhlwNosdRQ==$BEbGD6L66z6e7tO7ME2AeDN4rpVxf5S/SB7JBBWNsEY=:4v4DVTSqJHVGr3zAI/37BSY9Do6eT3W1Rm9B9YyjWFw='
APP_MIGRATOR_PASSWORD=local-only-migrator
APP_MIGRATOR_PASSWORD_VERIFIER='SCRAM-SHA-256$4096:fFag4EGknLlHh1wzRSbcSQ==$+F3sWmGl7IGIaHWov3n3VrRxJdc5GnHKE3P14g2aj3k=:1B5FFZuTi6QY+HyubdXELziD2qtq2nI0ebDnvEyh02A='
APP_API_PASSWORD=local-only-api
APP_API_PASSWORD_VERIFIER='SCRAM-SHA-256$4096:8j+XVk8A+MNPkE1F9ikN5w==$z6bHBDJ0y4QPtQ9jxvQ/l7ujPgoQm1MrTieEbeWqe84=:v7Nl6K7PD29975xasa4rpxOvw8C1NxNp0B+/J+lm0TM='
APP_POLLER_PASSWORD=local-only-poller
APP_POLLER_PASSWORD_VERIFIER='SCRAM-SHA-256$4096:tT5yYAjAkVHqonjfMp7j4g==$q9713kekQiaKeE1CeTPOxUbXZxkSF3Ol31Z5ytI1bA0=:1uuIg4T8q+HmXk74CWPysI7sUks2R/fo3FGZhEkJLmI='
APP_BACKUP_PASSWORD=local-only-backup
APP_BACKUP_PASSWORD_VERIFIER='SCRAM-SHA-256$4096:BrA5Lhm5KAs9pBzTNqI09w==$osr/xR86uUq/9OiTkZTHGsZj8MbGlUpbRklnwHlruF0=:QwfDvFC7xwvr2PBPf/3cOT/LT6CVailiThfjSoE5oVU='
APP_CI_CHECK_PASSWORD=local-only-ci-check
APP_CI_CHECK_PASSWORD_VERIFIER='SCRAM-SHA-256$4096:vdvsRpZ6HwQor6PwMaXVjQ==$OeGsH6F1hpi0+Iz/ROGyWRLqEbCIBhNh2kdjJHC8Zo0=:RyQyYGEBB4O2+lUPLbmNLWllElZSz5/4DlKRSUiiZpg='

# --- Local auth (HS256). DEMO VALUES. ---
JWT_HS256_SECRET=local-dev-only-jwt-secret-0123456789abcdef
PUBLIC_SUPABASE_URL=http://localhost:8000
# Origin the admin CLI prints in set-password links (local stack).
SITE_URL=http://localhost:8000
# Demo password that `make seed` gives every seed user (so e2e and humans can sign in without a link). Local only; fingerprinted.
LOCAL_DEMO_PASSWORD=local-only-demo-pass-1234
# HS256 JWTs signed with the secret above; role=anon / role=service_role; exp 2036.
PUBLIC_SUPABASE_ANON_KEY=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZS1sb2NhbCIsInJvbGUiOiJhbm9uIiwiaWF0IjoxNzkwMDAwMDAwLCJleHAiOjIxMDUwMDAwMDB9.VR-1wjYVUeZZJ9KTmBV81xUOupDvDxxZLREuZKgTatY
# Admin CLI only (never api/poller).
SUPABASE_SERVICE_ROLE_KEY=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZS1sb2NhbCIsInJvbGUiOiJzZXJ2aWNlX3JvbGUiLCJpYXQiOjE3OTAwMDAwMDAsImV4cCI6MjEwNTAwMDAwMH0.hYhT58DW3AfC0QY0fm2nc7FOeYKpK6zIZS1seXYo6fw

# --- Credential encryption: DEMO HPKE X25519 pair (base64 of raw 32-byte keys). Private half is public on purpose.
# (Interim pair made with openssl; task 9 replaces it with `admin keygen` output.)
CREDENTIALS_PUBLIC_KEY=RxUrKamgbeYZkmLE3mZtzRHQP3MBBU9kZKkedBhteQk=
CREDENTIALS_PRIVATE_KEY=aOsR//GEEIuFGuddaSfV8M83+qE2+2fpEBFCqQfltX4=
CREDENTIALS_KEY_VERSION=1

# --- AWN ---
# Default: replay recorded fixtures, poller idle. See guide 05 section 4.6 before changing.
AWN_MODE=replay
POLLER_ENABLED=false
# Your OWN *development* AWN application key (separate from the production key). Only used with AWN_MODE=live.
# The placeholder makes `make live` refuse to run.
AWN_APPLICATION_KEY=REPLACE_WITH_DEV_APPLICATION_KEY

# --- Alerting: blank locally (local never pings the real dead-man's switch) ---
HEALTHCHECK_URL_AWN_INGEST=
HEALTHCHECK_URL_DAILY_ROLLUP=
HEALTHCHECK_URL_REFERENCE_SYNC=
HEALTHCHECK_URL_BACKUP=
HEALTHCHECK_URL_DBSIZE=

# --- Misc ---
RUST_LOG=info
WEB_NODE_IMAGE=node:26-bookworm-slim
```

E2E and smoke defaults are not in this file: 01's helpers default `E2E_USER_A_EMAIL=alex@example.test`, `E2E_USER_B_EMAIL=pat@example.test`, `E2E_UNINVITED_EMAIL=nobody@example.test`, `E2E_VALID_AWN_KEY=fixture-valid-key`, `E2E_INVALID_AWN_KEY=fixture-invalid-key` themselves; `smoke.sh` defaults its route to `/api/v1/me`.

### 5.6 `ops/local/init-env.sh`

```sh
#!/bin/sh
# Creates .env from .env.example. Never overwrites.
set -eu
cd "$(dirname "$0")/../.."
if [ -f .env ]; then echo ".env exists; leaving it alone"; exit 0; fi
cp .env.example .env
chmod 600 .env
echo "created .env"
```

`ops/local/check-secret-logs.sh "<compose command prefix>"` (called by every `test-*` target before teardown): runs `<prefix> logs --no-color db auth migrate` (missing services give empty output) and fails (exit 1, printing only the matched variable name, never the value) if the output contains any of: the demo plaintext passwords and `JWT_HS256_SECRET` read from `.env.example` (names listed in `backend/demo-fingerprints.txt`), `fixture-poller-key`, or the sentinel prefix `canary-secret-` that 03/04 tests plant in credentials, API keys and passwords (8.12).

### 5.7 `Makefile` (recipe lines start with a TAB)

The Makefile does **not** `include` or `export` `.env` (verifier values contain `$`, and exporting would leak every secret to host `npm`, Vite and Playwright). Compose reads `.env` itself. The two targets that need a value read just that key with `grep`.

```make
SHELL := /bin/bash
C := docker compose
T := docker compose -p famweather-test -f docker-compose.yml -f docker-compose.test.yml
ADMIN := $(C) --profile tools run --rm -T admin admin
JOB := $(C) run --rm --no-deps poller poller run-job
envval = $(shell grep -E '^$(1)=' .env 2>/dev/null | head -1 | cut -d= -f2-)
# run a test-project command, always tear the project down, keep the command's exit code
TESTRUN = $(T) build api && { $(1); rc=$$?; ./ops/local/check-secret-logs.sh "$(T)" || rc=1; $(T) down -v --remove-orphans >/dev/null 2>&1; exit $$rc; }

.PHONY: init build web-build up down logs ps migrate seed invite reset-link add-station e2e-data psql web-dev \
        poller-replay poller-idle live check-local-only test test-backend test-web test-db test-e2e smoke reset

init:            ; ./ops/local/init-env.sh
build:           ; $(C) build api
web-build:       ; $(C) --profile tools run --rm web
up: build        ; $(C) up -d --wait
down:            ; $(C) down
logs:            ; $(C) logs -f --tail=100
ps:              ; $(C) ps -a
migrate:         ; $(C) run --rm migrate
seed:            ; $(ADMIN) seed-dev
invite:          ; @test -n "$(EMAIL)" || (echo "usage: make invite EMAIL=you@example.com NAME='You' [FOLLOW_ALL=1]"; exit 1)
	$(ADMIN) invite --email "$(EMAIL)" --display-name "$(NAME)" $(if $(FOLLOW_ALL),--follow-all)
reset-link:      ; @test -n "$(EMAIL)" || (echo "usage: make reset-link EMAIL=you@example.com"; exit 1)
	$(ADMIN) reset-link --email "$(EMAIL)"
add-station:     ; $(C) --profile tools run --rm admin admin add-station $(ARGS)
e2e-data:        ; ./ops/local/e2e-data.sh
psql:            ; $(C) exec db psql -U postgres -d postgres
web-dev:
	@d="$$(./ops/local/web-node.sh)" && cd web && PATH="$${d:+$$d:}$$PATH" npm ci && PATH="$${d:+$$d:}$$PATH" PUBLIC_SUPABASE_URL=http://localhost:5173 PUBLIC_SUPABASE_ANON_KEY='$(call envval,PUBLIC_SUPABASE_ANON_KEY)' npm run dev
poller-replay:   ; POLLER_ENABLED=true AWN_MODE=replay $(C) up -d --no-deps --force-recreate poller
poller-idle:     ; POLLER_ENABLED=false $(C) up -d --no-deps --force-recreate poller
live:            ; @test "$(call envval,AWN_APPLICATION_KEY)" != "REPLACE_WITH_DEV_APPLICATION_KEY" || (echo "set your DEV AWN application key in .env"; exit 1)
	@echo "LIVE AWN: use only your own station key. Ctrl-C now to abort." && sleep 5
	AWN_MODE=live POLLER_ENABLED=true $(C) up -d --no-deps --force-recreate api poller
check-local-only: ; ! grep -rEi 'supabase\.co|fly\.dev|netlify\.app|amazonaws\.com|healthchecks\.io|hc-ping' .env.example docker-compose.yml docker-compose.test.yml ops/caddy ops/local ops/db/init
test-db: init
	$(call TESTRUN,$(T) up -d --wait db && $(T) exec -T -u postgres -e MIGRATIONS_DIR=/work/backend/migrations -e OPS_DIR=/work/ops/db db sh /work/ops/db/tests/run.sh)
test-backend: init
	$(call TESTRUN,$(T) run --rm test-backend)
test-web: init
	@if [ -f web/package.json ]; then \
	{ $(T) run --rm web sh -c "mkdir -p /work/web && cd /src/web && tar cf - --exclude=./node_modules --exclude=./build --exclude=./.svelte-kit --exclude=./test-results --exclude=./playwright-report --exclude=./tests/.auth . | tar xf - -C /work/web && cd /work/web && npm ci && npm run lint && npm run check && npm run test && npm run test:tz && npm run build && node scripts/check-build.mjs && npm run test:bundle && npm run headers:check"; rc=$$?; ./ops/local/check-secret-logs.sh "$(T)" || rc=1; $(T) down -v --remove-orphans >/dev/null 2>&1; exit $$rc; }; \
	else echo "test-web: web/package.json absent, skipping"; fi
test:            ; $(MAKE) check-local-only test-db test-backend test-web
test-e2e:        ; @test "$(CONFIRM)" = 1 || (echo "test-e2e RESETS the working stack (make reset). Re-run: make test-e2e CONFIRM=1"; exit 1)
	$(MAKE) reset up web-build seed e2e-data
	@d="$$(./ops/local/web-node.sh)" && cd web && export PATH="$${d:+$$d:}$$PATH" && echo "e2e on node $$(node -v)" && npm ci && npx playwright install chromium && E2E_BASE_URL=http://localhost:8000 npm run test:e2e
smoke:           ; ./ops/local/smoke.sh $(SMOKE_ARGS)
reset:           ; $(C) --profile tools down -v --remove-orphans
```

Notes: `test-*` targets depend on `init` (the `.env` mask needs the file), run in project `famweather-test` (5.1b), build the image, start only what they need (`test-db` starts `db`; `test-backend` and `test-web` pull their `depends_on`), run `ops/local/check-secret-logs.sh` on the project's logs, and tear the project down with volumes even on failure (the script runs before teardown; a hit fails the target). `test-db` runs `run.sh` as `postgres` over the unix socket (`--auth-local=trust`) with `MIGRATIONS_DIR` and `OPS_DIR` set explicitly; the script's per-role login checks use `-h 127.0.0.1` so SCRAM is exercised. `test-web` copies `web/` into a tmpfs inside the container and builds there, so the host's `web/build` is never touched or made root-owned (it needs no `build api`, so it does not use the `TESTRUN` macro; it runs the same secret-log check and teardown inline). The two host steps (`web-dev`, `test-e2e`) need the Node major pinned in `web/.nvmrc` (`engines` is `>=26 <27` with `engine-strict`): `ops/local/web-node.sh` uses the current `node` if it matches, else nvm, fnm or volta, else exits with install instructions (nothing is downloaded automatically: no third-party install scripts). Playwright is not run in a container because the suite must reach the stack as `localhost:8000` (the pinned auth issuer and build-time URLs) and its helpers call the host's `docker compose`. `check-local-only` covers only files this guide owns. `make test` is exactly CI's list. `test-e2e` is the one target that touches the working stack, because Playwright needs `localhost:8000`; it therefore requires `CONFIRM=1`, resets, rebuilds and reseeds. `smoke` needs no reset (default mode).

### 5.8 `ops/local/smoke.sh` (specification; bash, `set -euo pipefail`, `curl`, `jq`)

```
Usage: ./ops/local/smoke.sh [--clean] [--replay] [--keep]
  default   NON-DESTRUCTIVE: never deletes volumes or touches .env. Starts the stack if not running
            (`make up`), builds the web app if web/build/200.html is missing (`make web-build`),
            uses a throwaway user, leaves the stack running.
  --clean   Destructive: copies .env to .local/env.<timestamp>.bak (if present), then copies .env.example
            over .env (so Compose can interpolate during teardown), then `make reset`, then continues.
            Prints what it will delete and waits 5 s.
  --replay  Also runs `make seed` and `make e2e-data` and asserts replay data exists.
  --keep    With --clean: do not tear down at the end (default with --clean is `make reset` at the end).
Env: BASE=http://localhost:8000  SMOKE_AUTHED_ROUTE (default /api/v1/me)
Output: "PASS <name>" or "FAIL <name>: <reason>"; exit non-zero on first failure.
```

The throwaway user is display name `Smoke-<epoch>`, email `smoke-<epoch>@example.test`, created **without** `--follow-all` (it must not see family data). The script generates a random 20-character password for it at run time (never printed, never committed). Without `--clean`, an existing `.env` that differs from `.env.example` is used as is and never rewritten; checks that need effective values read them from the running containers.

Checks, in order:

1. `preflight`: `docker compose version` (>= 2.24 for the test overlay), `jq`, `curl`, `openssl`, `shasum`. With `--clean`: do the destructive reset here, before any port check. Then, if our stack is not running (`docker compose ps -q caddy` empty), ports 8000, 5432 must be free.
2. `up`: `[ -f web/build/200.html ] || make web-build`; `make up`; every long-running service healthy; `auth-access` and `migrate` exited 0 (`docker inspect -f '{{.State.ExitCode}}' "$(docker compose ps -a -q migrate)"`, same for `auth-access`).
3. `fingerprints`: for every line `<sha256>  <NAME>` of `backend/demo-fingerprints.txt`, the SHA-256 of `NAME`'s value in `.env.example` equals column 1 (this guide's single source).
4. `health`: `GET $BASE/healthz` 200; `GET $BASE/api/health` 200 with a body with no data fields; `GET $BASE/auth/v1/health` 200 and `.name == "GoTrue"`; `GET $BASE/` 200 `text/html`, byte-equal to `web/build/200.html`; `GET $BASE/some/deep/route` same body; `GET $BASE/api/nope` is JSON (401 or 404), not HTML; `GET $BASE/_app/immutable/<first *.js under web/build/_app/immutable/>` 200; `GET $BASE/_app/immutable/missing.js` 404 (not the fallback).
5. `headers`: on `GET $BASE/` every header in `web/security-headers.json` (local variant) is present with the exact value; no `Strict-Transport-Security`, no `Server`; `Cache-Control: no-cache` on `/`, immutable on the asset; no `unsafe-eval` and no `default-src` in CSP; the only `unsafe-*` is the sanctioned `style-src-attr 'unsafe-hashes'`. (Parity with `netlify.toml` is 01's `headers.mjs check`.)
6. `signup_disabled` (S4): `POST $BASE/auth/v1/signup` `{"email":"x-<epoch>@example.test","password":"x12345678"}` is 4xx; `POST /auth/v1/otp` `{"email":"stranger-<epoch>@example.test","create_user":true}` is 4xx; `admin list-users` shows no `auth_only` line for either address (no user was created).
7. `invite_prints_link`: `make invite EMAIL=smoke-<epoch>@example.test NAME=Smoke-<epoch>` exits 0 and its stdout contains exactly one line starting `http://localhost:8000/set-password#token_hash=` (keep it in a shell variable; never echo it); `admin list-users` output (04 section 10.2; split on TAB) has a line whose field 2 (`display_name`) is `Smoke-<epoch>`, field 3 (`email`) is the address, field 4 is `active` and field 6 is `auth_ok` (ignore other lines and the exit code, which reflects other users). Before the link is used, `POST $BASE/auth/v1/token?grant_type=password` for that address with any password is 4xx (no password exists yet).
8. `set_password_login` (nothing is emailed; no mail check exists): take `token_hash` from the fragment; `POST $BASE/auth/v1/verify` `{"type":"recovery","token_hash":"<hash>"}` returns 200 with an `access_token`; `PUT $BASE/auth/v1/user` `{"password":"<random 20 chars>"}` with that token is 200; `POST $BASE/auth/v1/token?grant_type=password` with the address and the new password returns 200 and an `access_token` (keep it for step 9), and with a wrong password and with an unknown address both return the same 400 `invalid_credentials` body. Report (do not assert) whether a second `verify` with the same `token_hash` is rejected. Then `make reset-link EMAIL=<address>` prints a new link and the old refresh token no longer works; redeeming it and `PUT /user` with another password **without** the old one succeeds (proves the require-current-password and reauthentication switches are off locally).
9. `authed_api` (S1): `GET $BASE$SMOKE_AUTHED_ROUTE` with the token 200; no header 401; `Bearer $PUBLIC_SUPABASE_ANON_KEY` 401; tampered token (last char changed) 401; `alg:none` token 401.
10. `poller_idle` (D6): container running and healthy; `docker inspect --format '{{json .Config.Env}}' "$(docker compose ps -q poller)"` contains `POLLER_ENABLED=false` and `AWN_MODE=replay`; `docker compose logs poller` contains the exact line `poller disabled: POLLER_ENABLED=false`; after `sleep 20`, `select count(*) from app.job_runs where started_at > '<script start>'` is 0, queried with `docker compose exec -T -u postgres db psql -Atc`. With `--clean` also: total `app.job_runs` and `app.observations` are 0. (Skipped with a notice if the developer has enabled the poller via `make poller-replay`/`live`; the env check then reports the override.)
11. `env_isolation` (S5): `docker inspect` Config.Env (container ids from `docker compose ps -a -q <service>`, so the exited `migrate` is found too) of `api` has no `CREDENTIALS_PRIVATE_KEY` or `SUPABASE_SERVICE_ROLE_KEY`; `poller` has no `SUPABASE_SERVICE_ROLE_KEY`; `migrate` has only `APP_ENV`, `DATABASE_URL`, `RUST_LOG` from the app set; `db` env has no `*_PASSWORD` for app roles (only `*_VERIFIER`, `POSTGRES_PASSWORD`); every `DATABASE_URL` host is `db`.
12. `refusal_rules`: each `docker compose run --rm --no-deps -e ...` exits non-zero within 10 s with a message naming the setting. (a) `DATABASE_URL=postgres://app_api:x@prod.example.com:5432/postgres` on `api`. (b) `AWN_MODE=live` with the placeholder key on `poller`. (c) The demo refusal (D2), tested so that only one value is a demo value: `docker run --rm --env-file <temp file> -v <empty temp file>:/etc/ssl/app-certs/supabase-ca.crt:ro weather-backend:local api`, where the temp env file holds only these production variables: `APP_ENV=production`, `DATABASE_URL=postgres://app_api:<random non-demo password>@db.example.invalid:5432/postgres?sslmode=verify-full&sslrootcert=/etc/ssl/app-certs/supabase-ca.crt`, `SUPABASE_URL=https://example.invalid`, `SUPABASE_JWKS_URL=https://example.invalid/auth/v1/.well-known/jwks.json`, `AWN_MODE=live`, `AWN_APPLICATION_KEY=prod-looking-key-0123456789abcdef`, `CREDENTIALS_KEY_VERSION=1`, and `CREDENTIALS_PUBLIC_KEY=<the demo public key>` (no `JWT_HS256_SECRET`, no demo role password, so no other demo rule can fire; the empty mounted file satisfies the CA-file-exists rule). The combined output must contain exactly `refusing CREDENTIALS_PUBLIC_KEY: demo value`. The temp files are deleted afterwards. Per-variable proof for every fingerprinted value is 03's unit tests; this step only proves the wiring (file embedded in the image, check active in the real binary).
13. `secret_hygiene` (S7): `docker compose logs api poller auth` contains none of `$JWT_HS256_SECRET`, `$AWN_APPLICATION_KEY`, `fixture-poller-key`.
14. `replay` (only `--replay`): `make seed`, `make e2e-data` exit 0.
15. `teardown`: only with `--clean` and without `--keep`: `make reset`. Default mode leaves everything running and removes nothing.

### 5.9 `backend/demo-fingerprints.txt`

Inside the backend build context (conventions, Fixtures). Each line is `<sha256-hex>  <VARIABLE_NAME>`; the hash is of the exact value in `.env.example`. `.env.example` verifiers are not listed (they are derived from listed passwords). Content:

```
85a243959ded2ed75fa51989d3d77b1bbdce283f34e29c1e75dd50010c1ad979  JWT_HS256_SECRET
07f8532e612ea1fa00c624839d6db2555a42aea4a940c11eb6f0c70d4a82edd4  PUBLIC_SUPABASE_ANON_KEY
d94c798d484fde87ac3ed4df77e7ec07a672f36e962aca80d7dbf326af7d2bff  SUPABASE_SERVICE_ROLE_KEY
8e115672558ad6986ef6d44c08358441284ddbbcc338dd3fbff817c85aad6a12  CREDENTIALS_PUBLIC_KEY
117a9b33bf15d25829a77be7f2a67c6168f0fc40cc4f87dc62337245b85f415c  CREDENTIALS_PRIVATE_KEY
b7e3d630c3f7a6ba375e8ec7067f35ecb967ca18e502fd6c0fecfe52fa5dba13  APP_MIGRATOR_PASSWORD
5cd7ccd120834b12f2f957d8226e79e9e1102b4349765badece8fc7f4fc543c4  APP_API_PASSWORD
098c3db0b7cd89ad9cecebad36b356aea0cdbd4e536c57bb6576857c8ddd65ef  APP_POLLER_PASSWORD
6aca9830cbd72c96730dece55e1f6841bfee06160fb7f4c16be7c5aa887d4715  APP_BACKUP_PASSWORD
fde25bb766cb10389759c0780abf7ff67cf6326f8f2200b9d80099790aa7c913  APP_CI_CHECK_PASSWORD
5fb361f5a734d80541701758f8bb7219853dbd2a6723d8e6bb9319cf88ace70e  SUPABASE_AUTH_ADMIN_PASSWORD
17bde48cc129ebea5dfcb2b38269b9b413d19d9b3734ecaad451977ec7750092  POSTGRES_PASSWORD
739ae2208fa69f439d435b5d8dda613946199b633d4f688483e5a28d2c21ea0e  LOCAL_DEMO_PASSWORD
```

Whenever a demo value changes (task 9), regenerate the affected line with `printf '%s' "$VALUE" | shasum -a 256`.

## 6. Implementation tasks (ordered)

1. [ ] `make init` (creates `.env`) and write the files in section 5: `docker-compose.yml`, `docker-compose.test.yml`, `.env.example`, `Makefile`, `ops/caddy/Caddyfile`, `ops/db/init/01-auth-prereqs.sh`, `10-bootstrap.sh`, `ops/local/{init-env.sh,e2e-data.sh,check-secret-logs.sh,smoke.sh}`, `backend/demo-fingerprints.txt` (`ops/caddy/security-headers.caddy` comes from 01); `chmod +x` all `.sh`. **Done when** `docker compose --profile tools config -q` and `docker compose -p famweather-test -f docker-compose.yml -f docker-compose.test.yml config -q` exit 0, `grep -cP '^\t' Makefile` is non-zero, and `make -n test-db` prints the `famweather-test` commands.
2. [ ] Ask the repository owner to add `.env`, `.local/`, `web/build/`, `web/node_modules/`, `web/tests/.auth/`, `web/test-results/`, `web/playwright-report/` to `.gitignore` (this guide may not edit it). **Done when** `git check-ignore` lists all seven paths.
3. [ ] Bring up infrastructure: `docker compose up -d --wait db auth`, then `docker compose run --rm auth-access`. **Done when** the two services are healthy; `\dt auth.*` lists `users` and `sessions`; `\du` shows the six roles from `01-auth-prereqs.sh` plus the `app_*` roles from `roles.sql`; a TCP login as `app_api` with the demo password succeeds (SCRAM) and one with a wrong password fails; `auth-access` exits 0 and a second run also exits 0 (needs 04's SQL). If `auth` crashes citing a missing role, create it in `01-auth-prereqs.sh` and record it in 8.1.
4. [ ] `make build && make migrate` (starts `auth-access` first). **Done when** exit 0, schema `app` has tables, `select count(*) from information_schema.tables where table_schema='public'` is 0 and `app._sqlx_migrations` exists.
5. [ ] `make up` then `make web-build`, then `curl -sI localhost:8000/` and `curl -s localhost:8000/healthz`. **Done when** `/` is 200 with the headers from `web/security-headers.json`, `/healthz` is `ok`, and `GET /` equals `web/build/200.html`.
6. [ ] `make seed` (seed users get the `LOCAL_DEMO_PASSWORD`), then `make invite EMAIL=<you> NAME=<you> FOLLOW_ALL=1`. **Done when** the command prints one link starting exactly `http://localhost:8000/set-password#token_hash=`; opening it, choosing a password and signing in lands you on the dashboard; and `GET /api/v1/me` through Caddy returns 200. Nothing is emailed and there is no mail UI.
7. [ ] Implement `ops/local/e2e-data.sh` (4.9). **Done when** `make e2e-data` exits 0 on a seeded stack and re-running it exits 0; deleting one station's rollups makes it fail naming that station.
8. [ ] Implement `ops/local/smoke.sh` per 5.8. **Done when** `make smoke` (default mode) passes twice in a row against a running stack without deleting data; `make smoke SMOKE_ARGS=--clean` passes from a clean state and the previous `.env` is saved under `.local/`; a deliberately broken setting (`GOTRUE_DISABLE_SIGNUP=false`) fails at `signup_disabled`.
9. [ ] Demo keys. Until `admin keygen` exists, the interim pair must satisfy: both values decode to 32 bytes (`printf %s "$K" | base64 -d | wc -c` prints 32) and the public key equals the one derived from the private key: `(printf '\x30\x2e\x02\x01\x00\x30\x05\x06\x03\x2b\x65\x6e\x04\x22\x04\x20'; printf %s "$PRIV" | base64 -d) | openssl pkey -inform DER -pubout -outform DER | tail -c 32 | base64` equals `$PUB`; and smoke step 3 passes. Once `admin keygen` exists: regenerate the pair, update `.env.example`, verify with `admin keygen --check`, update the two lines in `backend/demo-fingerprints.txt`, and coordinate with 03/04/01/06 so their fingerprint checks hold the new values. **Done when** a production-mode API start refuses each demo value (per-variable unit tests in 03) and smoke step 3 passes.
10. [ ] `make poller-replay`, check observations appear, then `make poller-idle`. **Done when** rows arrive from fixtures with replay and no new `job_runs` appear after idle (needs 03).
11. [ ] Verify reset and the test project: `make reset && make up && make seed`, then `make test` while the working stack is up. **Done when** first-run succeeds from zero and `make test` passes without touching the working stack's data or ports (`docker volume ls` shows no `famweather-test_*` volumes afterwards).
12. [ ] Apple Silicon: repeat tasks 3-8 on an arm64 Mac without `platform:`. **Done when** `docker image inspect` for every image reports `arm64` and no emulation warning appears.

## 7. Testing

How to run: `make test` (= `check-local-only test-db test-backend test-web`; CI runs exactly this; every `test-*` target self-starts in project `famweather-test`, builds the image, tears down afterwards and never touches the working stack), `make smoke`, `make test-e2e CONFIRM=1`.

| Test | Where | Asserts |
| --- | --- | --- |
| compose config | CI | both `config -q` commands of task 1 succeed |
| Local-only strings | `make check-local-only` | no `supabase.co`, `fly.dev`, `netlify.app`, `amazonaws.com`, `healthchecks.io` in files this guide owns |
| Ports | smoke 1/2 | published ports are exactly 127.0.0.1:{8000,5432} |
| Demo fingerprints (D2) | smoke 3 | `.env.example` values hash to `backend/demo-fingerprints.txt` |
| Headers (S15) | smoke 5 | exact values from `web/security-headers.json`, cache rules, no `Server` |
| Signup disabled (S4) | smoke 6 | signup and OTP creation rejected, no user created |
| Invite prints a link | smoke 7 | user exists with no password, one set-password link printed |
| Set-password login | smoke 8 | link redeems (`verify`), password set, password sign-in works, wrong password and unknown email look the same, reset needs no old password |
| Token strictness (S1) | smoke 9 | anon, none, tampered, `alg:none` all 401 (full matrix in 02) |
| Poller idle (D6) | smoke 10 | healthy, env disabled/replay, no new `job_runs` |
| Env isolation (S5) | smoke 11 | `docker inspect` env, not `exec` |
| Startup refusal | smoke 12 | non-local DB host, live+placeholder, production+demo secret |
| Secret hygiene (S7) | smoke 13 | logs clean |
| Grants / `can_view` | `make test-db` | 04's `run.sh` as `postgres` inside the test project's `db` (repo at `/work`, tmpfs data, SCRAM over `-h 127.0.0.1`) |
| Rust workspace | `make test-backend` | after `migrate`; env per conventions (5.1b), `.env` masked at `/work/.env`; `cargo test` with `api/test-support`, admin `compose-tests` serialized; `db` logs grepped before teardown |
| Web | `make test-web` | lint, check, unit, `test:tz`, build, bundle-secrets, `check-build.mjs`, `headers:check` (with `ops/caddy` visible); built in tmpfs, never in host `web/build` |

### 7.2 Web e2e support (01 section 6.3)

`make test-e2e CONFIRM=1` chains `reset up web-build seed e2e-data` and then runs host `npm ci`, `npx playwright install chromium` and `npm run test:e2e` with `E2E_BASE_URL` (the seed users sign in with `LOCAL_DEMO_PASSWORD`, exposed to the specs as `E2E_DEMO_PASSWORD`; set-password specs use `make`-less `docker compose ... admin invite`/`reset-link` through 01's `helpers/invite.ts`). It is destructive for the working stack (Playwright needs `localhost:8000`), hence `CONFIRM=1`. The seed users and defaults come from 04 section 12.1 and 01's helpers (`alex@`, `pat@`, `nobody@example.test`, keys `fixture-valid-key`/`fixture-invalid-key`); nothing about them is restated in `.env.example`. 01 mocks inactive-account, invalid-credential and error states.

## 8. Open questions / assumptions

1. **GoTrue prerequisites unverified.** Whether `v2.197.0` needs `supabase_admin`/`dashboard_user`, `pgcrypto` or `uuid-ossp` must be confirmed on first boot; record the minimal set here.
2. **Set-password link behaviour (proposals/05, settled here by tests, not by Rodney).** That `generate_link` (type `recovery`) works with no SMTP configured, the real link lifetime, whether a token is single use, that a user can be created with no password, and that `PUT /user` needs neither a nonce nor the old password with the switches below off, are all asserted by smoke steps 7 and 8, 04's compose tests (`invite_link_sets_password`, `reset_link_*`) and 01's `set-password.spec.ts`. The exact GoTrue variable name for "require current password" on the pinned tag is an assumption (4.4): confirm it in task 3 and correct 4.4 and the compose file.
3. **`auth_access.sql` contract (04).** The `auth-access` service assumes the file is idempotent, runnable with `psql -f` as `postgres`, and needs no `\getenv` variables. `roles.sql` is assumed to read the five `*_VERIFIER` variables and to be re-runnable (the test project may re-run it).
4. **Header generator (01).** The Caddyfile imports 01's committed `ops/caddy/security-headers.caddy` (conventions, Web headers); until 01 delivers it, `docker compose up` fails at the Caddy mount. The local variant must omit HSTS, `upgrade-insecure-requests` and `Cache-Control`.
5. **`ops/db/tests/run.sh` (04).** Runs in the test project's `db` container with only `sh` and the image's `psql`, as `postgres`, with `MIGRATIONS_DIR` and `OPS_DIR` passed by `make test-db`; how it gets role passwords for its SCRAM checks is 04's concern and is settled there.
6. **`e2e-data.sh` column names.** The assertions use 04's table and column names for `stations`, `observations`, `daily_rollups`, `reference_daily`; confirm against 04 when writing.
7. **Seed vs fixtures.** Both must use the MACs, reference ids and stored key `fixture-poller-key` of conventions (Fixtures) and 04 section 12.1; replay must accept that key for the seeded MACs (03).
8. **Restore drill vs S12 (decided Oct 7, 2026).** 06 section 15.6's quarterly drill restores a production dump into this local compose stack on Rodney's laptop. This is an explicit, accepted exception to S12 ("avoid copying family data onto laptops"): the data (including the backed-up logins) is deleted after the drill with `docker compose down -v` plus removal of the downloaded and decrypted files. Agents still never run it; Rodney does. The drill uses the normal local stack (`docker compose up -d db auth`, `auth-access`, `migrate`) and `admin restore-auth-users` (04 10.2) against the local GoTrue; no extra Compose service is needed.
9. **Docker Hub tags** `supabase/auth:v2.197.0` comes from release pages and memory; confirm they pull and record the tested `docker compose version`.
10. **Production refusal message (03).** Smoke step 12(c) expects exactly `refusing <VARIABLE>: demo value` (here `refusing CREDENTIALS_PUBLIC_KEY: demo value`) and assumes that, with a valid production env, no other rule fires first and that the CA-file rule is satisfied by an empty mounted file.
11. **Compose version.** The test overlay needs `!reset`/`!override` (Compose >= 2.24); record the tested `docker compose version` on first pass.
12. **Secret sentinels.** `check-secret-logs.sh` greps for the prefix `canary-secret-`; 03/04 tests must use it for planted secrets (proposed). Which compose services' logs matter (currently `db auth migrate`) is open.
13. **No sqlx-cli in tests.** `test-backend` installs no tools and runs no `cargo sqlx prepare --check` (queries are runtime `sqlx::query`, 02 4.12; there is no `backend/.sqlx-cli-version`). The `cargo_tools` volume, `CARGO_INSTALL_ROOT` and `SQLX_OFFLINE` were removed (nothing wrote to them; queries are runtime project-wide). 06 still sets `CARGO_TOOLS_SRC` and `SQLX_OFFLINE` in its CI snippets; those are now no-ops and 06 should drop them.
14. **CI.** Whether CI also runs `make smoke SMOKE_ARGS=--clean` on every push (Docker-in-CI, a few minutes) or only before releases is undecided; the repository is public, so minutes are free, but the default stays **not** on every push (it is slow; run it before releases or on demand with `workflow_dispatch`).
15. **Baseline picker fixtures (Oct 7).** The connect-flow devices in `awn/devices_ok.json` sit near Loveland CO and carry `info.coords.elevation`, and `acis/stnmeta__*.json` (03 section 10) feed `ReplayAcis`, so the local stack and the e2e connect spec can suggest Northern Colorado Regional Airport (`USW00094035`) without network access. `AWN_MODE=replay` also replays ACIS (no separate switch). A reference station whose `sid` has no `StnData` fixture (for example a freshly picked baseline) gets an empty replay result: the `reference_sync` job succeeds with no rows and logs once; it is not an error and not retried (03 section 10), so the baseline shows "history not loaded" locally until a fixture exists.

## 9. Security checklist

- [ ] `.env` is git-ignored, mode 600, never printed; `.env.example` has only demo values and no production hostnames (`make check-local-only`).
- [ ] `APP_ENV` is a literal `local` in compose; every Rust service's `DATABASE_URL` has host `db`; none uses `postgres` (only `auth-access` and the test project's `test-backend` do). The `db` container receives only `*_VERIFIER` values, and TCP logins are SCRAM.
- [ ] The Makefile never sources or exports `.env`; `test-backend` masks `/work/.env`; tests run in project `famweather-test` and never touch the working stack.
- [ ] Only `127.0.0.1:8000` and `:5432` are published.
- [ ] `GOTRUE_DISABLE_SIGNUP=true`, anonymous users off, autoconfirm off; the redirect allow-list is empty and no SMTP or mailer URL settings exist (S4, proposals/05).
- [ ] `api` has no `CREDENTIALS_PRIVATE_KEY` or `SUPABASE_SERVICE_ROLE_KEY`; `poller` has no service key (S5); verified through `docker inspect`.
- [ ] Demo secrets and their fingerprints (`backend/demo-fingerprints.txt`, 3.4) are refused by production in `core::config`, `admin` and the web build check, and listed in 06's CI.
- [ ] `HEALTHCHECK_URL_*` blank; `AWN_APPLICATION_KEY` is the placeholder or the developer's own dev key; `AWN_MODE=replay` and `POLLER_ENABLED=false` unless opted in (D6).
- [ ] Caddy headers come only from the generated file; no `Server` header; no HSTS locally (S15).
- [ ] Logs from `api`, `poller`, `auth` hold no AWN key or JWT secret (S7).
- [ ] No family data on laptops: seed data is fake; restore-drill question 8.8 is decided before any production dump is restored locally (S12).
- [ ] Smoke test default mode deletes nothing; `--clean` backs up `.env` first.

## 10. References

- Supabase Auth (GoTrue): https://github.com/supabase/auth ; env reference: https://github.com/supabase/auth/blob/master/example.env ; releases: https://github.com/supabase/auth/releases ; Supabase self-hosted compose (URL paths): https://github.com/supabase/supabase/blob/master/docker/docker-compose.yml
- Caddy `reverse_proxy`, `handle_path`, `try_files`, `rewrite`, `header`: https://caddyserver.com/docs/caddyfile/directives
- Compose `depends_on` conditions, profiles, `up --wait`: https://docs.docker.com/reference/compose-file/services/#depends_on ; https://docs.docker.com/compose/how-tos/profiles/
- Official Postgres image init scripts: https://hub.docker.com/_/postgres ; proposal 03 (auth schema access): `docs/proposals/03-supabase-auth-schema-access.md`
