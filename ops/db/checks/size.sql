-- Size monitoring (04 section 8.6). Run: psql -X -v ON_ERROR_STOP=1 -f size.sql "$BACKUP_DATABASE_URL"; non-zero exit means alert.
\set ON_ERROR_STOP on
SELECT pg_size_pretty(pg_database_size(current_database())) AS database_size,
       round(pg_database_size(current_database()) / 1048576.0) AS database_mb;
SELECT c.relname, pg_size_pretty(pg_total_relation_size(c.oid)) AS total_size,
       pg_total_relation_size(c.oid) AS bytes
FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE n.nspname = 'app' AND c.relkind = 'r'
ORDER BY bytes DESC;
-- exit status helper: raises when above the alert threshold
DO $$ BEGIN
  IF pg_database_size(current_database()) > 400 * 1048576 THEN
    RAISE EXCEPTION 'database size above 400 MB alert threshold';
  END IF;
END $$;
