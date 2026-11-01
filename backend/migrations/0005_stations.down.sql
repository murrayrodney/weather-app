DO $$ BEGIN
  IF coalesce(current_setting('app.allow_destructive_down', true), 'off') <> 'on' THEN
    RAISE EXCEPTION 'destructive down migration refused (set app.allow_destructive_down = on)';
  END IF;
END $$;

DROP TABLE app.stations;
