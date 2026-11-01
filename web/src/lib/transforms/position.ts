import type { YtdStation } from '$lib/api/types';
import type { Metric } from './slots';
import { fieldOf } from './slots';

export function ordinal(n: number): string {
  const v = n % 100;
  if (v >= 11 && v <= 13) return `${n}th`;
  switch (n % 10) {
    case 1:
      return `${n}st`;
    case 2:
      return `${n}nd`;
    case 3:
      return `${n}rd`;
    default:
      return `${n}th`;
  }
}

/** Where the current value sits among the band values at the same day. `values` align with `percentiles`. */
export function describePosition(current: number, values: number[], percentiles: number[], yearsIncluded: number): string | null {
  if (values.length === 0 || values.length !== percentiles.length) return null;
  const order = percentiles.map((p, i) => ({ p, v: values[i] })).sort((a, b) => a.p - b.p);
  let text: string;
  const eq = order.find((o) => o.v === current);
  if (eq) text = `at about the ${ordinal(eq.p)} percentile`;
  else if (current < order[0].v) text = `below the ${ordinal(order[0].p)} percentile`;
  else if (current > order[order.length - 1].v) text = `above the ${ordinal(order[order.length - 1].p)} percentile`;
  else {
    let i = 0;
    while (i < order.length - 1 && !(current > order[i].v && current < order[i + 1].v)) i++;
    text = `between the ${ordinal(order[i].p)} and ${ordinal(order[i + 1].p)} percentile`;
  }
  return `${text} of ${yearsIncluded} years of airport record`;
}

/** Position text for a station, or null when it is not comparable (no bands, no complete day, or a mid-season start). */
export function positionFor(st: YtdStation, metric: Metric, windowStart: string): string | null {
  const field = fieldOf(metric);
  const rows = st.bands?.[field];
  const lc = st.current.last_complete_date;
  if (!st.bands || !rows || lc === null) return null;
  const first = st.current.first_date;
  if (first !== null && first.slice(5) > windowStart) return null;
  const md = lc.slice(5);
  const cur = st.current[field].find((p) => p.md === md);
  const band = rows.find((r) => r.md === md);
  if (!cur || !band) return null;
  return describePosition(cur.v, band.p, st.bands.percentiles, st.bands.years_included[metric]);
}
