// API response types (01 section 3.2, checked against backend/crates/api).

export interface Baseline {
  reference_station_id: string;
  name: string;
  source: string; // e.g. "ghcnd_acis"
  history_loaded: boolean;
}

export interface Station {
  id: string;
  name: string;
  is_own: boolean;
  owner: { id: string; display_name: string };
  lat: number;
  lon: number;
  coordinates_rounded: boolean;
  timezone: string;
  baseline: Baseline | null;
  latest_observation_at: string | null;
}

export interface Me {
  id: string;
  display_name: string;
  is_admin: boolean;
  credential: { status: 'ok' | 'invalid' | 'rate_limited' } | null;
}

export interface MdValue {
  md: string;
  v: number;
}
export interface GddSettings {
  method: 'simple_avg' | 'clamped_50_86';
  base_f: number;
  start: string;
}
export interface PerSeries<T> {
  rain: T;
  gdd: T;
}
export interface BandPoint {
  md: string;
  p: number[];
}
export interface YtdStation {
  station_id: string;
  baseline: Baseline | null;
  current: {
    first_date: string | null;
    last_date: string | null;
    last_complete_date: string | null;
    missing_days: PerSeries<number>;
    rain_cum_in: MdValue[];
    gdd_cum: MdValue[];
  };
  prior_years: {
    year: number;
    missing_days: PerSeries<number>;
    rain_cum_in: MdValue[] | null;
    gdd_cum: MdValue[] | null;
  }[];
  bands: {
    percentiles: number[];
    years_included: PerSeries<number>;
    rain_cum_in: BandPoint[] | null;
    gdd_cum: BandPoint[] | null;
  } | null;
}
export interface YtdResponse {
  year: number;
  start: string;
  end: string;
  gdd: GddSettings;
  day_definition: string;
  stations: YtdStation[];
}
export interface BaselineCandidate {
  reference_station_id: string;
  name: string;
  airport_code: string | null;
  distance_mi: number;
  elevation_ft: number | null;
  elevation_diff_ft: number | null;
  record_start: string;
  record_years: number;
  short_record: boolean;
}
export interface BaselineCandidates {
  station_id: string;
  station_elevation_ft: number | null;
  radius_mi: number;
  current: string | null;
  suggested: string | null;
  candidates: BaselineCandidate[];
}
export type FollowStatus = 'pending' | 'approved' | 'declined' | 'revoked';
export interface UserHit {
  id: string;
  display_name: string;
  follow_status: 'none' | FollowStatus;
}
export interface FollowRow {
  user: { id: string; display_name: string };
  status: FollowStatus;
  requested_at: string;
  decided_at: string | null;
}
export interface Band {
  p10: number | null;
  p50: number | null;
  p90: number | null;
}
export interface BandSeries {
  p10: (number | null)[];
  p50: (number | null)[];
  p90: (number | null)[];
}
export interface Forecast {
  station_id: string;
  timezone: string;
  generated_at: string;
  stale: boolean;
  attribution: string;
  units: { temperature: 'F'; precipitation: 'in' };
  deterministic: {
    model: string;
    hourly: { time: string[]; temperature_f: (number | null)[]; precip_in: (number | null)[]; prob_precip: (number | null)[] };
    daily: { date: string; tmax_f: number | null; tmin_f: number | null; precip_in: number | null; prob_precip_max: number | null }[];
  } | null;
  ensemble: {
    model: string;
    members: number;
    hourly: { time: string[]; temperature_f: BandSeries; precip_in: BandSeries };
    daily: { date: string; tmax_f: Band; tmin_f: Band; precip_in: Band; prob_precip: number | null }[];
  } | null;
}
export interface ForecastChanges {
  station_id: string;
  timezone: string;
  model: string;
  generated_at: string;
  attribution: string;
  units: { temperature: 'F'; precipitation: 'in' };
  lead_days: number[];
  days: { date: string; precip_in: (number | null)[]; prob_precip_max: (number | null)[]; tmax_f: (number | null)[]; tmin_f: (number | null)[] }[];
}
export interface ConnectDevice {
  mac: string;
  name: string;
  timezone: string | null;
}
export type ValidateResponse = { valid: true; devices: ConnectDevice[] } | { valid: false };
export interface SaveDevice {
  mac: string;
  name?: string;
  timezone?: string;
}
export interface SaveResponse {
  credential: { status: 'ok' };
  stations: Station[];
  not_covered: { id: string; name: string }[];
}
export interface ConnectStatus {
  credential: null | { status: string; last_ok_at: string | null; needs_reconnecting: boolean };
}
