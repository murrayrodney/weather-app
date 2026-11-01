-- 0005 stations (04 section 5.5). Coordinates are stored rounded to 2 decimals by the column type (S10).
CREATE TABLE app.stations (
  id                     uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  mac                    text NOT NULL UNIQUE CHECK (mac ~ '^([0-9A-F]{2}:){5}[0-9A-F]{2}$'),  -- upper-case, colon separated
  name                   text NOT NULL CHECK (char_length(btrim(name)) BETWEEN 1 AND 80),
  owner_id               uuid NOT NULL REFERENCES app.users (id) ON DELETE CASCADE,
  credential_id          uuid NOT NULL,                          -- composite FK below
  lat                    numeric(5,2) NOT NULL CHECK (lat BETWEEN -90 AND 90),
  lon                    numeric(5,2) NOT NULL CHECK (lon BETWEEN -180 AND 180),
  elevation_ft           integer CHECK (elevation_ft BETWEEN -500 AND 20000),   -- from AWN info.coords.elevation when present, rounded to 10 ft; NULL if unknown
  timezone               text NOT NULL,                          -- IANA name, validated by API/admin
  std_utc_offset_minutes smallint NOT NULL CHECK (std_utc_offset_minutes BETWEEN -720 AND 840),
  reference_station_id   text REFERENCES app.reference_stations (id) ON DELETE SET NULL,
  is_enabled             boolean NOT NULL DEFAULT true,          -- poller skips disabled stations
  last_observed_at       timestamptz,                            -- written by awn_ingest / awn_backfill
  backfill_floor_ts      timestamptz,                            -- earliest ts_utc AWN returned; awn_backfill resume cursor (F3)
  created_at             timestamptz NOT NULL DEFAULT now(),
  updated_at             timestamptz NOT NULL DEFAULT now(),
  -- A station can only use a credential owned by the station's owner (also holds after repointing).
  FOREIGN KEY (credential_id, owner_id) REFERENCES app.credentials (id, owner_id) DEFERRABLE INITIALLY DEFERRED
);
CREATE INDEX stations_owner_idx ON app.stations (owner_id);
CREATE INDEX stations_credential_idx ON app.stations (credential_id);
CREATE INDEX stations_reference_idx ON app.stations (reference_station_id);
CREATE TRIGGER stations_touch BEFORE UPDATE ON app.stations FOR EACH ROW EXECUTE FUNCTION app.touch_updated_at();

GRANT SELECT, INSERT ON app.stations TO app_api;
GRANT UPDATE (name, credential_id, reference_station_id) ON app.stations TO app_api;   -- credential_id: repoint after reconnect (5.3); reference_station_id: owner changes the baseline (02 5.10)
GRANT SELECT ON app.stations TO app_poller;
GRANT UPDATE (last_observed_at, backfill_floor_ts) ON app.stations TO app_poller;
