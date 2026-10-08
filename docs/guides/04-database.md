# 04 — Database, Roles, Encryption, Supabase Config, `admin` CLI

Binding shared names live in `docs/guides/00-conventions.md`. Decisions are cited by review ID (S5, G6, ...). This guide is the authority for table and column names; other guides were reconciled to it (03-poller's proposed column names were adopted where noted).

## 1. Purpose and scope

Owns:

- Postgres schema `app`: every table, index, constraint, function, as `sqlx` migrations in `backend/migrations/`.
- Database roles and grants (`app_migrator`, `app_api`, `app_poller`, `app_backup`, `app_ci_check`), the one-time bootstrap SQL that creates them, and the `app_auth` definer functions that are the only way app roles touch GoTrue's session data.
- `core::crypto` and `core::testdb` (shared Rust modules owned by this guide, 00-conventions).
- The credential encryption wire format (columns, HPKE parameters, AAD, rotation steps).
- Supabase project configuration that affects data exposure and login (S4, S6, S11, S16) and the SQL/shell checks that verify it.
- The `admin` CLI crate `backend/crates/admin`.
- Seed data for local dev and data-retention notes.

Does not own: HTTP routes and middleware (02-api; this guide supplies `app.can_view` and `app_auth.session_active`), poller job logic (03-poller; this guide fixes the tables it writes), Compose wiring and Caddy (05-local-dev), Fly/Netlify/S3/CI wiring (06-deploy-ops), SPA (01-web).

## 2. Decisions applied

| ID | Applied as |
| --- | --- |
| D5 | `sqlx` only, schema `app`; `_sqlx_migrations` lives in `app` (section 4.1), nothing in `public`. |
| S4, G5 | Sign-ups and anonymous sign-ins off (section 8); `app.users.is_active` is the allow-list; `admin invite` is the only way to create users. |
| S5, S12 | Separate roles, column-level grants. The API **cannot read stored keys**. It can insert new credential rows and change `status`, so a compromised API could substitute a key for a user or flip statuses; that residual risk is accepted and is detectable through `audit_log` and the retained `replaced` rows (section 5.3). |
| S6, G3 | `app` is not an exposed schema; Data API disabled; CI check `ops/db/checks/exposure.sql` (section 8.4). |
| S8, G6 | HPKE format and rotation (section 9); one credential per owner. |
| S10 | Station and forecast-location coordinates stored rounded by column type (section 5.5). |
| S2, S11 | `is_active` checked per request; `app_auth.session_active` (definer function; proposals/03, accepted); `deactivate-user` revokes sessions via `app_auth.revoke_sessions` (sections 6.5, 10). |
| S16 | `audit_log`, deletion rules (section 5.12), dashboard MFA (section 8.5). |
| G4 | Single admin enforced by a partial unique index. |
| G7 | `forecast_locations` one-to-one with stations; access follows `can_view`. |
| F1 | `forecast_snapshots` stores 6-hourly aggregates, idempotent by key. |
| F3, F5, F6 | Observations carry raw AWN counters; rollups are in local standard time with a reconciliation status. |
| P3, P4 | `job_runs` table; no table-level lock state (advisory locks only). |
| C2, C4 | Size query and thresholds (section 8.6). |
| P5 | Full follow model kept; `can_view` is the single access function. |

## 3. Cross-component contracts

### 3.1 Files this guide creates

| Path | Purpose |
| --- | --- |
| `backend/migrations/NNNN_*.up.sql` / `.down.sql` | Reversible sqlx migrations, sequential numbering (`sqlx migrate add -r --sequential <name>`). Applied by `admin migrate`; `sqlx-cli` is a dev/test tool only (revert, `cargo sqlx prepare` for `backend/.sqlx/`). |
| `ops/db/bootstrap/roles.sql` | Creates roles and schema `app`. Run once by an operator (prod) or by 05's init script (local). Idempotent. Reads SCRAM verifiers from env, never plaintext passwords. |
| `ops/db/bootstrap/scram.py` | Computes a SCRAM-SHA-256 verifier from a password on stdin. Used by the production operator; local verifiers are precomputed and committed in `.env.example` (05's `init-env.sh` copies them and does not run `scram.py`). |
| `backend/.sqlx-cli-version` | Owned here: one line with the pinned sqlx-cli version (equal to the `sqlx` crate version in `Cargo.lock`; update both together). Used by every `cargo install sqlx-cli` (4.1). |
| `ops/db/bootstrap/auth_access.sql` | Creates schema `app_auth` and its two definer functions. Plain SQL (no `\getenv`, no meta-commands), runnable with `psql -f` as `postgres` after GoTrue has migrated `auth`. Idempotent. |
| `ops/db/checks/exposure.sql`, `exposure.sh` | CI/ops check (section 8.4). |
| `ops/db/checks/size.sql` | Size monitoring (section 8.6). |
| `ops/db/checks/supabase_smoke.sh` | Black-box check of hosted auth/Data API settings. |
| `ops/db/tests/` | SQL-level tests, `auth_stub.sql`, runner `run.sh` (section 11). |
| `ops/db/seed/seed.toml` | Data read by `admin seed-dev` (section 12). No SQL files and no `psql` needed. |
| `backend/crates/admin/` | CLI. Uses `core` for config, crypto and the Supabase admin client. |
| `backend/crates/core/src/{crypto,testdb}.rs` | Owned here (section 9.3, 11.6). |

Layout is as in 00-conventions. `ops/db/init/` (05) runs `roles.sql`. `auth_access.sql` runs as `postgres`: locally by 05's `auth-access` one-shot (a `postgres:17` container with `psql`) after `auth` is healthy and before `admin migrate`; in production once by the operator after GoTrue has created `auth.*`. The same file is used in both places. The `backend` image has no `psql`, which is why `admin` never shells out to it. Seed data is not baked into the image: 05 mounts `./ops/db/seed:/seed:ro` into the `admin` service and `admin seed-dev` reads `SEED_DIR` (default `/seed`).

### 3.2 Tables other guides read or write

Exact column names are in section 5. Summary of who touches what:

| Table | `app_api` | `app_poller` | `admin` (as `app_migrator`) |
| --- | --- | --- | --- |
| `users` | SELECT | SELECT (id, is_active) | all |
| `follows` | SELECT, INSERT, UPDATE(status, decided_at) | none | all |
| `credentials` | SELECT non-secret columns; INSERT; UPDATE(status, status_changed_at) only | SELECT all; UPDATE status, last_ok_at, ciphertext columns, key_version | all |
| `stations`, `forecast_locations` | SELECT, INSERT; UPDATE(name, credential_id) on stations | SELECT; UPDATE(last_observed_at, backfill_floor_ts) on stations | all |
| `observations`, `daily_rollups` | SELECT | SELECT, INSERT, UPDATE | all |
| `rollup_dirty_days` | none | SELECT, INSERT, DELETE, UPDATE(local_date) | all |
| `reference_stations` | SELECT | SELECT; UPDATE(last_synced_date) | all |
| `reference_daily` | SELECT | SELECT, INSERT, UPDATE | all |
| `forecast_runs`, `forecast_snapshots` | SELECT | SELECT, INSERT | all |
| `job_runs` | SELECT | SELECT, INSERT, UPDATE, DELETE | all |
| `audit_log` | INSERT | INSERT | all |
| `connect_attempts` | SELECT, INSERT, DELETE | none | all |

Full per-column grants are in the migrations; they are authoritative.

### 3.3 Environment variables

Existing (conventions): `DATABASE_URL` (role-specific; for `admin` it is the **app_migrator** URL), `SUPABASE_URL`, `SUPABASE_SERVICE_ROLE_KEY` (admin only), `CREDENTIALS_PUBLIC_KEY`, `CREDENTIALS_PRIVATE_KEY`, `APP_ENV`.

Variables introduced by this guide (all now in 00-conventions):

| Variable | Used by | Meaning |
| --- | --- | --- |
| `CREDENTIALS_KEY_VERSION` | api, poller, admin | Integer >= 1: version stamped on new ciphertexts and matching `CREDENTIALS_PUBLIC_KEY` / `CREDENTIALS_PRIVATE_KEY`. |
| `CREDENTIALS_PRIVATE_KEY_PREVIOUS`, `CREDENTIALS_KEY_VERSION_PREVIOUS` | poller | Only during rotation (section 9.5). |
| `APP_MIGRATOR_PASSWORD_VERIFIER`, `APP_API_PASSWORD_VERIFIER`, `APP_POLLER_PASSWORD_VERIFIER`, `APP_BACKUP_PASSWORD_VERIFIER`, `APP_CI_CHECK_PASSWORD_VERIFIER` | `bootstrap/roles.sql` (read by `\getenv`, never argv) | SCRAM-SHA-256 verifiers computed client-side by `scram.py` from the role passwords (which only ever appear in the matching `DATABASE_URL`). FINAL (00-conventions). Local: `.env.example` commits demo passwords and their precomputed verifiers. Plaintext is never sent as SQL text in production. |
| `AWN_APPLICATION_KEY` | also `admin add-station` (optional) | If set (and `AWN_MODE=live`), `add-station` validates the key against AWN. `seed-dev` never calls AWN. |
| `SEED_DIR` | `admin seed-dev` | Default `/seed`. |

### 3.4 Functions exposed to 02-api

`app.can_view(user_id uuid, station_id uuid) -> boolean`, `app.visible_station_ids(user_id uuid) -> setof uuid`, `app.lst_date(ts timestamptz, std_offset_minutes int) -> date` (for 03), `app_auth.session_active(session_id uuid, user_id uuid) -> boolean` (EXECUTE for `app_api` only; the API's per-request session check and startup self-test; returns `false`, never an error, for unknown or NULL ids), `app_auth.revoke_sessions(user_id uuid) -> integer` (EXECUTE for `app_migrator` only). Table `app.rate_gates` (5.9) backs `core::ratelimit`. Usage in section 7.

### 3.5 Crypto contract for 02/03

Section 9. `core::crypto` is owned by this guide; signatures in 9.3 are final for 02, 03 and `admin`. `decrypt_credential` exists only behind cargo feature `decrypt`.

### 3.6 Names 03-poller must use (final, authoritative)

- `credentials`: statuses `ok|invalid|rate_limited|replaced`; poller uses rows reached through `stations.credential_id` with status `ok` or `rate_limited`; UPDATE columns `status, status_changed_at, last_ok_at, api_key_ciphertext, encapsulated_key, key_version`.
- `stations`: `is_enabled` (read only), `mac`, `credential_id`, `timezone`, `std_utc_offset_minutes`, `reference_station_id` (read only), `last_observed_at` and `backfill_floor_ts` (poller UPDATE; floor is the resume cursor).
- `observations`: exactly the columns in 5.6 (`humidity_pct`, `wind_dir_deg`, `baro_rel_inhg`, `baro_abs_inhg`, `rain_rate_in_hr`, `max_daily_gust_mph`, `rain_last_at`, `temp_indoor_f`, `humidity_indoor_pct`, `rain_inc_in`, ...). Ingest never writes `rain_inc_in`; the rollup does.
- `daily_rollups`: `station_id, local_date, rain_in, tmax_f, tmin_f, gdd, gdd_method, obs_count, is_complete, rain_resets, rain_capped, awn_daily_rain_in, rain_reconcile_status, rain_reconcile_diff_in, computed_at`; `gdd_method` in `avg_50|clamp_50_86`; `rain_reconcile_status` in `unchecked|ok|mismatch|no_awn_total`.
- `reference_stations`: poller SELECT and UPDATE(`last_synced_date`) only; never INSERT (created by `admin add-reference-station`). `reference_daily`: `tmax_f, tmin_f, precip_in, precip_trace, gdd, gdd_method, quality_flags, fetched_at`; poller SELECT/INSERT/UPDATE.
- `forecast_runs(location_id, model, run_time, fetched_at, row_count)` and `forecast_snapshots(location_id, model, run_time, variable, valid_time, period_hours, p10, p50, p90, prob_precip)`: INSERT only, `ON CONFLICT DO NOTHING`, run row and its snapshots in one transaction; `valid_time` is the UTC window start at 00/06/12/18 on an exact hour; `variable` is exactly `'precip_6h_in'`; `prob_precip` 0 to 100.
- `job_runs(id uuid, job, subject_id, started_at, finished_at, status, detail jsonb)`: `id` is a **uuid** (03 currently says bigint identity), `status` in `running|success|failed|cancelled|abandoned`, `detail` at most 8192 bytes (CHECK).
- Status writes on credentials are guarded: `UPDATE app.credentials SET status=... WHERE id=$1 AND status IN ('ok','rate_limited')` (a `replaced` credential is never revived or marked invalid). The poller skips stations whose credential is not `ok`/`rate_limited`.
- `audit_log` actions from the poller: `credential.invalid`, `credential.reencrypted` (dotted, not `credential_invalid`); `subject_type` `credential`; `actor_id` NULL. Full list for every writer in 5.9.
- `rate_gates(name text primary key, next_allowed_at timestamptz not null)`: poller SELECT/INSERT/UPDATE (the reservation statement in 00-conventions). Gate names are `awn:app:poller`, `awn:app:api`, `awn:key:<credential_id>`.
- Numeric columns are cast `::float8` in every select and `$n::float8` for parameters.
- `app.lst_date(ts, std_offset_minutes)` is available; there is no `UPDATE` on `is_enabled`, no DELETE on any ingest table, `DELETE` only on `job_runs`.

### 3.7 Changes required in other guides

Outstanding items only (done items were removed after the round-4 read).

- **00-conventions:** subject_type list `user|follow|credential|station` and the `follow.*` subject rule (5.9); `core::testdb::pool_as`. (Resolved: `admin invite --admin/--adopt` and the `famweather-admin` secrets are in.)
- **02-api:** `core::crypto` names `EncryptedCredential`, `CredentialPublicKey`, `CredentialPrivateKey` with the argument order of 9.3; follow audit shape exactly as 5.9 (`subject_id` = follower id, `detail.followee_id`); `info` constant `family-weather/credential/v1`; reconnect `UPDATE ... WHERE owner_id=$caller AND credential_id <> $new AND mac = ANY($covered)`.
- **03-poller:** connects with a **direct** connection only (no pooler; advisory locks and the dedicated lock connection need it); implement `core::config::load(Binary::Admin)` against the `AdminEnv`/`AdminCommand` of 10.1.
- **05-local-dev:** `test-backend` sets `DATABASE_URL` (superuser) as well as `TEST_SUPERUSER_DATABASE_URL`; `run.sh` is invoked as the `postgres` OS user (it creates a private `initdb` cluster for the bootstrap tests); repo mounted at `/work` into `db`; seed users Rodney, Alex, Sam, Pat (no follows), Dana (pending/declined/revoked), Casey (inactive); `supabase_smoke.sh` hosted-only; `list-users` columns in order `id`, `display_name`, `email`, `active|inactive`, `admin|-`, `auth_ok|auth_missing`, then `auth_only<TAB>id` lines; GoTrue settings in 05 section 4.4 match 8.1.
- **06-deploy-ops:** the `app_migrator` URL lives only in `famweather-admin` and the protected CI environment; `invite` sends no email; `size.sql` run as `psql -X -v ON_ERROR_STOP=1 -f size.sql` with the exit code checked; an operator without IPv6 may run `roles.sql` and `auth_access.sql` through the session-mode pooler (user `postgres.<project-ref>`, never transaction mode).

## 4. Design: layout and conventions

### 4.1 Migration rules

- Directory `backend/migrations/`, files `0001_schema_and_helpers.up.sql` ... with matching `.down.sql`. sqlx forbids mixing reversible and simple migrations; use reversible only.
- Applied by `admin migrate` (embedded `sqlx::migrate!("../../migrations")`, needs only `APP_ENV` and `DATABASE_URL`; the role must be `app_migrator`). Always migrate and revert as `app_migrator`, never as a superuser, so object ownership is right. `ALTER ROLE app_migrator SET search_path = app` (bootstrap) makes sqlx create `app._sqlx_migrations`, keeping `public` empty. Every other role has no grant on `_sqlx_migrations` (default privileges give nothing except SELECT to `app_backup`).
- Each migration that creates a table also issues its `GRANT`s (no central grants file); so a forgotten grant is visible in review. The grant test (section 11.2) fails when a table has no entry in its expectation matrix.
- Roles and schema `app` are **not** created by migrations (they need cluster privileges). Migration 0001 asserts they exist and fails with an actionable message.
- Down migrations exist for tests only. `0001.down.sql` refuses to run unless `SET app.allow_destructive_down = 'on'` is set in the session, so a stray revert in production fails. `admin migrate` only goes forward. Down files are exercised by `ops/db/tests/run.sh`, which applies the `.up.sql`/`.down.sql` files directly with `psql` as `app_migrator` (with `PGOPTIONS='-c app.allow_destructive_down=on'`). A developer reverting through sqlx-cli (`cargo install sqlx-cli --locked --no-default-features --features rustls,postgres --version "$(cat backend/.sqlx-cli-version)"`) passes the setting in the URL: `DATABASE_URL=postgres://app_migrator:...@db/postgres?options=-c%20app.allow_destructive_down%3Don`.
- Production migrations are forward-only; never edit an applied migration.
- UUIDs: `gen_random_uuid()` (core in PG13+, no extension). Local image is `postgres:17`; Supabase hosted is PG17.
- Timestamps: `timestamptz`, UTC. `updated_at` maintained by trigger `app.touch_updated_at()`.
- Enumerations: `text` with `CHECK (... IN (...))`, not Postgres `ENUM`, to avoid sqlx type-registration and `ALTER TYPE` friction. Rust maps them to `String`/`#[derive(sqlx::Type)] #[sqlx(type_name = "text")]` enums.
- Latitude/longitude are `numeric(5,2)`; select with `lat::float8` in queries (sqlx numeric needs an extra crate otherwise).

### 4.2 Migration list (apply in this order)

| # | File stem | Contents |
| --- | --- | --- |
| 0001 | `schema_and_helpers` | role/schema assertion, default privileges, `touch_updated_at`, `lst_date` |
| 0002 | `users_follows` | `users`, `follows` |
| 0003 | `credentials` | `credentials` with column grants |
| 0004 | `reference_data` | `reference_stations`, `reference_daily` |
| 0005 | `stations` | `stations`, `forecast_locations` |
| 0006 | `observations` | `observations` |
| 0007 | `daily_rollups` | `daily_rollups`, `rollup_dirty_days` |
| 0008 | `forecasts` | `forecast_runs`, `forecast_snapshots` |
| 0009 | `ops_tables` | `job_runs`, `audit_log`, `connect_attempts`, `rate_gates` |
| 0010 | `can_view` | `can_view`, `visible_station_ids` |

## 5. Schema (the SQL)

### 5.1 `0001_schema_and_helpers.up.sql`

```sql
DO $$
DECLARE r text;
BEGIN
  FOREACH r IN ARRAY ARRAY['app_migrator','app_api','app_poller','app_backup','app_ci_check'] LOOP
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = r) THEN
      RAISE EXCEPTION 'role % missing: run ops/db/bootstrap/roles.sql first', r;
    END IF;
  END LOOP;
  IF NOT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = 'app') THEN
    RAISE EXCEPTION 'schema app missing: run ops/db/bootstrap/roles.sql first';
  END IF;
  IF current_user <> 'app_migrator' AND NOT (SELECT rolsuper FROM pg_roles WHERE rolname = current_user) THEN
    RAISE EXCEPTION 'migrations must run as app_migrator (current_user = %)', current_user;
  END IF;
END $$;

REVOKE ALL ON SCHEMA app FROM PUBLIC;
GRANT USAGE ON SCHEMA app TO app_api, app_poller, app_backup;
-- _sqlx_migrations is created by sqlx before this migration runs, so default privileges do not cover it.
DO $$ BEGIN
  IF to_regclass('app._sqlx_migrations') IS NOT NULL THEN
    GRANT SELECT ON app._sqlx_migrations TO app_backup;
  END IF;
END $$;

-- Functions default to EXECUTE for PUBLIC; this cannot be reverted with IN SCHEMA, so it is global for objects
-- created by app_migrator (which only creates objects in app).
ALTER DEFAULT PRIVILEGES FOR ROLE app_migrator REVOKE EXECUTE ON FUNCTIONS FROM PUBLIC;
-- Read-only backup role sees every future table/sequence automatically.
ALTER DEFAULT PRIVILEGES FOR ROLE app_migrator IN SCHEMA app GRANT SELECT ON TABLES TO app_backup;
ALTER DEFAULT PRIVILEGES FOR ROLE app_migrator IN SCHEMA app GRANT SELECT ON SEQUENCES TO app_backup;

CREATE FUNCTION app.touch_updated_at() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS
$$ BEGIN NEW.updated_at := now(); RETURN NEW; END $$;

-- Local-standard-time date of an instant. std_offset_minutes is the station's STANDARD offset from UTC
-- (e.g. -420 for MST), never the DST-adjusted one (F5).
CREATE FUNCTION app.lst_date(ts timestamptz, std_offset_minutes int) RETURNS date
LANGUAGE sql IMMUTABLE PARALLEL SAFE SET search_path = pg_catalog AS
$$ SELECT ((ts AT TIME ZONE 'UTC') + make_interval(mins => std_offset_minutes))::date $$;
GRANT EXECUTE ON FUNCTION app.lst_date(timestamptz, int) TO app_api, app_poller;
```

Down: drop both functions (`DROP FUNCTION app.lst_date(timestamptz,int); DROP FUNCTION app.touch_updated_at();`), `ALTER DEFAULT PRIVILEGES ... GRANT EXECUTE ON FUNCTIONS TO PUBLIC` and `REVOKE SELECT ... FROM app_backup` mirrors, `REVOKE USAGE ON SCHEMA app FROM app_api, app_poller, app_backup`. First statement of the down file:

```sql
DO $$ BEGIN
  IF coalesce(current_setting('app.allow_destructive_down', true), 'off') <> 'on' THEN
    RAISE EXCEPTION 'destructive down migration refused (set app.allow_destructive_down = on)';
  END IF;
END $$;
```

Every later `.down.sql` begins with the same guard.

### 5.2 `0002_users_follows.up.sql`

```sql
CREATE TABLE app.users (
  id            uuid PRIMARY KEY,                       -- = auth.users.id; NO foreign key (integrity is application-level)
  display_name  text NOT NULL CHECK (char_length(btrim(display_name)) BETWEEN 1 AND 80),
  is_active     boolean NOT NULL DEFAULT true,
  is_admin      boolean NOT NULL DEFAULT false,
  deactivated_at timestamptz,
  created_at    timestamptz NOT NULL DEFAULT now(),
  updated_at    timestamptz NOT NULL DEFAULT now(),
  CHECK (is_active = (deactivated_at IS NULL))
);
-- G4: at most one admin row (Rodney).
CREATE UNIQUE INDEX users_single_admin ON app.users ((true)) WHERE is_admin;
-- Display names identify people in the UI and in admin commands: unique, case- and space-insensitive.
CREATE UNIQUE INDEX users_display_name_key ON app.users (lower(btrim(display_name)));
CREATE TRIGGER users_touch BEFORE UPDATE ON app.users FOR EACH ROW EXECUTE FUNCTION app.touch_updated_at();

CREATE TABLE app.follows (
  follower_id  uuid NOT NULL REFERENCES app.users (id) ON DELETE CASCADE,
  followee_id  uuid NOT NULL REFERENCES app.users (id) ON DELETE CASCADE,
  status       text NOT NULL DEFAULT 'pending'
               CHECK (status IN ('pending','approved','declined','revoked')),
  requested_at timestamptz NOT NULL DEFAULT now(),
  decided_at   timestamptz,
  updated_at   timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (follower_id, followee_id),
  CHECK (follower_id <> followee_id),
  CHECK ((status = 'pending') = (decided_at IS NULL))
);
CREATE INDEX follows_followee_idx ON app.follows (followee_id, status);
CREATE TRIGGER follows_touch BEFORE UPDATE ON app.follows FOR EACH ROW EXECUTE FUNCTION app.touch_updated_at();

GRANT SELECT ON app.users TO app_api;
GRANT SELECT (id, is_active) ON app.users TO app_poller;
GRANT SELECT, INSERT ON app.follows TO app_api;
GRANT UPDATE (status, decided_at) ON app.follows TO app_api;
```

Direction: row `(follower_id=A, followee_id=B, approved)` means A may see B's stations. Mutual family access is two rows.

There is **no foreign key** from `app.users` to `auth.users` (00-conventions; proposals/03, accepted): `app_migrator` cannot be granted `REFERENCES` on a GoTrue-owned table without a login we cannot use on hosted Supabase. `admin invite` and `admin delete-user` are the only writers of `app.users` and keep the two in step; `admin list-users` reports rows whose auth user is missing (it asks the Admin API). A user deleted only in the Supabase dashboard cannot reach the API anyway (no sessions, token `sub` stops resolving at refresh), but its `app.users` row and data remain until `admin delete-user` is run.

Status transitions (pending to approved/declined, approved to revoked, declined/revoked back to pending) are enforced by the API, not the DB (see Open questions).

Down: `DROP TABLE app.follows; DROP TABLE app.users;`

### 5.3 `0003_credentials.up.sql`

```sql
CREATE TABLE app.credentials (
  id                 uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  owner_id           uuid NOT NULL REFERENCES app.users (id) ON DELETE CASCADE,
  api_key_ciphertext bytea NOT NULL CHECK (octet_length(api_key_ciphertext) BETWEEN 17 AND 512),
  encapsulated_key   bytea NOT NULL CHECK (octet_length(encapsulated_key) = 32),
  key_version        smallint NOT NULL CHECK (key_version >= 1),
  status             text NOT NULL DEFAULT 'ok' CHECK (status IN ('ok','invalid','rate_limited','replaced')),
  last_ok_at         timestamptz,
  status_changed_at  timestamptz NOT NULL DEFAULT now(),
  created_at         timestamptz NOT NULL DEFAULT now(),
  updated_at         timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT credentials_id_owner_key UNIQUE (id, owner_id)   -- target of stations' composite FK (5.5)
);
-- One live credential per owner (G6). Replaced rows are kept (history), so the uniqueness is partial.
CREATE UNIQUE INDEX credentials_one_live_per_owner ON app.credentials (owner_id) WHERE status <> 'replaced';
CREATE INDEX credentials_owner_idx ON app.credentials (owner_id);
CREATE INDEX credentials_key_version_idx ON app.credentials (key_version);
CREATE TRIGGER credentials_touch BEFORE UPDATE ON app.credentials FOR EACH ROW EXECUTE FUNCTION app.touch_updated_at();

-- app_api: metadata readable; ciphertext is insert-only. NO table-level SELECT, NO UPDATE of ciphertext columns.
GRANT SELECT (id, owner_id, key_version, status, last_ok_at, status_changed_at, created_at, updated_at)
  ON app.credentials TO app_api;
GRANT INSERT (id, owner_id, api_key_ciphertext, encapsulated_key, key_version) ON app.credentials TO app_api;
GRANT UPDATE (status, status_changed_at) ON app.credentials TO app_api;

-- app_poller: reads ciphertext; updates status and (for rotation) the ciphertext columns.
GRANT SELECT ON app.credentials TO app_poller;
GRANT UPDATE (status, status_changed_at, last_ok_at, api_key_ciphertext, encapsulated_key, key_version)
  ON app.credentials TO app_poller;
```

Status values (00-conventions): `ok`, `invalid`, `rate_limited`, `replaced`. Reconnecting a key never overwrites ciphertext: the API inserts a **new** row (new `id`, so a new AAD) and marks the old row `replaced`. A compromised API therefore cannot overwrite a stored key row in place and cannot read any key.

**Residual risk (S5, accepted):** a compromised API can still INSERT a new credential row (attacker-chosen key encrypted to the public key) and repoint a user's stations to it, or flip `status` values (for example marking a healthy credential `invalid` to stop ingestion). It cannot read or exfiltrate keys. Such changes are visible afterwards: the previous row survives as `replaced`, `status_changed_at` moves, and the API's audit rows (`credential.saved`, `credential.replaced`) are written in the same transaction; the poller also logs `credential.invalid` for its own changes, so an `invalid` status without a matching poller audit row is suspicious. Prevention is not claimed. The poller must ignore `replaced` rows (it reads credentials through `stations.credential_id`, and the API repoints stations in the same transaction, below). Replaced rows keep their ciphertext until an operator deletes them; `--reuse-credential` and the poller never use a row that is not `ok`/`rate_limited`; rotation (9.5) re-encrypts every row including `replaced` ones so the old private key can be retired.

One live credential per owner: partial unique index `credentials_one_live_per_owner`. This follows G6 and the design doc and is a mild product assumption (a user with two Ambient accounts cannot connect both). Flagged in Open questions.

**Column-privilege behaviour (verified against the PostgreSQL INSERT docs, https://www.postgresql.org/docs/current/sql-insert.html):**

- `RETURNING` needs SELECT on every returned column. `app_api` must never `RETURNING api_key_ciphertext`/`encapsulated_key` (and must not `RETURNING *`).
- `ON CONFLICT ... DO UPDATE` needs SELECT on the conflict-target columns, on columns used in SET expressions and WHERE, and on columns read through `EXCLUDED`, and UPDATE on the updated columns. For `app_api` it can never touch ciphertext columns (no UPDATE grant on them). `app_api` must not use `ON CONFLICT DO UPDATE` on `credentials`.
- `UPDATE ... SET status = $1 WHERE id = $2 AND owner_id = $3` needs UPDATE on `status` and SELECT only on `id`, `owner_id` (granted).
- `SELECT *` or any listing of the ciphertext columns fails with `42501 insufficient_privilege`. `core` models must use the explicit column list `CREDENTIAL_META_COLUMNS` for API queries.

**Required reconnect pattern (02-api, and `admin add-station --replace-key`)**, one transaction, in this order (the partial unique index forbids two live rows, and `stations.credential_id` is deferred-checked so the repoint can come last):
1. `SELECT id FROM app.credentials WHERE owner_id = $1 AND status <> 'replaced'` (old id, may be none).
2. If an old row exists: `UPDATE app.credentials SET status='replaced', status_changed_at=now() WHERE id=$old AND owner_id=$1`.
3. Generate `new_id = Uuid::new_v4()` in the application (needed for AAD) and `INSERT INTO app.credentials (id, owner_id, api_key_ciphertext, encapsulated_key, key_version) VALUES ($new_id,$1,$2,$3,$4) RETURNING id, status`. A unique violation (`23505`) means a concurrent connect: roll back; the API returns 409 `conflict` and does **not** retry.
4. If an old row existed: repoint only the stations the new key covers: `UPDATE app.stations SET credential_id=$new_id WHERE owner_id=$1 AND credential_id <> $new_id AND mac = ANY($covered_macs)` (needs the `credential_id` column grant in 5.5). `not_covered` is every one of the caller's stations whose `credential_id` is still not `$new_id` after that statement (their MAC is not in the new key's AWN device list); they stay on their old (`replaced`) credential and are returned to the caller as `not_covered`; the poller skips them (their credential is neither `ok` nor `rate_limited`) and the operator disables them with manual SQL (`UPDATE app.stations SET is_enabled = false WHERE ...`; no command exists in v1). `admin add-station --replace-key` (no validation in production) repoints all of the owner's stations.
5. Insert any new `stations`/`forecast_locations`, and audit rows: on reconnect **both** `credential.saved` (subject = new row) and `credential.replaced` (subject = old row); on first connect only `credential.saved`.

First connect is steps 3 and 5 only. The API reads the owner's current credential with `WHERE owner_id=$1 AND status <> 'replaced'`.

Down: `DROP TABLE app.credentials;`

### 5.4 `0004_reference_data.up.sql`

```sql
CREATE TABLE app.reference_stations (
  id                     text PRIMARY KEY CHECK (id ~ '^[A-Z0-9]{11}$'),   -- GHCN-Daily id, e.g. USW00024144
  name                   text NOT NULL,
  lat                    numeric(8,5) NOT NULL CHECK (lat BETWEEN -90 AND 90),   -- public airport, exact is fine
  lon                    numeric(8,5) NOT NULL CHECK (lon BETWEEN -180 AND 180),
  elevation_ft           integer,
  timezone               text NOT NULL,
  std_utc_offset_minutes smallint NOT NULL CHECK (std_utc_offset_minutes BETWEEN -720 AND 840),
  source                 text NOT NULL DEFAULT 'ghcnd_acis',
  last_synced_date       date,
  created_at             timestamptz NOT NULL DEFAULT now(),
  updated_at             timestamptz NOT NULL DEFAULT now()
);
CREATE TRIGGER reference_stations_touch BEFORE UPDATE ON app.reference_stations
  FOR EACH ROW EXECUTE FUNCTION app.touch_updated_at();

CREATE TABLE app.reference_daily (
  reference_station_id text NOT NULL REFERENCES app.reference_stations (id) ON DELETE CASCADE,
  date          date NOT NULL,                         -- midnight-to-midnight local standard time (F5)
  tmax_f        real,
  tmin_f        real,
  precip_in     real CHECK (precip_in IS NULL OR precip_in >= 0),
  precip_trace  boolean NOT NULL DEFAULT false,        -- ACIS "T": precip_in is NULL and this is true
  gdd           real CHECK (gdd IS NULL OR gdd >= 0),  -- written by the poller with core::gdd (same function as daily_rollups)
  gdd_method    text CHECK (gdd_method IN ('avg_50','clamp_50_86')),
  quality_flags text,                                  -- comma-joined '<m|n|p>:<flag>' entries (03's ACIS parser)
  fetched_at    timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (reference_station_id, date),
  CHECK ((gdd IS NULL) = (gdd_method IS NULL))
);

GRANT SELECT ON app.reference_stations, app.reference_daily TO app_api;
GRANT SELECT ON app.reference_stations TO app_poller;
GRANT UPDATE (last_synced_date) ON app.reference_stations TO app_poller;   -- rows are created by admin add-reference-station only
GRANT SELECT, INSERT, UPDATE ON app.reference_daily TO app_poller;
```

Final design: both `reference_daily` and `daily_rollups` store `gdd` and `gdd_method` (00-conventions); the poller computes both with `core::gdd`, and the API refuses to overlay rows whose `gdd_method` differs. `precip_in` is NULL for missing or trace values; `precip_trace` and `quality_flags` say which. `reference_station_id` is the GHCN-Daily id string. The poller can update only `reference_stations.last_synced_date`; reference stations are created by `admin add-reference-station`.

Down: drop both (daily first).

### 5.5 `0005_stations.up.sql`

Decision on coordinates (S10, **flagged as a proposal**): store coordinates **rounded to 2 decimal places (about 1.1 km)** in the database, enforced by the column type `numeric(5,2)` (PostgreSQL rounds on insert). Nothing needs more precision: Open-Meteo grids are several km and the reference-station choice is made once by a human or the connect flow. This means no exact home location is ever persisted by the app (the AWN API returns the exact coordinates to the connect flow only; the API/admin must discard them after rounding). The API still rounds again if it ever returns more precision. If the product later wants exact coordinates, add a new migration; do not widen silently.

```sql
CREATE TABLE app.stations (
  id                     uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  mac                    text NOT NULL UNIQUE CHECK (mac ~ '^([0-9A-F]{2}:){5}[0-9A-F]{2}$'),  -- upper-case, colon separated
  name                   text NOT NULL CHECK (char_length(btrim(name)) BETWEEN 1 AND 80),
  owner_id               uuid NOT NULL REFERENCES app.users (id) ON DELETE CASCADE,
  credential_id          uuid NOT NULL,                          -- composite FK below
  lat                    numeric(5,2) NOT NULL CHECK (lat BETWEEN -90 AND 90),
  lon                    numeric(5,2) NOT NULL CHECK (lon BETWEEN -180 AND 180),
  timezone               text NOT NULL,                          -- IANA name, validated by API/admin
  std_utc_offset_minutes smallint NOT NULL CHECK (std_utc_offset_minutes BETWEEN -720 AND 840),
  reference_station_id   text REFERENCES app.reference_stations (id) ON DELETE SET NULL,
  is_enabled             boolean NOT NULL DEFAULT true,          -- poller skips disabled stations
  last_observed_at       timestamptz,                            -- written by awn_ingest / awn_backfill
  backfill_floor_ts      timestamptz,                            -- earliest ts_utc AWN returned; awn_backfill resume cursor (F3)
  created_at             timestamptz NOT NULL DEFAULT now(),
  updated_at             timestamptz NOT NULL DEFAULT now(),
  -- A station can only use a credential owned by the station's owner (also holds after repointing).
  FOREIGN KEY (credential_id, owner_id) REFERENCES app.credentials (id, owner_id) DEFERRABLE INITIALLY DEFERRED
);
CREATE INDEX stations_owner_idx ON app.stations (owner_id);
CREATE INDEX stations_credential_idx ON app.stations (credential_id);
CREATE INDEX stations_reference_idx ON app.stations (reference_station_id);
CREATE TRIGGER stations_touch BEFORE UPDATE ON app.stations FOR EACH ROW EXECUTE FUNCTION app.touch_updated_at();

CREATE TABLE app.forecast_locations (
  id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  station_id uuid NOT NULL UNIQUE REFERENCES app.stations (id) ON DELETE CASCADE,
  lat        numeric(5,2) NOT NULL CHECK (lat BETWEEN -90 AND 90),
  lon        numeric(5,2) NOT NULL CHECK (lon BETWEEN -180 AND 180),
  created_at timestamptz NOT NULL DEFAULT now()
);

GRANT SELECT, INSERT ON app.stations, app.forecast_locations TO app_api;
GRANT UPDATE (name, credential_id) ON app.stations TO app_api;   -- credential_id: repoint after reconnect (5.3)
GRANT SELECT ON app.stations, app.forecast_locations TO app_poller;
GRANT UPDATE (last_observed_at, backfill_floor_ts) ON app.stations TO app_poller;
```

`backfill_floor_ts` is a column rather than `job_runs.detail` so the resume cursor is a plain indexed-free lookup that survives `job_runs` pruning (90 days) and needs no JSON parsing. `is_enabled` is the per-station off switch (a deactivated owner's stations can be disabled without deleting data); only the admin/operator sets it, so the poller has no UPDATE on it.

The composite FK `(credential_id, owner_id) -> credentials(id, owner_id)` stops a (compromised or buggy) API from pointing a station at someone else's credential; `owner_id` cannot be updated by any app role. It is `DEFERRABLE INITIALLY DEFERRED` with default NO ACTION, because: deleting a user cascades to both `credentials` and `stations`; an immediate RESTRICT/NO ACTION check could fire after the credential row is deleted but before the station rows are, depending on trigger order, and abort the whole account deletion. Deferred NO ACTION is evaluated at commit, when both are gone. A standalone `DELETE FROM app.credentials` while stations still reference it still fails at commit (disconnecting a key never deletes station history). A `replaced` credential may still be referenced by `not_covered` stations; an operator can delete a replaced credential only once no station references it. Do not use `ON DELETE CASCADE` here.

Timezone: `timezone` is the IANA zone for display; `std_utc_offset_minutes` is the standard (non-DST) offset used by `app.lst_date`. Mountain Standard Time is `-420`. Both are set at station creation (API connect flow or `admin add-station`); how the timezone is determined from coordinates is an Open question for 02.

Down: `DROP TABLE app.forecast_locations; DROP TABLE app.stations;`

### 5.6 `0006_observations.up.sql`

Final names (this is the authoritative list; 03's 7.1 must match). All sensor columns are nullable; a field AWN omits stays NULL.

| AWN field | Column | Type |
| --- | --- | --- |
| `dateutc` (ms epoch) | `ts_utc` | timestamptz |
| `tempf` | `temp_f` | real |
| `feelsLike` | `feels_like_f` | real |
| `dewPoint` | `dew_point_f` | real |
| `humidity` | `humidity_pct` | smallint |
| `windspeedmph` | `wind_speed_mph` | real |
| `windgustmph` | `wind_gust_mph` | real |
| `maxdailygust` | `max_daily_gust_mph` | real |
| `winddir` | `wind_dir_deg` | smallint |
| `baromrelin` | `baro_rel_inhg` | real |
| `baromabsin` | `baro_abs_inhg` | real |
| `hourlyrainin` | `rain_rate_in_hr` | real |
| `eventrainin` | `rain_event_in` | real |
| `dailyrainin` | `rain_daily_in` | real |
| `weeklyrainin` | `rain_weekly_in` | real |
| `monthlyrainin` | `rain_monthly_in` | real |
| `yearlyrainin` | `rain_yearly_in` | real |
| `totalrainin` | `rain_total_in` | real |
| `lastRain` (ISO string) | `rain_last_at` | timestamptz |
| `solarradiation` | `solar_wm2` | real |
| `uv` | `uv_index` | smallint |
| `tempinf` | `temp_indoor_f` | real |
| `humidityin` | `humidity_indoor_pct` | smallint |
| `battout` | `battery_out` | smallint |
| (derived by `daily_rollup`, not by ingest) | `rain_inc_in` | numeric(6,3) |

`rain_inc_in` is the per-interval rain increment after reset handling and jump capping (F6). It is recomputed by the rollup (so a late row fixes its neighbours) and `daily_rollups.rain_in = sum(rain_inc_in)`. Ingest upserts must not touch it. Not stored: `24hourrainin`, `tz`, `macAddress`, extra sensors, `battin`, raw JSON (C4). Adding a sensor is a migration plus a row in 03's mapping table.

```sql
CREATE TABLE app.observations (
  station_id          uuid NOT NULL REFERENCES app.stations (id) ON DELETE CASCADE,
  ts_utc              timestamptz NOT NULL,
  temp_f              real,
  feels_like_f        real,
  dew_point_f         real,
  humidity_pct        smallint CHECK (humidity_pct BETWEEN 0 AND 100),
  wind_speed_mph      real CHECK (wind_speed_mph >= 0),
  wind_gust_mph       real CHECK (wind_gust_mph >= 0),
  max_daily_gust_mph  real CHECK (max_daily_gust_mph >= 0),
  wind_dir_deg        smallint CHECK (wind_dir_deg BETWEEN 0 AND 360),
  baro_rel_inhg       real,
  baro_abs_inhg       real,
  rain_rate_in_hr     real CHECK (rain_rate_in_hr >= 0),
  rain_event_in       real CHECK (rain_event_in >= 0),
  rain_daily_in       real CHECK (rain_daily_in >= 0),
  rain_weekly_in      real CHECK (rain_weekly_in >= 0),
  rain_monthly_in     real CHECK (rain_monthly_in >= 0),
  rain_yearly_in      real CHECK (rain_yearly_in >= 0),
  rain_total_in       real CHECK (rain_total_in >= 0),
  rain_last_at        timestamptz,
  solar_wm2           real CHECK (solar_wm2 >= 0),
  uv_index            smallint CHECK (uv_index >= 0),
  temp_indoor_f       real,
  humidity_indoor_pct smallint CHECK (humidity_indoor_pct BETWEEN 0 AND 100),
  battery_out         smallint,
  rain_inc_in         numeric(6,3) CHECK (rain_inc_in >= 0),
  received_at         timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (station_id, ts_utc)
);

GRANT SELECT ON app.observations TO app_api;
GRANT SELECT, INSERT, UPDATE ON app.observations TO app_poller;
```

Idempotent write pattern for 03: `INSERT ... ON CONFLICT (station_id, ts_utc) DO UPDATE SET <all sensor columns> = EXCLUDED.<...>` (poller has SELECT, so EXCLUDED and `COALESCE(EXCLUDED.x, app.observations.x)` are readable; 03 also uses an `IS DISTINCT FROM` WHERE guard). `ts_utc` is AWN `dateutc` converted verbatim (no rounding); re-fetching the same history page must produce identical keys. No DELETE grant: nothing deletes observations except cascade from station deletion.

Size: about 105,000 rows per station-year, roughly 130 bytes per row plus PK index, so 3 stations is on the order of 60 to 100 MB per year (C4).

Down: `DROP TABLE app.observations;`

### 5.7 `0007_daily_rollups.up.sql`

```sql
CREATE TABLE app.daily_rollups (
  station_id        uuid NOT NULL REFERENCES app.stations (id) ON DELETE CASCADE,
  local_date        date NOT NULL,                -- local STANDARD time day (app.lst_date), F5
  rain_in           numeric(7,3) CHECK (rain_in >= 0),
  tmax_f            real,
  tmin_f            real,
  gdd               real CHECK (gdd >= 0),
  gdd_method        text NOT NULL CHECK (gdd_method IN ('avg_50','clamp_50_86')),
  obs_count         smallint NOT NULL DEFAULT 0 CHECK (obs_count >= 0),   -- readings with temp_f (288 at 5 min, 48 at 30 min when complete)
  is_complete       boolean NOT NULL DEFAULT false,    -- true only for a finished day with coverage >= 0.9 (interval-aware, below)
  rain_resets       smallint NOT NULL DEFAULT 0,        -- counter drops treated as resets (F6)
  rain_capped       smallint NOT NULL DEFAULT 0,        -- impossible jumps capped (F6)
  awn_daily_rain_in numeric(7,3),                       -- AWN's own daily total used for reconciliation
  rain_reconcile_status text NOT NULL DEFAULT 'unchecked'
                    CHECK (rain_reconcile_status IN ('unchecked','ok','mismatch','no_awn_total')),
  rain_reconcile_diff_in numeric(7,3),                  -- console-computed minus awn_daily_rain_in
  computed_at       timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (station_id, local_date)
);
CREATE INDEX daily_rollups_date_idx ON app.daily_rollups (local_date);

-- Days whose observations changed and need a rollup (00-conventions). Ingest/backfill insert in the same transaction as
-- the observation upsert (ON CONFLICT DO NOTHING); daily_rollup deletes the rows it has processed. No timestamp-based dirtiness.
CREATE TABLE app.rollup_dirty_days (
  station_id uuid NOT NULL REFERENCES app.stations (id) ON DELETE CASCADE,
  local_date date NOT NULL,
  PRIMARY KEY (station_id, local_date)
);

GRANT SELECT ON app.daily_rollups TO app_api;
GRANT SELECT, INSERT, UPDATE ON app.daily_rollups TO app_poller;
GRANT SELECT, INSERT, DELETE ON app.rollup_dirty_days TO app_poller;   -- app_api: no access
-- The locking enqueue `INSERT ... ON CONFLICT (station_id, local_date) DO UPDATE SET local_date = EXCLUDED.local_date`
-- (a no-op that takes the row lock) needs UPDATE on the column it sets.
GRANT UPDATE (local_date) ON app.rollup_dirty_days TO app_poller;
```

Final names: per-day facts are columns, not a flags array (queryable, no CHECK-widening migration for new flags). Rule (aligned with 03): `rain_in` is computed whenever a usable rain counter exists, whatever the coverage; it is NULL only when no counter qualifies (then `rain_reconcile_status` stays `unchecked`). Coverage is interval-aware (03): per day `interval = 5 min` if the median gap between consecutive readings is at most 7.5 min, else 30 min; `coverage = min(1, obs_count * interval / 24 h)`. `tmax_f`, `tmin_f` and `gdd` are NULL when coverage is below 0.5; `is_complete` is true only when coverage is at least 0.9 and the day does not include now. It never nulls rain. The temperature-outlier count has no column (03 keeps it in `job_runs.detail`). `gdd_method` is `avg_50` or `clamp_50_86` (the design doc leaves the choice open; see Open questions).

Down: `DROP TABLE app.rollup_dirty_days; DROP TABLE app.daily_rollups;`

### 5.8 `0008_forecasts.up.sql`

Final design: a `forecast_runs` row per `(location, model, run)` plus its snapshot rows, inserted in one transaction. The runs table is chosen over `fetched_at` on each snapshot because (a) "has this run been fetched" is a primary-key lookup rather than a scan of snapshots, (b) a run that yields zero usable windows is still recorded and is not refetched every 10 minutes, (c) completeness is atomic (run row exists only with all of its snapshot rows), and (d) `fetched_at` is stored once per run rather than once per row (size, F1).

```sql
CREATE TABLE app.forecast_runs (
  location_id uuid NOT NULL REFERENCES app.forecast_locations (id) ON DELETE CASCADE,
  model       text NOT NULL,
  run_time    timestamptz NOT NULL,          -- model initialisation time as published by Open-Meteo
  fetched_at  timestamptz NOT NULL DEFAULT now(),
  row_count   integer NOT NULL CHECK (row_count >= 0),
  PRIMARY KEY (location_id, model, run_time)
);

CREATE TABLE app.forecast_snapshots (
  location_id  uuid NOT NULL,
  model        text NOT NULL,
  run_time     timestamptz NOT NULL,
  variable     text NOT NULL CHECK (variable IN ('precip_6h_in')),   -- the only value in v1 (inches); a new variable needs a migration
  valid_time   timestamptz NOT NULL,         -- START of the 6-hour window, UTC, aligned to 00/06/12/18
  period_hours smallint NOT NULL DEFAULT 6 CHECK (period_hours = 6),
  p10          real,
  p50          real,
  p90          real,
  prob_precip  real CHECK (prob_precip IS NULL OR prob_precip BETWEEN 0 AND 100),   -- percent
  PRIMARY KEY (location_id, model, variable, valid_time, run_time),
  FOREIGN KEY (location_id, model, run_time)
    REFERENCES app.forecast_runs (location_id, model, run_time) ON DELETE CASCADE,
  CHECK (valid_time AT TIME ZONE 'UTC' = date_trunc('hour', valid_time AT TIME ZONE 'UTC')
         AND extract(hour FROM valid_time AT TIME ZONE 'UTC')::int % 6 = 0)
);
CREATE INDEX forecast_snapshots_run_idx ON app.forecast_snapshots (location_id, model, run_time DESC);

GRANT SELECT ON app.forecast_runs, app.forecast_snapshots TO app_api;
GRANT SELECT, INSERT ON app.forecast_runs, app.forecast_snapshots TO app_poller;   -- immutable: no UPDATE/DELETE
```

Idempotency: `INSERT ... ON CONFLICT DO NOTHING` on both tables. The query "how did the forecast for valid_time T change across runs" is served by the snapshot primary-key order. Units: inches. Size is about 100 MB/year (F1); no pruning in v1.

Down: drop snapshots then runs.

### 5.9 `0009_ops_tables.up.sql`

```sql
CREATE TABLE app.job_runs (
  id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),     -- uuid: conventions say IDs are UUIDs; no sequence grants
  job         text NOT NULL CHECK (job ~ '^[a-z][a-z_]*$'),   -- awn_ingest, awn_backfill, daily_rollup, reference_sync, forecast_snapshot, credential_reencrypt
  subject_id  uuid,                                            -- optional station / location / credential id
  started_at  timestamptz NOT NULL DEFAULT now(),
  finished_at timestamptz,
  status      text NOT NULL DEFAULT 'running' CHECK (status IN ('running','success','failed','cancelled','abandoned')),
  detail      jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (octet_length(detail::text) <= 8192),
  CHECK (finished_at IS NULL OR finished_at >= started_at)
);
CREATE INDEX job_runs_job_started_idx ON app.job_runs (job, started_at DESC);
CREATE INDEX job_runs_last_success_idx ON app.job_runs (job, finished_at DESC) WHERE status = 'success';

CREATE TABLE app.audit_log (
  id           bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  at           timestamptz NOT NULL DEFAULT now(),
  actor_id     uuid,            -- no FK: rows must survive user deletion (section 5.12)
  action       text NOT NULL CHECK (action ~ '^[a-z][a-z_]*(\.[a-z][a-z_]*)*$'),   -- dotted, e.g. follow.approved
  subject_type text NOT NULL CHECK (subject_type IN ('user','follow','credential','station')),
  subject_id   uuid,
  detail       jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (octet_length(detail::text) <= 4096)
);
CREATE INDEX audit_log_at_idx ON app.audit_log (at DESC);
CREATE INDEX audit_log_subject_idx ON app.audit_log (subject_id, at DESC);

CREATE TABLE app.connect_attempts (
  id           bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  user_id      uuid NOT NULL REFERENCES app.users (id) ON DELETE CASCADE,
  attempted_at timestamptz NOT NULL DEFAULT now(),
  outcome      text NOT NULL CHECK (outcome IN ('started','valid','invalid','rate_limited','error'))
);   -- 02 inserts 'started' BEFORE calling AWN, then updates the outcome
CREATE INDEX connect_attempts_user_idx ON app.connect_attempts (user_id, attempted_at DESC);

-- Slot reservations for shared AWN rate gates (00-conventions, core::ratelimit).
CREATE TABLE app.rate_gates (
  name            text PRIMARY KEY CHECK (char_length(name) BETWEEN 1 AND 100),   -- awn:app:poller, awn:app:api, awn:key:<credential_id>
  next_allowed_at timestamptz NOT NULL
);

GRANT SELECT ON app.job_runs TO app_api;
GRANT SELECT, INSERT, UPDATE, DELETE ON app.job_runs TO app_poller;   -- uuid key: no sequence grant needed
GRANT INSERT ON app.audit_log TO app_api, app_poller;           -- append only; neither can read or alter
GRANT SELECT, INSERT, DELETE ON app.connect_attempts TO app_api;
GRANT UPDATE (outcome) ON app.connect_attempts TO app_api;          -- 'started' -> final outcome
GRANT SELECT, INSERT, UPDATE ON app.rate_gates TO app_api, app_poller;
-- Belt and braces for identity sequences (privilege is normally not needed for GENERATED ... AS IDENTITY; tested):
GRANT USAGE ON SEQUENCE app.audit_log_id_seq TO app_api, app_poller;
GRANT USAGE ON SEQUENCE app.connect_attempts_id_seq TO app_api;
```

**Audit action list (exact; every writer uses these strings verbatim, `subject_type` in parentheses, never secrets or emails in `detail`). `subject_type` is one of `user|follow|credential|station`. For `follow.*` rows `subject_id` is the **follower's** user id and `detail` is `{"followee_id": "<uuid>"}`; for `follow.seeded` the same, one row per direction:**

| Writer | Actions |
| --- | --- |
| `api` | `follow.requested`, `follow.approved`, `follow.declined`, `follow.revoked` (follow); `credential.saved` (credential; the new row), `credential.replaced` (credential; the old row, written on reconnect together with `credential.saved`); `station.created` (station) |
| `admin` | `user.invited`, `user.deactivated`, `user.deleted` (user); `follow.seeded` (follow); `credential.saved`, `credential.replaced`, `station.created` |
| `poller` | `credential.invalid`, `credential.reencrypted` (credential; `actor_id` NULL) |

`credential.validate` does **not** exist: key validation writes no audit row (the `connect_attempts` row is its record, per 02). The old underscore forms (`user_invited`, `follow_seeded`, `credential_invalid`, ...) are not used anywhere. Logins are recorded by GoTrue in `auth.audit_log_entries`, which no app role can read; copying them is an Open question.

`connect_attempts` is 02's durable per-user limit for the connect flow (S9). `rate_gates` rows (one per gate name; `awn:key:<credential_id>` rows accumulate, a few bytes each) are never deleted by app roles; an operator may prune rows for replaced credentials.

Down: drop the four tables.

### 5.10 `0010_can_view.up.sql`

```sql
CREATE FUNCTION app.can_view(p_user_id uuid, p_station_id uuid) RETURNS boolean
LANGUAGE sql STABLE SECURITY INVOKER PARALLEL SAFE SET search_path = pg_catalog AS
$$
  SELECT EXISTS (
    SELECT 1
    FROM app.users v
    JOIN app.stations s ON s.id = p_station_id
    WHERE v.id = p_user_id
      AND v.is_active
      AND (
        s.owner_id = v.id
        OR EXISTS (
          SELECT 1 FROM app.follows f
          WHERE f.follower_id = v.id AND f.followee_id = s.owner_id AND f.status = 'approved'
        )
      )
  )
$$;

CREATE FUNCTION app.visible_station_ids(p_user_id uuid) RETURNS SETOF uuid
LANGUAGE sql STABLE SECURITY INVOKER PARALLEL SAFE SET search_path = pg_catalog AS
$$
  SELECT s.id
  FROM app.users v
  JOIN app.stations s ON (
    s.owner_id = v.id
    OR EXISTS (SELECT 1 FROM app.follows f
               WHERE f.follower_id = v.id AND f.followee_id = s.owner_id AND f.status = 'approved'))
  WHERE v.id = p_user_id AND v.is_active
$$;

REVOKE EXECUTE ON FUNCTION app.can_view(uuid, uuid), app.visible_station_ids(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION app.can_view(uuid, uuid), app.visible_station_ids(uuid) TO app_api;
```

Semantics: true only when the viewer exists, is active, and either owns the station or holds an `approved` follow on the owner. `pending`, `declined`, `revoked`, a follow in the opposite direction, an unknown station and a NULL argument all return false (fail closed). Whether an inactive **owner's** stations stay visible to followers is not decided; this implementation does not hide them (Open questions).

Down: drop both functions.

### 5.11 Other tables considered and not created

- No `sessions` table (session validity comes from `app_auth.session_active`).
- No RLS inside `app` (section 6.6).
- No station-sharing table beyond `follows` (approval shares all of the owner's stations, per the design doc).

### 5.12 Deletion and recovery rules (S16)

| Event | Effect |
| --- | --- |
| Deactivate user (`admin deactivate-user`) | `users.is_active=false`, `deactivated_at` set. All data retained, including stations (the poller keeps ingesting; whether it should is in Open questions). Takes effect on the next API request. Reversible by SQL. |
| Delete user (`admin delete-user`) | Requires the user to be inactive. Deletes the `app.users` row, cascading to `follows` (both directions), `credentials`, `stations`, `forecast_locations`, `forecast_runs`, `forecast_snapshots`, `observations`, `daily_rollups`, `connect_attempts`. Then deletes the Supabase auth user. `audit_log` rows are kept (they hold only UUIDs, no emails or names). `reference_*` tables are shared and untouched. |
| Delete station (admin SQL only; no API route) | Cascades to its observations, rollups, forecast location, runs, snapshots. The credential stays. |
| Delete credential | Refused while stations reference it (a `replaced` credential is still referenced by any `not_covered` station until those are deleted or repointed). |
| Delete auth user in the Supabase dashboard | Not blocked (no FK). The user can no longer sign in, but `app.users` and all data remain until `admin delete-user` runs. |

Recovery: restore from the daily backup (06). `app_backup` reads only `app`; how auth users are recovered (re-invite, or a separate Supabase-side export) is an Open question (owner's choice; proposal 03 gives `app_backup` no direct grants on `auth.*`). `admin invite --adopt`-style recreation requires the original user ids, which `app.users` retains.

## 6. Roles and grants

### 6.1 Role summary (as in conventions)

| Role | Used by | Notes |
| --- | --- | --- |
| `app_migrator` | `admin` (including `admin migrate`) | Owns schema `app` and all its objects. Created without SUPERUSER, REPLICATION and BYPASSRLS (asserted by `roles.sql`). EXECUTE on `app_auth.revoke_sessions` only; no direct rights on any `auth.*` table. Connection limit 10. |
| `app_api` | `api` | Column-level grants as in migrations. `credentials`: INSERT, SELECT of non-ciphertext columns, UPDATE(status, status_changed_at) only. EXECUTE on `app_auth.session_active` only. No DELETE except `connect_attempts`. Limit 20. |
| `app_poller` | `poller` | SELECT and UPDATE on `credentials` (status, last_ok_at, ciphertext columns, key_version); writes ingest tables, rollups, reference_daily, forecast tables, `job_runs`, `rate_gates`; UPDATE(last_observed_at, backfill_floor_ts) on `stations`. Limit 15. |
| `app_backup` | backup job | SELECT on every `app` table including `app._sqlx_migrations` (default privileges plus the explicit grant in 0001). No access to `auth.*` or `app_auth` (see Open questions). `default_transaction_read_only = on`. Limit 3. |
| `app_ci_check` | CI and production exposure check | LOGIN, no grants on any object, reads catalogs only. `default_transaction_read_only = on`. Limit 2. |

Developers have no production role. The Supabase `postgres` login is used only for bootstrap by the operator, never put in Fly secrets (S12).

### 6.2 `ops/db/bootstrap/roles.sql`

Run as the admin login (`postgres`): `psql -X -v ON_ERROR_STOP=1 -f roles.sql <connection string>`, using the direct (IPv6) connection or, for an IPv4-only operator, the Supavisor **session-mode** pooler (user `postgres.<project-ref>`; transaction mode would break `\gexec`). Role passwords never travel as SQL text: the script reads **SCRAM-SHA-256 verifiers** from the environment (`\getenv`, so not on argv either), and aborts if any is unset or malformed. Compute a verifier on the operator's machine with `printf '%s\n' "$PASSWORD" | python3 ops/db/bootstrap/scram.py` (PostgreSQL stores a pre-hashed password as given). Use generated alphanumeric passwords (SASLprep-safe). Do not paste this script into the Supabase dashboard SQL editor (its history is stored).

`ops/db/bootstrap/scram.py`:

```python
#!/usr/bin/env python3
import base64, hashlib, hmac, os, sys
pw = sys.stdin.readline().rstrip("\n").encode()
salt, iters = os.urandom(16), 4096
salted = hashlib.pbkdf2_hmac("sha256", pw, salt, iters)
stored = hashlib.sha256(hmac.new(salted, b"Client Key", "sha256").digest()).digest()
server = hmac.new(salted, b"Server Key", "sha256").digest()
b = lambda x: base64.b64encode(x).decode()
print(f"SCRAM-SHA-256${iters}:{b(salt)}${b(stored)}:{b(server)}")
```

```sql
\set ON_ERROR_STOP on
\getenv migrator_vf APP_MIGRATOR_PASSWORD_VERIFIER
\getenv api_vf      APP_API_PASSWORD_VERIFIER
\getenv poller_vf   APP_POLLER_PASSWORD_VERIFIER
\getenv backup_vf   APP_BACKUP_PASSWORD_VERIFIER
\getenv ci_check_vf APP_CI_CHECK_PASSWORD_VERIFIER

-- Abort when any variable is unset (\getenv leaves it undefined). Repeat for each variable.
\if :{?migrator_vf}
\else
  \echo 'APP_MIGRATOR_PASSWORD_VERIFIER is not set'
  SELECT 1/0;
\endif
-- ... same \if block for api_vf, poller_vf, backup_vf, ci_check_vf ...

-- Abort when any value is not a well-formed SCRAM-SHA-256 verifier (16-byte salt = 24 base64 chars with ==; StoredKey and ServerKey = 44 chars with =).
SELECT bool_or(v !~ '^SCRAM-SHA-256\$[0-9]+:[A-Za-z0-9+/]{22}==\$[A-Za-z0-9+/]{43}=:[A-Za-z0-9+/]{43}=$') AS bad   -- 16-byte salt, 32-byte keys, all base64
FROM (VALUES (:'migrator_vf'), (:'api_vf'), (:'poller_vf'), (:'backup_vf'), (:'ci_check_vf')) t(v) \gset
\if :bad
  \echo 'a password verifier is malformed (expected SCRAM-SHA-256$...)'
  SELECT 1/0;
\endif

SELECT format('CREATE ROLE %I LOGIN', r)
FROM (VALUES ('app_migrator'),('app_api'),('app_poller'),('app_backup'),('app_ci_check')) v(r)
WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = r) \gexec

ALTER ROLE app_migrator WITH LOGIN NOCREATEDB NOCREATEROLE CONNECTION LIMIT 10 PASSWORD :'migrator_vf';
ALTER ROLE app_api      WITH LOGIN NOCREATEDB NOCREATEROLE CONNECTION LIMIT 20 PASSWORD :'api_vf';
ALTER ROLE app_poller   WITH LOGIN NOCREATEDB NOCREATEROLE CONNECTION LIMIT 15 PASSWORD :'poller_vf';
ALTER ROLE app_backup   WITH LOGIN NOCREATEDB NOCREATEROLE CONNECTION LIMIT 3  PASSWORD :'backup_vf';
ALTER ROLE app_ci_check WITH LOGIN NOCREATEDB NOCREATEROLE CONNECTION LIMIT 2  PASSWORD :'ci_check_vf';

-- Hosted Supabase's postgres is not a superuser and cannot set SUPERUSER/REPLICATION/BYPASSRLS attributes, so those are not
-- named above. Roles are created without them; assert it (works the same on a local superuser).
DO $$
DECLARE bad text;
BEGIN
  SELECT string_agg(rolname, ', ') INTO bad FROM pg_roles
  WHERE rolname IN ('app_migrator','app_api','app_poller','app_backup','app_ci_check')
    AND (rolsuper OR rolreplication OR rolbypassrls);
  IF bad IS NOT NULL THEN RAISE EXCEPTION 'roles with forbidden attributes: %', bad; END IF;
END $$;

-- app_migrator: schema only (pg_catalog is searched implicitly first), so sqlx creates app._sqlx_migrations, never in public.
ALTER ROLE app_migrator SET search_path = app;
ALTER ROLE app_api      SET search_path = pg_catalog, app;
ALTER ROLE app_poller   SET search_path = pg_catalog, app;
ALTER ROLE app_backup   SET search_path = pg_catalog, app;
ALTER ROLE app_api      SET statement_timeout = '15s';
ALTER ROLE app_poller   SET statement_timeout = '120s';
ALTER ROLE app_api      SET idle_in_transaction_session_timeout = '30s';
ALTER ROLE app_poller   SET idle_in_transaction_session_timeout = '60s';
ALTER ROLE app_backup   SET default_transaction_read_only = on;
ALTER ROLE app_ci_check SET default_transaction_read_only = on;
ALTER ROLE app_ci_check SET statement_timeout = '10s';

-- Hosted Supabase: postgres is not a superuser; it must be able to SET ROLE to the owner to create the schema.
DO $$ BEGIN
  IF NOT (SELECT rolsuper FROM pg_roles WHERE rolname = current_user) THEN
    EXECUTE format('GRANT app_migrator TO %I WITH INHERIT FALSE, SET TRUE', current_user);
  END IF;
END $$;

CREATE SCHEMA IF NOT EXISTS app AUTHORIZATION app_migrator;
-- No REVOKE statements on schema app here. On hosted Supabase postgres is not a superuser and holds app_migrator with
-- INHERIT FALSE, so it is not the owner for REVOKE purposes and such statements would fail. They are also unnecessary:
-- a new schema has no PUBLIC or API-role privileges, migration 0001 (run as owner) revokes explicitly, and the exposure
-- check (8.4) proves that anon/authenticated/service_role/PUBLIC hold nothing in app. (If a REVOKE is ever needed here,
-- wrap it in SET ROLE app_migrator; ... RESET ROLE;)
```

Checks after running: `\du app_*` shows five roles; `SELECT nspowner::regrole FROM pg_namespace WHERE nspname='app'` is `app_migrator`; running with one verifier unset exits non-zero before creating anything (tested in 11.1). The `ALTER DEFAULT PRIVILEGES` statements live in migration 0001 (run as `app_migrator`).

### 6.3 Connection strings

- Production: `postgres://app_api:<pw>@<host>:5432/postgres?sslmode=verify-full&sslrootcert=<supabase CA>` for the direct (IPv6) connection; with the Supavisor **session-mode** pooler (port 5432 on the pooler host) the user name is `app_api.<project-ref>`. Never transaction mode (C3). Same pattern for `app_poller`, `app_migrator`, `app_backup`. Exact hosts are in 06.
- Local: `postgres://app_api:<pw>@db:5432/<db>`, `sslmode=disable` allowed only when `APP_ENV=local`.

### 6.4 Where roles are created

- Production (hosted): the operator (Rodney; agents never run this) runs `roles.sql` once as `postgres`, then, after GoTrue has created `auth.*`, `auth_access.sql` as `postgres`, from a trusted workstation over the direct connection, then clears the verifiers from shell history/env. Migrations are applied by CI running `admin migrate` (06).
- Local: `ops/db/init/` (05) runs `roles.sql` with `psql -U postgres` at first database init (verifiers are the precomputed values from `.env.example`). The `auth-access` one-shot runs `auth_access.sql` as `postgres` after `auth` is healthy, then the `migrate` service runs `admin migrate`.

### 6.5 `ops/db/bootstrap/auth_access.sql` (proposals/03, accepted)

App roles never read or write GoTrue-owned tables. Access goes through two `SECURITY DEFINER` functions in schema `app_auth`, owned by `postgres` (the schema is not `auth`, so creating it is allowed on hosted Supabase). Plain SQL with no psql meta-commands, runnable with `psql -v ON_ERROR_STOP=1 -f`, idempotent, run as `postgres` after GoTrue has migrated `auth` (locally by the `auth-access` one-shot, in production once by the operator).

```sql
CREATE SCHEMA IF NOT EXISTS app_auth;
REVOKE ALL ON SCHEMA app_auth FROM PUBLIC;
GRANT USAGE ON SCHEMA app_auth TO app_api, app_migrator;

-- Per-request session check (S2). Returns only true/false.
CREATE OR REPLACE FUNCTION app_auth.session_active(p_session_id uuid, p_user_id uuid)
RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path = '' AS
$$
  SELECT EXISTS (
    SELECT 1 FROM auth.sessions s
    WHERE s.id = p_session_id AND s.user_id = p_user_id
      AND (s.not_after IS NULL OR s.not_after > pg_catalog.now())
  )
$$;

-- Used by admin deactivate-user (S11). Returns the number of sessions removed.
CREATE OR REPLACE FUNCTION app_auth.revoke_sessions(p_user_id uuid)
RETURNS integer LANGUAGE sql SECURITY DEFINER SET search_path = '' AS
$$
  WITH d AS (DELETE FROM auth.sessions s WHERE s.user_id = p_user_id RETURNING 1)
  SELECT pg_catalog.count(*)::integer FROM d
$$;

REVOKE ALL ON FUNCTION app_auth.session_active(uuid, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION app_auth.revoke_sessions(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION app_auth.session_active(uuid, uuid) TO app_api;
GRANT EXECUTE ON FUNCTION app_auth.revoke_sessions(uuid) TO app_migrator;
```

Rules: `search_path = ''` and every object schema-qualified (including `pg_catalog.`); no other function may exist in `app_auth` (the exposure check enforces this); EXECUTE exactly as above. 02's startup self-test calls `SELECT app_auth.session_active(gen_random_uuid(), gen_random_uuid())` and must get `false`, failing fast on any error. A GoTrue column rename would break these functions, which is what the self-test and the exposure/grant tests catch.

**Task 0 (required pre-build verification, before anything relies on this):** run the scratch-project checklist at the end of `docs/proposals/03-supabase-auth-schema-access.md` on a throwaway hosted project (a definer function can read and delete `auth.sessions`; `session_id` claim round-trips; `ban_duration` stops refresh; role grants behave as the proposal expects). **Only if checklist step 3 fails** (the definer function cannot read `auth.sessions`), fall back to the proposal's option (c) (drop the session check, rely on `is_active` plus token expiry), record it as a downgrade of S2 in the review findings, and change 02. The local environment proves nothing about hosted behaviour (the local `postgres` is a real superuser).

### 6.6 Why no RLS in `app`

RLS enabled with no policies would return zero rows to `app_api`/`app_poller`; adding permissive policies adds nothing over grants because `can_view` is applied per query by the API. The control for `app` is: not exposed (6.7) plus no grants to `anon`/`authenticated`/`service_role`/`PUBLIC`. RLS is required (and checked) only on tables in exposed schemas.

### 6.7 Default privileges and PUBLIC

Covered in migration 0001: no PUBLIC EXECUTE on functions created by `app_migrator`, SELECT on new tables/sequences and on `_sqlx_migrations` for `app_backup`. Every other grant is explicit per table. `app_auth` functions have their own REVOKE/GRANT in `auth_access.sql`. The exposure check (8.4) fails if `anon`, `authenticated`, `service_role` or `PUBLIC` hold any privilege in `app`.

## 7. How the API uses `can_view`

- Every handler that takes a station id (or a forecast location id, which it first maps to its station id) calls `SELECT app.can_view($user_id, $station_id)` as the first statement of the request's transaction/connection, and returns the same error for "no such station" and "not allowed" (02 chooses 404).
- List endpoints select only from `app.visible_station_ids($user_id)` (`WHERE s.id IN (SELECT app.visible_station_ids($1))`) or join the same predicate. No list query may take station ids from the client without this filter.
- Forecast location access: `SELECT app.can_view($1, fl.station_id) FROM app.forecast_locations fl WHERE fl.id = $2`.
- Revocation is immediate: the function is `STABLE` and reads committed rows each call; the API must not cache its result across requests (no in-process cache, no memoised follow list). A follow set to `revoked` is invisible to the next statement in READ COMMITTED.
- The active-user check (`users.is_active`) is also inside `can_view`; the middleware still checks it separately for non-station routes.

## 8. Supabase project configuration

Dashboard names change; the intent is binding. Sources: Data API security https://supabase.com/docs/guides/api/securing-your-api, signing keys https://supabase.com/docs/guides/auth/signing-keys, passwordless email https://supabase.com/docs/guides/auth/auth-email-passwordless, custom SMTP https://supabase.com/docs/guides/auth/auth-smtp, roles https://supabase.com/docs/guides/database/postgres/roles, database size https://supabase.com/docs/guides/platform/database-size.

### 8.1 Auth settings (S4, S11)

| Setting | Value |
| --- | --- |
| Allow new users to sign up | **Off** |
| Allow anonymous sign-ins | **Off** |
| Email provider | On; "Confirm email" On |
| Other providers (phone, OAuth, SAML) | Off |
| Email OTP expiration (also governs magic links) | 900 seconds (default is 3600; values above 86400 are discouraged by Supabase). Recommendation, not a decided product value. |
| Email OTP length | 6 |
| JWT expiry | 3600 s default; 1800 recommended for a browser-held token. Recommendation. Minimum allowed by Supabase is 300 s. |
| Refresh-token reuse detection ("detect and revoke potentially compromised refresh tokens") | On |
| Site URL | `https://<site>` (the production Netlify URL) |
| Redirect URL allow-list | exactly `https://<site>/auth/callback` in production plus the one decided preview pattern `https://deploy-preview-*--<site>.netlify.app/auth/callback` (no localhost, no other wildcards). Local GoTrue: exactly `http://localhost:8000/auth/callback` and `http://localhost:5173/auth/callback` (00-conventions). |
| Time-boxed sessions, inactivity timeout, single session per user | Plan-gated (not available on the free plan; do not rely on them); the API's session check (6.5) covers revocation |
| Custom SMTP | **Required.** Supabase's built-in sender only delivers to project team addresses and is limited to 2 emails/hour, so family members could not log in. Provider undecided (Open questions). |

How `signInWithOtp` behaves: the web app must call it with `shouldCreateUser: false`; with sign-ups disabled an unknown address must not create a user (verified by the smoke test 8.3).

Local GoTrue settings (signup off, anonymous off, OTP expiry, JWT expiry, SMTP to Mailpit, site URL and exact redirect allow-list, HS256 secret) are defined in 05-local-dev section 4.4; they must match the intent of this table (signup and anonymous sign-in disabled, exact callback URLs only, 900 s OTP, 1800 s JWT).

### 8.2 JWT signing keys (S1, D2)

Production uses asymmetric signing keys: migrate the project from the legacy shared secret via the dashboard (JWT Signing Keys), rotate to a new **ES256 (P-256)** key, then revoke the legacy secret once tokens signed with it have expired. Public keys: `https://<project-ref>.supabase.co/auth/v1/.well-known/jwks.json` (this is `SUPABASE_JWKS_URL`; edge caches it for about 10 minutes). The API never receives or stores any signing secret in production. The legacy `anon`/`service_role` keys are JWTs signed by the legacy secret; once the legacy secret is revoked they stop validating, so move `PUBLIC_SUPABASE_ANON_KEY` to the new publishable key (`sb_publishable_...`) and `SUPABASE_SERVICE_ROLE_KEY` to the new secret key (`sb_secret_...`). The variable names stay as in conventions.

### 8.3 Smoke test `ops/db/checks/supabase_smoke.sh`

Inputs: `SUPABASE_URL`, `PUBLIC_SUPABASE_ANON_KEY`; hosted projects only (local Caddy serves `200.html` for `/rest/v1`, so the script is meaningless locally and 05 must not run it). It has no `admin` dependency. Each step must produce a **non-2xx** status or the script exits 1:

1. `GET $SUPABASE_URL/rest/v1/` with anon key (Data API off).
2. `GET $SUPABASE_URL/rest/v1/users`, `.../credentials`, `.../stations` with anon key.
3. `POST $SUPABASE_URL/auth/v1/signup` with `{"email":"probe-<random>@example.invalid","password":"<random 24 chars>"}`.
4. `POST $SUPABASE_URL/auth/v1/signup` with body `{}` (anonymous sign-in).
5. `POST $SUPABASE_URL/auth/v1/otp` with `{"email":"probe-<random>@example.invalid","create_user":false}`; any status is accepted. The script then prints a reminder: the operator runs `admin list-users` (one-off machine) and the smoke test fails if its output contains any `auth_only` line.

Headers: `apikey: $PUBLIC_SUPABASE_ANON_KEY`. Never print keys. Run after every settings change and weekly by 06.

### 8.4 Data API and exposure check (S6, G3)

Decision: **turn the Data API off** (Data API settings, "Enable Data API" off). Nothing in the product uses PostgREST (auth uses GoTrue; data goes through the Rust API). Defence in depth: `app` is never added to exposed schemas and `anon`/`authenticated`/`service_role` hold no privileges in `app`. Known risk: Supabase issues report the dashboard setting and PostgREST's running schema list can disagree (https://github.com/supabase/supabase/issues/45904), hence the black-box probe in 8.3 plus the SQL check below.

**Which check is the real gate.** Check 1 below (reading `pgrst.db_schemas` from the `authenticator` role) is best effort: on hosted Supabase PostgREST's live schema list can differ from that role setting. The black-box smoke test (8.3) is the real gate for "Data API off". The privilege checks (4 to 9) do not depend on Supabase internals and are the durable control.

`ops/db/checks/exposure.sql` runs as `app_ci_check` (no grants, catalog reads only) in a read-only session (`PGOPTIONS='-c default_transaction_read_only=on'`), creates no temp tables, and fails by raising. One `WITH ... SELECT` collects violations and a `DO` block raises with the full list:

```sql
DO $$
DECLARE v text;
BEGIN
  WITH
  pgrst AS (  -- best effort (see above)
    SELECT btrim(sch) AS schema_name
    FROM pg_roles r, LATERAL unnest(r.rolconfig) cfg,
         LATERAL unnest(string_to_array(substring(cfg from '^pgrst\.db_schemas=(.*)$'), ',')) sch
    WHERE r.rolname = 'authenticator' AND cfg LIKE 'pgrst.db_schemas=%'
  ),
  exposed AS (
    SELECT schema_name FROM pgrst
    UNION ALL SELECT 'public' WHERE NOT EXISTS (SELECT 1 FROM pgrst)       -- PostgREST default
  ),
  watched(role_name) AS (  -- roles that must hold nothing in app / app_auth (PUBLIC included)
    SELECT 'public'
    UNION ALL SELECT rolname::text FROM pg_roles
              WHERE rolname IN ('anon','authenticated','service_role','app_ci_check')
  ),
  rel AS (
    SELECT c.oid AS relid, c.relname, c.relkind, c.relowner, c.relrowsecurity, n.nspname
    FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE NOT EXISTS (SELECT 1 FROM pg_depend d WHERE d.objid = c.oid AND d.deptype = 'e')
  ),
  fn AS (
    SELECT p.oid AS fnid, p.proname, p.proowner, p.prosecdef, p.proconfig, n.nspname
    FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
    WHERE NOT EXISTS (SELECT 1 FROM pg_depend d WHERE d.objid = p.oid AND d.deptype = 'e')
  ),
  violations(check_name, object) AS (
    -- 1 app / app_auth in an exposed schema list
    SELECT 'SCHEMA_EXPOSED', schema_name FROM exposed WHERE schema_name IN ('app','app_auth')
    UNION ALL  -- 2 tables without RLS, or any view, in an exposed schema
    SELECT CASE WHEN relkind IN ('r','p') THEN 'EXPOSED_TABLE_WITHOUT_RLS' ELSE 'EXPOSED_VIEW' END, nspname || '.' || relname
    FROM rel WHERE nspname IN (SELECT schema_name FROM exposed)
      AND relkind IN ('r','p','v','m','f') AND (relkind NOT IN ('r','p') OR NOT relrowsecurity)
    UNION ALL  -- 3 nothing app-owned in public
    SELECT 'OBJECT_IN_PUBLIC', 'public.' || relname FROM rel WHERE nspname = 'public' AND relkind IN ('r','p','v','m','f','S')
    UNION ALL
    SELECT 'FUNCTION_IN_PUBLIC', 'public.' || proname FROM fn WHERE nspname = 'public'
    UNION ALL  -- 4 watched roles hold nothing in app / app_auth (schema, relations, columns, sequences, functions)
    SELECT 'WATCHED_ROLE_PRIVILEGE', w.role_name || ' on schema ' || n.nspname
    FROM watched w, pg_namespace n
    WHERE n.nspname IN ('app','app_auth') AND has_schema_privilege(w.role_name, n.oid, 'USAGE,CREATE')
    UNION ALL
    SELECT 'WATCHED_ROLE_PRIVILEGE', w.role_name || ' on ' || r.nspname || '.' || r.relname
    FROM watched w, rel r
    WHERE r.nspname IN ('app','app_auth') AND r.relkind IN ('r','p','v','m')
      AND (has_table_privilege(w.role_name, r.relid, 'SELECT,INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER')
           OR has_any_column_privilege(w.role_name, r.relid, 'SELECT,INSERT,UPDATE,REFERENCES'))
    UNION ALL
    SELECT 'WATCHED_ROLE_PRIVILEGE', w.role_name || ' on sequence ' || r.relname
    FROM watched w, rel r
    WHERE r.nspname IN ('app','app_auth') AND r.relkind = 'S' AND has_sequence_privilege(w.role_name, r.relid, 'USAGE,SELECT,UPDATE')
    UNION ALL
    SELECT 'WATCHED_ROLE_PRIVILEGE', w.role_name || ' EXECUTE ' || f.nspname || '.' || f.proname
    FROM watched w, fn f
    WHERE f.nspname IN ('app','app_auth') AND has_function_privilege(w.role_name, f.fnid, 'EXECUTE')
    UNION ALL  -- 5 ownership: relations and functions in app belong to app_migrator
    SELECT 'RELATION_NOT_OWNED_BY_MIGRATOR', nspname || '.' || relname FROM rel
    WHERE nspname = 'app' AND relkind IN ('r','p','v','m','S') AND relowner <> 'app_migrator'::regrole
    UNION ALL
    SELECT 'FOREIGN_OWNED_FUNCTION', nspname || '.' || proname FROM fn
    WHERE nspname = 'app' AND proowner <> 'app_migrator'::regrole
    UNION ALL  -- 6 SECURITY DEFINER: none in app/public; app_auth holds exactly the two expected functions
    SELECT 'SECURITY_DEFINER_OUTSIDE_APP_AUTH', nspname || '.' || proname FROM fn
    WHERE prosecdef AND nspname IN ('app','public')
    UNION ALL
    SELECT 'UNEXPECTED_APP_AUTH_FUNCTION', proname FROM fn
    WHERE nspname = 'app_auth' AND proname NOT IN ('session_active','revoke_sessions')
    UNION ALL
    SELECT 'APP_AUTH_FUNCTION_NOT_HARDENED', proname FROM fn
    WHERE nspname = 'app_auth'
      AND (NOT prosecdef OR NOT EXISTS (SELECT 1 FROM unnest(coalesce(proconfig, '{}')) c WHERE c ~ '^search_path=("")?$'))
    UNION ALL  -- 6b EXECUTE on the definer functions is exactly app_api / app_migrator
    SELECT 'APP_AUTH_EXECUTE_GRANT', r.rolname || ' on ' || f.proname
    FROM fn f, pg_roles r
    WHERE f.nspname = 'app_auth' AND r.rolname IN ('app_api','app_migrator','app_poller','app_backup')
      AND has_function_privilege(r.rolname, f.fnid, 'EXECUTE')
      AND NOT ((f.proname = 'session_active' AND r.rolname = 'app_api')
            OR (f.proname = 'revoke_sessions' AND r.rolname = 'app_migrator'))
    UNION ALL  -- 7 app_api must not be able to read or modify key material, ownership or identity columns
    SELECT 'API_COLUMN_PRIVILEGE', x.tbl || '.' || x.col || ' ' || x.priv
    FROM rel r JOIN (VALUES
      ('credentials','api_key_ciphertext','SELECT'), ('credentials','encapsulated_key','SELECT'),
      ('credentials','api_key_ciphertext','UPDATE'), ('credentials','encapsulated_key','UPDATE'),
      ('credentials','key_version','UPDATE'), ('credentials','owner_id','UPDATE'), ('credentials','id','UPDATE'),
      ('stations','owner_id','UPDATE'), ('stations','mac','UPDATE'), ('stations','lat','UPDATE'), ('stations','lon','UPDATE'),
      ('users','is_active','UPDATE'), ('users','is_admin','UPDATE')
    ) x(tbl, col, priv) ON r.nspname = 'app' AND r.relname = x.tbl
    WHERE has_column_privilege('app_api', r.relid, x.col, x.priv)   -- OIDs from the catalog: no regclass casts (they need schema USAGE)
    UNION ALL  -- 8 no app role holds any privilege on GoTrue tables
    SELECT 'APP_ROLE_PRIVILEGE_ON_AUTH', r.rolname || ' on auth.' || a.relname
    FROM rel a, pg_roles r
    WHERE a.nspname = 'auth' AND a.relname IN ('sessions','users','refresh_tokens')
      AND r.rolname IN ('app_api','app_poller','app_migrator','app_backup','app_ci_check')
      AND (has_table_privilege(r.rolname, a.relid, 'SELECT,INSERT,UPDATE,DELETE') OR has_any_column_privilege(r.rolname, a.relid, 'SELECT,INSERT,UPDATE'))
  )
  SELECT string_agg(check_name || ': ' || object, E'\n' ORDER BY check_name, object) INTO v FROM violations;

  IF v IS NOT NULL THEN
    RAISE EXCEPTION E'exposure check failed:\n%', v;
  END IF;
END $$;
```

The script uses no `::regclass` casts and no `to_regclass` on `app`/`auth` objects (they need schema USAGE, which `app_ci_check` does not have); relations and functions are identified by OID from `pg_class`/`pg_proc` joined to `pg_namespace`. The baseline test (11.4) must pass **as `app_ci_check`**.

`exposure.sh <connection-url>` runs `PGOPTIONS='-c default_transaction_read_only=on' psql -X -v ON_ERROR_STOP=1 -f exposure.sql "$1"` and propagates the exit code. It runs (a) in CI against Compose Postgres after migrations (`make test-db`), as `app_ci_check`; (b) against production by the operator after each deploy and weekly (06), as `app_ci_check`. The `authenticator` role does not exist locally; the check then treats `public` as the exposed schema, which is also correct.

### 8.5 Dashboard MFA (S16)

Enable MFA on the Supabase account (and on the GitHub account if used for Supabase sign-in). This is operator-only; add a line to the 06 runbook. No code.

### 8.6 Size and plan limits (C2, C4)

Free plan: the project goes read-only (`25006`) when database size exceeds **500 MB**, and a fresh project already uses roughly 40 to 60 MB. Treat 400 MB as the alert level. Read-only mode would stop ingestion and create permanent gaps, so alert early.

`ops/db/checks/size.sql`, run as `psql -X -v ON_ERROR_STOP=1 -f size.sql "$BACKUP_DATABASE_URL"` with the exit code checked by the caller (non-zero means alert):

```sql
\set ON_ERROR_STOP on
SELECT pg_size_pretty(pg_database_size(current_database())) AS database_size,
       round(pg_database_size(current_database()) / 1048576.0) AS database_mb;
SELECT c.relname, pg_size_pretty(pg_total_relation_size(c.oid)) AS total_size,
       pg_total_relation_size(c.oid) AS bytes
FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE n.nspname = 'app' AND c.relkind = 'r'
ORDER BY bytes DESC;
-- exit status helper: raises when above the alert threshold
DO $$ BEGIN
  IF pg_database_size(current_database()) > 400 * 1048576 THEN
    RAISE EXCEPTION 'database size above 400 MB alert threshold';
  END IF;
END $$;
```

Run **daily** by the backup machine (06) with `app_backup`; non-zero exit means "fail" ping to healthchecks.io. Expected growth: observations 60 to 100 MB per year, snapshots about 100 MB per year, so the free plan lasts roughly two years (C4); upgrade to Pro on alert.

## 9. Credential encryption format (S8, G6)

### 9.1 Threat model

Protects against: a leaked database dump or backup, SQL injection or compromise of the `app_api` login, a Supabase operator or dashboard compromise, anyone with `app_backup`. Does **not** protect against compromise of the poller process or its Fly secrets (it holds both the DB login and the private key), or of an `admin` machine holding the migrator login (it only has the public key but can overwrite rows).

### 9.2 Format

- Scheme: HPKE (RFC 9180) base mode (`mode_base`), KEM `DHKEM(X25519, HKDF-SHA256)`, KDF `HKDF-SHA256`, AEAD `AES-256-GCM`. Rust crate `hpke` with `X25519HkdfSha256`, `HkdfSha256`, `AesGcm256`.
- Plaintext: the AWN API key as UTF-8 bytes (no padding, no trailing newline).
- `info` (HPKE context binding): the constant bytes `b"family-weather/credential/v1"` (00-conventions; 02-api's draft text says `fwa-credential-v1`, which is wrong).
- AAD (per conventions): 32 bytes = raw 16-byte `credential_id` followed by raw 16-byte `owner_id` (UUID byte order as in `uuid::Uuid::as_bytes()`).
- Single-shot `seal`. Columns: `encapsulated_key` = the 32-byte KEM output; `api_key_ciphertext` = AEAD output (ciphertext followed by the 16-byte tag, as `hpke` returns it); `key_version` = value of `CREDENTIALS_KEY_VERSION` at encryption time. A fresh ephemeral key (hence fresh AEAD key and nonce) is created per `seal` by HPKE; never reuse an encapsulation.
- Key encodings: `CREDENTIALS_PUBLIC_KEY` and `CREDENTIALS_PRIVATE_KEY` are standard base64 (with padding) of the raw 32-byte X25519 public/private key.
- A swapped row (ciphertext copied to another credential id or owner) fails AEAD authentication; the poller must treat decryption failure as `status='invalid'`-adjacent error: log `credential_decrypt_failed` with the credential id only, set no status, alert.

### 9.3 `core::crypto` (owned here; final for 02, 03, admin)

```rust
// backend/crates/core/src/crypto.rs
pub struct EncryptedCredential { pub ciphertext: Vec<u8>, pub encapsulated_key: Vec<u8>, pub key_version: i16 }
pub fn encrypt_credential(pk: &CredentialPublicKey, key_version: i16, credential_id: Uuid, owner_id: Uuid,
                          api_key: &SecretString) -> Result<EncryptedCredential, CryptoError>;
#[cfg(feature = "decrypt")]
pub fn decrypt_credential(sk: &CredentialPrivateKey, enc: &EncryptedCredential, credential_id: Uuid,
                          owner_id: Uuid) -> Result<SecretString, CryptoError>;
pub fn public_key_from_private(sk: &CredentialPrivateKey) -> CredentialPublicKey;   // always compiled (no decrypt code)
// also: parse_public_key(&str) / parse_private_key(&str) from base64, generate_keypair(), keypair_matches(sk, pk) (= public_key_from_private(sk) == pk)
```

02 and 03 must use exactly these names and this argument order (`EncryptedCredential`, `CredentialPublicKey`, `CredentialPrivateKey`). The key types and `public_key_from_private` are available without the `decrypt` feature (so `admin keygen --check` and the poller's rotation can derive public keys); only decryption is gated. `CryptoError` must not carry key material or plaintext. Never log `SecretString`. Unit tests: RFC 9180 test vectors for the chosen suite, round trip, wrong AAD (swapped `credential_id` or `owner_id`) fails, two encryptions of one key differ, and CI check `cargo tree -p api -e features` shows `decrypt` is not enabled.

### 9.4 Key pair generation

`admin keygen` (no DB or network access) prints two lines to stdout:

```
CREDENTIALS_PUBLIC_KEY=<base64>
CREDENTIALS_PRIVATE_KEY=<base64>
```

and a reminder to set `CREDENTIALS_KEY_VERSION`. `admin keygen --check` reads `CREDENTIALS_PRIVATE_KEY` and `CREDENTIALS_PUBLIC_KEY` from the environment (or `--private`/`--public` via stdin, never argv) and exits 0 only if the pair matches; use it before and after rotating secrets. Store the private key in the password manager and as the poller's Fly secret only. Keys for local dev are generated the same way and live in `.env`; production refuses to start if the public key equals the one committed in `.env.example` (05/02 guard).

### 9.5 Rotation procedure (03-poller performs step 4)

1. `admin keygen`; choose version `N+1`.
2. Deploy the poller with `CREDENTIALS_PRIVATE_KEY=<new>`, `CREDENTIALS_KEY_VERSION=N+1`, `CREDENTIALS_PRIVATE_KEY_PREVIOUS=<old>`, `CREDENTIALS_KEY_VERSION_PREVIOUS=N`. The poller decrypts by `key_version`.
3. Deploy the API (and update the operator's `admin` environment) with `CREDENTIALS_PUBLIC_KEY=<new public>`, `CREDENTIALS_KEY_VERSION=N+1`. New or replaced keys are now written at `N+1`.
4. Poller re-encryption (`job_runs.job = 'credential_reencrypt'`, on demand; the poller subcommand is named in 03): for **every** row with `key_version < N+1`, including `replaced` rows: decrypt with the matching old key, encrypt with the new public key using the same `credential_id` and `owner_id`, then compare-and-swap:
   `UPDATE app.credentials SET api_key_ciphertext=$new_ct, encapsulated_key=$new_ek, key_version=$new_v WHERE id=$id AND key_version=$old_v AND api_key_ciphertext=$old_ct`.
   Zero rows updated means the row changed meanwhile (the API can no longer overwrite ciphertext, so this is a concurrent poller run); skip it. Write an `audit_log` row `credential.reencrypted` per row (no secrets).
5. Verify: `SELECT key_version, count(*) FROM app.credentials GROUP BY 1` (run as migrator or poller) shows only `N+1`.
6. Remove the `*_PREVIOUS` poller secrets; retire the old private key. Old backups remain decryptable only with the old private key; keep it in the password manager until the oldest backup that references version `N` has aged out (06's retention).

## 10. `admin` CLI (`backend/crates/admin`)

### 10.1 Overview

Binary `admin`, built into the shared image. Crates: `clap` (derive), `tokio`, `sqlx` (postgres), `reqwest` (rustls; errors pass through core's URL-scrubbing wrapper, S7), `secrecy`, `rpassword`, `uuid`, `serde`/`toml`, `anyhow`, and `core` (config, crypto, awn client). The image has no `psql`; every SQL statement `admin` runs goes through sqlx.

**Required environment per subcommand** (validated by `core::config` per subcommand; a subcommand must not demand variables it does not use):

| Subcommand | Required | Optional |
| --- | --- | --- |
| `migrate` | `APP_ENV`, `DATABASE_URL` | none |
| `keygen` (and `--check`) | none (`--check` reads `CREDENTIALS_PRIVATE_KEY`, `CREDENTIALS_PUBLIC_KEY`) | none |
| `wait` | none | `APP_ENV` |
| `invite` | `APP_ENV`, `DATABASE_URL`, `SUPABASE_URL`, `SUPABASE_SERVICE_ROLE_KEY` | none |
| `add-reference-station`, `list-users` | `APP_ENV`, `DATABASE_URL` | none (`list-users` also uses `SUPABASE_URL` + service key when set, to report orphans) |
| `add-station` | `APP_ENV`, `DATABASE_URL`, `CREDENTIALS_PUBLIC_KEY`, `CREDENTIALS_KEY_VERSION` | `AWN_APPLICATION_KEY` (+ `AWN_MODE`, `FIXTURES_DIR`): validation, local only (below) |
| `deactivate-user` | `APP_ENV`, `DATABASE_URL` | `SUPABASE_URL` + `SUPABASE_SERVICE_ROLE_KEY` (best-effort ban; warns when absent) |
| `delete-user` | `APP_ENV`, `DATABASE_URL`, `SUPABASE_URL`, `SUPABASE_SERVICE_ROLE_KEY` | none |
| `seed-dev` | `APP_ENV=local`, `DATABASE_URL`, `SUPABASE_URL`, `SUPABASE_SERVICE_ROLE_KEY`, `CREDENTIALS_PUBLIC_KEY`, `CREDENTIALS_KEY_VERSION` | `SEED_DIR` (default `/seed`); never uses AWN variables |

`DATABASE_URL` is the **app_migrator** URL for every subcommand.

The Rust shapes below are final; 03 implements `core::config::load(Binary::Admin)` against them (00-conventions). `load()` parses the subcommand first, then fails with `MissingEnv(<VAR>)` for any variable in the table's Required column that is unset, and never demands anything else.

```rust
// backend/crates/core/src/config/admin.rs
pub struct AdminEnv {
    pub app_env: Option<AppEnv>,                       // required except keygen, wait
    pub database_url: Option<SecretString>,            // required except keygen, wait (app_migrator URL)
    pub supabase_url: Option<Url>,                     // required: invite, delete-user, seed-dev; optional: list-users, deactivate-user
    pub supabase_service_role_key: Option<SecretString>, // same as supabase_url
    pub credentials_public_key: Option<CredentialPublicKey>, // required: add-station, seed-dev
    pub credentials_key_version: Option<i16>,          // required: add-station, seed-dev
    pub awn_application_key: Option<SecretString>,     // optional (add-station validation, local only); never used by seed-dev
    pub awn_mode: Option<AwnMode>,
    pub fixtures_dir: Option<PathBuf>,
    pub seed_dir: PathBuf,                             // SEED_DIR, default /seed (seed-dev only)
}

pub enum AdminCommand {
    Migrate,
    Keygen { check: bool },                            // check reads CREDENTIALS_PRIVATE_KEY / CREDENTIALS_PUBLIC_KEY from the process env
    Wait { minutes: u32 },
    Invite { email: String, display_name: String, follow_all: bool, admin: bool, adopt: bool },
    AddReferenceStation { id: String, name: String, lat: f64, lon: f64, timezone: String, std_offset_minutes: i16, elevation_ft: Option<i32> },
    AddStation { owner: OwnerRef, mac: String, name: String, lat: f64, lon: f64, timezone: String, std_offset_minutes: i16,
                 reference_station: Option<String>, credential: CredentialMode },   // CredentialMode: New | Reuse | Replace
    DeactivateUser { who: UserRef, force: bool },     // UserRef: Id(Uuid) | DisplayName(String)
    DeleteUser { id: Uuid, confirm_email: String },
    ListUsers,
    SeedDev { no_stations: bool },
}
// OwnerRef: Id(Uuid) | DisplayName(String)
```

Conventions:
- Admin API base: `${SUPABASE_URL}/auth/v1/admin/...` (locally Caddy strips `/auth/v1` and forwards to GoTrue). Local GoTrue reached directly (compose test project, `SUPABASE_URL=http://auth:9999`) serves the admin routes at the root, so the client uses base `SUPABASE_URL` when the host is `auth` and `SUPABASE_URL/auth/v1` otherwise. Locally the service key is an HS256 JWT with `role=service_role`, sent as `apikey` and `Authorization: Bearer`. A production `sb_secret_...` key is sent **only** in the `apikey` header, never as a bearer token (00-conventions); a value starting `eyJ` (legacy JWT key) is sent as both.
- Output is human-readable; errors go to stderr without URLs, keys or other users' emails. Exit 0 success, 1 failure, 2 usage error.
- Every mutating command writes `audit_log` rows (actions in 5.9; `actor_id` = the admin's user id, i.e. the single `is_admin` row, else NULL).
- Refuses `APP_ENV=production` unless `DATABASE_URL` contains `sslmode=verify-full`, and refuses (every subcommand, in production) any `CREDENTIALS_PUBLIC_KEY`, `SUPABASE_SERVICE_ROLE_KEY` or `DATABASE_URL` password whose SHA-256 appears in `backend/demo-fingerprints.txt` (embedded by `core::config`).
- `--dry-run` is optional in v1; if implemented, only for `invite` and `add-station`.

### 10.2 Commands

| Command | Purpose |
| --- | --- |
| `admin migrate` | Applies `backend/migrations/` forward (embedded `sqlx::migrate!`); refuses unless `current_user = app_migrator`. Idempotent. |
| `admin keygen [--check]` | Section 9.4. |
| `admin wait --minutes N` | Idles (sleep) so an operator can run commands in an interactive one-off machine (00-conventions). |
| `admin invite --email E --display-name N [--follow-all] [--admin] [--adopt]` | G4/G5. Without `--follow-all` no follows are seeded. `--admin` (settled: kept; it is the only way to create the single admin, refused when an admin already exists) and `--adopt` are additions to the conventions flag list; 00-conventions must add them. |
| `admin add-reference-station --id ID --name N --lat X --lon Y --timezone TZ --std-offset-minutes M [--elevation-ft F]` | Needed before `add-station --reference-station`. |
| `admin add-station --owner UUID\|DISPLAY_NAME --mac M --name N --lat X --lon Y --timezone TZ --std-offset-minutes M [--reference-station ID] [--reuse-credential \| --replace-key]` | Stopgap (S7, G6). API key from hidden prompt / stdin only. |
| `admin deactivate-user --id UUID\|--display-name N [--force]` | S11. |
| `admin delete-user --id UUID --confirm-email E` | S16, hard delete (5.12). |
| `admin list-users` | Read-only. One tab-separated line per `app.users` row: `id`, `display_name`, `email` (from the Admin API; empty when the service key is not configured), `active\|inactive`, `admin\|-`, `auth_ok\|auth_missing` (last two only with the service key). The email is printed for the operator (the smoke script matches probe emails against `auth_only` lines, below); then one `auth_only<TAB>id` line per Supabase auth user that has no `app.users` row. Exit 1 if any `auth_missing` or `auth_only` line exists. After the hosted smoke test the operator runs it, and the check **fails on any `auth_only` line** (an auth user without an `app.users` row, e.g. a probe address that got created). |
| `admin seed-dev [--no-stations]` | Local only; section 12. |

There is deliberately no `--api-key` option anywhere (S7); a test asserts clap rejects it.

### 10.3 `invite` algorithm (all-or-nothing)

**`invite` sends no email.** It creates a confirmed Supabase user and the app rows; the new user then opens the site and requests their own sign-in link (the SPA calls `signInWithOtp` with `shouldCreateUser:false`, so an invited address works and any other does not). 05 and 06 must not promise an invite email; custom SMTP is still required for that sign-in link.

1. Validate: email syntax, `--display-name` 1 to 80 chars. **Reject a duplicate display name** (case/space-insensitive, `app.users_display_name_key`) with exit 1 *before* any Supabase call. If `--admin`, abort if an admin row already exists (G4).
2. Follow targets: with `--follow-all`, every existing `app.users` row with `is_active`; otherwise none.
3. `POST /auth/v1/admin/users` with `{"email": E, "email_confirm": true, "user_metadata": {"display_name": N}}`. Success returns `id`. If the email already exists: abort with "user already exists" unless `--adopt`; with `--adopt`, locate the auth user by paging `GET /admin/users` and continue only if `app.users` has no row for that id (repairs an orphan left by a failed earlier run).
4. One SQL transaction as `app_migrator`: `INSERT INTO app.users (id, display_name, is_admin)`; for each target insert both approved directions into `app.follows` (`decided_at = now()`); insert `audit_log` rows (`user.invited`, one `follow.seeded` per pair). Commit.
5. If step 4 fails: the transaction rolled back; call `DELETE /auth/v1/admin/users/{id}` (skipped when `--adopt` found a pre-existing auth user). If that delete also fails, print `admin invite --email E --display-name N --adopt` as the repair command and exit 1 (state is "auth user exists, no app rows", which `--adopt` repairs).
6. Print the new user's id.

Fault injection for tests: a hidden flag `--test-fail-after-auth`, compiled in only under cargo feature `compose-tests`, makes step 4 fail after step 3. This is the single fault-injection mechanism; do not add others.

### 10.4 `add-station` algorithm

1. Resolve `--owner` (a UUID, or a display name looked up in `app.users`; both DB-only, no Supabase call); require `is_active`.
2. Read the AWN API key without echo: TTY gives `rpassword::prompt_password("AWN API key: ")`; otherwise exactly one line from stdin (tests). Keep it in `SecretString`; reject empty/non-printable.
3. Validation happens only when `APP_ENV=local`, `AWN_MODE=live` and `AWN_APPLICATION_KEY` is set: validate through the core AWN client (`devices`); fail on 401/403; require the MAC in the list; never print the key or any URL. **In production `add-station` never validates** (the admin app has no `AWN_APPLICATION_KEY`; the operator confirms the key through the connect flow or the poller's first run). Otherwise print "key not validated". (`seed-dev` never reaches this step.)
4. Normalise: MAC to upper-case colon form; round `--lat/--lon` to 2 decimals (the column also rounds).
5. One transaction. Look up the owner's live credential (`status <> 'replaced'`):
   - none: new `credential_id`, encrypt (AAD `(credential_id, owner_id)`), `INSERT app.credentials`;
   - exists and neither flag: error;
   - `--reuse-credential`: allowed **only when that credential's status is `ok`** (otherwise error, "reconnect with --replace-key"); skip the prompt and add the station under it;
   - `--replace-key`: the reconnect pattern of 5.3 (mark old `replaced`, insert new row, repoint the owner's stations).
   Then `INSERT app.stations`, `INSERT app.forecast_locations (station_id, lat, lon)`, audit rows (`credential.saved` and, on replace, `credential.replaced`; `station.created`). Commit.
6. Print station id and location id. `stations.is_enabled` defaults to true; disabling a station is manual SQL in v1 (no command).

Never log: the key, URLs, ciphertext.

### 10.5 `deactivate-user` algorithm

1. Look up the user; refuse the admin (`is_admin`) unless `--force`.
2. Transaction: `UPDATE app.users SET is_active=false, deactivated_at=now() WHERE id=$1`; `audit_log` `user.deactivated`. Commit. From this moment the API rejects the user on the next request (the authoritative S11 guarantee).
3. `SELECT app_auth.revoke_sessions($1)` (stops refresh-token use; EXECUTE is granted to `app_migrator`). Failure is an error (exit 1) but step 2 stays committed.
4. Best effort: `PUT /auth/v1/admin/users/{id}` with `{"ban_duration":"876000h"}` when the service key is configured; a failure is a warning only.
5. Idempotent: re-running repeats steps 3 and 4 and reports "already inactive".

### 10.6 `delete-user`

Requires the user to be inactive, `--id` and `--confirm-email` matching the Supabase email (looked up via the Admin API). Transaction: `audit_log` `user.deleted` (actor admin, subject user id, empty detail), `DELETE FROM app.users WHERE id=$1` (cascades per 5.12), commit; then `DELETE /auth/v1/admin/users/{id}`; if that call fails print the manual step. No `--yes` flag exists; the confirmation argument is the safeguard.

### 10.7 How it is run

- Local: the Compose service `admin` under profile `tools` (05): backend image, env per 10.1 (`DATABASE_URL` migrator URL with host `db`, `SUPABASE_URL=http://caddy`), mount `./ops/db/seed:/seed:ro`. Example: `docker compose --profile tools run --rm admin admin invite --email a@example.test --display-name "A" --follow-all`. For `add-station` pipe the key and use `-T`: `printf '%s\n' "$KEY" | docker compose --profile tools run --rm -T admin admin add-station ...`.
- Production: from a one-off machine of the separate Fly app `famweather-admin` (zero machines at rest; secrets `DATABASE_URL` for `app_migrator`, `SUPABASE_URL`, `SUPABASE_SERVICE_ROLE_KEY`, `CREDENTIALS_PUBLIC_KEY`, `CREDENTIALS_KEY_VERSION`; no `AWN_APPLICATION_KEY`), same image: the machine runs `admin wait`, and the operator runs commands with `fly ssh console --pty -C "/usr/local/bin/admin ..."` (00-conventions; `fly machine run` has no `-it`). Fly secrets are app-wide, which is why admin secrets are not in the api or poller apps (S5). The four-app layout is accepted (proposals/02). Agents prepare production commands; Rodney runs them. Migrations run from CI in a protected environment as `admin migrate` with the `app_migrator` URL (06), so that URL exists in the `famweather-admin` app **and** in the protected CI environment.

## 11. Testing

Harness (00-conventions, "Testing harness"): `make test-db` and `make test-backend` run inside Compose after `migrate`. Rust tests use `#[sqlx::test(migrations = false)]` with `core::testdb::prepare` and the **superuser** URL `TEST_SUPERUSER_DATABASE_URL`; grant and exposure assertions run **only** in `ops/db/tests/run.sh`, executed inside the `db` container (`docker compose -p famweather-test ... exec -T db sh /work/ops/db/tests/run.sh`). It runs in the **throwaway test-project** `db` container (tmpfs, torn down afterwards), so it may re-run `roles.sql` and change role passwords freely; the repo is mounted read-only at `/work`, and the script defaults `MIGRATIONS_DIR=/work/backend/migrations` and `OPS_DIR=/work/ops/db` (`: "${MIGRATIONS_DIR:=/work/backend/migrations}"`, same for `OPS_DIR`) and both can be overridden. `run.sh` is POSIX `sh` and needs only `sh` and `psql` (no bash, no python). The container's environment already holds the five `APP_*_PASSWORD_VERIFIER` values (demo verifiers from `.env.example`); plaintext passwords are not available inside `db`, so the real-login test first sets a test password with `ALTER ROLE <role> PASSWORD 'test-only-password'` as superuser (throwaway cluster only). 05 sets `POSTGRES_INITDB_ARGS="--auth-local=trust --auth-host=scram-sha-256"`, so per-role logins connect over TCP with `-h 127.0.0.1` and exercise SCRAM. Production migrations and every test migration run as `app_migrator`.

### 11.1 `ops/db/tests/run.sh`

Uses `psql -X -v ON_ERROR_STOP=1` against a scratch database `dbtest_<random>` (dropped on exit). Steps, in order:

1. **Private cluster for bootstrap tests.** Steps 1 and 1b run on a private throwaway cluster so that "no role exists yet" is true and nothing from the shared test cluster (roles, a `postgres` database with `app` objects) interferes. `run.sh` is invoked as the `postgres` OS user and creates it with `initdb -D /tmp/privcluster --auth-local=trust --auth-host=scram-sha-256` (binaries from `$(pg_config --bindir)`; `pg_ctl -o "-p 55432 -c listen_addresses=''" start`, socket in `/tmp`), removed on exit. On that cluster, with **no** `app_*` roles present:
   - one verifier unset: `roles.sql` exits non-zero and `SELECT count(*) FROM pg_roles WHERE rolname LIKE 'app\_%'` is 0 (genuinely checks that nothing was created before the abort);
   - a plaintext (non-SCRAM) value: non-zero, still 0 roles;
   - a malformed SCRAM value (wrong key length): non-zero, still 0 roles.
1b. **Hosted-shape bootstrap (same private cluster):** create `bootstrap_nonsuper` (`LOGIN CREATEROLE NOSUPERUSER`) with `GRANT CREATE ON DATABASE postgres`, run `roles.sql` **as that login** (it is not a superuser and ends up holding `app_migrator` with `INHERIT FALSE`). It must succeed, the schema must be owned by `app_migrator`, and the attribute assertion must pass; then run migration 0001 as `app_migrator`. Run `roles.sql` a second time as the same login (idempotence). Because the cluster is private and fresh there is nothing to drop; the private cluster is deleted afterwards.
2. Create the scratch database; run `roles.sql` (idempotent; verifiers from the container environment), then assert each role's `pg_authid.rolpassword` equals the verifier from the environment (superuser, local only). Run `$OPS_DIR/tests/auth_stub.sql`, then `$OPS_DIR/bootstrap/auth_access.sql` (the real file, as superuser, against the stubs).
3. As `app_migrator` create a stub `app._sqlx_migrations (version bigint PRIMARY KEY)` (sqlx creates the real one before 0001; this lets 0001's conditional `GRANT SELECT ... TO app_backup` and grant test 16 run), then apply every `$MIGRATIONS_DIR/*.up.sql` in order **as `app_migrator`** (`PGUSER=app_migrator`; real sqlx tracking is covered by `admin migrate` in the compose tests). Dump `pg_dump -s -n app` as `A`.
4. With `PGOPTIONS='-c app.allow_destructive_down=on'` apply every `*.down.sql` in reverse order; assert no relations (except `_sqlx_migrations` if present) and no functions remain in `app`. Re-apply all up files; dump `B`; assert `A == B`.
5. Apply a down file without the setting: assert the "destructive down migration refused" error.
6. Run `grants_test.sql`, `can_view_test.sql`, `data_model_test.sql`, then `exposure_test.sh` (11.4).
7. Real-login test per role over TCP (`psql -h 127.0.0.1`, password `test-only-password` set in 11.1's preface): `SELECT current_user`; `app_api` cannot `SET ROLE app_poller`; `app_ci_check` can connect but `SELECT 1 FROM app.users` fails.

Tests use `SET LOCAL ROLE <role>` inside a rolled-back transaction (privilege checks follow the current role) and these helpers:

```sql
CREATE OR REPLACE FUNCTION pg_temp.expect_denied(stmt text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
  EXECUTE stmt;
  RAISE EXCEPTION 'expected insufficient_privilege but succeeded: %', stmt;
EXCEPTION WHEN insufficient_privilege THEN NULL;
END $$;
CREATE OR REPLACE FUNCTION pg_temp.expect_ok(stmt text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN EXECUTE stmt; END $$;
```

### 11.2 Grant tests (`grants_test.sql`)

**Completeness:** every relation in `app` (`relkind` r/S, including `_sqlx_migrations`) and every function in `app` and `app_auth` has a row in the expectation matrices; a missing row fails the test.

**Tier 1, table-level (`has_table_privilege`)**, for each table and role, the privilege set must equal the matrix exactly (all of `SELECT, INSERT, UPDATE, DELETE, TRUNCATE, REFERENCES, TRIGGER` checked; unlisted means false). Examples: `app_api` has table-level SELECT on `users, follows, stations, forecast_locations, observations, daily_rollups, reference_*, forecast_*, job_runs, connect_attempts, rate_gates` and **not** on `credentials`; table-level `INSERT` on `follows, stations, forecast_locations, connect_attempts, audit_log, rate_gates` (not on `credentials`: its INSERT is column-level, tier 3); `DELETE` only on `connect_attempts`. `app_poller` has table-level SELECT on `credentials`, `stations`, `forecast_locations`, ingest tables, `rollup_dirty_days`, `job_runs`, `rate_gates`; DELETE only on `job_runs` and `rollup_dirty_days`; no privilege on `users` (column-level only), `follows`, `connect_attempts`. `app_backup` has SELECT on **every** `app` table including `app._sqlx_migrations`, nothing else. `app_ci_check`, `anon`, `authenticated`, `service_role`, `PUBLIC`: nothing.

There is no tier 2: any-column checks are redundant with tier 3 for the tables that matter and with the behavioural table for the rest.

**Tier 3, per column (`has_column_privilege`)** for `credentials`, `stations` and `users` (the only tables with column-level grants). For `credentials`:

| Column | `app_api` | `app_poller` |
| --- | --- | --- |
| `id`, `owner_id` | SELECT, INSERT | SELECT |
| `api_key_ciphertext`, `encapsulated_key` | INSERT only | SELECT, UPDATE |
| `key_version` | SELECT, INSERT | SELECT, UPDATE |
| `status`, `status_changed_at` | SELECT, UPDATE | SELECT, UPDATE |
| `last_ok_at` | SELECT | SELECT, UPDATE |
| `created_at`, `updated_at` | SELECT | SELECT |

For `stations`: `app_api` UPDATE only on `name`, `credential_id`; `app_poller` UPDATE only on `last_observed_at`, `backfill_floor_ts`; nobody can UPDATE `owner_id`, `mac`, `lat`, `lon`, `is_enabled`, `reference_station_id`. For `users`: `app_api` SELECT all columns, UPDATE none; `app_poller` SELECT only `id`, `is_active`. `connect_attempts`: `app_api` UPDATE only `outcome`.

**Behavioural cases (as `app_api` unless stated):**

| # | Statement | Expected |
| --- | --- | --- |
| 1 | `SELECT api_key_ciphertext FROM app.credentials` and `SELECT * FROM app.credentials` | denied |
| 2 | `SELECT id, owner_id, key_version, status, last_ok_at FROM app.credentials` | ok |
| 3 | `INSERT ... (id, owner_id, api_key_ciphertext, encapsulated_key, key_version) ... RETURNING id, status` | ok |
| 4 | same INSERT `RETURNING api_key_ciphertext` | denied |
| 5 | `INSERT ... ON CONFLICT (id) DO UPDATE SET api_key_ciphertext = EXCLUDED.api_key_ciphertext` | denied |
| 6 | `INSERT ... ON CONFLICT (id) DO NOTHING` | ok |
| 7 | `UPDATE app.credentials SET status='replaced', status_changed_at=now() WHERE id=... AND owner_id=...` | ok |
| 8 | `UPDATE app.credentials SET api_key_ciphertext=...` / `encapsulated_key` / `key_version` | denied |
| 9 | `UPDATE app.stations SET credential_id=... WHERE owner_id=...` (to a credential of the same owner) | ok; to another owner's credential: fails with `23503` once constraints are checked (`SET CONSTRAINTS ALL IMMEDIATE` or real `COMMIT`) |
| 10 | `DELETE` on credentials, stations, follows, users | denied |
| 11 | `INSERT INTO app.users`, `UPDATE app.users SET is_admin=true` | denied |
| 12 | `UPDATE app.follows SET status='approved', decided_at=now()` | ok; `SET follower_id=...` denied |
| 13 | `INSERT INTO app.observations ...`, `INSERT INTO app.job_runs ...` | denied |
| 14 | `SELECT app_auth.session_active(gen_random_uuid(), gen_random_uuid())` | ok (`false`); `SELECT * FROM auth.sessions`, `SELECT * FROM auth.users`, `SELECT app_auth.revoke_sessions(gen_random_uuid())` | denied |
| 15 | `SELECT app.can_view(...)` | ok; as `app_poller` denied |
| 16 | `SELECT * FROM app._sqlx_migrations` | denied (`app_backup`: ok) |
| 17 | `INSERT INTO app.audit_log ...` ok; `SELECT`, `UPDATE`, `DELETE` on it denied |
| 18 | `INSERT INTO app.connect_attempts (user_id, outcome) VALUES (..., 'started') RETURNING id`; then `UPDATE ... SET outcome='valid'` | ok; `SET user_id=...` denied |
| 19 | the reservation statement from 00-conventions against `app.rate_gates`, as `app_api` and as `app_poller` | ok |

As `app_poller`: `SELECT api_key_ciphertext FROM app.credentials` ok; `UPDATE app.credentials SET status='invalid', status_changed_at=now() WHERE id=...` ok; `UPDATE ... SET api_key_ciphertext=..., encapsulated_key=..., key_version=...` ok (rotation); `UPDATE ... SET owner_id=...` denied; `INSERT INTO app.credentials` denied; observation upsert with `ON CONFLICT (station_id, ts_utc) DO UPDATE SET temp_f = EXCLUDED.temp_f` ok; `UPDATE app.observations SET rain_inc_in=...` ok; `DELETE FROM app.observations` denied; `INSERT INTO app.reference_stations` denied, `UPDATE app.reference_stations SET last_synced_date=...` ok; `INSERT INTO app.forecast_runs/forecast_snapshots` ok, `UPDATE` of them denied; `SELECT * FROM app.users` denied but `SELECT id, is_active FROM app.users` ok; `SELECT app_auth.session_active(...)` denied.

As `app_backup`: SELECT on every `app` table ok (a table created later by `app_migrator` in the test is selectable); any write denied; `auth.*` and `app_auth` denied. As `app_migrator`: can create a table in `app`; cannot `CREATE ROLE`; `SELECT app_auth.revoke_sessions(...)` ok; `SELECT * FROM auth.sessions` denied.

### 11.3 `can_view` and data-model tests

Fixture: users `owner`, `viewer`, `other` (active), `inactive_viewer`; station `S` owned by `owner`. `can_view(viewer, S)` expectations:

| Case | Expected |
| --- | --- |
| viewer is owner | true |
| no follow row | false |
| `pending` | false |
| `approved` | true |
| `declined` | false |
| `revoked` | false |
| approved follow in the opposite direction only (`owner -> viewer`) | false |
| approved but viewer `is_active=false` | false |
| unknown station id | false |
| NULL user or NULL station | false (not NULL) |
| viewer approved on a different owner only | false |
| mutual seeded follows both ways | true both ways |
| transition `approved -> revoked` in the same session | true before UPDATE, false on the very next call (no caching) |
| `visible_station_ids(viewer)` | exactly the stations the matrix above allows |

Constraint tests: self-follow fails (`23514`); `status='pending'` with `decided_at` set fails; `status='approved'` with `decided_at IS NULL` fails; a second `is_admin` user fails (`23505`); `deactivated_at` inconsistent with `is_active` fails; two users whose display names differ only by case or surrounding spaces fail (`23505`).

Data-model tests: `lat = 46.58765` is stored as `46.59`; a second live credential for one owner fails (`23505`) but succeeds after the first is set to `replaced`; a station whose `credential_id` belongs to another owner fails when the deferred constraint is checked (the test runs `SET CONSTRAINTS ALL IMMEDIATE` or issues a real `COMMIT`; inside a rolled-back test transaction a deferred check never fires); `DELETE FROM app.credentials` while a station references it fails (same: `SET CONSTRAINTS ALL IMMEDIATE` or a real `COMMIT`); `DELETE FROM app.users WHERE id = owner` succeeds and leaves zero rows in `credentials`, `stations`, `observations`, `daily_rollups`, `rollup_dirty_days`, `forecast_locations`, `forecast_runs`, `forecast_snapshots`, `follows`, `connect_attempts` for that owner and keeps `audit_log` rows; `app.lst_date('2026-07-01 06:30+00', -420)` is `2026-06-30` and `app.lst_date('2026-07-01 07:00+00', -420)` is `2026-07-01`; duplicate `(station_id, ts_utc)` and duplicate forecast snapshot key are rejected; a snapshot without its `forecast_runs` row is rejected; a snapshot with `valid_time` off a 00/06/12/18 UTC hour or with non-zero minutes/seconds is rejected; `variable` other than `precip_6h_in` is rejected; `job_runs.status='finished'`, `daily_rollups.gdd_method='simple_avg'` and `rain_reconcile_status='bad'` are rejected; `connect_attempts.outcome='started'` is accepted.

### 11.4 Exposure check tests (`exposure_test.sh`)

Run as `app_ci_check` (read-only session) against the scratch database; violating changes are made as superuser in a separate connection and reverted:
1. Baseline after migrations and `auth_access.sql`: exit 0.
2. `CREATE TABLE public.leak (id int)`: non-zero with `OBJECT_IN_PUBLIC` and `EXPOSED_TABLE_WITHOUT_RLS`; with RLS enabled only `OBJECT_IN_PUBLIC`; dropped: 0.
3. `ALTER ROLE authenticator SET pgrst.db_schemas = 'public,app'`: `SCHEMA_EXPOSED`; reset: 0.
4. Each separately: `GRANT SELECT ON app.users TO anon`; `GRANT SELECT (id) ON app.users TO authenticated`; `GRANT EXECUTE ON FUNCTION app.can_view(uuid,uuid) TO PUBLIC`; `GRANT USAGE ON SCHEMA app_auth TO anon`; `GRANT EXECUTE ON FUNCTION app_auth.session_active(uuid,uuid) TO PUBLIC`: `WATCHED_ROLE_PRIVILEGE`; revoke: 0.
5. `GRANT SELECT ON app.credentials TO app_api` or `GRANT UPDATE (key_version) ON app.credentials TO app_api` or `GRANT UPDATE (owner_id) ON app.stations TO app_api`: `API_COLUMN_PRIVILEGE`.
6. `ALTER TABLE app.users OWNER TO postgres`: `RELATION_NOT_OWNED_BY_MIGRATOR`; `ALTER FUNCTION app.can_view(uuid,uuid) OWNER TO postgres`: `FOREIGN_OWNED_FUNCTION`.
7. `CREATE FUNCTION app.evil() ... SECURITY DEFINER`: `SECURITY_DEFINER_OUTSIDE_APP_AUTH`; `CREATE FUNCTION app_auth.extra() ...`: `UNEXPECTED_APP_AUTH_FUNCTION`; `ALTER FUNCTION app_auth.session_active(uuid,uuid) RESET search_path`: `APP_AUTH_FUNCTION_NOT_HARDENED`; `GRANT EXECUTE ON FUNCTION app_auth.revoke_sessions(uuid) TO app_api`: `APP_AUTH_EXECUTE_GRANT`.
8. `GRANT SELECT ON auth.sessions TO app_api` (stub): `APP_ROLE_PRIVILEGE_ON_AUTH`.
9. The check performs no writes: run it with `default_transaction_read_only=on` (a write attempt would error), and a DO-block with a temp table would fail the test (guards against regressions).

### 11.5 `admin` CLI tests against Compose (`backend/crates/admin/tests/`, cargo feature `compose-tests`)

Run via the `test-backend` service (`docker compose up -d db auth mail caddy`, then `migrate`, then `cargo test -p admin --features compose-tests -- --test-threads=1`). Tests invoke the binary with `assert_cmd`. Required environment is the `test-backend` set of 00-conventions' harness: `TEST_MIGRATOR_DATABASE_URL` (passed to the binary as `DATABASE_URL`), `SUPABASE_URL=http://auth:9999` with a service-role JWT minted from `TEST_JWT_HS256_SECRET` as `SUPABASE_SERVICE_ROLE_KEY`, `MAILPIT_URL=http://mail:8025`, and the demo `CREDENTIALS_PUBLIC_KEY` / `CREDENTIALS_PRIVATE_KEY` / `CREDENTIALS_KEY_VERSION`. Tests must be re-runnable on a stack that may contain seed data: every test creates its own users with unique emails (`t-<uuid>@example.test`), display names (`T <uuid>`) and MACs (`AA:BB:CC:<random 3 bytes>`), asserts only on its own rows and on deltas (for `--follow-all`: follows between the new user and every active user that existed before), and cleans up what it created.

| Test | Assertion |
| --- | --- |
| `migrate_env_and_idempotence` | With only `APP_ENV` and `DATABASE_URL` set, `admin migrate` succeeds; a second run is a no-op; a non-`app_migrator` URL exits 1. |
| `keygen_check` | `keygen` output passes `keygen --check` through the environment; a mismatched pair exits 1; `keygen` runs with an empty environment. |
| `invite_creates_user_and_follows` | Users A, B active. `invite --email c@example.test --display-name C --follow-all`: auth user exists, `app.users` row with the same id, 4 approved follows with `decided_at`, audit `user.invited` and 2 `follow.seeded`. No mail arrives in Mailpit. |
| `invite_without_follow_all` | No follows. |
| `invite_duplicate_display_name` | Same name in a different case exits 1 before an auth user is created. |
| `invite_duplicate_email` | Second invite of the same email exits 1 and writes nothing. |
| `invite_second_admin_refused` | `--admin` twice: second exits 1 before any auth call. |
| `invite_rolls_back_on_db_failure` | With the `compose-tests`-only `--test-fail-after-auth`: exit 1, the GoTrue user no longer exists, no `app.users` row. |
| `add_station_encrypts_and_roundtrips` | stdin key `canary-secret-awnkey-0123`: credential row, `key_version` as configured, `encapsulated_key` 32 bytes; `decrypt_credential` (feature `decrypt`, test only) returns the sentinel; swapped `owner_id` fails. No AWN call is made without `AWN_APPLICATION_KEY`. |
| `add_station_secret_not_leaked` | `RUST_LOG=trace`; the sentinel and the ciphertext base64 appear in neither stdout nor stderr; the key is passed only on stdin. The `canary-secret-` prefix is what 05's `ops/local/check-secret-logs.sh` greps for in the test project's `db` logs; that script runs inside `test-backend` **before teardown** (there is no separate `test-secret-logs` target). |
| `add_station_rejects_api_key_flag` | `add-station --api-key x` exits 2. |
| `add_station_reuse_only_when_ok` | `--reuse-credential` on a status-`ok` credential adds a station and leaves the row unchanged; with status `invalid` it exits 1. Neither flag with an existing credential exits 1. |
| `add_station_replace_key` | Old row `replaced` with unchanged ciphertext; new row (new id) `ok` and decrypts with the new AAD; every station of the owner points to the new id; exactly one live credential. |
| `add_station_rounds_coordinates` | `--lat 46.58765` stores `46.59` in `stations` and `forecast_locations`. |
| `add_station_atomic` | Duplicate MAC exits 1; no credential row is created for a new owner. |
| `deactivate_user` | `is_active=false`; `auth.sessions` rows for the user are gone (a real OTP login via Mailpit first); `can_view` false for the user's own station; ban applied when the service key is set (`banned_until` non-null); without the key only a warning. Re-run exits 0 ("already inactive"). |
| `deactivate_admin_refused` | Exits 1 without `--force`. |
| `delete_user_cascades` | After deactivate then delete: dependents gone, `audit_log` kept, auth user deleted; active user exits 1; wrong `--confirm-email` exits 1. |
| `seed_dev_idempotent` | Section 12: first run seeds; second run changes nothing and exits 0; `APP_ENV=production` exits 1; never calls AWN. |
| `production_guard` | `APP_ENV=production` with a non-`verify-full` URL exits 1; with a demo `CREDENTIALS_PUBLIC_KEY`, a demo service key or a demo database password (fingerprints from `backend/demo-fingerprints.txt`) exits 1 for every subcommand. |

Unit tests in `admin`: email validation, MAC normalisation (reject malformed), coordinate rounding, per-subcommand required-env validation, `keygen` round trip via `core::crypto`, and a test that no clap argument named `api-key` exists. Optional (not required): `--dry-run`, and a test for a failing Supabase delete during rollback.

### 11.6 `core::testdb` and the auth stub (owned here)

`core::testdb::prepare(pool)` lives in `backend/crates/core` behind the cargo feature `testdb` (enabled only through dev-dependencies, never in a release binary). It is used as `#[sqlx::test(migrations = false)]` + `prepare`; `#[sqlx::test]` reads the superuser URL from `DATABASE_URL`, which `test-backend` sets (00-conventions; `TEST_SUPERUSER_DATABASE_URL` is the same value). It loads `auth_stub.sql` and `auth_access.sql` **at runtime** from `$OPS_DIR/tests/auth_stub.sql` and `$OPS_DIR/bootstrap/auth_access.sql` (`OPS_DIR` defaults to `/work/ops/db`); it never `include_str!`s files outside `backend/`. It runs, in order, as the superuser: the stub and `auth_access.sql`; then on the same connection `SET ROLE app_migrator; SET search_path = app;` and `sqlx::migrate!("../../migrations")` (so ownership, default privileges and the location of `_sqlx_migrations` match production), then `RESET ROLE`. Roles already exist from `roles.sql` (cluster-level, created at database init). It also provides `core::testdb::pool_as(&db_name, role)`, a pool whose connections run `SET ROLE <role>` (`app_api`, `app_poller`, `app_migrator`) in `after_connect`, so 02/03 repository code is exercised with the real grants. The stub contains `auth.sessions` and `prepare` installs the real `app_auth` functions. Grant behaviour is never *asserted* under `core::testdb` (only `ops/db/tests/run.sh` asserts grants and exposure).

`auth_stub.sql` (idempotent; includes columns the app must **not** be able to reach):

```sql
-- Each role in its own block: parallel #[sqlx::test] databases race on these cluster-level roles.
DO $$ BEGIN CREATE ROLE anon NOLOGIN;          EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$;
DO $$ BEGIN CREATE ROLE authenticated NOLOGIN; EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$;
DO $$ BEGIN CREATE ROLE service_role NOLOGIN;  EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$;
DO $$ BEGIN CREATE ROLE authenticator NOLOGIN; EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$;
CREATE SCHEMA IF NOT EXISTS auth;
CREATE SCHEMA IF NOT EXISTS app AUTHORIZATION app_migrator;
CREATE TABLE IF NOT EXISTS auth.users (id uuid PRIMARY KEY, email text, encrypted_password text,
  raw_user_meta_data jsonb, banned_until timestamptz, created_at timestamptz DEFAULT now());
CREATE TABLE IF NOT EXISTS auth.sessions (id uuid PRIMARY KEY, user_id uuid NOT NULL, not_after timestamptz,
  ip inet, user_agent text, factor_id uuid, created_at timestamptz DEFAULT now());
CREATE TABLE IF NOT EXISTS auth.identities (id uuid PRIMARY KEY, user_id uuid NOT NULL, identity_data jsonb);
CREATE TABLE IF NOT EXISTS auth.refresh_tokens (id bigserial PRIMARY KEY, token text, user_id text, session_id uuid);
```

(In the real compose stack GoTrue creates these tables; the stub exists only for throwaway databases.)

## 12. Seed data and retention

### 12.1 Local seed (`admin seed-dev`, local only)

`admin seed-dev` (`make seed`, 05) refuses unless `APP_ENV=local` and the database host is `db`/`localhost`. It reads `SEED_DIR/seed.toml` and runs everything through the same internal functions as the real commands, over sqlx and the Admin API (no `psql`, no SQL files). It **never validates against AWN** (no AWN client is constructed). Contents must match the 00-conventions Fixtures section exactly:

- **Users** (display name, email, role): `Rodney` `rodney@example.test` (admin), `Alex` `alex@example.test`, `Sam` `sam@example.test`, invited with `--follow-all` semantics so these three are mutually approved (P5); `Pat` `pat@example.test` with **no follows** (the stranger); `Dana` `dana@example.test` (security-state follows, below); `Casey` `casey@example.test`, invited and then deactivated (the inactive user).
- **Reference stations**: ids `USW00099001` (Helena), `USW00099002` (Laramie), `USW00099003` (Loveland); fake or approximate names, coordinates and offsets; no real airport data claimed. (Real ids are USW00024144 and USW00024022; Loveland's baseline is undecided.) 03's ACIS fixtures are stored under these ids.
- **Stations** (unless `--no-stations`): MACs `AA:BB:CC:00:00:01`, `...:02`, `...:03` owned by Rodney (Helena), Alex (Laramie) and Sam (Loveland), timezone `America/Denver` (`-420`), each with its reference station; every station keeps a baseline. One credential per owner, key `fixture-poller-key` encrypted to the local `CREDENTIALS_PUBLIC_KEY`, so it decrypts with the local private key and matches 03's `awn/stations/<MAC>/` fixtures. The connect-flow keys (`fixture-valid-key` and the others) are never stored, and the MACs `AA:BB:CC:00:01:01/02` are not seeded.
- **Follow states** (in `seed.toml` as `[[follow]]` rows with `follower`, `followee`, `status`): Dana -> Alex `pending`; Dana -> Sam `declined`; Dana -> Rodney `revoked` (inserted approved, then set revoked, so the history is realistic). Pat has none.

Re-running skips anything that already exists (users by display name, stations by MAC, follows by pair) and exits 0; it changes nothing on the second run (tested). The seed uses throwaway values from the local `.env`.

No observations, rollups or `reference_daily` rows are seeded directly: they come from the poller in `AWN_MODE=replay` and `reference_sync` replay (05/03), so seed and fixtures cannot drift.

### 12.2 Retention

| Data | Retention |
| --- | --- |
| `observations`, `daily_rollups`, `reference_*` | Keep (small). |
| `forecast_runs`, `forecast_snapshots` | Keep indefinitely in v1 (cannot be rebuilt, F1). Revisit when the size alert fires. |
| `job_runs` | Recommend 90 days; poller deletes older rows daily (`app_poller` has DELETE). |
| `connect_attempts` | Recommend 7 days; API deletes older rows opportunistically. |
| `rate_gates` | Never pruned by app roles (tiny); operator may prune rows for replaced credentials. |
| `audit_log` | Recommend keep 2 years; deletion is a manual migrator operation (no app role can delete). |
| Backups | 06. |

These retention numbers are recommendations, not decisions.

## 13. Implementation tasks

Order matters. Each has a done-when.

0. **Hosted verification (operator, before relying on `app_auth`).** Run the scratch-project checklist in `docs/proposals/03-supabase-auth-schema-access.md`. Required before the first production bootstrap. Done when its results are recorded in this guide (6.5) and `app_auth` is confirmed (the option (c) downgrade applies only if the check fails).
1. **Bootstrap SQL and helpers.** Write `ops/db/bootstrap/roles.sql`, `scram.py`, `auth_access.sql`, and `ops/db/tests/auth_stub.sql`. Done when running `roles.sql`, the stub and `auth_access.sql` twice against an empty `postgres:17` succeeds both times, `\du app_*` lists five non-superuser roles, `roles.sql` aborts with an unset or malformed verifier, and `roles.sql` also succeeds when run as a non-superuser `CREATEROLE` login (11.1 step 1b).
2. **Migration 0001 and `admin migrate`.** Done when `admin migrate` as `app_migrator` (with only `APP_ENV` and `DATABASE_URL`) succeeds, fails as any other role, `app._sqlx_migrations` exists and is SELECT-able by `app_backup`, and `public` has no tables.
3. **Migrations 0002 to 0010.** Done when all apply in order with the SQL of section 5 and the down files revert them (11.1 step 4).
4. **`ops/db/tests/run.sh` and the SQL tests** (11.1 to 11.4). Done when all pass and each deliberately broken grant (revoke a needed grant, grant SELECT on ciphertext, grant UPDATE on `key_version`) makes the right test fail.
5. **`exposure.sql`, `exposure.sh`, `size.sql`, `supabase_smoke.sh`.** Done when 11.4 passes and `make test-db` runs `exposure.sh` as `app_ci_check` after migrations.
6. **`core::crypto` and `core::testdb`.** Done when the crypto unit tests of 9.3 pass and `#[sqlx::test(migrations = false)]` + `prepare` yields a migrated database for 02/03 tests.
7. **`admin` scaffold**: clap subcommands (including `migrate`, `wait`, `keygen --check`, `seed-dev`), per-subcommand env validation, Admin API client with URL scrubbing, audit helper. Done when `admin --help` lists all commands, `--api-key` is rejected and the env table of 10.1 is enforced.
8. **`admin invite`** with rollback. Done when the invite tests of 11.5 pass.
9. **`admin add-reference-station`, `add-station`.** Done when the add-station tests pass including the leak test.
10. **`admin deactivate-user`, `delete-user`, `list-users`.** Done when their tests pass.
11. **`admin seed-dev` and `ops/db/seed/seed.toml`.** Done when `docker compose up` then `make seed` yields 6 users (one inactive), 3 stations, the expected follow statuses, and a second `make seed` exits 0 changing nothing.
12. **Hosted Supabase checklist** (operator, with 06): apply 8.1, run `roles.sql`, `auth_access.sql`, migrations, `exposure.sh`, `supabase_smoke.sh`, `size.sql`. Done when all exit 0.

## 14. Security checklist

- [ ] `app_api` cannot `SELECT` either ciphertext column and cannot `UPDATE` any ciphertext column, `key_version`, `owner_id` or `id` (grant tests 1, 8; exposure check 7). The residual compromised-API risk in 5.3 is documented, not denied.
- [ ] No `ON CONFLICT DO UPDATE` and no `RETURNING *` against `credentials` anywhere in `api`.
- [ ] `anon`, `authenticated`, `service_role`, `PUBLIC`, `app_ci_check` hold no privilege in `app` or `app_auth`; `app` not in `pgrst.db_schemas`; Data API disabled; smoke test passes.
- [ ] `app_auth` contains exactly `session_active` and `revoke_sessions`, both `SECURITY DEFINER` with `search_path = ''`, EXECUTE only for `app_api` / `app_migrator`; no app role has any privilege on `auth.*`.
- [ ] Sign-ups and anonymous sign-ins off (smoke steps 3, 4); `shouldCreateUser:false` in the SPA (01).
- [ ] No tables or functions in `public`; `_sqlx_migrations` in `app`.
- [ ] No app role has SUPERUSER, REPLICATION or BYPASSRLS (`roles.sql` asserts it from `pg_roles`); role passwords never appear as SQL text or argv (SCRAM verifiers only); `sslmode=verify-full` in production URLs.
- [ ] `SUPABASE_SERVICE_ROLE_KEY` exists only in `famweather-admin`. The `app_migrator` URL exists only in `famweather-admin` **and** the protected CI environment, never in `famweather-api` or `famweather-poller`.
- [ ] A station's credential always belongs to the station's owner (composite FK test).
- [ ] No coordinates with more than 2 decimals are persisted for stations or locations.
- [ ] `admin` has no argv option for the AWN key; the sentinel leak test passes for stdout, stderr, logs, Postgres logs; `seed-dev` makes no AWN call.
- [ ] `audit_log` has no UPDATE/DELETE grants; `detail` never contains keys, emails or ciphertext; audit actions use the dotted list in 5.9 only.
- [ ] `down` migrations refuse to run without `app.allow_destructive_down`.
- [ ] Private keys are never printed except by `admin keygen`; production refuses the committed local public key.
- [ ] Dashboard MFA enabled on Supabase (and Fly, Netlify, AWS) (S16).

## 15. Open questions / assumptions

1. **One live credential per user** (partial unique index `credentials_one_live_per_owner`): consistent with G6 but a product decision; a user with two Ambient accounts (the open Laramie/Mesa question in the design doc) needs it relaxed and `admin add-station` extended.
2. **Rounded coordinates stored** (2 decimals, `numeric(5,2)`): proposal under S10; confirm no feature needs exact coordinates.
3. **Follow transitions**: allowed transitions (declined or revoked back to pending? who can revoke?) are enforced by 02, not the DB.
4. **Inactive owner visibility**: an inactive owner's stations stay visible to followers (not hidden). `deactivate-user` does not set `stations.is_enabled=false`, so the poller keeps ingesting; disabling a station is manual SQL in v1.
5. **GDD method** (`avg_50` vs `clamp_50_86`) and the day-boundary semantics of an observation timestamp are 03/design decisions; the schema accepts both methods.
6. **Timezone determination** from coordinates for a new station is unspecified (02; `add-station` takes it as an argument).
7. **Who populates `reference_stations`**: only `admin add-reference-station` (the poller cannot insert). The Loveland airport is unconfirmed, so no real rows are seeded.
8. **`app_auth` on hosted Supabase** is accepted but unverified until task 0; option (c) in proposal 03 is the fallback only if task 0 step 3 fails. The FK to `auth.users` is dropped by decision (optional upgrade only if the checklist's FK step succeeds cheaply; not designed in).
9. **Auth users in backups**: `app_backup` reads only `app`; whether to capture `auth.users` (a definer function returning `(id, email)`) or to rebuild logins by re-inviting is the owner's open choice (proposal 03).
10. **Service key header form** for `sb_secret_` keys on `/auth/v1/admin/*` must be confirmed against the live gateway.
11. **Sign-in email delivery**: `invite` sends no email; the user requests a magic link. Requires a custom SMTP provider (undecided) or no family member can receive links.
12. **Login audit**: logins stay in GoTrue's `auth.audit_log_entries` (no app role reads it); copying them into `app.audit_log` is undecided (02 shares this).
13. **`reactivate-user`** is not implemented (manual SQL plus clearing the ban).
14. **Retention numbers**, role `statement_timeout`s and connection limits are recommendations.
15. **Production admin execution** uses the separate `famweather-admin` Fly app (proposals/02, accepted).
16. **Replaced credentials** keep their ciphertext until an operator deletes them (no app role can); decide a purge rule (for example 30 days) and who runs it.
17. **Replaced-row and status tampering** by a compromised API is accepted and only detectable (5.3); decide whether an alert on unexpected `credential.saved` outside business hours is worth building.
