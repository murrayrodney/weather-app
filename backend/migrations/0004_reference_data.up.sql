-- 0004 reference_data (04 section 5.4)
CREATE TABLE app.reference_stations (
  id                     text PRIMARY KEY CHECK (id ~ '^[A-Z0-9]{11}$'),   -- GHCN-Daily id, e.g. USW00024144
  name                   text NOT NULL,
  lat                    numeric(8,5) NOT NULL CHECK (lat BETWEEN -90 AND 90),   -- public airport, exact is fine
  lon                    numeric(8,5) NOT NULL CHECK (lon BETWEEN -180 AND 180),
  elevation_ft           integer,
  timezone               text NOT NULL,
  std_utc_offset_minutes smallint NOT NULL CHECK (std_utc_offset_minutes BETWEEN -720 AND 840),
  source                 text NOT NULL DEFAULT 'ghcnd_acis',
  last_synced_date       date,
  created_at             timestamptz NOT NULL DEFAULT now(),
  updated_at             timestamptz NOT NULL DEFAULT now()
);
CREATE TRIGGER reference_stations_touch BEFORE UPDATE ON app.reference_stations
  FOR EACH ROW EXECUTE FUNCTION app.touch_updated_at();

CREATE TABLE app.reference_daily (
  reference_station_id text NOT NULL REFERENCES app.reference_stations (id) ON DELETE CASCADE,
  date          date NOT NULL,                         -- midnight-to-midnight local standard time (F5)
  tmax_f        real,
  tmin_f        real,
  precip_in     real CHECK (precip_in IS NULL OR precip_in >= 0),
  precip_trace  boolean NOT NULL DEFAULT false,        -- ACIS "T": precip_in is NULL and this is true
  quality_flags text,                                  -- comma-joined '<m|n|p>:<flag>' entries (03's ACIS parser)
  fetched_at    timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (reference_station_id, date)
);

GRANT SELECT ON app.reference_stations, app.reference_daily TO app_api;
-- Baseline picker (02): the API inserts the row for the airport the owner chose, from ACIS metadata it fetched itself (never from the
-- client), with ON CONFLICT DO NOTHING. No UPDATE or DELETE. last_synced_date stays NULL, which makes the next reference_sync load the full history.
GRANT INSERT (id, name, lat, lon, elevation_ft, timezone, std_utc_offset_minutes) ON app.reference_stations TO app_api;
GRANT SELECT ON app.reference_stations TO app_poller;
GRANT UPDATE (last_synced_date) ON app.reference_stations TO app_poller;   -- rows are created by admin add-reference-station or by the API's baseline PUT (INSERT of seven columns)
GRANT SELECT, INSERT, UPDATE ON app.reference_daily TO app_poller;
