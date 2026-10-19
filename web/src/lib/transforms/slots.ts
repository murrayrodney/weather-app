import type { YtdStation } from '$lib/api/types';

export type Metric = 'rain' | 'gdd';
export type Field = 'rain_cum_in' | 'gdd_cum';

export function fieldOf(metric: Metric): Field {
  return metric === 'rain' ? 'rain_cum_in' : 'gdd_cum';
}

/** Sorted union of md keys over bands, the current year and every non-null prior year. Position = index. */
export function buildSlots(ytd: YtdStation, metric: Metric): string[] {
  const field = fieldOf(metric);
  const set = new Set<string>();
  for (const b of ytd.bands?.[field] ?? []) set.add(b.md);
  for (const p of ytd.current[field]) set.add(p.md);
  for (const y of ytd.prior_years) for (const p of y[field] ?? []) set.add(p.md);
  set.delete('02-29');
  return [...set].sort();
}
