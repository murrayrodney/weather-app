DO $$ BEGIN
  IF coalesce(current_setting('app.allow_destructive_down', true), 'off') <> 'on' THEN
    RAISE EXCEPTION 'destructive down migration refused (set app.allow_destructive_down = on)';
  END IF;
END $$;

DROP FUNCTION app.gdd_day(real, real, text, real);
DROP FUNCTION app.lst_date(timestamptz, int);
DROP FUNCTION app.touch_updated_at();

ALTER DEFAULT PRIVILEGES FOR ROLE app_migrator GRANT EXECUTE ON FUNCTIONS TO PUBLIC;
ALTER DEFAULT PRIVILEGES FOR ROLE app_migrator IN SCHEMA app REVOKE SELECT ON TABLES FROM app_backup;
ALTER DEFAULT PRIVILEGES FOR ROLE app_migrator IN SCHEMA app REVOKE SELECT ON SEQUENCES FROM app_backup;
DO $$ BEGIN
  IF to_regclass('app._sqlx_migrations') IS NOT NULL THEN
    REVOKE SELECT ON app._sqlx_migrations FROM app_backup;
  END IF;
END $$;
REVOKE USAGE ON SCHEMA app FROM app_api, app_poller, app_backup;
