import { GDD_BASE_RANGE, GDD_DEFAULTS } from '$lib/config';
import type { GddSettings } from '$lib/api/types';

const DAYS_IN_MONTH = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

export function validMd(s: string | null | undefined): s is string {
  const m = /^(\d{2})-(\d{2})$/.exec(s ?? '');
  if (!m) return false;
  const mo = Number(m[1]);
  const d = Number(m[2]);
  return mo >= 1 && mo <= 12 && d >= 1 && d <= DAYS_IN_MONTH[mo - 1];
}

export function validBase(n: number): boolean {
  return Number.isInteger(n) && n >= GDD_BASE_RANGE.min && n <= GDD_BASE_RANGE.max;
}

export const DEFAULT_GDD: GddSettings = { method: GDD_DEFAULTS.method, base_f: GDD_DEFAULTS.base_f, start: GDD_DEFAULTS.start };

/** The API rejects a gdd_start after the window end (repo/ytd.rs: "gdd_start ... not after end"). */
export function startAllowed(start: string, windowEnd: string): boolean {
  return validMd(start) && start <= windowEnd;
}

/** Invalid or out-of-range values (including a start after `windowEnd`) fall back to the defaults. */
export function parseGddSettings(sp: URLSearchParams, windowEnd: string = '12-31'): GddSettings {
  const method = sp.get('gdd_method');
  const baseRaw = sp.get('gdd_base');
  const start = sp.get('gdd_start');
  const base = baseRaw !== null && /^\d+$/.test(baseRaw) ? Number(baseRaw) : NaN;
  return {
    method: method === 'simple_avg' || method === 'clamped_50_86' ? method : DEFAULT_GDD.method,
    base_f: validBase(base) ? base : DEFAULT_GDD.base_f,
    start: start !== null && startAllowed(start, windowEnd) ? start : DEFAULT_GDD.start
  };
}

/** Writes only values that differ from the defaults. */
export function toSearchParams(s: GddSettings): URLSearchParams {
  const sp = new URLSearchParams();
  if (s.method !== DEFAULT_GDD.method) sp.set('gdd_method', s.method);
  if (s.base_f !== DEFAULT_GDD.base_f) sp.set('gdd_base', String(s.base_f));
  if (s.start !== DEFAULT_GDD.start) sp.set('gdd_start', s.start);
  return sp;
}
