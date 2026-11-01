// Forecast transforms. No percentile maths here: the API computes every p10/p50/p90.
import type { Band, BandSeries, Forecast, ForecastChanges } from '$lib/api/types';
import { addDays, localDate, zoneDate } from './dates';
import type { Pt } from './series';

interface ForecastBand {
  lower: (Pt | null)[]; // p10
  upper: (Pt | null)[]; // p90
  median: (Pt | null)[]; // p50
}

const pt = (v: number | null, x: number): Pt | null => (v === null || v === undefined ? null : { x, y: v });

/** Ribbon and median over shared x positions (index); null steps break the path. */
export function forecastBand(series: BandSeries): ForecastBand {
  return {
    lower: series.p10.map((v, i) => pt(v, i)),
    upper: series.p90.map((v, i) => pt(v, i)),
    median: series.p50.map((v, i) => pt(v, i))
  };
}

/** Converts per-day Band objects to a BandSeries. */
export function dailyBandSeries(days: Band[]): BandSeries {
  return { p10: days.map((d) => d.p10), p50: days.map((d) => d.p50), p90: days.map((d) => d.p90) };
}

/** The retired `/stations/{id}/forecast` route sends people (and bookmarks) to the station page's forecast section. */
export function forecastRedirectTarget(stationId: string): string {
  return `/stations/${encodeURIComponent(stationId)}#forecast`;
}

export function lineFrom(values: (number | null)[]): (Pt | null)[] {
  return values.map((v, i) => pt(v, i));
}

/** The deterministic hourly series the stack draws (one array per series, aligned to `time`). */
interface HourlyDet {
  temperature_f: (number | null)[];
  apparent_temp_f: (number | null)[];
  dew_point_f: (number | null)[];
  precip_in: (number | null)[];
  prob_precip: (number | null)[];
  precip_accum_in: (number | null)[];
  cloud_pct: (number | null)[];
  humidity_pct: (number | null)[];
  pressure_inhg: (number | null)[];
  wind_mph: (number | null)[];
  gust_mph: (number | null)[];
  wind_dir_deg: (number | null)[];
  snow_in: (number | null)[];
}

interface HourlyWindow {
  time: string[];
  deterministic: HourlyDet | null;
  ensemble: { temperature_f: BandSeries; precip_in: BandSeries; precip_accum_in: BandSeries } | null;
}

const HOURS_PER_DAY = 24;

/**
 * The hourly steps from `startIndex` covering `days` days (24 h each, from that step's timestamp). Both
 * series are cut by the same instants, so they stay aligned.
 */
export function hourlyWindow(f: Forecast, days: number, startIndex = 0): HourlyWindow {
  const all = f.deterministic?.hourly.time ?? f.ensemble?.hourly.time ?? [];
  const t0 = startIndex < all.length ? Date.parse(all[startIndex]) : 0;
  const end = t0 + days * 86400000;
  let stop = startIndex;
  while (stop < all.length && Date.parse(all[stop]) < end) stop++;
  const d = f.deterministic?.hourly;
  const e = f.ensemble?.hourly;
  const full: HourlyWindow = {
    time: all,
    deterministic: d
      ? {
          temperature_f: d.temperature_f,
          apparent_temp_f: d.apparent_temp_f,
          dew_point_f: d.dew_point_f,
          precip_in: d.precip_in,
          prob_precip: d.prob_precip,
          precip_accum_in: d.precip_accum_in,
          cloud_pct: d.cloud_pct,
          humidity_pct: d.humidity_pct,
          pressure_inhg: d.pressure_inhg,
          wind_mph: d.wind_mph,
          gust_mph: d.gust_mph,
          wind_dir_deg: d.wind_dir_deg,
          snow_in: d.snow_in
        }
      : null,
    ensemble: e ? { temperature_f: e.temperature_f, precip_in: e.precip_in, precip_accum_in: e.precip_accum_in } : null
  };
  return sliceHourly(full, startIndex, stop);
}

/** The steps [start, stop) of every hourly series in `w` (the same cut for each, so they stay aligned). */
export function sliceHourly(w: HourlyWindow, start: number, stop: number): HourlyWindow {
  const s = <T>(a: T[]) => a.slice(start, stop);
  const b = (x: BandSeries): BandSeries => ({ p10: s(x.p10), p50: s(x.p50), p90: s(x.p90) });
  const d = w.deterministic;
  const e = w.ensemble;
  return {
    time: s(w.time),
    deterministic: d
      ? {
          temperature_f: s(d.temperature_f),
          apparent_temp_f: s(d.apparent_temp_f),
          dew_point_f: s(d.dew_point_f),
          precip_in: s(d.precip_in),
          prob_precip: s(d.prob_precip),
          precip_accum_in: s(d.precip_accum_in),
          cloud_pct: s(d.cloud_pct),
          humidity_pct: s(d.humidity_pct),
          pressure_inhg: s(d.pressure_inhg),
          wind_mph: s(d.wind_mph),
          gust_mph: s(d.gust_mph),
          wind_dir_deg: s(d.wind_dir_deg),
          snow_in: s(d.snow_in)
        }
      : null,
    ensemble: e ? { temperature_f: b(e.temperature_f), precip_in: b(e.precip_in), precip_accum_in: b(e.precip_accum_in) } : null
  };
}

/**
 * Pages a window of `visible` steps can move through over `total` steps, moving one day (24 steps) at a time.
 * The last page is the last full window, so the count is at least 1.
 */
export function pageCount(total: number, visible: number): number {
  return total > visible ? Math.ceil((total - visible) / HOURS_PER_DAY) + 1 : 1;
}

/** First step of a page (clamped to the pages that exist). The last page ends on the last step. */
export function pageStart(page: number, total: number, visible: number): number {
  const last = pageCount(total, visible) - 1;
  const p = Math.min(Math.max(0, Math.floor(page)), last);
  return Math.min(p * HOURS_PER_DAY, Math.max(0, total - visible));
}

/** Hourly steps that carry a wind arrow: local hours divisible by `everyHours` where the direction is known. */
export function arrowIndexes(times: string[], ianaTz: string, dirDeg: (number | null)[], everyHours = 3): number[] {
  return times.flatMap((t, i) => (typeof dirDeg[i] === 'number' && localHour(t, ianaTz) % everyHours === 0 ? [i] : []));
}

/**
 * Rotation (degrees clockwise from up) of an arrow that points the way the wind blows. Open-Meteo gives the
 * direction the wind comes from, so the arrow turns 180 degrees.
 */
export function arrowRotation(dirDeg: number): number {
  return (((dirDeg + 180) % 360) + 360) % 360;
}

const COMPASS = ['N', 'NNE', 'NE', 'ENE', 'E', 'ESE', 'SE', 'SSE', 'S', 'SSW', 'SW', 'WSW', 'W', 'WNW', 'NW', 'NNW'];

/** 16-point compass name for the direction the wind comes from. */
export function windCompass(dirDeg: number): string {
  const norm = (((dirDeg % 360) + 360) % 360) / 22.5;
  return COMPASS[Math.round(norm) % 16];
}

/** Lowest and highest number over several series (nulls ignored); null when there is none. */
export function valueExtent(series: (number | null | undefined)[][]): [number, number] | null {
  let lo = Infinity;
  let hi = -Infinity;
  for (const a of series) {
    for (const v of a) {
      if (typeof v !== 'number') continue;
      lo = Math.min(lo, v);
      hi = Math.max(hi, v);
    }
  }
  return Number.isFinite(lo) ? [lo, hi] : null;
}

/** Min and max rounded outward to a multiple of `step`. Not forced to include zero. */
export function niceRange(lo: number, hi: number, step: number): [number, number] {
  return [Math.floor(lo / step) * step, Math.ceil(hi / step) * step];
}

/**
 * Temperature axis for the window: the data (and the 32 F reference) rounded outward to 5 F, with 2 F of air.
 * Pass the whole range to keep the axis pinned while paging.
 */
export function temperatureDomain(w: HourlyWindow): [number, number] {
  const s: (number | null)[][] = [[32]];
  if (w.deterministic) s.push(w.deterministic.temperature_f, w.deterministic.apparent_temp_f, w.deterministic.dew_point_f);
  if (w.ensemble) s.push(w.ensemble.temperature_f.p10, w.ensemble.temperature_f.p90);
  const [lo, hi] = valueExtent(s) ?? [32, 32];
  return niceRange(lo - 2, hi + 2, 5);
}

/**
 * Left axis for hourly liquid precipitation (bars and the hourly ensemble range). Built from the hourly values
 * only: the accumulated total has its own axis, so it cannot flatten the bars.
 */
export function hourlyRainDomain(w: HourlyWindow): [number, number] {
  const s: (number | null)[][] = [];
  if (w.deterministic) s.push(w.deterministic.precip_in);
  if (w.ensemble) s.push(w.ensemble.precip_in.p90);
  const hi = valueExtent(s)?.[1] ?? 0;
  return [0, Math.max(hi * 1.1, 0.1)];
}

/** Right axis for the accumulated total since "now" (deterministic and ensemble p90), from zero. */
export function accumDomain(w: HourlyWindow): [number, number] {
  const s: (number | null)[][] = [];
  if (w.deterministic) s.push(w.deterministic.precip_accum_in);
  if (w.ensemble) s.push(w.ensemble.precip_accum_in.p90);
  const hi = valueExtent(s)?.[1] ?? 0;
  return [0, Math.max(hi * 1.1, 0.1)];
}

export type ChangeMetric = 'precip_in' | 'prob_precip_max' | 'tmax_f' | 'tmin_f';

/**
 * Today's date in the station's zone (YYYY-MM-DD), taken from the response: `generated_at` is the instant the
 * forecast was made, and `timezone` is the station's zone. The page uses it for "today" in the change view.
 */
export function changesToday(changes: ForecastChanges): string {
  return localDate(changes.timezone, new Date(changes.generated_at));
}

/**
 * Whether lead N of target date D was issued by `today` (all YYYY-MM-DD, station-local). Lead 0 (the latest run)
 * always is. Lead N was issued on D minus N days, so it counts only when that day is before today. Checked against
 * the Previous Runs API on 2026-11-01 (proposal 09 section 5): for a date k days ahead, leads 1 to k are copies of
 * lead 0, so lead k is not yet issued (strict, not "today or earlier").
 */
export function leadIssued(date: string, lead: number, today: string): boolean {
  if (lead === 0) return true;
  return addDays(date, -lead) < today;
}

export interface DayLead {
  lead: number; // lead days before the target date (0 = latest run)
  value: number | null; // the API's value, null where the API has none
  issued: boolean; // false for a lead the API filled with a copy of the latest run (proposal 09 section 5)
}

/**
 * One target day's leads for a metric, oldest first (lead 7 to lead 0). The lead is the array index, since the
 * API lists `lead_days` as 0 to 7 in order. `today` is the station-local date (YYYY-MM-DD).
 */
export function dayLeads(day: ForecastChanges['days'][number], metric: ChangeMetric, today: string): DayLead[] {
  const arr = day[metric];
  const out: DayLead[] = [];
  for (let lead = arr.length - 1; lead >= 0; lead--) {
    out.push({ lead, value: arr[lead] ?? null, issued: leadIssued(day.date, lead, today) });
  }
  return out;
}

/** The range of a day's issued, non-null values (proposal 09 section 5: not-issued copies are left out). */
export function spreadAcrossLeads(day: ForecastChanges['days'][number], metric: ChangeMetric, today: string): { min: number; max: number } | null {
  const vals = dayLeads(day, metric, today).flatMap((l) => (l.issued && l.value !== null ? [l.value] : []));
  if (vals.length < 2) return null;
  return { min: Math.min(...vals), max: Math.max(...vals) };
}

// ---- Proposal 07 phase 3: the hourly stack (temperature panel, day strip, day/night, now line) ----

const HOUR_MS = 3600000;

/** Local calendar date (YYYY-MM-DD) of a UTC instant in an IANA zone. */
export function localDayKey(isoUtc: string, ianaTz: string): string {
  const d = zoneDate(ianaTz, new Date(isoUtc));
  return `${d.year}-${String(d.month).padStart(2, '0')}-${String(d.day).padStart(2, '0')}`;
}

/** Local hour (0 to 23) of a UTC instant in an IANA zone. */
function localHour(isoUtc: string, ianaTz: string): number {
  const parts = new Intl.DateTimeFormat('en-US', { timeZone: ianaTz, hour: 'numeric', hourCycle: 'h23' }).formatToParts(new Date(isoUtc));
  return Number(parts.find((p) => p.type === 'hour')?.value);
}

/** Indexes of the hourly steps that start a local day (local hour 0). */
export function midnightIndexes(times: string[], ianaTz: string): number[] {
  return times.flatMap((t, i) => (localHour(t, ianaTz) === 0 ? [i] : []));
}

export interface DayCell {
  date: string; // local YYYY-MM-DD
  startMs: number;
  endMs: number; // exclusive: one hourly step past the last hour of the day
}

/** One cell per local day over hourly times (one UTC instant per hour, ascending). A DST day has its real length. */
export function dayCells(times: string[], ianaTz: string): DayCell[] {
  const out: DayCell[] = [];
  for (const t of times) {
    const ms = Date.parse(t);
    const key = localDayKey(t, ianaTz);
    const last = out[out.length - 1];
    if (last && last.date === key) last.endMs = ms + HOUR_MS;
    else out.push({ date: key, startMs: ms, endMs: ms + HOUR_MS });
  }
  return out;
}

/** X position of "now" on a window [startMs, endMs], or null when now is outside it. */
export function nowX(nowMs: number, startMs: number, endMs: number, x: (ms: number) => number): number | null {
  if (nowMs < startMs || nowMs > endMs) return null;
  return x(nowMs);
}

export interface NightSpan {
  start: number; // UTC ms
  end: number;
}

/**
 * Night spans (ms) clipped to [startMs, endMs]: from each sunset to the next sunrise. Before the first
 * sunrise in the list it is night; after the last sunset it is night. Sunrise and sunset are UTC instants.
 */
export function dayNight(days: { sunrise: string | null; sunset: string | null }[], startMs: number, endMs: number): NightSpan[] {
  const events: { t: number; rise: boolean }[] = [];
  for (const d of days) {
    if (d.sunrise) events.push({ t: Date.parse(d.sunrise), rise: true });
    if (d.sunset) events.push({ t: Date.parse(d.sunset), rise: false });
  }
  events.sort((a, b) => a.t - b.t);
  const out: NightSpan[] = [];
  let nightFrom: number | null = events.length && events[0].rise ? startMs : null;
  for (const e of events) {
    if (!e.rise) nightFrom = e.t;
    else if (nightFrom !== null) {
      out.push({ start: nightFrom, end: e.t });
      nightFrom = null;
    }
  }
  if (nightFrom !== null) out.push({ start: nightFrom, end: endMs });
  return out.map((s) => ({ start: Math.max(s.start, startMs), end: Math.min(s.end, endMs) })).filter((s) => s.end > s.start);
}

export type IconKind = 'clear' | 'partly' | 'cloudy' | 'fog' | 'drizzle' | 'rain' | 'snow' | 'storm' | 'unknown';
export interface WeatherIcon {
  kind: IconKind;
  label: string;
}

/** WMO weather interpretation codes (Open-Meteo `weather_code`) to an icon kind and short label. */
const WMO: Record<number, { kind: IconKind; label: string }> = {
  0: { kind: 'clear', label: 'Clear' },
  1: { kind: 'partly', label: 'Mostly clear' },
  2: { kind: 'partly', label: 'Partly cloudy' },
  3: { kind: 'cloudy', label: 'Overcast' },
  45: { kind: 'fog', label: 'Fog' },
  48: { kind: 'fog', label: 'Fog' },
  51: { kind: 'drizzle', label: 'Drizzle' },
  53: { kind: 'drizzle', label: 'Drizzle' },
  55: { kind: 'drizzle', label: 'Drizzle' },
  56: { kind: 'drizzle', label: 'Freezing drizzle' },
  57: { kind: 'drizzle', label: 'Freezing drizzle' },
  61: { kind: 'rain', label: 'Rain' },
  63: { kind: 'rain', label: 'Rain' },
  65: { kind: 'rain', label: 'Heavy rain' },
  66: { kind: 'rain', label: 'Freezing rain' },
  67: { kind: 'rain', label: 'Freezing rain' },
  71: { kind: 'snow', label: 'Snow' },
  73: { kind: 'snow', label: 'Snow' },
  75: { kind: 'snow', label: 'Heavy snow' },
  77: { kind: 'snow', label: 'Snow grains' },
  80: { kind: 'rain', label: 'Showers' },
  81: { kind: 'rain', label: 'Showers' },
  82: { kind: 'rain', label: 'Heavy showers' },
  85: { kind: 'snow', label: 'Snow showers' },
  86: { kind: 'snow', label: 'Snow showers' },
  95: { kind: 'storm', label: 'Thunderstorm' },
  96: { kind: 'storm', label: 'Thunderstorm, hail' },
  99: { kind: 'storm', label: 'Thunderstorm, hail' }
};

/** Icon and label for a WMO code. Null when there is no code. */
export function iconFor(code: number | null | undefined): WeatherIcon | null {
  if (code === null || code === undefined) return null;
  const hit = WMO[code] ?? { kind: 'unknown' as const, label: 'Unknown' };
  return { kind: hit.kind, label: hit.label };
}
