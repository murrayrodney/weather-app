-- 0006 observations (04 section 5.6)
CREATE TABLE app.observations (
  station_id          uuid NOT NULL REFERENCES app.stations (id) ON DELETE CASCADE,
  ts_utc              timestamptz NOT NULL,
  temp_f              real,
  feels_like_f        real,
  dew_point_f         real,
  humidity_pct        smallint CHECK (humidity_pct BETWEEN 0 AND 100),
  wind_speed_mph      real CHECK (wind_speed_mph >= 0),
  wind_gust_mph       real CHECK (wind_gust_mph >= 0),
  max_daily_gust_mph  real CHECK (max_daily_gust_mph >= 0),
  wind_dir_deg        smallint CHECK (wind_dir_deg BETWEEN 0 AND 360),
  baro_rel_inhg       real,
  baro_abs_inhg       real,
  rain_rate_in_hr     real CHECK (rain_rate_in_hr >= 0),
  rain_event_in       real CHECK (rain_event_in >= 0),
  rain_daily_in       real CHECK (rain_daily_in >= 0),
  rain_weekly_in      real CHECK (rain_weekly_in >= 0),
  rain_monthly_in     real CHECK (rain_monthly_in >= 0),
  rain_yearly_in      real CHECK (rain_yearly_in >= 0),
  rain_total_in       real CHECK (rain_total_in >= 0),
  rain_last_at        timestamptz,
  solar_wm2           real CHECK (solar_wm2 >= 0),
  uv_index            smallint CHECK (uv_index >= 0),
  temp_indoor_f       real,
  humidity_indoor_pct smallint CHECK (humidity_indoor_pct BETWEEN 0 AND 100),
  battery_out         smallint,
  rain_inc_in         numeric(6,3) CHECK (rain_inc_in >= 0),
  received_at         timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (station_id, ts_utc)
);

GRANT SELECT ON app.observations TO app_api;
GRANT SELECT, INSERT, UPDATE ON app.observations TO app_poller;
