-- Exposure check (04 section 8.4). Run as app_ci_check in a read-only session; fails by raising.
-- Uses no ::regclass casts and no to_regclass on app/auth objects (they need schema USAGE, which app_ci_check lacks):
-- relations and functions are identified by OID from pg_class / pg_proc joined to pg_namespace.
DO $$
DECLARE v text;
BEGIN
  WITH
  pgrst AS (  -- best effort: on hosted Supabase PostgREST's live schema list can differ from this role setting
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
  -- Privilege functions raise on the wrong relkind, and the planner may evaluate them before the relkind/name
  -- filters: they only ever see these pre-filtered, materialized sets.
  rel_t AS MATERIALIZED (SELECT * FROM rel WHERE relkind IN ('r','p','v','m')),
  rel_s AS MATERIALIZED (SELECT * FROM rel WHERE relkind = 'S'),
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
    FROM watched w, rel_t r
    WHERE r.nspname IN ('app','app_auth')
      AND (has_table_privilege(w.role_name, r.relid, 'SELECT,INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER')
           OR has_any_column_privilege(w.role_name, r.relid, 'SELECT,INSERT,UPDATE,REFERENCES'))
    UNION ALL
    SELECT 'WATCHED_ROLE_PRIVILEGE', w.role_name || ' on sequence ' || r.relname
    FROM watched w, rel_s r
    WHERE r.nspname IN ('app','app_auth') AND has_sequence_privilege(w.role_name, r.relid, 'USAGE,SELECT,UPDATE')
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
    UNION ALL  -- 6 SECURITY DEFINER: none in app/public; app_auth holds exactly the three expected functions
    SELECT 'SECURITY_DEFINER_OUTSIDE_APP_AUTH', nspname || '.' || proname FROM fn
    WHERE prosecdef AND nspname IN ('app','public')
    UNION ALL
    SELECT 'UNEXPECTED_APP_AUTH_FUNCTION', proname FROM fn
    WHERE nspname = 'app_auth' AND proname NOT IN ('session_active','revoke_sessions','backup_users')
    UNION ALL
    SELECT 'APP_AUTH_FUNCTION_NOT_HARDENED', proname FROM fn
    WHERE nspname = 'app_auth'
      AND (NOT prosecdef OR NOT EXISTS (SELECT 1 FROM unnest(coalesce(proconfig, '{}')) c WHERE c ~ '^search_path=("")?$'))
    UNION ALL  -- 6b EXECUTE on the definer functions is exactly app_api / app_migrator / app_backup
    SELECT 'APP_AUTH_EXECUTE_GRANT', r.rolname || ' on ' || f.proname
    FROM fn f, pg_roles r
    WHERE f.nspname = 'app_auth' AND r.rolname IN ('app_api','app_migrator','app_poller','app_backup')
      AND has_function_privilege(r.rolname, f.fnid, 'EXECUTE')
      AND NOT ((f.proname = 'session_active' AND r.rolname = 'app_api')
            OR (f.proname = 'revoke_sessions' AND r.rolname = 'app_migrator')
            OR (f.proname = 'backup_users' AND r.rolname = 'app_backup'))
    UNION ALL  -- 7 app_api must not be able to read or modify key material, ownership or identity columns
    SELECT 'API_COLUMN_PRIVILEGE', x.tbl || '.' || x.col || ' ' || x.priv
    FROM rel_t r JOIN (VALUES
      ('credentials','api_key_ciphertext','SELECT'), ('credentials','encapsulated_key','SELECT'),
      ('credentials','api_key_ciphertext','UPDATE'), ('credentials','encapsulated_key','UPDATE'),
      ('credentials','key_version','UPDATE'), ('credentials','owner_id','UPDATE'), ('credentials','id','UPDATE'),
      ('stations','owner_id','UPDATE'), ('stations','mac','UPDATE'), ('stations','lat','UPDATE'), ('stations','lon','UPDATE'),
      ('users','is_active','UPDATE'), ('users','is_admin','UPDATE')
    ) x(tbl, col, priv) ON r.nspname = 'app' AND r.relname = x.tbl
    WHERE has_column_privilege('app_api', r.relid, x.col, x.priv)   -- OIDs from the catalog: no regclass casts (they need schema USAGE)
    UNION ALL  -- 8 no app role holds any privilege on GoTrue tables
    SELECT 'APP_ROLE_PRIVILEGE_ON_AUTH', r.rolname || ' on auth.' || a.relname
    FROM rel_t a, pg_roles r
    WHERE a.nspname = 'auth' AND a.relname IN ('sessions','users','refresh_tokens')
      AND r.rolname IN ('app_api','app_poller','app_migrator','app_backup','app_ci_check')
      AND (has_table_privilege(r.rolname, a.relid, 'SELECT,INSERT,UPDATE,DELETE') OR has_any_column_privilege(r.rolname, a.relid, 'SELECT,INSERT,UPDATE'))
  )
  SELECT string_agg(check_name || ': ' || object, E'\n' ORDER BY check_name, object) INTO v FROM violations;

  IF v IS NOT NULL THEN
    RAISE EXCEPTION E'exposure check failed:\n%', v;
  END IF;
END $$;
