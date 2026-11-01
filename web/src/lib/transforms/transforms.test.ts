import { describe, expect, it } from 'vitest';
import type { YtdStation } from '$lib/api/types';
import { ytdBaselineLoading, ytdFull, ytdNoBaseline, ytdPartialStation, ytdStation } from '../../test/fixtures';
import { bandPaths } from './bands';
import { addDays, formatInZone, localMd, localYear } from './dates';
import { parseGddSettings, startAllowed, toSearchParams } from './gddSettings';
import { describePosition, positionFor } from './position';
import { baselineViewState, buildProvenanceLines, windowHasWinter } from './provenance';
import { displaySeason } from './season';
import { splitCurrent, toPoints } from './series';
import { buildSlots } from './slots';

const win = { start: '04-01', end: '10-31' };

describe('buildSlots', () => {
  it('gives 214 sorted slots for Apr-Oct and never 02-29', () => {
    const s = buildSlots(ytdFull().stations[0], 'rain');
    expect(s).toHaveLength(214);
    expect([...s].sort()).toEqual(s);
    const withLeap = ytdStation();
    withLeap.current.rain_cum_in.push({ md: '02-29', v: 1 });
    expect(buildSlots(withLeap, 'rain')).not.toContain('02-29');
  });
  it('is empty for empty input', () => {
    const st = ytdStation({ lastDate: null, baseline: null });
    expect(buildSlots(st, 'rain')).toEqual([]);
  });
});

describe('toPoints', () => {
  it('lands values on the right index with null gaps and no zero fill', () => {
    const slots = ['04-01', '04-02', '04-03'];
    expect(toPoints([{ md: '04-01', v: 1 }, { md: '04-03', v: 3 }], slots)).toEqual([{ x: 0, y: 1 }, null, { x: 2, y: 3 }]);
    expect(toPoints(null, slots)).toEqual([]);
  });
});

describe('bandPaths', () => {
  const st = ytdFull().stations[0];
  const slots = buildSlots(st, 'rain');
  it('uses outer 10/90, inner 25/75 and median 50', () => {
    const l = bandPaths(st.bands, 'rain', slots);
    expect(l.outer).toBeDefined();
    expect(l.inner).toBeDefined();
    expect(l.median).toBeDefined();
    expect(l.outer!.lower[10]!.y).toBe(st.bands!.rain_cum_in![10].p[0]);
    expect(l.outer!.upper[10]!.y).toBe(st.bands!.rain_cum_in![10].p[4]);
  });
  it('omits inner when only [10,50,90]', () => {
    const b = structuredClone(st.bands)!;
    b.percentiles = [10, 50, 90];
    b.rain_cum_in = b.rain_cum_in!.map((r) => ({ md: r.md, p: [r.p[0], r.p[2], r.p[4]] }));
    const l = bandPaths(b, 'rain', slots);
    expect(l.inner).toBeUndefined();
    expect(l.outer).toBeDefined();
  });
  it('gives no layers for null bands or a null metric', () => {
    expect(bandPaths(null, 'rain', slots)).toEqual({});
    const b = structuredClone(st.bands)!;
    b.gdd_cum = null;
    expect(bandPaths(b, 'gdd', slots)).toEqual({});
    expect(bandPaths(b, 'rain', slots).outer).toBeDefined();
  });
});

describe('describePosition', () => {
  const ps = [10, 25, 50, 75, 90];
  const vals = [1, 2, 3, 4, 5];
  it('describes positions', () => {
    expect(describePosition(0.5, vals, ps, 30)).toBe('below the 10th percentile of 30 years of airport record');
    expect(describePosition(2.5, vals, ps, 30)).toBe('between the 25th and 50th percentile of 30 years of airport record');
    expect(describePosition(9, vals, ps, 30)).toBe('above the 90th percentile of 30 years of airport record');
    expect(describePosition(3, vals, ps, 30)).toBe('at about the 50th percentile of 30 years of airport record');
  });
  it('is null without bands / without a complete day / with a late first_date', () => {
    expect(positionFor(ytdNoBaseline().stations[0], 'rain', '04-01')).toBeNull();
    expect(positionFor(ytdStation({ lastComplete: null }), 'rain', '04-01')).toBeNull();
    expect(positionFor(ytdPartialStation().stations[0], 'rain', '04-01')).toBeNull();
    expect(positionFor(ytdFull().stations[0], 'rain', '04-01')).not.toBeNull();
  });
});

describe('provenance and baseline state', () => {
  it('lists baseline name, id, label and years', () => {
    const r = ytdFull();
    const lines = buildProvenanceLines(r, r.stations[0], 'rain', win);
    expect(lines[0]).toContain('Demo Airport (USW00099001), NOAA GHCN-Daily via ACIS; 34 years in the percentile bands, last 4 years drawn.');
    expect(lines).toContain('Days are midnight to midnight in local standard time (no daylight-saving shift).');
  });
  it('handles no baseline, unknown day definition and labels', () => {
    const r = ytdNoBaseline();
    expect(buildProvenanceLines(r, r.stations[0], 'rain', win)[0]).toBe('No baseline is set for this station yet.');
    r.day_definition = 'some_new_rule';
    expect(buildProvenanceLines(r, r.stations[0], 'rain', win)).toContain('some new rule');
  });
  it('shows GDD lines only for gdd and the winter note only for rain with a winter month', () => {
    const r = ytdFull();
    const gdd = buildProvenanceLines(r, r.stations[0], 'gdd', { start: '04-01', end: '12-31' });
    expect(gdd.some((l) => l.startsWith('GDD: Simple average, base 50 F, counted from April 1.'))).toBe(true);
    expect(gdd.some((l) => l.startsWith('Shaded months'))).toBe(false);
    const full = buildProvenanceLines(r, r.stations[0], 'rain', { start: '01-01', end: '12-31' });
    expect(full.some((l) => l.startsWith('Shaded months'))).toBe(true);
    expect(buildProvenanceLines(r, r.stations[0], 'rain', win).some((l) => l.startsWith('Shaded months'))).toBe(false);
    expect(windowHasWinter(win)).toBe(false);
  });
  it('baselineViewState: ready, none, loading', () => {
    expect(baselineViewState(ytdFull().stations[0])).toBe('ready');
    expect(baselineViewState(ytdNoBaseline().stations[0])).toBe('none');
    expect(baselineViewState(ytdBaselineLoading().stations[0])).toBe('loading');
    const l = ytdBaselineLoading();
    expect(buildProvenanceLines(l, l.stations[0], 'rain', win)[0]).toContain('Its history is loading and should appear within the hour.');
    expect(l.stations[0].bands).toBeNull();
  });
  it('off-season line', () => {
    const r = ytdFull();
    expect(buildProvenanceLines(r, r.stations[0], 'rain', win, { offSeason: true })).toContain(
      'Showing the 2026 season (April-October), the most recent completed season. The 2027 season starts April 1.'
    );
  });
  it('reads metric-specific fields (a year with null rain is drawn only on GDD)', () => {
    const st = ytdFull().stations[0];
    const last = st.prior_years.find((y) => y.year === 2021)!;
    expect(last.rain_cum_in).toBeNull();
    expect(last.gdd_cum).not.toBeNull();
    const r = ytdFull();
    expect(buildProvenanceLines(r, st, 'rain', win)[0]).toContain('last 4 years drawn');
    expect(buildProvenanceLines(r, st, 'gdd', win)[0]).toContain('last 5 years drawn');
    expect(buildProvenanceLines(r, st, 'gdd', win)[0]).toContain('35 years in the percentile bands');
  });
});

describe('displaySeason', () => {
  it.each([
    [{ year: 2026, month: 4, day: 1 }, 2026, false],
    [{ year: 2026, month: 10, day: 31 }, 2026, false],
    [{ year: 2026, month: 11, day: 1 }, 2026, true],
    [{ year: 2026, month: 12, day: 31 }, 2026, true],
    [{ year: 2027, month: 1, day: 1 }, 2026, true],
    [{ year: 2027, month: 3, day: 31 }, 2026, true],
    [{ year: 2027, month: 4, day: 1 }, 2027, false]
  ])('%o', (d, year, off) => {
    expect(displaySeason(d)).toEqual({ year, offSeason: off });
  });
  it('takes month boundaries in the station zone', () => {
    // 2026-11-01 03:00 UTC is still Oct 31 in Denver and already Nov 1 in UTC.
    const now = new Date('2026-11-01T03:00:00Z');
    expect(localMd('America/Denver', now)).toBe('10-31');
    expect(localMd('UTC', now)).toBe('11-01');
    expect(localYear('America/Denver', new Date('2027-01-01T03:00:00Z'))).toBe(2026);
    expect(addDays('2026-12-30', 3)).toBe('2027-01-02');
    expect(formatInZone('2026-07-10T18:00:00Z', 'America/Denver')).toContain('12:00');
  });
});

describe('gddSettings', () => {
  it('defaults give an empty query string and non-defaults round-trip', () => {
    expect(toSearchParams(parseGddSettings(new URLSearchParams())).toString()).toBe('');
    const sp = new URLSearchParams('gdd_method=clamped_50_86&gdd_base=45&gdd_start=01-01');
    const s = parseGddSettings(sp);
    expect(s).toEqual({ method: 'clamped_50_86', base_f: 45, start: '01-01' });
    expect(parseGddSettings(toSearchParams(s))).toEqual(s);
  });
  it('rejects a gdd_start after the window end (the API refuses it)', () => {
    const sp = new URLSearchParams('gdd_start=11-15');
    expect(parseGddSettings(sp, '10-31').start).toBe('04-01');
    expect(parseGddSettings(sp, '12-31').start).toBe('11-15');
    expect(parseGddSettings(new URLSearchParams('gdd_start=10-31'), '10-31').start).toBe('10-31'); // equal is allowed
    expect(startAllowed('10-31', '10-31')).toBe(true);
    expect(startAllowed('11-01', '10-31')).toBe(false);
  });
  it('falls back to defaults for invalid values', () => {
    const s = parseGddSettings(new URLSearchParams('gdd_base=31&gdd_method=avg_50&gdd_start=13-40'));
    expect(s).toEqual({ method: 'simple_avg', base_f: 50, start: '04-01' });
  });
});

describe('splitCurrent', () => {
  it('marks points after last_complete_date partial and joins them', () => {
    const st = ytdFull().stations[0];
    const s = splitCurrent({ ...st.current, last_complete_date: '2026-07-09' }, 'rain_cum_in');
    expect(s.complete[s.complete.length - 1].md).toBe('07-09');
    expect(s.partial.map((p) => p.md)).toEqual(['07-09', '07-10']);
  });
  it('makes everything partial when last_complete_date is null', () => {
    const st: YtdStation = ytdFull().stations[0];
    const s = splitCurrent({ ...st.current, last_complete_date: null }, 'rain_cum_in');
    expect(s.complete).toEqual([]);
    expect(s.partial.length).toBe(st.current.rain_cum_in.length);
  });
  it('a late first_date gives no earlier points', () => {
    const st = ytdPartialStation().stations[0];
    expect(st.current.rain_cum_in[0].md).toBe('06-01');
    const slots = buildSlots(st, 'rain');
    const pts = toPoints(st.current.rain_cum_in, slots);
    expect(pts[0]).toBeNull();
  });
});
