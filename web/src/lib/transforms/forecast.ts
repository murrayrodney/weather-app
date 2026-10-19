// Forecast transforms. No percentile maths here: the API computes every p10/p50/p90.
import type { Band, BandSeries, Forecast, ForecastChanges } from '$lib/api/types';
import type { Pt } from './series';

export interface ForecastBand {
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

export function lineFrom(values: (number | null)[]): (Pt | null)[] {
  return values.map((v, i) => pt(v, i));
}

export interface HourlyWindow {
  time: string[];
  deterministic: { temperature_f: (number | null)[]; precip_in: (number | null)[]; prob_precip: (number | null)[] } | null;
  ensemble: { temperature_f: BandSeries; precip_in: BandSeries } | null;
}

/** Keeps the first `days` days (24 h each, from the first timestamp) of both hourly series. */
export function hourlyWindow(f: Forecast, days: number): HourlyWindow {
  const time = f.deterministic?.hourly.time ?? f.ensemble?.hourly.time ?? [];
  const t0 = time.length ? Date.parse(time[0]) : 0;
  const end = t0 + days * 86400000;
  let n = time.filter((t) => Date.parse(t) < end).length;
  if (n === 0) n = 0;
  const cutS = <T>(a: T[]) => a.slice(0, n);
  const cutB = (b: BandSeries): BandSeries => ({ p10: cutS(b.p10), p50: cutS(b.p50), p90: cutS(b.p90) });
  const d = f.deterministic?.hourly;
  const e = f.ensemble?.hourly;
  return {
    time: cutS(time),
    deterministic: d ? { temperature_f: cutS(d.temperature_f), precip_in: cutS(d.precip_in), prob_precip: cutS(d.prob_precip) } : null,
    ensemble: e ? { temperature_f: cutB(e.temperature_f), precip_in: cutB(e.precip_in) } : null
  };
}

export type ChangeMetric = 'precip_in' | 'prob_precip_max' | 'tmax_f' | 'tmin_f';

/** One series per lead (index = lead day), x = day index; nulls stay null. */
export function leadSeries(changes: ForecastChanges, metric: ChangeMetric): (Pt | null)[][] {
  return changes.lead_days.map((_, li) => changes.days.map((d, x) => pt(d[metric][li] ?? null, x)));
}

export function leadLabel(n: number): string {
  if (n === 0) return 'latest';
  return n === 1 ? '1 day earlier' : `${n} days earlier`;
}

export function spreadAcrossLeads(day: ForecastChanges['days'][number], metric: ChangeMetric): { min: number; max: number } | null {
  const vals = day[metric].filter((v): v is number => v !== null && v !== undefined);
  if (vals.length < 2) return null;
  return { min: Math.min(...vals), max: Math.max(...vals) };
}
