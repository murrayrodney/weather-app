// Facet grid geometry, the shared y domain and the panel titles for "How the forecast changed" (proposal 09 sections
// 2 to 3 and 8). Pure: no DOM, so the chart and the tests use the same numbers. The lead-issued rule lives in
// transforms/forecast.ts (leadIssued) with the other transforms.
import { scaleLinear } from 'd3-scale';
import { addDays } from '$lib/transforms/dates';
import type { ChangeMetric } from '$lib/transforms/forecast';

export const FACET_MIN_CELL = 150; // px, the narrowest panel that still reads
export const FACET_GAP = 12; // px between panels, across and down
export const FACET_CELL_H = 150; // px, title, plot and tick labels
// Inside one panel: room for the y tick labels (left), the title (top) and the x labels (bottom). A panel without
// tick labels (every column but the first) uses `bare` on the left, so its plot is wider.
export const FACET_PAD = { left: 30, bare: 6, right: 6, top: 26, bottom: 22 } as const;

export interface FacetOpts {
  minCell?: number;
  gap?: number;
  cellH?: number;
}

export interface FacetCell {
  index: number; // panel index (target day order)
  col: number;
  row: number;
  x: number; // px offset of the panel's left edge
  y: number; // px offset of the panel's top edge
}

export interface FacetLayout {
  cols: number;
  rows: number;
  cellW: number;
  cellH: number;
  cells: FacetCell[];
}

/**
 * Balanced grid for `n` panels in `width` px. The most columns that fit at `minCell` set the row count, then the
 * columns are shared out evenly, so 7 panels give 4 + 3 rather than 6 + 1. Panels fill row by row.
 */
export function facetLayout(width: number, n: number, opts: FacetOpts = {}): FacetLayout {
  const minCell = opts.minCell ?? FACET_MIN_CELL;
  const gap = opts.gap ?? FACET_GAP;
  const cellH = opts.cellH ?? FACET_CELL_H;
  if (n <= 0) return { cols: 0, rows: 0, cellW: 0, cellH, cells: [] };
  const maxCols = Math.max(1, Math.floor((width + gap) / (minCell + gap)));
  const rows = Math.ceil(n / maxCols);
  const cols = Math.ceil(n / rows);
  const cellW = Math.max(0, Math.floor((width - gap * (cols - 1)) / cols));
  const cells = Array.from({ length: n }, (_, index) => {
    const col = index % cols;
    const row = Math.floor(index / cols);
    return { index, col, row, x: col * (cellW + gap), y: row * (cellH + gap) };
  });
  return { cols, rows, cellW, cellH, cells };
}

/** A panel's title: "Today", "Tomorrow", "Yesterday", otherwise the weekday and day ("Mon 3"). Station-local dates. */
export function facetTitle(date: string, today: string): string {
  if (date === today) return 'Today';
  if (date === addDays(today, 1)) return 'Tomorrow';
  if (date === addDays(today, -1)) return 'Yesterday';
  const [y, m, d] = date.split('-').map(Number);
  const parts = new Intl.DateTimeFormat('en-US', { timeZone: 'UTC', weekday: 'short', day: 'numeric' }).formatToParts(new Date(Date.UTC(y, m - 1, d)));
  const get = (type: Intl.DateTimeFormatPartTypes) => parts.find((p) => p.type === type)?.value ?? '';
  return `${get('weekday')} ${get('day')}`;
}

export interface PanelX {
  pl: number; // the plot's left edge inside the panel (wider where the y labels are)
  pr: number; // the plot's right edge
  slot: number; // px per lead slot
  xAt: (i: number) => number; // centre of slot i, for the marks and the crosshair
  stepAt: (px: number) => number; // the fractional slot under a panel-local x (the pointer rounds and clamps it)
}

/** One panel's x scale: the lead slots, oldest first, across the plot. Each column has its own left edge (the first has y labels). */
export function panelX(cellW: number, showAxis: boolean, n: number): PanelX {
  const pl = showAxis ? FACET_PAD.left : FACET_PAD.bare;
  const pr = cellW - FACET_PAD.right;
  const slot = (pr - pl) / Math.max(n, 1);
  return { pl, pr, slot, xAt: (i) => pl + (i + 0.5) * slot, stepAt: (px) => (px - pl) / slot - 0.5 };
}

export interface FacetPoint {
  value: number | null;
  issued: boolean;
}

/**
 * The one y domain (d3 linear, `nice()`d) shared by every panel of a metric. Only issued values count. Rain is
 * zero-based; chance of rain is 0 to 100; temperature is the lowest to highest issued value, padded 2 and floored
 * or ceiled to a nice step, as the previous chart did.
 */
export function sharedYDomain(metric: ChangeMetric, points: FacetPoint[]): [number, number] {
  if (metric === 'prob_precip_max') return [0, 100];
  const vals = points.flatMap((p) => (p.issued && p.value !== null ? [p.value] : []));
  if (vals.length === 0) return [0, 1];
  let lo = Math.min(...vals);
  let hi = Math.max(...vals);
  if (metric === 'precip_in') {
    lo = 0;
    hi = Math.max(hi * 1.1, 0.05);
  } else {
    lo = Math.floor(lo - 2);
    hi = hi + 2;
  }
  const d = scaleLinear().domain([lo, hi]).nice().domain();
  return [d[0], d[1]];
}
