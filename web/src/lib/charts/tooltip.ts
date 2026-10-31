// Chart tooltips (proposal 08 section 2): placement, box size and row formatting. Pure: no DOM and no getBBox, so the
// flip rule, the clamp and the text widths are unit-tested. ChartTooltip.svelte draws the box from these numbers.
import type { LegendMark } from './legend';

export const TIP_GAP = 12; // px between the crosshair and the box
export const TIP_PAD = 8; // px inside the box
export const TIP_TITLE = 20; // px for the title row (the hour)
export const TIP_ROW = 16; // px per value row (12 px text)
export const TIP_SWATCH = 14; // px across a row's swatch
export const TIP_LABEL_X = TIP_PAD + 20; // x of a row's label: the swatch column, then a gap
const VALUE_GAP = 12; // px between the longest label and the longest value
// Generous, as in legend.ts: a 12 px system-font character is about 6.5 px.
const CHAR_PX = 7;

/** One value row. `mark` and `cls` draw the same swatch as the legend for that series; rows without `mark` have none. */
export interface TipRow {
  label: string;
  value: string;
  mark?: LegendMark;
  cls?: string;
}

/** One panel's tooltip: the hour as its title, then the panel's rows. */
export interface TipBox {
  title: string;
  rows: TipRow[];
}

interface PlaceInput {
  x: number; // crosshair x (SVG units)
  y: number; // anchor y: the box is centred on it, then clamped
  w: number; // box width
  h: number; // box height
  left: number; // left edge of the plot: the box never starts left of this
  plotW: number; // right edge of the plot: the box flips left when it would pass this
  top: number; // the panel's top and bottom: the box stays inside them vertically
  bottom: number;
}

/**
 * Where the box's top-left corner goes. Default: TIP_GAP to the right of the crosshair. It flips to the left of the
 * crosshair when the right-hand placement would pass the plot's right edge, and is clamped so it never starts left of
 * the plot. Vertically it is centred on `y` and kept inside [top, bottom]; if the box is taller than the panel, its top
 * stays at `top`.
 */
export function placeTooltip(p: PlaceInput): { tx: number; ty: number } {
  const right = p.x + TIP_GAP + p.w;
  const flipped = right > p.plotW ? p.x - TIP_GAP - p.w : p.x + TIP_GAP;
  const tx = Math.max(p.left, flipped);
  const ty = Math.max(p.top, Math.min(p.y - p.h / 2, p.bottom - p.h));
  return { tx, ty };
}

/** Box height from its row count (title plus rows). */
export function tipHeight(rowCount: number): number {
  return TIP_PAD * 2 + TIP_TITLE + rowCount * TIP_ROW;
}

/** Box width from the text: the title, or the longest label, gap and longest value, whichever is wider. */
export function tipWidth(box: TipBox): number {
  const longestLabel = Math.max(0, ...box.rows.map((r) => r.label.length));
  const longestValue = Math.max(0, ...box.rows.map((r) => r.value.length));
  const rowsWide = TIP_LABEL_X + longestLabel * CHAR_PX + VALUE_GAP + longestValue * CHAR_PX + TIP_PAD;
  const titleWide = TIP_PAD * 2 + box.title.length * CHAR_PX;
  return Math.max(rowsWide, titleWide);
}

/** A value as the readouts print it: fixed decimals and a unit, or "no value" when the step is missing. */
export function fmtValue(v: number | null | undefined, digits: number, suffix: string): string {
  return typeof v === 'number' ? `${v.toFixed(digits)}${suffix}` : 'no value';
}

/** A p10-p90 pair as "58 F to 70 F", the wording the readouts use. */
export function fmtRange(lo: number | null | undefined, hi: number | null | undefined, digits: number, suffix: string): string {
  return `${fmtValue(lo, digits, suffix)} to ${fmtValue(hi, digits, suffix)}`;
}

/** A YYYY-MM-DD day as a tooltip title: "Fri Jul 10". Built in UTC from parts, so the day never shifts with the zone. */
export function tipDate(ymd: string): string {
  const [y, m, d] = ymd.split('-').map(Number);
  const parts = new Intl.DateTimeFormat('en-US', { timeZone: 'UTC', weekday: 'short', month: 'short', day: 'numeric' }).formatToParts(
    new Date(Date.UTC(y, m - 1, d))
  );
  const get = (type: Intl.DateTimeFormatPartTypes) => parts.find((p) => p.type === type)?.value ?? '';
  return `${get('weekday')} ${get('month')} ${get('day')}`;
}

/** A tooltip box as one readout sentence, for the chart's aria-live line: "Fri Jul 10: High 80 F, Low 50 F." */
export function tipText(box: TipBox): string {
  if (box.rows.length === 0) return `${box.title}: no values.`;
  return `${box.title}: ${box.rows.map((r) => `${r.label} ${r.value}`).join(', ')}.`;
}

/** The hour of a UTC instant in a zone, as the tooltip title: "Tue Oct 27, 3 PM". Built from parts, so no comma after the weekday. */
export function tipHour(ms: number, zone: string): string {
  const parts = new Intl.DateTimeFormat('en-US', {
    timeZone: zone,
    weekday: 'short',
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    hour12: true
  }).formatToParts(new Date(ms));
  const get = (type: Intl.DateTimeFormatPartTypes) => parts.find((p) => p.type === type)?.value ?? '';
  return `${get('weekday')} ${get('month')} ${get('day')}, ${get('hour')} ${get('dayPeriod')}`;
}
