// API-shaped fixtures (02 sections 5.5, 5.6, 5.6a), built in code so the 214-slot season stays small to read.
// Shared by Vitest and Playwright (tests/). Dates are fixed; nothing here is real data.
import type {
  BaselineCandidates,
  Forecast,
  ForecastChanges,
  GddSettings,
  MdValue,
  Station,
  UserHit,
  YtdResponse,
  YtdStation
} from '../lib/api/types';

export const STATION_ID = '11111111-1111-4111-8111-111111111111';
export const STATION_ID_2 = '22222222-2222-4222-8222-222222222222';
const DIM = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
const p2 = (n: number) => String(n).padStart(2, '0');

export function mdRange(start: string, end: string): string[] {
  const out: string[] = [];
  for (let m = 1; m <= 12; m++) {
    for (let d = 1; d <= DIM[m - 1]; d++) {
      const md = `${p2(m)}-${p2(d)}`;
      if (md >= start && md <= end) out.push(md);
    }
  }
  return out;
}

function cumulative(mds: string[], perDay: (i: number) => number): MdValue[] {
  let sum = 0;
  return mds.map((md, i) => {
    sum += perDay(i);
    return { md, v: Math.round(sum * 100) / 100 };
  });
}

export const BASELINE = { reference_station_id: 'USW00099001', name: 'Demo Airport', source: 'ghcnd_acis', history_loaded: true };

interface Opts {
  baseline?: YtdStation['baseline'];
  firstDate?: string | null;
  lastDate?: string | null;
  lastComplete?: string | null;
  bands?: boolean;
  start?: string;
  end?: string;
  stationId?: string;
}

export function ytdStation(o: Opts = {}): YtdStation {
  const start = o.start ?? '04-01';
  const end = o.end ?? '10-31';
  const all = mdRange(start, end);
  const lastDate = o.lastDate === undefined ? '07-10' : o.lastDate;
  const first = o.firstDate === undefined ? `2026-${start}` : o.firstDate;
  const cur = lastDate === null ? [] : all.filter((md) => md >= (first ?? `2026-${start}`).slice(5) && md <= lastDate);
  const baseline = o.baseline === undefined ? BASELINE : o.baseline;
  const hasBase = baseline !== null && baseline.history_loaded;
  const withBands = (o.bands ?? true) && hasBase;
  return {
    station_id: o.stationId ?? STATION_ID,
    baseline,
    current: {
      first_date: lastDate === null ? null : first,
      last_date: lastDate === null ? null : `2026-${lastDate}`,
      last_complete_date: o.lastComplete === undefined ? (lastDate === null ? null : '2026-07-09') : o.lastComplete,
      missing_days: { rain: 2, gdd: 1 },
      rain_cum_in: cumulative(cur, (i) => (i % 5 === 0 ? 0.2 : 0.02)),
      gdd_cum: cumulative(cur, (i) => 8 + (i % 7))
    },
    prior_years: hasBase
      ? [2025, 2024, 2023, 2022, 2021].map((year, k) => ({
          year,
          missing_days: { rain: k === 0 ? 3 : 0, gdd: k === 1 ? 2 : 0 },
          rain_cum_in: k === 4 ? null : cumulative(all, (i) => (i % 6 === 0 ? 0.25 : 0.03) + k * 0.001),
          gdd_cum: cumulative(all, (i) => 9 + ((i + k) % 6))
        }))
      : [],
    bands: withBands
      ? {
          percentiles: [10, 25, 50, 75, 90],
          years_included: { rain: 34, gdd: 35 },
          rain_cum_in: all.map((md, i) => ({ md, p: [0.5, 0.8, 1, 1.3, 1.7].map((f) => Math.round(f * (i / 5 + 0.1) * 100) / 100) })),
          gdd_cum: all.map((md, i) => ({ md, p: [0.6, 0.8, 1, 1.2, 1.4].map((f) => Math.round(f * (i + 1) * 11 * 100) / 100) }))
        }
      : null
  };
}

export function ytdResponse(stations: YtdStation[], extra: Partial<YtdResponse> = {}): YtdResponse {
  return {
    year: 2026,
    start: '04-01',
    end: '10-31',
    gdd: { method: 'simple_avg', base_f: 50, start: '04-01' },
    day_definition: 'local_standard_time_midnight_to_midnight',
    stations,
    ...extra
  };
}

export const ytdFull = () => ytdResponse([ytdStation()]);
export const ytdNoBaseline = () => ytdResponse([ytdStation({ baseline: null })]);
export const ytdBaselineLoading = () => ytdResponse([ytdStation({ baseline: { ...BASELINE, history_loaded: false } })]);
export const ytdGddClamped = () => {
  const gdd: GddSettings = { method: 'clamped_50_86', base_f: 45, start: '01-01' };
  return ytdResponse([ytdStation({ start: '01-01' })], { gdd });
};
export const ytdPartialStation = () => ytdResponse([ytdStation({ firstDate: '2026-06-01', lastDate: '07-10', lastComplete: '2026-07-09' })]);

export function station(over: Partial<Station> = {}): Station {
  return {
    id: STATION_ID,
    name: 'Fixture Station',
    is_own: true,
    owner: { id: '33333333-3333-4333-8333-333333333333', display_name: 'Alex' },
    lat: 40.5,
    lon: -105.1,
    coordinates_rounded: false,
    timezone: 'America/Denver',
    baseline: BASELINE,
    latest_observation_at: '2026-07-10T18:00:00Z',
    ...over
  };
}

export const me = (credential: 'ok' | 'invalid' | 'rate_limited' | null = 'ok') => ({
  id: '33333333-3333-4333-8333-333333333333',
  display_name: 'Alex',
  is_admin: false,
  credential: credential ? { status: credential } : null
});

function hours(n: number): string[] {
  const t0 = Date.UTC(2026, 6, 10, 6);
  return Array.from({ length: n }, (_, i) => new Date(t0 + i * 3600000).toISOString().replace('.000Z', 'Z'));
}
const band = (f: (i: number) => number) => (n: number) => ({
  p10: Array.from({ length: n }, (_, i) => (i === 5 ? null : f(i) - 3)),
  p50: Array.from({ length: n }, (_, i) => (i === 5 ? null : f(i))),
  p90: Array.from({ length: n }, (_, i) => (i === 5 ? null : f(i) + 3))
});

export function forecast(over: Partial<Forecast> = {}): Forecast {
  const n = 15 * 24;
  const days = Array.from({ length: 15 }, (_, i) => `2026-07-${p2(10 + i)}`);
  const t = band((i) => 60 + 15 * Math.sin(i / 3.8));
  // Hourly ensemble rain: a range from zero up to about twice the median, as the API gives (not median plus or minus 3).
  const pr = (n: number) => ({
    p10: Array.from({ length: n }, () => 0),
    p50: Array.from({ length: n }, (_, i) => (i === 5 ? null : i % 9 === 0 ? 0.04 : 0.005)),
    p90: Array.from({ length: n }, (_, i) => (i === 5 ? null : i % 9 === 0 ? 0.08 : 0.01))
  });
  const acc = band((i) => 3 + 0.01 * i);
  const precip = Array.from({ length: n }, (_, i) => (i % 9 === 0 ? 0.03 : 0));
  // Running total of the deterministic precipitation, as the API computes it.
  const precipAccum = precip.reduce<number[]>((out, v, i) => [...out, (i === 0 ? 0 : out[i - 1]) + v], []);
  return {
    station_id: STATION_ID,
    timezone: 'America/Denver',
    generated_at: '2026-07-10T12:00:00Z',
    stale: false,
    attribution: 'Weather data by Open-Meteo.com (CC BY 4.0)',
    units: { temperature: 'F', precipitation: 'in', wind: 'mph', pressure: 'inHg', snow: 'in', percent: '%' },
    deterministic: {
      model: 'gfs_seamless',
      hourly: {
        time: hours(n),
        temperature_f: Array.from({ length: n }, (_, i) => 60 + 15 * Math.sin(i / 3.8)),
        precip_in: precip,
        prob_precip: Array.from({ length: n }, () => 20),
        precip_accum_in: precipAccum,
        apparent_temp_f: Array.from({ length: n }, (_, i) => 58 + 14 * Math.sin(i / 3.8)),
        dew_point_f: Array.from({ length: n }, (_, i) => 40 + 3 * Math.sin(i / 5)),
        cloud_pct: Array.from({ length: n }, (_, i) => (i * 37) % 101),
        humidity_pct: Array.from({ length: n }, (_, i) => 50 + (i % 40)),
        pressure_inhg: Array.from({ length: n }, (_, i) => Math.round((30.1 + 0.1 * Math.sin(i / 10)) * 100) / 100),
        wind_mph: Array.from({ length: n }, (_, i) => Math.abs(5 + 4 * Math.sin(i / 4))),
        wind_dir_deg: Array.from({ length: n }, (_, i) => (i * 15) % 360),
        gust_mph: Array.from({ length: n }, (_, i) => Math.abs(5 + 4 * Math.sin(i / 4)) + 5),
        weather_code: Array.from({ length: n }, (_, i) => (i % 20 === 0 ? 61 : 3)),
        snow_in: Array.from({ length: n }, () => 0)
      },
      daily: days.map((date, i) => ({
        date,
        tmax_f: 80 - i,
        tmin_f: 50 - i / 2,
        precip_in: i % 3 === 0 ? 0.1 : 0,
        prob_precip_max: 30,
        weather_code: i % 2 === 0 ? 3 : 61,
        snow_in: 0,
        sunrise: `${date}T12:30:00Z`,
        sunset: `2026-07-${p2(11 + i)}T02:20:00Z`
      }))
    },
    ensemble: {
      model: 'ecmwf_ifs025',
      members: 51,
      hourly: { time: hours(n), temperature_f: t(n), precip_in: pr(n), precip_accum_in: acc(n) },
      daily: days.map((date, i) => ({
        date,
        tmax_f: { p10: 74 - i, p50: 80 - i, p90: 86 - i },
        tmin_f: { p10: 44 - i / 2, p50: 50 - i / 2, p90: 55 - i / 2 },
        precip_in: { p10: 0, p50: 0.02, p90: 0.3 },
        prob_precip: 35
      }))
    },
    ...over
  };
}

export const forecastEnsembleNull = () => forecast({ ensemble: null });

export function forecastChanges(): ForecastChanges {
  const dates = Array.from({ length: 7 }, (_, i) => `2026-07-${p2(10 + i)}`);
  const leads = [0, 1, 2, 3, 4, 5, 6, 7];
  return {
    station_id: STATION_ID,
    timezone: 'America/Denver',
    model: 'gfs_seamless',
    generated_at: '2026-07-10T12:00:00Z',
    attribution: 'Weather data by Open-Meteo.com (CC BY 4.0)',
    units: { temperature: 'F', precipitation: 'in' },
    lead_days: leads,
    days: dates.map((date, d) => ({
      date,
      precip_in: leads.map((l) => (d === 2 && l === 4 ? null : Math.round((0.05 * ((d + l) % 4)) * 100) / 100)),
      prob_precip_max: leads.map((l) => (d === 0 ? null : 10 * ((d + l) % 7))),
      tmax_f: leads.map((l) => 80 - d + (l % 3)),
      tmin_f: leads.map((l) => 50 - d + (l % 2))
    }))
  };
}

export const candidatesLoveland = (): BaselineCandidates => ({
  station_id: STATION_ID,
  station_elevation_ft: 5000,
  radius_mi: 60,
  current: null,
  suggested: 'USW00099003',
  candidates: [
    { reference_station_id: 'USW00099003', name: 'Demo Loveland Airport', airport_code: 'KLVX', distance_mi: 4.4, elevation_ft: 5100, elevation_diff_ft: 100, record_start: '1948-01-01', record_years: 78, short_record: false },
    { reference_station_id: 'USW00099004', name: 'Demo Mountain Airport', airport_code: null, distance_mi: 21, elevation_ft: 6500, elevation_diff_ft: 1500, record_start: '2010-01-01', record_years: 16, short_record: true }
  ]
});
export const candidatesEmpty = (): BaselineCandidates => ({ ...candidatesLoveland(), suggested: null, candidates: [] });

export const userSearch = (): { users: UserHit[] } => ({
  users: [
    { id: '44444444-4444-4444-8444-444444444441', display_name: 'Dana', follow_status: 'none' },
    { id: '44444444-4444-4444-8444-444444444442', display_name: 'Danny', follow_status: 'pending' },
    { id: '44444444-4444-4444-8444-444444444443', display_name: 'Daria', follow_status: 'approved' },
    { id: '44444444-4444-4444-8444-444444444444', display_name: 'Dale', follow_status: 'declined' },
    { id: '44444444-4444-4444-8444-444444444445', display_name: 'Dawn', follow_status: 'revoked' }
  ]
});
