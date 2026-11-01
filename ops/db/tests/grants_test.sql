-- Grant tests (04 section 11.2). Run as superuser against a scratch database that has had roles.sql, auth_stub.sql,
-- auth_access.sql, a stub app._sqlx_migrations and every migration applied. Fails (non-zero exit) on the first problem.
\set ON_ERROR_STOP on

CREATE FUNCTION pg_temp.expect_denied(stmt text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
  EXECUTE stmt;
  RAISE EXCEPTION 'expected insufficient_privilege but succeeded: %', stmt;
EXCEPTION WHEN insufficient_privilege THEN NULL;
END $$;
CREATE FUNCTION pg_temp.expect_ok(stmt text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN EXECUTE stmt; END $$;
-- Runs stmt, then forces deferred constraints, and requires exactly the given SQLSTATE.
CREATE FUNCTION pg_temp.expect_sqlstate(stmt text, expected text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
  EXECUTE stmt;
  SET CONSTRAINTS ALL IMMEDIATE;
  RAISE EXCEPTION 'expected SQLSTATE % but succeeded: %', expected, stmt USING ERRCODE = 'XX001';
EXCEPTION WHEN OTHERS THEN
  IF SQLSTATE <> expected THEN
    RAISE EXCEPTION 'expected SQLSTATE %, got % (%): %', expected, SQLSTATE, SQLERRM, stmt;
  END IF;
END $$;

-- ---------------------------------------------------------------------------------------------------------------
-- Expectation matrices. A relation or function with no row here fails the completeness checks.
CREATE TEMP TABLE exp_table (tbl text PRIMARY KEY, api text NOT NULL, poller text NOT NULL, backup text NOT NULL);
INSERT INTO exp_table VALUES
  ('users',              'SELECT',               '',                         'SELECT'),
  ('follows',            'SELECT,INSERT',        '',                         'SELECT'),
  ('credentials',        '',                     'SELECT',                   'SELECT'),
  ('reference_stations', 'SELECT',               'SELECT',                   'SELECT'),
  ('reference_daily',    'SELECT',               'SELECT,INSERT,UPDATE',     'SELECT'),
  ('stations',           'SELECT,INSERT',        'SELECT',                   'SELECT'),
  ('observations',       'SELECT',               'SELECT,INSERT,UPDATE',     'SELECT'),
  ('daily_rollups',      'SELECT',               'SELECT,INSERT,UPDATE',     'SELECT'),
  ('rollup_dirty_days',  '',                     'SELECT,INSERT,DELETE',     'SELECT'),
  ('job_runs',           'SELECT',               'SELECT,INSERT,UPDATE,DELETE', 'SELECT'),
  ('audit_log',          'INSERT',               'INSERT',                   'SELECT'),
  ('connect_attempts',   'SELECT,INSERT,DELETE', '',                         'SELECT'),
  ('rate_gates',         'SELECT,INSERT,UPDATE', 'SELECT,INSERT,UPDATE',     'SELECT'),
  ('_sqlx_migrations',   '',                     '',                         'SELECT');

-- Column-level grants on top of the table-level ones above: (table, role, privilege, columns).
CREATE TEMP TABLE exp_col (tbl text, role_name text, priv text, cols text[]);
INSERT INTO exp_col VALUES
  ('users', 'app_poller', 'SELECT', ARRAY['id','is_active']),
  ('follows', 'app_api', 'UPDATE', ARRAY['status','decided_at']),
  ('credentials', 'app_api', 'SELECT', ARRAY['id','owner_id','key_version','status','last_ok_at','status_changed_at','created_at','updated_at']),
  ('credentials', 'app_api', 'INSERT', ARRAY['id','owner_id','api_key_ciphertext','encapsulated_key','key_version']),
  ('credentials', 'app_api', 'UPDATE', ARRAY['status','status_changed_at']),
  ('credentials', 'app_poller', 'UPDATE', ARRAY['status','status_changed_at','last_ok_at','api_key_ciphertext','encapsulated_key','key_version']),
  ('reference_stations', 'app_api', 'INSERT', ARRAY['id','name','lat','lon','elevation_ft','timezone','std_utc_offset_minutes']),
  ('reference_stations', 'app_poller', 'UPDATE', ARRAY['last_synced_date']),
  ('stations', 'app_api', 'UPDATE', ARRAY['name','credential_id','reference_station_id']),
  ('stations', 'app_poller', 'UPDATE', ARRAY['last_observed_at','backfill_floor_ts']),
  ('rollup_dirty_days', 'app_poller', 'UPDATE', ARRAY['local_date']),
  ('connect_attempts', 'app_api', 'UPDATE', ARRAY['outcome']);

-- Sequences: (name, role, privileges)
CREATE TEMP TABLE exp_seq (seq text, role_name text, privs text);
INSERT INTO exp_seq VALUES
  ('audit_log_id_seq', 'app_api', 'USAGE'), ('audit_log_id_seq', 'app_poller', 'USAGE'), ('audit_log_id_seq', 'app_backup', 'SELECT'),
  ('connect_attempts_id_seq', 'app_api', 'USAGE'), ('connect_attempts_id_seq', 'app_backup', 'SELECT');

-- Functions: (schema, name, roles that hold EXECUTE; the owner is ignored for app.* functions)
CREATE TEMP TABLE exp_fn (nsp text, fname text, roles text[]);
INSERT INTO exp_fn VALUES
  ('app', 'touch_updated_at', ARRAY[]::text[]),
  ('app', 'lst_date', ARRAY['app_api','app_poller']),
  ('app', 'gdd_day', ARRAY['app_api']),
  ('app', 'can_view', ARRAY['app_api']),
  ('app', 'visible_station_ids', ARRAY['app_api']),
  ('app_auth', 'session_active', ARRAY['app_api']),
  ('app_auth', 'revoke_sessions', ARRAY['app_migrator']),
  ('app_auth', 'backup_users', ARRAY['app_backup']);

-- ---------------------------------------------------------------------------------------------------------------
-- Completeness and tier 1 (table-level) and per-column checks.
DO $$
DECLARE
  rec record; r text; p text; problems text := '';
  exp_set text[]; act_set text[];
  tbl_privs text[] := ARRAY['SELECT','INSERT','UPDATE','DELETE','TRUNCATE','REFERENCES','TRIGGER'];
  watched text[] := ARRAY['app_ci_check','anon','authenticated','service_role','public'];
BEGIN
  -- completeness: relations
  FOR rec IN
    SELECT c.relname FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE n.nspname = 'app' AND c.relkind IN ('r','p') AND c.relname NOT IN (SELECT tbl FROM exp_table)
  LOOP problems := problems || format(E'\nno expectation row for table app.%s', rec.relname); END LOOP;
  FOR rec IN SELECT tbl FROM exp_table WHERE to_regclass('app.' || quote_ident(tbl)) IS NULL
  LOOP problems := problems || format(E'\nexpectation row for missing table app.%s', rec.tbl); END LOOP;
  FOR rec IN
    SELECT c.relname FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE n.nspname = 'app' AND c.relkind = 'S' AND c.relname NOT IN (SELECT seq FROM exp_seq)
  LOOP problems := problems || format(E'\nno expectation row for sequence app.%s', rec.relname); END LOOP;
  -- completeness: functions
  FOR rec IN
    SELECT n.nspname, p.proname FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
    WHERE n.nspname IN ('app','app_auth')
      AND NOT EXISTS (SELECT 1 FROM exp_fn e WHERE e.nsp = n.nspname AND e.fname = p.proname)
  LOOP problems := problems || format(E'\nno expectation row for function %s.%s', rec.nspname, rec.proname); END LOOP;

  -- tier 1: table-level privileges, exact
  FOR rec IN SELECT e.*, to_regclass('app.' || quote_ident(e.tbl)) AS relid FROM exp_table e WHERE to_regclass('app.' || quote_ident(e.tbl)) IS NOT NULL LOOP
    FOREACH r IN ARRAY ARRAY['app_api','app_poller','app_backup'] LOOP
      exp_set := ARRAY(SELECT x FROM unnest(string_to_array(nullif(CASE r WHEN 'app_api' THEN rec.api WHEN 'app_poller' THEN rec.poller ELSE rec.backup END, ''), ',')) x ORDER BY x);
      act_set := ARRAY(SELECT x FROM unnest(tbl_privs) x WHERE has_table_privilege(r, rec.relid, x) ORDER BY x);
      IF exp_set IS DISTINCT FROM act_set THEN
        problems := problems || format(E'\ntable-level app.%s %s: expected %s, got %s', rec.tbl, r, coalesce(exp_set::text, '{}'), act_set);
      END IF;
    END LOOP;
    FOREACH r IN ARRAY watched LOOP
      act_set := ARRAY(SELECT x FROM unnest(tbl_privs) x WHERE has_table_privilege(r, rec.relid, x));
      IF cardinality(act_set) > 0 OR has_any_column_privilege(r, rec.relid, 'SELECT,INSERT,UPDATE,REFERENCES') THEN
        problems := problems || format(E'\napp.%s: %s must hold nothing', rec.tbl, r);
      END IF;
    END LOOP;

    -- tier 3: every column, SELECT/INSERT/UPDATE/REFERENCES, expected = table-level OR column extras
    FOR p IN SELECT unnest(ARRAY['SELECT','INSERT','UPDATE','REFERENCES']) LOOP
      FOREACH r IN ARRAY ARRAY['app_api','app_poller','app_backup'] LOOP
        FOR exp_set IN
          SELECT ARRAY[a.attname::text,
                       (   coalesce(p = ANY (string_to_array(nullif(CASE r WHEN 'app_api' THEN rec.api WHEN 'app_poller' THEN rec.poller ELSE rec.backup END, ''), ',')), false)
                        OR EXISTS (SELECT 1 FROM exp_col c WHERE c.tbl = rec.tbl AND c.role_name = r AND c.priv = p AND a.attname::text = ANY (c.cols))
                       )::text,
                       has_column_privilege(r, rec.relid, a.attname, p)::text]
          FROM pg_attribute a WHERE a.attrelid = rec.relid AND a.attnum > 0 AND NOT a.attisdropped
        LOOP
          IF exp_set[2] IS DISTINCT FROM exp_set[3] THEN
            problems := problems || format(E'\ncolumn app.%s.%s %s %s: expected %s, got %s', rec.tbl, exp_set[1], r, p, exp_set[2], exp_set[3]);
          END IF;
        END LOOP;
      END LOOP;
    END LOOP;
  END LOOP;

  -- sequences
  FOR rec IN SELECT c.oid AS relid, c.relname FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = 'app' AND c.relkind = 'S' LOOP
    FOREACH r IN ARRAY ARRAY['app_api','app_poller','app_backup'] LOOP
      exp_set := ARRAY(SELECT x FROM unnest(string_to_array(nullif((SELECT privs FROM exp_seq WHERE seq = rec.relname AND role_name = r), ''), ',')) x ORDER BY x);
      act_set := ARRAY(SELECT x FROM unnest(ARRAY['USAGE','SELECT','UPDATE']) x WHERE has_sequence_privilege(r, rec.relid, x) ORDER BY x);
      -- a USAGE grant implies nothing about SELECT, and vice versa; compare as sets of explicit grants
      IF exp_set IS DISTINCT FROM act_set THEN
        problems := problems || format(E'\nsequence app.%s %s: expected %s, got %s', rec.relname, r, coalesce(exp_set::text, '{}'), act_set);
      END IF;
    END LOOP;
    FOREACH r IN ARRAY watched LOOP
      IF has_sequence_privilege(r, rec.relid, 'USAGE,SELECT,UPDATE') THEN problems := problems || format(E'\nsequence app.%s: %s must hold nothing', rec.relname, r); END IF;
    END LOOP;
  END LOOP;

  -- functions
  FOR rec IN SELECT p.oid AS fnid, n.nspname, p.proname, e.roles
             FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace JOIN exp_fn e ON e.nsp = n.nspname AND e.fname = p.proname LOOP
    FOREACH r IN ARRAY ARRAY['app_api','app_poller','app_backup','app_migrator'] || watched LOOP
      CONTINUE WHEN r = 'app_migrator' AND rec.nspname = 'app';   -- owner
      IF has_function_privilege(r, rec.fnid, 'EXECUTE') <> (r = ANY (rec.roles)) THEN
        problems := problems || format(E'\nfunction %s.%s EXECUTE for %s: expected %s', rec.nspname, rec.proname, r, r = ANY (rec.roles));
      END IF;
    END LOOP;
  END LOOP;

  IF problems <> '' THEN RAISE EXCEPTION 'grant matrix failures:%', problems; END IF;
END $$;

-- ---------------------------------------------------------------------------------------------------------------
-- auth.* matrix: no app role holds any privilege on any GoTrue table (they reach it only through app_auth).
DO $$
DECLARE rec record; r text; problems text := '';
BEGIN
  FOR rec IN SELECT c.oid AS relid, c.relname FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
             WHERE n.nspname = 'auth' AND c.relkind IN ('r','p','v','m') LOOP
    FOREACH r IN ARRAY ARRAY['app_api','app_poller','app_backup','app_migrator','app_ci_check'] LOOP
      IF has_table_privilege(r, rec.relid, 'SELECT,INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER')
         OR has_any_column_privilege(r, rec.relid, 'SELECT,INSERT,UPDATE,REFERENCES') THEN
        problems := problems || format(E'\n%s holds a privilege on auth.%s', r, rec.relname);
      END IF;
    END LOOP;
  END LOOP;
  IF problems <> '' THEN RAISE EXCEPTION 'auth.* matrix failures:%', problems; END IF;
END $$;

-- ---------------------------------------------------------------------------------------------------------------
-- Behavioural cases. Fixture is built as superuser inside a transaction that is rolled back at the end.
BEGIN;
INSERT INTO app.users (id, display_name) VALUES
  ('11111111-1111-1111-1111-111111111111', 'Owner One'),
  ('22222222-2222-2222-2222-222222222222', 'Owner Two'),
  ('33333333-3333-3333-3333-333333333333', 'Owner Three');
INSERT INTO app.credentials (id, owner_id, api_key_ciphertext, encapsulated_key, key_version) VALUES
  ('c1111111-1111-1111-1111-111111111111', '11111111-1111-1111-1111-111111111111', decode(repeat('ab', 40), 'hex'), decode(repeat('cd', 32), 'hex'), 1),
  ('c2222222-2222-2222-2222-222222222222', '22222222-2222-2222-2222-222222222222', decode(repeat('ab', 40), 'hex'), decode(repeat('cd', 32), 'hex'), 1);
INSERT INTO app.stations (id, mac, name, owner_id, credential_id, lat, lon, timezone, std_utc_offset_minutes) VALUES
  ('51111111-1111-1111-1111-111111111111', 'AA:BB:CC:00:09:01', 'S one', '11111111-1111-1111-1111-111111111111', 'c1111111-1111-1111-1111-111111111111', 40.40, -105.00, 'America/Denver', -420);
INSERT INTO app.reference_stations (id, name, lat, lon, timezone, std_utc_offset_minutes) VALUES ('USW00099001', 'Ref', 46.6, -112.0, 'America/Denver', -420);
INSERT INTO app.follows (follower_id, followee_id) VALUES ('22222222-2222-2222-2222-222222222222', '11111111-1111-1111-1111-111111111111');
INSERT INTO app.observations (station_id, ts_utc, temp_f) VALUES ('51111111-1111-1111-1111-111111111111', '2026-01-01T00:00:00Z', 20);
INSERT INTO auth.users (id, email, encrypted_password, raw_user_meta_data)
  VALUES ('99999999-9999-9999-9999-999999999999', 'stub@example.test', 'HASH-MUST-NOT-LEAK', '{"secret":"META-MUST-NOT-LEAK"}');
INSERT INTO auth.sessions (id, user_id) VALUES ('88888888-8888-8888-8888-888888888888', '99999999-9999-9999-9999-999999999999');

-- ===== as app_api =====
SET LOCAL ROLE app_api;
SELECT pg_temp.expect_denied($q$SELECT api_key_ciphertext FROM app.credentials$q$);                                      -- 1
SELECT pg_temp.expect_denied($q$SELECT * FROM app.credentials$q$);
SELECT pg_temp.expect_ok($q$SELECT id, owner_id, key_version, status, last_ok_at FROM app.credentials$q$);               -- 2
SELECT pg_temp.expect_ok($q$INSERT INTO app.credentials (id, owner_id, api_key_ciphertext, encapsulated_key, key_version)
  VALUES ('c3333333-3333-3333-3333-333333333333', '33333333-3333-3333-3333-333333333333', decode(repeat('ab', 40), 'hex'), decode(repeat('cd', 32), 'hex'), 1)
  RETURNING id, status$q$);                                                                                              -- 3
SELECT pg_temp.expect_denied($q$INSERT INTO app.credentials (id, owner_id, api_key_ciphertext, encapsulated_key, key_version)
  VALUES ('c4444444-4444-4444-4444-444444444444', '33333333-3333-3333-3333-333333333333', decode(repeat('ab', 40), 'hex'), decode(repeat('cd', 32), 'hex'), 1)
  RETURNING api_key_ciphertext$q$);                                                                                      -- 4
SELECT pg_temp.expect_denied($q$INSERT INTO app.credentials (id, owner_id, api_key_ciphertext, encapsulated_key, key_version)
  VALUES ('c1111111-1111-1111-1111-111111111111', '11111111-1111-1111-1111-111111111111', decode(repeat('ab', 40), 'hex'), decode(repeat('cd', 32), 'hex'), 1)
  ON CONFLICT (id) DO UPDATE SET api_key_ciphertext = EXCLUDED.api_key_ciphertext$q$);                                   -- 5
SELECT pg_temp.expect_ok($q$INSERT INTO app.credentials (id, owner_id, api_key_ciphertext, encapsulated_key, key_version)
  VALUES ('c1111111-1111-1111-1111-111111111111', '11111111-1111-1111-1111-111111111111', decode(repeat('ab', 40), 'hex'), decode(repeat('cd', 32), 'hex'), 1)
  ON CONFLICT (id) DO NOTHING$q$);                                                                                       -- 6
SELECT pg_temp.expect_ok($q$UPDATE app.credentials SET status = 'replaced', status_changed_at = now()
  WHERE id = 'c3333333-3333-3333-3333-333333333333' AND owner_id = '33333333-3333-3333-3333-333333333333'$q$);           -- 7
SELECT pg_temp.expect_denied($q$UPDATE app.credentials SET api_key_ciphertext = decode(repeat('ab', 40), 'hex')$q$);      -- 8
SELECT pg_temp.expect_denied($q$UPDATE app.credentials SET encapsulated_key = decode(repeat('cd', 32), 'hex')$q$);
SELECT pg_temp.expect_denied($q$UPDATE app.credentials SET key_version = 2$q$);
SELECT pg_temp.expect_ok($q$UPDATE app.stations SET credential_id = 'c1111111-1111-1111-1111-111111111111'
  WHERE owner_id = '11111111-1111-1111-1111-111111111111'$q$);                                                           -- 9
SELECT pg_temp.expect_sqlstate($q$UPDATE app.stations SET credential_id = 'c2222222-2222-2222-2222-222222222222'
  WHERE owner_id = '11111111-1111-1111-1111-111111111111'$q$, '23503');
SELECT pg_temp.expect_denied($q$DELETE FROM app.credentials$q$);                                                         -- 10
SELECT pg_temp.expect_denied($q$DELETE FROM app.stations$q$);
SELECT pg_temp.expect_denied($q$DELETE FROM app.follows$q$);
SELECT pg_temp.expect_denied($q$DELETE FROM app.users$q$);
SELECT pg_temp.expect_denied($q$INSERT INTO app.users (id, display_name) VALUES (gen_random_uuid(), 'X')$q$);            -- 11
SELECT pg_temp.expect_denied($q$UPDATE app.users SET is_admin = true$q$);
SELECT pg_temp.expect_ok($q$UPDATE app.follows SET status = 'approved', decided_at = now()
  WHERE follower_id = '22222222-2222-2222-2222-222222222222'$q$);                                                        -- 12
SELECT pg_temp.expect_denied($q$UPDATE app.follows SET follower_id = '11111111-1111-1111-1111-111111111111'$q$);
SELECT pg_temp.expect_denied($q$INSERT INTO app.observations (station_id, ts_utc) VALUES ('51111111-1111-1111-1111-111111111111', now())$q$);  -- 13
SELECT pg_temp.expect_denied($q$INSERT INTO app.job_runs (job) VALUES ('awn_ingest')$q$);
SELECT pg_temp.expect_ok($q$SELECT app_auth.session_active(gen_random_uuid(), gen_random_uuid())$q$);                    -- 14
SELECT pg_temp.expect_denied($q$SELECT * FROM auth.sessions$q$);
SELECT pg_temp.expect_denied($q$SELECT * FROM auth.users$q$);
SELECT pg_temp.expect_denied($q$SELECT app_auth.revoke_sessions(gen_random_uuid())$q$);
SELECT pg_temp.expect_denied($q$SELECT * FROM app_auth.backup_users()$q$);
SELECT pg_temp.expect_ok($q$SELECT app.can_view('22222222-2222-2222-2222-222222222222', '51111111-1111-1111-1111-111111111111')$q$);   -- 15
SELECT pg_temp.expect_denied($q$SELECT * FROM app._sqlx_migrations$q$);                                                  -- 16
SELECT pg_temp.expect_ok($q$INSERT INTO app.audit_log (action, subject_type) VALUES ('follow.requested', 'follow')$q$);   -- 17
SELECT pg_temp.expect_denied($q$SELECT * FROM app.audit_log$q$);
SELECT pg_temp.expect_denied($q$UPDATE app.audit_log SET detail = '{}'$q$);
SELECT pg_temp.expect_denied($q$DELETE FROM app.audit_log$q$);
SELECT pg_temp.expect_ok($q$INSERT INTO app.connect_attempts (user_id, outcome) VALUES ('11111111-1111-1111-1111-111111111111', 'started') RETURNING id$q$);  -- 18
SELECT pg_temp.expect_ok($q$UPDATE app.connect_attempts SET outcome = 'valid'$q$);
SELECT pg_temp.expect_denied($q$UPDATE app.connect_attempts SET user_id = '22222222-2222-2222-2222-222222222222'$q$);
-- 19: the reservation statement from 00-conventions
SELECT pg_temp.expect_ok($q$INSERT INTO app.rate_gates (name, next_allowed_at) VALUES ('awn:app:api', now() + interval '1 second')
  ON CONFLICT (name) DO UPDATE
    SET next_allowed_at = greatest(app.rate_gates.next_allowed_at, now()) + interval '1 second'
    WHERE greatest(app.rate_gates.next_allowed_at, now()) <= now() + interval '3 seconds'
  RETURNING greatest(next_allowed_at - interval '1 second' - now(), interval '0') AS wait$q$);
-- baseline picker
SELECT pg_temp.expect_ok($q$INSERT INTO app.reference_stations (id, name, lat, lon, timezone, std_utc_offset_minutes)
  VALUES ('USW00099002', 'Picked', 40.4, -105.0, 'America/Denver', -420) ON CONFLICT DO NOTHING$q$);
SELECT pg_temp.expect_denied($q$INSERT INTO app.reference_stations (id, name, lat, lon, timezone, std_utc_offset_minutes, last_synced_date)
  VALUES ('USW00099003', 'Picked', 40.4, -105.0, 'America/Denver', -420, '2026-01-01')$q$);
SELECT pg_temp.expect_denied($q$UPDATE app.reference_stations SET name = 'x'$q$);
SELECT pg_temp.expect_ok($q$UPDATE app.stations SET reference_station_id = 'USW00099001' WHERE id = '51111111-1111-1111-1111-111111111111'$q$);
SELECT pg_temp.expect_ok($q$SELECT app.gdd_day(95::real, 60::real, 'simple_avg', 50::real)$q$);
SELECT pg_temp.expect_ok($q$SELECT app.visible_station_ids('11111111-1111-1111-1111-111111111111')$q$);

-- ===== as app_poller =====
RESET ROLE; SET LOCAL ROLE app_poller;
SELECT pg_temp.expect_ok($q$SELECT api_key_ciphertext FROM app.credentials$q$);
SELECT pg_temp.expect_ok($q$UPDATE app.credentials SET status = 'invalid', status_changed_at = now() WHERE id = 'c2222222-2222-2222-2222-222222222222'$q$);
SELECT pg_temp.expect_ok($q$UPDATE app.credentials SET api_key_ciphertext = decode(repeat('ee', 40), 'hex'), encapsulated_key = decode(repeat('ff', 32), 'hex'), key_version = 2
  WHERE id = 'c2222222-2222-2222-2222-222222222222'$q$);
SELECT pg_temp.expect_denied($q$UPDATE app.credentials SET owner_id = '11111111-1111-1111-1111-111111111111'$q$);
SELECT pg_temp.expect_denied($q$INSERT INTO app.credentials (id, owner_id, api_key_ciphertext, encapsulated_key, key_version)
  VALUES (gen_random_uuid(), '11111111-1111-1111-1111-111111111111', decode(repeat('ab', 40), 'hex'), decode(repeat('cd', 32), 'hex'), 1)$q$);
SELECT pg_temp.expect_ok($q$INSERT INTO app.observations (station_id, ts_utc, temp_f) VALUES ('51111111-1111-1111-1111-111111111111', '2026-01-01T00:00:00Z', 21)
  ON CONFLICT (station_id, ts_utc) DO UPDATE SET temp_f = EXCLUDED.temp_f$q$);
SELECT pg_temp.expect_ok($q$UPDATE app.observations SET rain_inc_in = 0.01$q$);
SELECT pg_temp.expect_denied($q$DELETE FROM app.observations$q$);
SELECT pg_temp.expect_denied($q$INSERT INTO app.reference_stations (id, name, lat, lon, timezone, std_utc_offset_minutes)
  VALUES ('USW00099004', 'No', 40.4, -105.0, 'America/Denver', -420)$q$);
SELECT pg_temp.expect_ok($q$UPDATE app.reference_stations SET last_synced_date = '2026-01-01'$q$);
SELECT pg_temp.expect_denied($q$SELECT * FROM app.users$q$);
SELECT pg_temp.expect_ok($q$SELECT id, is_active FROM app.users$q$);
SELECT pg_temp.expect_denied($q$SELECT app_auth.session_active(gen_random_uuid(), gen_random_uuid())$q$);
SELECT pg_temp.expect_denied($q$SELECT app.can_view('22222222-2222-2222-2222-222222222222', '51111111-1111-1111-1111-111111111111')$q$);
SELECT pg_temp.expect_denied($q$SELECT app.gdd_day(95::real, 60::real, 'simple_avg', 50::real)$q$);
SELECT pg_temp.expect_denied($q$SELECT * FROM app_auth.backup_users()$q$);
SELECT pg_temp.expect_denied($q$SELECT * FROM app._sqlx_migrations$q$);
SELECT pg_temp.expect_ok($q$INSERT INTO app.audit_log (action, subject_type) VALUES ('credential.invalid', 'credential')$q$);
SELECT pg_temp.expect_denied($q$SELECT * FROM app.audit_log$q$);
SELECT pg_temp.expect_ok($q$INSERT INTO app.rate_gates (name, next_allowed_at) VALUES ('awn:app:poller', now() + interval '500 milliseconds')
  ON CONFLICT (name) DO UPDATE
    SET next_allowed_at = greatest(app.rate_gates.next_allowed_at, now()) + interval '500 milliseconds'
    WHERE greatest(app.rate_gates.next_allowed_at, now()) <= now() + interval '3 seconds'
  RETURNING greatest(next_allowed_at - interval '500 milliseconds' - now(), interval '0') AS wait$q$);
SELECT pg_temp.expect_ok($q$INSERT INTO app.rollup_dirty_days (station_id, local_date) VALUES ('51111111-1111-1111-1111-111111111111', '2026-01-01')
  ON CONFLICT (station_id, local_date) DO UPDATE SET local_date = EXCLUDED.local_date$q$);
SELECT pg_temp.expect_ok($q$INSERT INTO app.job_runs (job) VALUES ('awn_ingest')$q$);
SELECT pg_temp.expect_denied($q$SELECT * FROM app.follows$q$);
SELECT pg_temp.expect_denied($q$SELECT * FROM app.connect_attempts$q$);
SELECT pg_temp.expect_denied($q$UPDATE app.stations SET is_enabled = false$q$);
SELECT pg_temp.expect_ok($q$UPDATE app.stations SET last_observed_at = now(), backfill_floor_ts = now()$q$);

-- ===== as app_backup =====
RESET ROLE; SET LOCAL ROLE app_migrator;
SELECT pg_temp.expect_ok($q$CREATE TABLE app.zz_probe (id int)$q$);
RESET ROLE; SET LOCAL ROLE app_backup;
SELECT pg_temp.expect_ok($q$SELECT * FROM app.zz_probe$q$);                                   -- default privileges cover later tables
SELECT pg_temp.expect_ok($q$SELECT * FROM app.users$q$);
SELECT pg_temp.expect_ok($q$SELECT * FROM app._sqlx_migrations$q$);
SELECT pg_temp.expect_denied($q$INSERT INTO app.users (id, display_name) VALUES (gen_random_uuid(), 'X')$q$);
SELECT pg_temp.expect_denied($q$UPDATE app.stations SET name = 'x'$q$);
SELECT pg_temp.expect_denied($q$DELETE FROM app.audit_log$q$);
SELECT pg_temp.expect_denied($q$SELECT * FROM auth.users$q$);
SELECT pg_temp.expect_denied($q$SELECT * FROM auth.sessions$q$);
SELECT pg_temp.expect_ok($q$SELECT id, email, created_at FROM app_auth.backup_users()$q$);
SELECT pg_temp.expect_denied($q$SELECT app_auth.session_active(gen_random_uuid(), gen_random_uuid())$q$);
SELECT pg_temp.expect_denied($q$SELECT app_auth.revoke_sessions(gen_random_uuid())$q$);

-- ===== as app_migrator =====
RESET ROLE; SET LOCAL ROLE app_migrator;
SELECT pg_temp.expect_ok($q$CREATE TABLE app.zz_probe2 (id int)$q$);
SELECT pg_temp.expect_denied($q$CREATE ROLE zz_probe_role$q$);
SELECT pg_temp.expect_ok($q$SELECT app_auth.revoke_sessions(gen_random_uuid())$q$);
SELECT pg_temp.expect_denied($q$SELECT * FROM auth.sessions$q$);
SELECT pg_temp.expect_denied($q$SELECT * FROM app_auth.backup_users()$q$);
SELECT pg_temp.expect_denied($q$SELECT app_auth.session_active(gen_random_uuid(), gen_random_uuid())$q$);

-- ===== as app_ci_check / anon / authenticated / service_role: nothing =====
RESET ROLE; SET LOCAL ROLE app_ci_check;
SELECT pg_temp.expect_denied($q$SELECT 1 FROM app.users$q$);
SELECT pg_temp.expect_denied($q$SELECT * FROM app_auth.backup_users()$q$);
RESET ROLE; SET LOCAL ROLE anon;
SELECT pg_temp.expect_denied($q$SELECT 1 FROM app.users$q$);
RESET ROLE; SET LOCAL ROLE authenticated;
SELECT pg_temp.expect_denied($q$SELECT 1 FROM app.users$q$);
RESET ROLE; SET LOCAL ROLE service_role;
SELECT pg_temp.expect_denied($q$SELECT 1 FROM app.users$q$);

-- ===== backup_users returns exactly three columns and no secrets =====
RESET ROLE;
DO $$
DECLARE cols text; leaked int;
BEGIN
  SELECT pg_get_function_result('app_auth.backup_users()'::regprocedure) INTO cols;
  IF cols <> 'TABLE(id uuid, email text, created_at timestamp with time zone)' THEN
    RAISE EXCEPTION 'backup_users() result shape changed: %', cols;
  END IF;
  SELECT count(*) INTO leaked FROM app_auth.backup_users() b WHERE b::text LIKE '%HASH-MUST-NOT-LEAK%' OR b::text LIKE '%META-MUST-NOT-LEAK%';
  IF leaked <> 0 THEN RAISE EXCEPTION 'backup_users() leaked secret columns'; END IF;
  IF NOT EXISTS (SELECT 1 FROM app_auth.backup_users() WHERE email = 'stub@example.test') THEN
    RAISE EXCEPTION 'backup_users() did not return the stub user';
  END IF;
END $$;

ROLLBACK;
\echo 'grants_test: OK'
