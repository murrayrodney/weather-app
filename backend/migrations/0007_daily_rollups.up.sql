-- 0007 daily_rollups (04 section 5.7)
CREATE TABLE app.daily_rollups (
  station_id        uuid NOT NULL REFERENCES app.stations (id) ON DELETE CASCADE,
  local_date        date NOT NULL,                -- local STANDARD time day (app.lst_date), F5
  rain_in           numeric(7,3) CHECK (rain_in >= 0),
  tmax_f            real,
  tmin_f            real,
  obs_count         smallint NOT NULL DEFAULT 0 CHECK (obs_count >= 0),   -- readings with temp_f (288 at 5 min, 48 at 30 min when complete)
  is_complete       boolean NOT NULL DEFAULT false,    -- true only for a finished day with coverage >= 0.9 (interval-aware)
  rain_resets       smallint NOT NULL DEFAULT 0,        -- counter drops treated as resets (F6)
  rain_capped       smallint NOT NULL DEFAULT 0,        -- impossible jumps capped (F6)
  awn_daily_rain_in numeric(7,3),                       -- AWN's own daily total used for reconciliation
  rain_reconcile_status text NOT NULL DEFAULT 'unchecked'
                    CHECK (rain_reconcile_status IN ('unchecked','ok','mismatch','no_awn_total')),
  rain_reconcile_diff_in numeric(7,3),                  -- console-computed minus awn_daily_rain_in
  computed_at       timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (station_id, local_date)
);
CREATE INDEX daily_rollups_date_idx ON app.daily_rollups (local_date);

-- Days whose observations changed and need a rollup (00-conventions). Ingest/backfill insert in the same transaction as
-- the observation upsert; daily_rollup deletes the rows it has processed. No timestamp-based dirtiness.
CREATE TABLE app.rollup_dirty_days (
  station_id uuid NOT NULL REFERENCES app.stations (id) ON DELETE CASCADE,
  local_date date NOT NULL,
  PRIMARY KEY (station_id, local_date)
);

GRANT SELECT ON app.daily_rollups TO app_api;
GRANT SELECT, INSERT, UPDATE ON app.daily_rollups TO app_poller;
GRANT SELECT, INSERT, DELETE ON app.rollup_dirty_days TO app_poller;   -- app_api: no access
-- The locking enqueue `INSERT ... ON CONFLICT (station_id, local_date) DO UPDATE SET local_date = EXCLUDED.local_date`
-- (a no-op that takes the row lock) needs UPDATE on the column it sets.
GRANT UPDATE (local_date) ON app.rollup_dirty_days TO app_poller;
