export interface YMD {
  year: number;
  month: number;
  day: number;
}

export function monthOfMd(md: string): number {
  return Number(md.slice(0, 2));
}

export function formatInZone(isoUtc: string, ianaTz: string): string {
  const d = new Date(isoUtc);
  if (Number.isNaN(d.getTime())) return '';
  return new Intl.DateTimeFormat('en-US', { timeZone: ianaTz, dateStyle: 'medium', timeStyle: 'short' }).format(d);
}

/** The calendar date at `now` in the given IANA zone. */
export function zoneDate(ianaTz: string, now: Date = new Date()): YMD {
  const parts = new Intl.DateTimeFormat('en-US', { timeZone: ianaTz, year: 'numeric', month: '2-digit', day: '2-digit' }).formatToParts(now);
  const get = (t: string) => Number(parts.find((p) => p.type === t)?.value);
  return { year: get('year'), month: get('month'), day: get('day') };
}

export function localYear(ianaTz: string, now: Date = new Date()): number {
  return zoneDate(ianaTz, now).year;
}

const p2 = (n: number) => String(n).padStart(2, '0');

export function localMd(ianaTz: string, now: Date = new Date()): string {
  const d = zoneDate(ianaTz, now);
  return `${p2(d.month)}-${p2(d.day)}`;
}

export function localDate(ianaTz: string, now: Date = new Date()): string {
  const d = zoneDate(ianaTz, now);
  return `${d.year}-${p2(d.month)}-${p2(d.day)}`;
}

/** Adds days to a YYYY-MM-DD string using UTC arithmetic only (no local-time Date maths). */
export function addDays(ymd: string, n: number): string {
  const [y, m, d] = ymd.split('-').map(Number);
  const t = new Date(Date.UTC(y, m - 1, d + n));
  return `${t.getUTCFullYear()}-${p2(t.getUTCMonth() + 1)}-${p2(t.getUTCDate())}`;
}

/** Whole days from a to b (YYYY-MM-DD). */
export function daysBetween(a: string, b: string): number {
  const [y1, m1, d1] = a.split('-').map(Number);
  const [y2, m2, d2] = b.split('-').map(Number);
  return Math.round((Date.UTC(y2, m2 - 1, d2) - Date.UTC(y1, m1 - 1, d1)) / 86400000);
}

export function formatDateShort(ymd: string): string {
  const [y, m, d] = ymd.split('-').map(Number);
  return new Intl.DateTimeFormat('en-US', { timeZone: 'UTC', month: 'short', day: 'numeric' }).format(new Date(Date.UTC(y, m - 1, d)));
}

export function formatDateLong(ymd: string): string {
  const [y, m, d] = ymd.split('-').map(Number);
  return new Intl.DateTimeFormat('en-US', { timeZone: 'UTC', dateStyle: 'medium' }).format(new Date(Date.UTC(y, m - 1, d)));
}
