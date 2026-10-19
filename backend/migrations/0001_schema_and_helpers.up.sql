-- 0001 schema_and_helpers (04 section 5.1)
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

-- Growing degree days for ONE day, computed at query time and never stored (owner decision, Oct 7).
-- The API calls this for station rollups and baseline rows alike, so both sides always use the same method and base.
--   simple_avg    : max(0, (tmax + tmin) / 2 - base)
--   clamped_50_86 : both temperatures clamped to [base, 86] first, then (tmax' + tmin') / 2 - base
--                   (the name keeps "50" because 50 is the default base; a different base moves the lower clamp too)
-- NULL when either temperature is NULL, and NULL for an unknown method (the API validates the parameter and never relies on this).
CREATE FUNCTION app.gdd_day(tmax_f real, tmin_f real, method text, base_f real) RETURNS real
LANGUAGE sql IMMUTABLE PARALLEL SAFE SET search_path = pg_catalog AS
$$
  SELECT CASE
    WHEN tmax_f IS NULL OR tmin_f IS NULL THEN NULL
    WHEN method = 'simple_avg' THEN greatest(0::real, (tmax_f + tmin_f) / 2 - base_f)
    WHEN method = 'clamped_50_86' THEN
      (least(greatest(tmax_f, base_f), 86::real) + least(greatest(tmin_f, base_f), 86::real)) / 2 - base_f
    ELSE NULL
  END
$$;
GRANT EXECUTE ON FUNCTION app.gdd_day(real, real, text, real) TO app_api;
