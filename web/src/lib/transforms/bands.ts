import type { YtdStation } from '$lib/api/types';
import type { Metric } from './slots';
import { fieldOf } from './slots';
import type { Pt } from './series';

export interface BandLayers {
  outer?: { lower: (Pt | null)[]; upper: (Pt | null)[] };
  inner?: { lower: (Pt | null)[]; upper: (Pt | null)[] };
  median?: (Pt | null)[];
}

function column(bands: NonNullable<YtdStation['bands']>, metric: Metric, percentile: number, slots: string[]): (Pt | null)[] | null {
  const idx = bands.percentiles.indexOf(percentile);
  const rows = bands[fieldOf(metric)];
  if (idx < 0 || !rows) return null;
  const byMd = new Map(rows.map((r) => [r.md, r.p[idx]]));
  return slots.map((md, x) => {
    const v = byMd.get(md);
    return v === undefined || v === null ? null : { x, y: v };
  });
}

/** Outer = lowest and highest listed; inner = 25/75 if present; median = 50 if present. Rain and GDD are independent. */
export function bandPaths(bands: YtdStation['bands'], metric: Metric, slots: string[]): BandLayers {
  if (!bands || !bands[fieldOf(metric)]) return {};
  const ps = [...bands.percentiles].sort((a, b) => a - b);
  const out: BandLayers = {};
  if (ps.length >= 2) {
    const lo = column(bands, metric, ps[0], slots);
    const hi = column(bands, metric, ps[ps.length - 1], slots);
    if (lo && hi) out.outer = { lower: lo, upper: hi };
  }
  const l25 = column(bands, metric, 25, slots);
  const u75 = column(bands, metric, 75, slots);
  if (l25 && u75) out.inner = { lower: l25, upper: u75 };
  const med = column(bands, metric, 50, slots);
  if (med) out.median = med;
  return out;
}
