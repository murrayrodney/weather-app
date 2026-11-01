-- can_view / visible_station_ids tests (04 section 11.3). Superuser, scratch database, rolled back.
\set ON_ERROR_STOP on

CREATE FUNCTION pg_temp.expect(label text, expected boolean, actual boolean) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
  IF actual IS DISTINCT FROM expected THEN
    RAISE EXCEPTION 'can_view case "%": expected %, got %', label, expected, actual;
  END IF;
END $$;

BEGIN;
-- owner, viewer, other, inactive_viewer; station S owned by owner; station T owned by other.
INSERT INTO app.users (id, display_name, is_active, deactivated_at) VALUES
  ('a0000000-0000-0000-0000-000000000001', 'owner', true, NULL),
  ('a0000000-0000-0000-0000-000000000002', 'viewer', true, NULL),
  ('a0000000-0000-0000-0000-000000000003', 'other', true, NULL),
  ('a0000000-0000-0000-0000-000000000004', 'inactive_viewer', false, now());
INSERT INTO app.credentials (id, owner_id, api_key_ciphertext, encapsulated_key, key_version) VALUES
  ('c0000000-0000-0000-0000-000000000001', 'a0000000-0000-0000-0000-000000000001', decode(repeat('ab', 40), 'hex'), decode(repeat('cd', 32), 'hex'), 1),
  ('c0000000-0000-0000-0000-000000000003', 'a0000000-0000-0000-0000-000000000003', decode(repeat('ab', 40), 'hex'), decode(repeat('cd', 32), 'hex'), 1);
INSERT INTO app.stations (id, mac, name, owner_id, credential_id, lat, lon, timezone, std_utc_offset_minutes) VALUES
  ('50000000-0000-0000-0000-00000000000a', 'AA:BB:CC:00:08:01', 'S', 'a0000000-0000-0000-0000-000000000001', 'c0000000-0000-0000-0000-000000000001', 40.40, -105.00, 'America/Denver', -420),
  ('50000000-0000-0000-0000-00000000000b', 'AA:BB:CC:00:08:02', 'T', 'a0000000-0000-0000-0000-000000000003', 'c0000000-0000-0000-0000-000000000003', 41.10, -105.50, 'America/Denver', -420);

-- viewer is owner
SELECT pg_temp.expect('owner sees own station', true,
  app.can_view('a0000000-0000-0000-0000-000000000001', '50000000-0000-0000-0000-00000000000a'));
-- no follow row
SELECT pg_temp.expect('no follow', false,
  app.can_view('a0000000-0000-0000-0000-000000000002', '50000000-0000-0000-0000-00000000000a'));
-- each status
INSERT INTO app.follows (follower_id, followee_id, status, decided_at) VALUES
  ('a0000000-0000-0000-0000-000000000002', 'a0000000-0000-0000-0000-000000000001', 'pending', NULL);
SELECT pg_temp.expect('pending', false,
  app.can_view('a0000000-0000-0000-0000-000000000002', '50000000-0000-0000-0000-00000000000a'));
UPDATE app.follows SET status = 'approved', decided_at = now() WHERE follower_id = 'a0000000-0000-0000-0000-000000000002';
SELECT pg_temp.expect('approved', true,
  app.can_view('a0000000-0000-0000-0000-000000000002', '50000000-0000-0000-0000-00000000000a'));
-- visible_station_ids is exactly the stations the matrix allows
SELECT pg_temp.expect('visible_station_ids after approval', true,
  (SELECT array_agg(x ORDER BY x) = ARRAY['50000000-0000-0000-0000-00000000000a'::uuid]
     FROM app.visible_station_ids('a0000000-0000-0000-0000-000000000002') x));
-- transition approved -> revoked in the same session: no caching
SELECT pg_temp.expect('before revoke', true,
  app.can_view('a0000000-0000-0000-0000-000000000002', '50000000-0000-0000-0000-00000000000a'));
UPDATE app.follows SET status = 'revoked' WHERE follower_id = 'a0000000-0000-0000-0000-000000000002';
SELECT pg_temp.expect('revoked, very next call', false,
  app.can_view('a0000000-0000-0000-0000-000000000002', '50000000-0000-0000-0000-00000000000a'));
UPDATE app.follows SET status = 'declined' WHERE follower_id = 'a0000000-0000-0000-0000-000000000002';
SELECT pg_temp.expect('declined', false,
  app.can_view('a0000000-0000-0000-0000-000000000002', '50000000-0000-0000-0000-00000000000a'));
SELECT pg_temp.expect('visible_station_ids when not approved is empty', true,
  NOT EXISTS (SELECT 1 FROM app.visible_station_ids('a0000000-0000-0000-0000-000000000002')));
-- approved follow in the opposite direction only
DELETE FROM app.follows;
INSERT INTO app.follows (follower_id, followee_id, status, decided_at) VALUES
  ('a0000000-0000-0000-0000-000000000001', 'a0000000-0000-0000-0000-000000000002', 'approved', now());
SELECT pg_temp.expect('opposite direction only', false,
  app.can_view('a0000000-0000-0000-0000-000000000002', '50000000-0000-0000-0000-00000000000a'));
-- approved on a different owner only
DELETE FROM app.follows;
INSERT INTO app.follows (follower_id, followee_id, status, decided_at) VALUES
  ('a0000000-0000-0000-0000-000000000002', 'a0000000-0000-0000-0000-000000000003', 'approved', now());
SELECT pg_temp.expect('approved on a different owner', false,
  app.can_view('a0000000-0000-0000-0000-000000000002', '50000000-0000-0000-0000-00000000000a'));
SELECT pg_temp.expect('approved on a different owner sees that owners station', true,
  app.can_view('a0000000-0000-0000-0000-000000000002', '50000000-0000-0000-0000-00000000000b'));
-- approved but viewer inactive
DELETE FROM app.follows;
INSERT INTO app.follows (follower_id, followee_id, status, decided_at) VALUES
  ('a0000000-0000-0000-0000-000000000004', 'a0000000-0000-0000-0000-000000000001', 'approved', now());
SELECT pg_temp.expect('approved but viewer inactive', false,
  app.can_view('a0000000-0000-0000-0000-000000000004', '50000000-0000-0000-0000-00000000000a'));
SELECT pg_temp.expect('inactive viewer has no visible stations', true,
  NOT EXISTS (SELECT 1 FROM app.visible_station_ids('a0000000-0000-0000-0000-000000000004')));
-- unknown station, NULLs
SELECT pg_temp.expect('unknown station', false,
  app.can_view('a0000000-0000-0000-0000-000000000001', '50000000-0000-0000-0000-0000000000ff'));
SELECT pg_temp.expect('NULL user is false, not NULL', false,
  app.can_view(NULL, '50000000-0000-0000-0000-00000000000a'));
SELECT pg_temp.expect('NULL station is false, not NULL', false,
  app.can_view('a0000000-0000-0000-0000-000000000001', NULL));
-- mutual seeded follows both ways
DELETE FROM app.follows;
INSERT INTO app.follows (follower_id, followee_id, status, decided_at) VALUES
  ('a0000000-0000-0000-0000-000000000001', 'a0000000-0000-0000-0000-000000000003', 'approved', now()),
  ('a0000000-0000-0000-0000-000000000003', 'a0000000-0000-0000-0000-000000000001', 'approved', now());
SELECT pg_temp.expect('mutual: owner sees T', true,
  app.can_view('a0000000-0000-0000-0000-000000000001', '50000000-0000-0000-0000-00000000000b'));
SELECT pg_temp.expect('mutual: other sees S', true,
  app.can_view('a0000000-0000-0000-0000-000000000003', '50000000-0000-0000-0000-00000000000a'));
SELECT pg_temp.expect('mutual: visible set of owner', true,
  (SELECT array_agg(x ORDER BY x) = ARRAY['50000000-0000-0000-0000-00000000000a'::uuid, '50000000-0000-0000-0000-00000000000b'::uuid]
     FROM app.visible_station_ids('a0000000-0000-0000-0000-000000000001') x));

-- The same function through the real API grants (SECURITY INVOKER needs the API's own SELECT rights).
SET LOCAL ROLE app_api;
SELECT pg_temp.expect('as app_api: mutual', true,
  app.can_view('a0000000-0000-0000-0000-000000000003', '50000000-0000-0000-0000-00000000000a'));
SELECT pg_temp.expect('as app_api: stranger', false,
  app.can_view('a0000000-0000-0000-0000-000000000002', '50000000-0000-0000-0000-00000000000a'));
RESET ROLE;
ROLLBACK;
\echo 'can_view_test: OK'
