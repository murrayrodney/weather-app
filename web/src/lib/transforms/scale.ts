import type { YtdStation } from '$lib/api/types';
import { fieldOf, type Metric } from './slots';

/** Largest plotted value for one station and metric (for the optional shared y scale). */
export function seriesMax(st: YtdStation, metric: Metric): number {
  const field = fieldOf(metric);
  let m = 0;
  for (const p of st.current[field]) m = Math.max(m, p.v);
  for (const y of st.prior_years) for (const p of y[field] ?? []) m = Math.max(m, p.v);
  for (const b of st.bands?.[field] ?? []) for (const v of b.p) m = Math.max(m, v);
  return m;
}
