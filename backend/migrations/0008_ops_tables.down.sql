DO $$ BEGIN
  IF coalesce(current_setting('app.allow_destructive_down', true), 'off') <> 'on' THEN
    RAISE EXCEPTION 'destructive down migration refused (set app.allow_destructive_down = on)';
  END IF;
END $$;

DROP TABLE app.rate_gates;
DROP TABLE app.connect_attempts;
DROP TABLE app.audit_log;
DROP TABLE app.job_runs;
