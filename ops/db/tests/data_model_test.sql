-- Constraint and data-model tests (04 section 11.3). Superuser, scratch database, rolled back.
-- Deferred constraints never fire inside a rolled-back transaction, so the tests force them with SET CONSTRAINTS ALL IMMEDIATE.
\set ON_ERROR_STOP on

-- Runs stmt (and forces deferred constraints); requires exactly this SQLSTATE.
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
CREATE FUNCTION pg_temp.expect_ok(stmt text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN EXECUTE stmt; END $$;
CREATE FUNCTION pg_temp.assert_true(label text, cond boolean) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF cond IS NOT TRUE THEN RAISE EXCEPTION 'assertion failed: %', label; END IF; END $$;

BEGIN;
INSERT INTO app.users (id, display_name, is_admin) VALUES
  ('b0000000-0000-0000-0000-000000000001', 'Alpha', true),
  ('b0000000-0000-0000-0000-000000000002', 'Beta', false);
INSERT INTO app.credentials (id, owner_id, api_key_ciphertext, encapsulated_key, key_version) VALUES
  ('c0000000-0000-0000-0000-0000000000a1', 'b0000000-0000-0000-0000-000000000001', decode(repeat('ab', 40), 'hex'), decode(repeat('cd', 32), 'hex'), 1),
  ('c0000000-0000-0000-0000-0000000000b1', 'b0000000-0000-0000-0000-000000000002', decode(repeat('ab', 40), 'hex'), decode(repeat('cd', 32), 'hex'), 1);

-- ---- users / follows constraints
SELECT pg_temp.expect_sqlstate($q$INSERT INTO app.users (id, display_name, is_admin) VALUES (gen_random_uuid(), 'Second Admin', true)$q$, '23505');
SELECT pg_temp.expect_sqlstate($q$INSERT INTO app.users (id, display_name, is_active) VALUES (gen_random_uuid(), 'Inconsistent', false)$q$, '23514');
SELECT pg_temp.expect_sqlstate($q$INSERT INTO app.users (id, display_name, is_active, deactivated_at) VALUES (gen_random_uuid(), 'Inconsistent2', true, now())$q$, '23514');
SELECT pg_temp.expect_sqlstate($q$INSERT INTO app.users (id, display_name) VALUES (gen_random_uuid(), ' ALPHA ')$q$, '23505');
SELECT pg_temp.expect_sqlstate($q$INSERT INTO app.users (id, display_name) VALUES (gen_random_uuid(), 'beta')$q$, '23505');
SELECT pg_temp.expect_sqlstate($q$INSERT INTO app.users (id, display_name) VALUES (gen_random_uuid(), '   ')$q$, '23514');
SELECT pg_temp.expect_sqlstate($q$INSERT INTO app.follows (follower_id, followee_id) VALUES ('b0000000-0000-0000-0000-000000000001', 'b0000000-0000-0000-0000-000000000001')$q$, '23514');
SELECT pg_temp.expect_sqlstate($q$INSERT INTO app.follows (follower_id, followee_id, status, decided_at) VALUES ('b0000000-0000-0000-0000-000000000001', 'b0000000-0000-0000-0000-000000000002', 'pending', now())$q$, '23514');
SELECT pg_temp.expect_sqlstate($q$INSERT INTO app.follows (follower_id, followee_id, status) VALUES ('b0000000-0000-0000-0000-000000000001', 'b0000000-0000-0000-0000-000000000002', 'approved')$q$, '23514');
SELECT pg_temp.expect_sqlstate($q$INSERT INTO app.follows (follower_id, followee_id, status, decided_at) VALUES ('b0000000-0000-0000-0000-000000000001', 'b0000000-0000-0000-0000-000000000002', 'bogus', now())$q$, '23514');

-- ---- credentials: one live credential per owner; allowed again after the first is replaced
SELECT pg_temp.expect_sqlstate($q$INSERT INTO app.credentials (id, owner_id, api_key_ciphertext, encapsulated_key, key_version)
  VALUES (gen_random_uuid(), 'b0000000-0000-0000-0000-000000000001', decode(repeat('ab', 40), 'hex'), decode(repeat('cd', 32), 'hex'), 1)$q$, '23505');
UPDATE app.credentials SET status = 'replaced' WHERE id = 'c0000000-0000-0000-0000-0000000000a1';
SELECT pg_temp.expect_ok($q$INSERT INTO app.credentials (id, owner_id, api_key_ciphertext, encapsulated_key, key_version)
  VALUES ('c0000000-0000-0000-0000-0000000000a2', 'b0000000-0000-0000-0000-000000000001', decode(repeat('ab', 40), 'hex'), decode(repeat('cd', 32), 'hex'), 1)$q$);
SELECT pg_temp.expect_sqlstate($q$INSERT INTO app.credentials (id, owner_id, api_key_ciphertext, encapsulated_key, key_version)
  VALUES (gen_random_uuid(), 'b0000000-0000-0000-0000-000000000002', decode('ab', 'hex'), decode(repeat('cd', 32), 'hex'), 1)$q$, '23514');   -- ciphertext too short
SELECT pg_temp.expect_sqlstate($q$INSERT INTO app.credentials (id, owner_id, api_key_ciphertext, encapsulated_key, key_version)
  VALUES (gen_random_uuid(), 'b0000000-0000-0000-0000-000000000002', decode(repeat('ab', 40), 'hex'), decode(repeat('cd', 31), 'hex'), 1)$q$, '23514');   -- encapsulated key not 32 bytes
UPDATE app.credentials SET status = 'replaced' WHERE id = 'c0000000-0000-0000-0000-0000000000a2';
UPDATE app.credentials SET status = 'ok' WHERE id = 'c0000000-0000-0000-0000-0000000000a1';

-- ---- stations: coordinates are stored rounded to 2 decimals
INSERT INTO app.stations (id, mac, name, owner_id, credential_id, lat, lon, timezone, std_utc_offset_minutes) VALUES
  ('50000000-0000-0000-0000-0000000000a1', 'AA:BB:CC:00:07:01', 'Alpha station', 'b0000000-0000-0000-0000-000000000001',
   'c0000000-0000-0000-0000-0000000000a1', 46.58765, -112.03999, 'America/Denver', -420);
SELECT pg_temp.assert_true('lat 46.58765 stored as 46.59',
  (SELECT lat = 46.59 AND lon = -112.04 FROM app.stations WHERE id = '50000000-0000-0000-0000-0000000000a1'));
SELECT pg_temp.expect_sqlstate($q$INSERT INTO app.stations (mac, name, owner_id, credential_id, lat, lon, timezone, std_utc_offset_minutes)
  VALUES ('aa:bb:cc:00:07:02', 'bad mac', 'b0000000-0000-0000-0000-000000000001', 'c0000000-0000-0000-0000-0000000000a1', 40, -105, 'America/Denver', -420)$q$, '23514');
-- a station whose credential belongs to another owner fails when the deferred FK is checked
SELECT pg_temp.expect_sqlstate($q$INSERT INTO app.stations (mac, name, owner_id, credential_id, lat, lon, timezone, std_utc_offset_minutes)
  VALUES ('AA:BB:CC:00:07:03', 'wrong owner', 'b0000000-0000-0000-0000-000000000001', 'c0000000-0000-0000-0000-0000000000b1', 40, -105, 'America/Denver', -420)$q$, '23503');
-- deleting a credential that a station still references fails (at commit / SET CONSTRAINTS)
SELECT pg_temp.expect_sqlstate($q$DELETE FROM app.credentials WHERE id = 'c0000000-0000-0000-0000-0000000000a1'$q$, '23503');
SELECT pg_temp.expect_sqlstate($q$INSERT INTO app.stations (mac, name, owner_id, credential_id, lat, lon, timezone, std_utc_offset_minutes, reference_station_id)
  VALUES ('AA:BB:CC:00:07:04', 'no ref', 'b0000000-0000-0000-0000-000000000001', 'c0000000-0000-0000-0000-0000000000a1', 40, -105, 'America/Denver', -420, 'USW00000000')$q$, '23503');

-- ---- observations / rollups / ops tables
INSERT INTO app.observations (station_id, ts_utc, temp_f) VALUES ('50000000-0000-0000-0000-0000000000a1', '2026-01-01T00:00:00Z', 20);
SELECT pg_temp.expect_sqlstate($q$INSERT INTO app.observations (station_id, ts_utc, temp_f) VALUES ('50000000-0000-0000-0000-0000000000a1', '2026-01-01T00:00:00Z', 21)$q$, '23505');
SELECT pg_temp.expect_sqlstate($q$INSERT INTO app.observations (station_id, ts_utc, humidity_pct) VALUES ('50000000-0000-0000-0000-0000000000a1', '2026-01-01T00:05:00Z', 101)$q$, '23514');
INSERT INTO app.daily_rollups (station_id, local_date, rain_in) VALUES ('50000000-0000-0000-0000-0000000000a1', '2026-01-01', 0.12);
INSERT INTO app.rollup_dirty_days (station_id, local_date) VALUES ('50000000-0000-0000-0000-0000000000a1', '2026-01-01');
SELECT pg_temp.expect_sqlstate($q$INSERT INTO app.job_runs (job, status) VALUES ('awn_ingest', 'finished')$q$, '23514');
SELECT pg_temp.expect_sqlstate($q$INSERT INTO app.daily_rollups (station_id, local_date, rain_reconcile_status) VALUES ('50000000-0000-0000-0000-0000000000a1', '2026-01-02', 'bad')$q$, '23514');
SELECT pg_temp.expect_ok($q$INSERT INTO app.connect_attempts (user_id, outcome) VALUES ('b0000000-0000-0000-0000-000000000001', 'started')$q$);
SELECT pg_temp.expect_sqlstate($q$INSERT INTO app.connect_attempts (user_id, outcome) VALUES ('b0000000-0000-0000-0000-000000000001', 'bogus')$q$, '23514');
SELECT pg_temp.expect_sqlstate($q$INSERT INTO app.audit_log (action, subject_type) VALUES ('Follow.Requested', 'follow')$q$, '23514');       -- must be lower-case dotted (the old underscore forms are a convention, not a constraint)
SELECT pg_temp.expect_sqlstate($q$INSERT INTO app.audit_log (action, subject_type) VALUES ('follow.requested', 'forecast')$q$, '23514');
SELECT pg_temp.expect_ok($q$INSERT INTO app.audit_log (action, subject_type, subject_id, detail)
  VALUES ('follow.requested', 'follow', 'b0000000-0000-0000-0000-000000000002', '{"followee_id":"b0000000-0000-0000-0000-000000000001"}')$q$);
INSERT INTO app.audit_log (actor_id, action, subject_type, subject_id) VALUES ('b0000000-0000-0000-0000-000000000001', 'user.invited', 'user', 'b0000000-0000-0000-0000-000000000001');
INSERT INTO app.follows (follower_id, followee_id, status, decided_at) VALUES ('b0000000-0000-0000-0000-000000000002', 'b0000000-0000-0000-0000-000000000001', 'approved', now());

-- ---- deleting a user cascades everywhere except audit_log
DELETE FROM app.users WHERE id = 'b0000000-0000-0000-0000-000000000001';
SET CONSTRAINTS ALL IMMEDIATE;
SELECT pg_temp.assert_true('delete user cascades',
  (SELECT count(*) FROM app.credentials WHERE owner_id = 'b0000000-0000-0000-0000-000000000001') = 0
  AND (SELECT count(*) FROM app.stations WHERE owner_id = 'b0000000-0000-0000-0000-000000000001') = 0
  AND (SELECT count(*) FROM app.observations WHERE station_id = '50000000-0000-0000-0000-0000000000a1') = 0
  AND (SELECT count(*) FROM app.daily_rollups WHERE station_id = '50000000-0000-0000-0000-0000000000a1') = 0
  AND (SELECT count(*) FROM app.rollup_dirty_days WHERE station_id = '50000000-0000-0000-0000-0000000000a1') = 0
  AND (SELECT count(*) FROM app.follows WHERE follower_id = 'b0000000-0000-0000-0000-000000000001' OR followee_id = 'b0000000-0000-0000-0000-000000000001') = 0
  AND (SELECT count(*) FROM app.connect_attempts WHERE user_id = 'b0000000-0000-0000-0000-000000000001') = 0);
SELECT pg_temp.assert_true('audit_log rows survive user deletion',
  (SELECT count(*) FROM app.audit_log WHERE actor_id = 'b0000000-0000-0000-0000-000000000001') = 1);

-- ---- app.lst_date
SELECT pg_temp.assert_true('lst_date 06:30Z at -420 is the previous day', app.lst_date('2026-07-01 06:30+00', -420) = DATE '2026-06-30');
SELECT pg_temp.assert_true('lst_date 07:00Z at -420 is the same day', app.lst_date('2026-07-01 07:00+00', -420) = DATE '2026-07-01');

-- ---- app.gdd_day vectors
SELECT pg_temp.assert_true('gdd (95,60) simple = 27.5', app.gdd_day(95, 60, 'simple_avg', 50) = 27.5);
SELECT pg_temp.assert_true('gdd (95,60) clamped = 23.0', app.gdd_day(95, 60, 'clamped_50_86', 50) = 23.0);
SELECT pg_temp.assert_true('gdd (45,30) simple = 0', app.gdd_day(45, 30, 'simple_avg', 50) = 0);
SELECT pg_temp.assert_true('gdd (45,30) clamped = 0', app.gdd_day(45, 30, 'clamped_50_86', 50) = 0);
SELECT pg_temp.assert_true('gdd (70,40) simple = 5.0', app.gdd_day(70, 40, 'simple_avg', 50) = 5.0);
SELECT pg_temp.assert_true('gdd (70,40) clamped = 10.0', app.gdd_day(70, 40, 'clamped_50_86', 50) = 10.0);
SELECT pg_temp.assert_true('gdd (70,40) base 45 simple = 10.0', app.gdd_day(70, 40, 'simple_avg', 45) = 10.0);
SELECT pg_temp.assert_true('gdd (70,40) base 45 clamped = 12.5', app.gdd_day(70, 40, 'clamped_50_86', 45) = 12.5);
SELECT pg_temp.assert_true('gdd NULL tmax', app.gdd_day(NULL, 40, 'simple_avg', 50) IS NULL);
SELECT pg_temp.assert_true('gdd NULL tmin', app.gdd_day(70, NULL, 'clamped_50_86', 50) IS NULL);
SELECT pg_temp.assert_true('gdd unknown method', app.gdd_day(70, 40, 'bogus', 50) IS NULL);

-- ---- catalog checks
SELECT pg_temp.assert_true('users_display_name_prefix_idx exists and is partial on is_active',
  EXISTS (SELECT 1 FROM pg_indexes WHERE schemaname = 'app' AND indexname = 'users_display_name_prefix_idx'
          AND indexdef LIKE '%text_pattern_ops%' AND indexdef LIKE '%WHERE is_active%'));
SET LOCAL enable_seqscan = off;
DO $$
DECLARE plan text := ''; line text;
BEGIN
  FOR line IN EXECUTE $e$EXPLAIN SELECT id FROM app.users WHERE is_active AND lower(btrim(display_name)) LIKE 'al%'$e$ LOOP
    plan := plan || line || E'\n';
  END LOOP;
  IF plan NOT LIKE '%users_display_name_prefix_idx%' THEN
    RAISE EXCEPTION E'prefix search does not use users_display_name_prefix_idx:\n%', plan;
  END IF;
END $$;
SELECT pg_temp.assert_true('no forecast_* relation in app',
  NOT EXISTS (SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = 'app' AND c.relname LIKE 'forecast%'));
SELECT pg_temp.assert_true('no gdd columns on daily_rollups / reference_daily',
  NOT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = 'app'
              AND table_name IN ('daily_rollups', 'reference_daily') AND column_name LIKE 'gdd%'));
SELECT pg_temp.assert_true('nothing in public',
  NOT EXISTS (SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = 'public' AND c.relkind IN ('r','v','m','S','p','f')));

ROLLBACK;
\echo 'data_model_test: OK'
