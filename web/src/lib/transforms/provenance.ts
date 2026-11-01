import type { Baseline, YtdResponse, YtdStation } from '$lib/api/types';
import { WINTER_MONTHS } from '$lib/config';
import { labels, mdLabel } from '$lib/labels';
import { monthOfMd } from './dates';
import { ordinal } from './position';
import type { Metric } from './slots';
import { fieldOf } from './slots';

export type BaselineViewState = 'none' | 'loading' | 'ready';

export function baselineViewState(st: { baseline: Baseline | null }): BaselineViewState {
  if (!st.baseline) return 'none';
  return st.baseline.history_loaded ? 'ready' : 'loading';
}

export function windowHasWinter(win: { start: string; end: string }): boolean {
  const a = monthOfMd(win.start);
  const b = monthOfMd(win.end);
  for (let m = a; m <= b; m++) if ((WINTER_MONTHS as readonly number[]).includes(m)) return true;
  return false;
}

export function offSeasonLine(year: number): string {
  return `Showing the ${year} season (April-October), the most recent completed season. The ${year + 1} season starts April 1.`;
}

export function bandsLine(percentiles: number[]): string | null {
  if (percentiles.length < 2) return null;
  const ps = [...percentiles].sort((a, b) => a - b);
  const outer = `${ps[0]}th-${ps[ps.length - 1]}th`;
  const inner = ps.includes(25) && ps.includes(75) ? ' and 25th-75th' : '';
  return `Bands: ${outer}${inner} percentile of prior years.`.replace(/(\d+)th/g, (_, n) => ordinal(Number(n)));
}

/** Caption lines for one chart. Values come from the API response, never from UI state. */
export function buildProvenanceLines(
  ytd: YtdResponse,
  st: YtdStation,
  metric: Metric,
  win: { start: string; end: string },
  opts: { offSeason?: boolean } = {}
): string[] {
  const field = fieldOf(metric);
  const lines: string[] = [];
  const view = baselineViewState(st);
  if (view === 'none') {
    lines.push('No baseline is set for this station yet.');
  } else if (view === 'loading') {
    lines.push(`Baseline chosen: ${st.baseline!.name}. Its history is loading and should appear within the hour.`);
  } else {
    const b = st.baseline!;
    const drawn = st.prior_years.filter((y) => y[field] !== null).length;
    if (st.bands && st.bands[field]) {
      lines.push(
        `Baseline: ${b.name} (${b.reference_station_id}), ${labels.baselineSource(b.source)}; ${st.bands.years_included[metric]} years in the percentile bands, last ${drawn} years drawn.`
      );
      const bl = bandsLine(st.bands.percentiles);
      if (bl) lines.push(bl);
    } else {
      lines.push(`Baseline: ${b.name} (${b.reference_station_id}), ${labels.baselineSource(b.source)}; last ${drawn} years drawn. Not enough years for percentile bands.`);
    }
  }
  lines.push(labels.dayDefinition(ytd.day_definition));
  if (metric === 'gdd') {
    lines.push(`GDD: ${labels.gddMethod(ytd.gdd.method)}, base ${ytd.gdd.base_f} F, counted from ${mdLabel(ytd.gdd.start)}. The airport baseline uses the same settings.`);
  }
  if (opts.offSeason) lines.push(offSeasonLine(ytd.year));
  if (metric === 'rain' && windowHasWinter(win)) {
    lines.push('Shaded months: unheated rain gauges undercount snow, so totals read low against the baseline.');
  }
  return lines;
}
