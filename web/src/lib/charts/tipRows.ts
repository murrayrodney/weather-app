// Tooltip rows for the charts (proposal 08 section 2.2). Pure: each builder returns the rows of one hovered step. Labels
// are the chart's legend labels, and each row's swatch class is the legend's class for that series, so a tooltip reads
// as a legend for the values under the pointer. ChartTooltip.svelte draws the boxes from these rows.
import type { BandSeries } from '$lib/api/types';
import type { ChangeMetric, DayLead } from '$lib/transforms/forecast';
import { fmtRange, fmtValue, type TipRow } from './tooltip';

/** One day of the daily view: the deterministic row and the ensemble row for the same date (either may be absent). */
interface DetDay {
  tmax_f: number | null;
  tmin_f: number | null;
  precip_in: number | null;
  prob_precip_max: number | null;
}
interface EnsDay {
  tmax_f: { p10: number | null; p50: number | null; p90: number | null };
  tmin_f: { p10: number | null; p50: number | null; p90: number | null };
  precip_in: { p10: number | null; p50: number | null; p90: number | null };
  prob_precip: number | null;
}

/** The daily view's temperature rows: High, High range, Low, Low range, Median (the same five labels as its legend). */
export function dailyTempRows(det: DetDay | undefined, ens: EnsDay | undefined): TipRow[] {
  const rows: TipRow[] = [];
  if (det) rows.push({ label: 'High', value: fmtValue(det.tmax_f, 0, ' F'), mark: 'line', cls: 'line-tmax' });
  if (ens) rows.push({ label: 'High p10-p90', value: fmtRange(ens.tmax_f.p10, ens.tmax_f.p90, 0, ' F'), mark: 'band', cls: 'band-tmax' });
  if (det) rows.push({ label: 'Low', value: fmtValue(det.tmin_f, 0, ' F'), mark: 'line', cls: 'line-tmin' });
  if (ens) rows.push({ label: 'Low p10-p90', value: fmtRange(ens.tmin_f.p10, ens.tmin_f.p90, 0, ' F'), mark: 'band', cls: 'band-tmin' });
  if (ens) {
    rows.push({
      label: 'Median',
      value: `high ${fmtValue(ens.tmax_f.p50, 0, ' F')}, low ${fmtValue(ens.tmin_f.p50, 0, ' F')}`,
      mark: 'line',
      cls: 'line-median'
    });
  }
  return rows;
}

/**
 * The daily view's precipitation rows: the ensemble range and median (or the deterministic amount when there is no
 * ensemble), then the chance of rain. The chance is text on the chart, so its row has no swatch.
 */
export function dailyPrecipRows(det: DetDay | undefined, ens: EnsDay | undefined): TipRow[] {
  const rows: TipRow[] = [];
  if (ens) {
    rows.push({ label: 'Range p10-p90', value: fmtRange(ens.precip_in.p10, ens.precip_in.p90, 2, ' in'), mark: 'band', cls: 'band-outer' });
    rows.push({ label: 'Median', value: fmtValue(ens.precip_in.p50, 2, ' in'), mark: 'line', cls: 'line-median' });
  } else if (det) {
    rows.push({ label: 'Precipitation', value: fmtValue(det.precip_in, 2, ' in'), mark: 'line', cls: 'line-forecast' });
  }
  // The same fallback the chart's labels use: ensemble chance first, then the deterministic daily maximum.
  const chance = ens?.prob_precip ?? det?.prob_precip_max ?? null;
  rows.push({ label: 'Chance of rain', value: fmtValue(chance, 0, '%') });
  return rows;
}

const FACET_VALUE: Record<ChangeMetric, (v: number) => string> = {
  precip_in: (v) => `${v.toFixed(2)} in`,
  prob_precip_max: (v) => `${Math.round(v)}%`,
  tmax_f: (v) => `${Math.round(v)}°`,
  tmin_f: (v) => `${Math.round(v)}°`
};

/**
 * The changes grid's tooltip rows for one panel (proposal 09 decision 6: terse). The hovered lead as `3d before 49°`,
 * then `now 49°` (lead 0, the latest run). At lead 0 the hovered row is the same as `now`, so only `now` is listed. A
 * lead that is not issued shows `–`, and a null value shows `-` (the table's own marks for the two); no value is
 * invented. The swatch is the tone of the metric.
 */
export function facetRows(metric: ChangeMetric, hovered: DayLead, latest: DayLead): TipRow[] {
  const value = (l: DayLead) => (!l.issued ? '–' : l.value === null ? '-' : FACET_VALUE[metric](l.value));
  // Rain is bars, so its swatch is the bar class (fill --c-rain-bar, the rain tone). The other metrics are lines, whose
  // stroke is --tone from the chart's tone class.
  const swatch = metric === 'precip_in' ? { mark: 'bar' as const, cls: 'bar-precip' } : { mark: 'line' as const, cls: 'line-facet' };
  const rows: TipRow[] = [];
  if (hovered.lead > 0) rows.push({ label: `${hovered.lead}d before`, value: value(hovered), ...swatch });
  rows.push({ label: 'now', value: value(latest), ...swatch });
  return rows;
}

/** The values of one cumulative-chart step; each is null when the step has no value there. */
interface CumulativeStep {
  thisYear: number | null; // a complete day's value
  partial: number | null; // the open day's value
  median: number | null;
  bandLo: number | null; // the outer percentile band, lowest and highest listed
  bandHi: number | null;
  priors: { year: number; v: number | null }[];
}

/**
 * The cumulative chart's rows: this year, today (partial), median, percentile band, then one row per prior year drawn.
 * A row is left out when its value is missing at this step, so the box lists only what the chart draws there.
 */
export function cumulativeRows(s: CumulativeStep, digits: number, suffix: string): TipRow[] {
  const rows: TipRow[] = [];
  if (s.thisYear !== null) rows.push({ label: 'This year', value: fmtValue(s.thisYear, digits, suffix), mark: 'line', cls: 'line-current' });
  if (s.partial !== null) {
    rows.push({ label: 'Today (partial)', value: fmtValue(s.partial, digits, suffix), mark: 'line', cls: 'line-current-partial' });
  }
  if (s.median !== null) rows.push({ label: 'Median', value: fmtValue(s.median, digits, suffix), mark: 'line', cls: 'line-median' });
  if (s.bandLo !== null && s.bandHi !== null) {
    rows.push({ label: 'Percentile band', value: fmtRange(s.bandLo, s.bandHi, digits, suffix), mark: 'band', cls: 'band-outer' });
  }
  for (const p of s.priors) {
    if (p.v !== null) rows.push({ label: String(p.year), value: fmtValue(p.v, digits, suffix), mark: 'line', cls: 'line-prior' });
  }
  return rows;
}

/** One hour's temperature rows, the same rows in the full stack's temperature box and the compact dashboard row. */
export function hourTempRows(
  det: { temperature_f: (number | null)[]; apparent_temp_f: (number | null)[]; dew_point_f: (number | null)[] },
  ens: { temperature_f: BandSeries } | null,
  i: number
): TipRow[] {
  const rows: TipRow[] = [{ label: 'Forecast', value: fmtValue(det.temperature_f[i], 0, ' F'), mark: 'line', cls: 'line-forecast' }];
  if (ens) {
    const t = ens.temperature_f;
    rows.push(
      { label: 'Ensemble median', value: fmtValue(t.p50[i], 0, ' F'), mark: 'line', cls: 'line-median' },
      { label: 'Range p10-p90', value: fmtRange(t.p10[i], t.p90[i], 0, ' F'), mark: 'band', cls: 'band-outer' }
    );
  }
  rows.push(
    { label: 'Feels like', value: fmtValue(det.apparent_temp_f[i], 0, ' F'), mark: 'line', cls: 'line-feels' },
    { label: 'Dew point', value: fmtValue(det.dew_point_f[i], 0, ' F'), mark: 'line', cls: 'line-dew' }
  );
  return rows;
}
