-- Schema app_auth: the only way app roles touch GoTrue's data (04 section 6.5, proposals/03). Idempotent.
-- Plain SQL (no psql meta-commands): psql -X -v ON_ERROR_STOP=1 -f auth_access.sql, as postgres, after GoTrue
-- has migrated auth. Locally run by the auth-access one-shot; in production once by the operator.
CREATE SCHEMA IF NOT EXISTS app_auth;
REVOKE ALL ON SCHEMA app_auth FROM PUBLIC;
GRANT USAGE ON SCHEMA app_auth TO app_api, app_migrator, app_backup;

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

-- Used by admin deactivate-user and reset-link (S11). Returns the number of sessions removed.
CREATE OR REPLACE FUNCTION app_auth.revoke_sessions(p_user_id uuid)
RETURNS integer LANGUAGE sql SECURITY DEFINER SET search_path = '' AS
$$
  WITH d AS (DELETE FROM auth.sessions s WHERE s.user_id = p_user_id RETURNING 1)
  SELECT pg_catalog.count(*)::integer FROM d
$$;

-- Backup of logins (owner decision, Oct 7): exactly what a restore needs and nothing else. No password hashes, tokens, metadata or
-- sessions. Used only by the backup job, which writes the result into the encrypted backup.
CREATE OR REPLACE FUNCTION app_auth.backup_users()
RETURNS TABLE (id uuid, email text, created_at timestamptz) LANGUAGE sql STABLE SECURITY DEFINER SET search_path = '' AS
$$
  SELECT u.id, u.email::text, u.created_at
  FROM auth.users u
  WHERE u.deleted_at IS NULL AND u.email IS NOT NULL
  ORDER BY u.created_at, u.id
$$;

REVOKE ALL ON FUNCTION app_auth.session_active(uuid, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION app_auth.revoke_sessions(uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION app_auth.backup_users() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION app_auth.session_active(uuid, uuid) TO app_api;
GRANT EXECUTE ON FUNCTION app_auth.revoke_sessions(uuid) TO app_migrator;
GRANT EXECUTE ON FUNCTION app_auth.backup_users() TO app_backup;

-- Self-checks: fail the script if a GoTrue table or column used above was renamed or is unreadable.
DO $$ BEGIN
  PERFORM app_auth.session_active(pg_catalog.gen_random_uuid(), pg_catalog.gen_random_uuid());
  PERFORM app_auth.revoke_sessions(pg_catalog.gen_random_uuid());
  PERFORM 1 FROM app_auth.backup_users() LIMIT 1;
END $$;
