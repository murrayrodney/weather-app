// Layout for PanelStack: vertically stacked panels on one shared time x axis (UTC ms), each panel with its own y axis.
// Pure geometry (d3-scale only, no DOM), so the SVG components only draw what this returns.
import { scaleLinear } from 'd3-scale';
import type { IconKind } from '$lib/transforms/forecast';
import { legendLayout, LEGEND_ROW, type LegendItem } from './legend';

const STACK_LEFT = 52; // room for y tick labels
export const STACK_RIGHT = 16; // plot margin when no panel has a right-hand axis (dashboard rows)
export const STACK_RIGHT_AXES = 192; // plot margin for up to three right-hand axes (total, percent, snow; or pressure)
const RIGHT_COLUMN = 60; // x offset of each right-hand axis from the previous one
const STACK_TOP = 22; // room for the "Now" label above the first panel's header
const HEAD_PAD = 8; // px under a panel's legend rows, above its plot (also holds the right-hand unit labels)
const PANEL_GAP = 6; // px between one panel's plot and the next panel's header
const AXIS_HEIGHT = 26; // x tick labels under the last panel
export const STRIP_HEIGHT = 100; // day strip above the panels (date, icon, text, high/low, precipitation, snow)

/** The arrow that marks wind direction (drawn pointing up at the origin; the caller rotates it). Also the legend swatch. */
export const WIND_ARROW = 'M0 -6L3.6 2H1.2V6H-1.2V2H-3.6Z';

/** One right-hand y axis: its own domain (extended to round numbers) and unit label. */
export interface AxisSpec {
  domain: [number, number];
  unit: string;
  digits?: number; // decimal places on the tick labels (default 0)
}

export interface PanelSpec {
  name: string; // temperature, precipitation, sky or wind: names the legend's test id
  height: number; // px
  domain: [number, number]; // y values; the scale is extended to round numbers
  unit: string; // shown inside the panel's top-left corner
  right?: AxisSpec[]; // right-hand axes, first one nearest the plot
  legend: LegendItem[]; // the series this panel draws, in the header row(s) above its plot
}

interface RightAxisLayout {
  unit: string;
  digits: number;
  x: number; // x of the tick labels (start-anchored)
  y: (v: number) => number;
  ticks: number[];
}

interface PanelLayout {
  name: string;
  head: number; // top of the legend header
  top: number; // top of the plot
  bottom: number;
  unit: string;
  legend: LegendItem[];
  y: (v: number) => number;
  ticks: number[];
  right: RightAxisLayout[];
}

/** A marker drawn in a panel (wind direction arrows): centre and rotation in degrees clockwise from up. */
export interface Glyph {
  x: number;
  y: number;
  rot: number;
}

export interface StackLayout {
  width: number;
  left: number;
  right: number;
  x: (ms: number) => number;
  invertX: (px: number) => number;
  panels: PanelLayout[];
  axisY: number; // baseline for x tick labels
  height: number; // total SVG height
}

/** One SVG `path` drawn inside a panel (`cls` is a CSS class, `d` the path data). `hatch` fills it with the snow pattern. */
export interface Layer {
  cls: string;
  d: string;
  hatch?: boolean;
}

/** One cell of the day strip: local date, its x extent on the stack, and the values to print. */
export interface StripCell {
  key: string;
  date: string;
  x0: number;
  x1: number;
  cx: number;
  label: string | null; // condition text
  icon: IconKind | null;
  hi: number | null;
  lo: number | null;
  precip: number | null;
  snow: number | null; // inches; shown only when above zero
}

export function layoutStack(width: number, startMs: number, endMs: number, specs: PanelSpec[], rightMargin = STACK_RIGHT): StackLayout {
  const left = STACK_LEFT;
  const right = width - rightMargin;
  const x = scaleLinear().domain([startMs, endMs]).range([left, right]);
  let head = STACK_TOP;
  const panels: PanelLayout[] = specs.map((s) => {
    // The header holds this panel's legend (rows wrap to the plot width) and its right-hand unit labels.
    const rows = legendLayout(s.legend, right - left).rows;
    const top = head + rows * LEGEND_ROW + HEAD_PAD;
    const bottom = top + s.height;
    const y = scaleLinear().domain(s.domain).range([bottom, top]).nice();
    const rightAxes: RightAxisLayout[] = (s.right ?? []).map((a, k) => {
      const ay = scaleLinear().domain(a.domain).range([bottom, top]).nice();
      return { unit: a.unit, digits: a.digits ?? 0, x: right + 6 + k * RIGHT_COLUMN, y: (v: number) => ay(v), ticks: ay.ticks(5) };
    });
    const panel: PanelLayout = {
      name: s.name,
      head,
      top,
      bottom,
      unit: s.unit,
      legend: s.legend,
      y: (v: number) => y(v),
      ticks: y.ticks(5),
      right: rightAxes
    };
    head = bottom + PANEL_GAP;
    return panel;
  });
  const lastBottom = panels.length ? panels[panels.length - 1].bottom : STACK_TOP;
  return {
    width,
    left,
    right,
    x: (ms: number) => x(ms),
    invertX: (px: number) => x.invert(px),
    panels,
    axisY: lastBottom + AXIS_HEIGHT - 8,
    height: lastBottom + AXIS_HEIGHT
  };
}
