-- Creates the five application roles and schema app (04 section 6.2). Idempotent.
-- Run as the admin login (postgres): psql -X -v ON_ERROR_STOP=1 -f roles.sql <connection string>
-- Reads SCRAM-SHA-256 verifiers from the environment (\getenv, never argv); aborts before creating anything
-- if one is unset or malformed. Compute a verifier: printf '%s\n' "$PASSWORD" | python3 ops/db/bootstrap/scram.py
\set ON_ERROR_STOP on
\getenv migrator_vf APP_MIGRATOR_PASSWORD_VERIFIER
\getenv api_vf      APP_API_PASSWORD_VERIFIER
\getenv poller_vf   APP_POLLER_PASSWORD_VERIFIER
\getenv backup_vf   APP_BACKUP_PASSWORD_VERIFIER
\getenv ci_check_vf APP_CI_CHECK_PASSWORD_VERIFIER

-- Abort when any variable is unset (\getenv leaves it undefined).
\if :{?migrator_vf}
\else
  \echo 'APP_MIGRATOR_PASSWORD_VERIFIER is not set'
  SELECT 1/0;
\endif
\if :{?api_vf}
\else
  \echo 'APP_API_PASSWORD_VERIFIER is not set'
  SELECT 1/0;
\endif
\if :{?poller_vf}
\else
  \echo 'APP_POLLER_PASSWORD_VERIFIER is not set'
  SELECT 1/0;
\endif
\if :{?backup_vf}
\else
  \echo 'APP_BACKUP_PASSWORD_VERIFIER is not set'
  SELECT 1/0;
\endif
\if :{?ci_check_vf}
\else
  \echo 'APP_CI_CHECK_PASSWORD_VERIFIER is not set'
  SELECT 1/0;
\endif

-- Abort when any value is not a well-formed SCRAM-SHA-256 verifier (16-byte salt = 22 base64 chars + ==; StoredKey and ServerKey = 43 chars + =).
SELECT bool_or(v !~ '^SCRAM-SHA-256\$[0-9]+:[A-Za-z0-9+/]{22}==\$[A-Za-z0-9+/]{43}=:[A-Za-z0-9+/]{43}=$') AS bad
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
-- check (ops/db/checks/exposure.sql) proves that anon/authenticated/service_role/PUBLIC hold nothing in app.
