import { scaleLinear } from 'd3-scale';
import { area, line } from 'd3-shape';
import type { Pt } from '$lib/transforms/series';

export const W = 520;
export const H = 280;
export const M = { l: 46, r: 10, t: 10, b: 28 } as const;

export function makeScales(nX: number, yMax: number) {
  const x = scaleLinear()
    .domain([0, Math.max(nX - 1, 1)])
    .range([M.l, W - M.r]);
  const y = scaleLinear()
    .domain([0, yMax > 0 ? yMax : 1])
    .range([H - M.b, M.t])
    .nice();
  return { x, y };
}

type Scale = (v: number) => number;

/** Line path over points; null entries break the path. Returns '' when there is nothing to draw. */
export function linePath(pts: (Pt | null)[], x: Scale, y: Scale): string {
  const gen = line<Pt | null>()
    .defined((d): d is Pt => d !== null)
    .x((d) => x(d!.x))
    .y((d) => y(d!.y));
  return gen(pts) ?? '';
}

/** Filled ribbon between two aligned series; steps where either is null break the ribbon. */
export function ribbonPath(lower: (Pt | null)[], upper: (Pt | null)[], x: Scale, y: Scale): string {
  const idx = lower.map((_, i) => i);
  const gen = area<number>()
    .defined((i) => lower[i] !== null && upper[i] !== null)
    .x((i) => x(lower[i]!.x))
    .y0((i) => y(lower[i]!.y))
    .y1((i) => y(upper[i]!.y));
  return gen(idx) ?? '';
}
