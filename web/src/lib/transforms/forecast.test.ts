import { describe, expect, it } from 'vitest';
import { FORECAST_HOURLY_DAYS } from '$lib/config';
import { forecast, forecastChanges } from '../../test/fixtures';
import {
  arrowIndexes,
  arrowRotation,
  dayCells,
  dayNight,
  forecastBand,
  forecastRedirectTarget,
  hourlyWindow,
  iconFor,
  leadLabel,
  leadSeries,
  localDayKey,
  midnightIndexes,
  nowX,
  pageCount,
  pageStart,
  spreadAcrossLeads,
  accumDomain,
  hourlyRainDomain,
  niceRange,
  temperatureDomain,
  valueExtent,
  windCompass
} from './forecast';

const HOUR = 3600000;
// America/Denver: MDT (UTC-6) until 2026-11-01 08:00Z, then MST (UTC-7). Local 2026-11-01 is a 25-hour day.
const DENVER = 'America/Denver';
const denverHours = (startIso: string, n: number) => Array.from({ length: n }, (_, i) => new Date(Date.parse(startIso) + i * HOUR).toISOString().replace('.000Z', 'Z'));

describe('forecast transforms', () => {
  it('the retired forecast route redirects to the station page forecast anchor', () => {
    expect(forecastRedirectTarget('11111111-1111-4111-8111-111111111111')).toBe('/stations/11111111-1111-4111-8111-111111111111#forecast');
  });
  it('forecastBand breaks at null steps and keeps p10 to p90', () => {
    const f = forecast().ensemble!;
    const b = forecastBand(f.hourly.temperature_f);
    expect(b.lower[5]).toBeNull();
    expect(b.upper[5]).toBeNull();
    expect(b.median[5]).toBeNull();
    expect(b.lower[4]!.y).toBeLessThan(b.upper[4]!.y);
  });
  it('hourlyWindow keeps exactly FORECAST_HOURLY_DAYS days', () => {
    const w = hourlyWindow(forecast(), FORECAST_HOURLY_DAYS);
    expect(w.time).toHaveLength(FORECAST_HOURLY_DAYS * 24);
    expect(w.deterministic!.temperature_f).toHaveLength(FORECAST_HOURLY_DAYS * 24);
    expect(w.ensemble!.temperature_f.p50).toHaveLength(FORECAST_HOURLY_DAYS * 24);
  });
  it('leadSeries returns 8 series with nulls kept', () => {
    const c = forecastChanges();
    const s = leadSeries(c, 'precip_in');
    expect(s).toHaveLength(8);
    expect(s[0]).toHaveLength(c.days.length);
    expect(s[4][2]).toBeNull();
    expect(leadSeries(c, 'prob_precip_max')[0][0]).toBeNull();
  });
  it('labels leads', () => {
    expect(leadLabel(0)).toBe('latest');
    expect(leadLabel(1)).toBe('1 day earlier');
    expect(leadLabel(7)).toBe('7 days earlier');
  });
  it('spreadAcrossLeads ignores nulls and needs two values', () => {
    const c = forecastChanges();
    expect(spreadAcrossLeads(c.days[0], 'tmax_f')).toEqual({ min: 80, max: 82 });
    expect(spreadAcrossLeads({ ...c.days[0], tmax_f: [null, null, 5, null] }, 'tmax_f')).toBeNull();
  });
});

describe('nowX', () => {
  const x = (ms: number) => (ms - Date.parse('2026-11-01T00:00:00Z')) / HOUR; // 1 unit per hour
  it('places now on the window and returns null outside it', () => {
    const s = Date.parse('2026-10-31T06:00:00Z');
    const e = Date.parse('2026-11-07T06:00:00Z');
    expect(nowX(Date.parse('2026-11-01T01:30:00Z'), s, e, x)).toBe(x(Date.parse('2026-11-01T01:30:00Z')));
    expect(nowX(s - 1, s, e, x)).toBeNull();
    expect(nowX(e + 1, s, e, x)).toBeNull();
    expect(nowX(s, s, e, x)).toBe(x(s));
  });
  it('is linear across the DST change: the 25-hour local day is 25 UTC hours wide', () => {
    const s = Date.parse('2026-10-31T06:00:00Z');
    const e = Date.parse('2026-11-03T06:00:00Z');
    const a = nowX(Date.parse('2026-11-01T06:00:00Z'), s, e, x)!; // local midnight, MDT
    const b = nowX(Date.parse('2026-11-02T07:00:00Z'), s, e, x)!; // next local midnight, MST
    expect(b - a).toBe(25);
  });
});

describe('dayNight', () => {
  // Sunrise and sunset are UTC instants from the API. Oct 31 to Nov 2 in Denver spans the DST change.
  const days = [
    { sunrise: '2026-10-31T12:51:00Z', sunset: '2026-10-31T23:55:00Z' },
    { sunrise: '2026-11-01T13:58:00Z', sunset: '2026-11-01T23:53:00Z' },
    { sunrise: '2026-11-02T13:59:00Z', sunset: '2026-11-02T23:52:00Z' }
  ];
  const s = Date.parse('2026-10-31T06:00:00Z');
  const e = Date.parse('2026-11-03T06:00:00Z');
  const iso = (ms: number) => new Date(ms).toISOString().replace('.000Z', 'Z');

  it('shades from each sunset to the next sunrise, with the pre-dawn and after-dusk ends clipped to the window', () => {
    const spans = dayNight(days, s, e).map((n) => [iso(n.start), iso(n.end)]);
    expect(spans).toEqual([
      ['2026-10-31T06:00:00Z', '2026-10-31T12:51:00Z'],
      ['2026-10-31T23:55:00Z', '2026-11-01T13:58:00Z'],
      ['2026-11-01T23:53:00Z', '2026-11-02T13:59:00Z'],
      ['2026-11-02T23:52:00Z', '2026-11-03T06:00:00Z']
    ]);
  });
  it('keeps the real UTC length of the night that spans the DST change', () => {
    const night = dayNight(days, s, e)[2];
    expect(night.end - night.start).toBe(14 * HOUR + 6 * 60000);
  });
  it('returns nothing for days without sun times', () => {
    expect(dayNight([{ sunrise: null, sunset: null }], s, e)).toEqual([]);
  });
});

describe('dayCells and local days', () => {
  // Hourly steps from local midnight 2026-10-31 (MDT) for four local days.
  const times = denverHours('2026-10-31T06:00:00Z', 96);

  it('keys each hour by its local calendar date in the station zone', () => {
    expect(localDayKey('2026-11-01T06:00:00Z', DENVER)).toBe('2026-11-01');
    expect(localDayKey('2026-11-01T05:59:00Z', DENVER)).toBe('2026-10-31');
  });
  it('gives the DST day its real 25 hours and one cell per local day', () => {
    const cells = dayCells(times, DENVER);
    expect(cells.map((c) => c.date)).toEqual(['2026-10-31', '2026-11-01', '2026-11-02', '2026-11-03']);
    const nov1 = cells[1];
    expect(nov1.endMs - nov1.startMs).toBe(25 * HOUR);
    expect(cells[0].endMs - cells[0].startMs).toBe(24 * HOUR);
  });
  it('finds the local midnights: the day after the DST change is 25 steps after the previous one', () => {
    expect(midnightIndexes(times, DENVER)).toEqual([0, 24, 49, 73]);
  });
});

describe('proposal 07 phase 4: paging, slices, wind arrows, axis extents', () => {
  it('pageCount moves one day at a time through the range and is 1 when the range fits', () => {
    expect(pageCount(360, FORECAST_HOURLY_DAYS * 24)).toBe(9); // 15 days, 7 visible: days 0 to 8 are first days
    expect(pageCount(168, 168)).toBe(1);
    expect(pageCount(100, 168)).toBe(1);
  });

  it('pageStart steps 24 hours and stops at the last full window', () => {
    expect(pageStart(0, 360, 168)).toBe(0);
    expect(pageStart(1, 360, 168)).toBe(24);
    expect(pageStart(8, 360, 168)).toBe(192);
    expect(pageStart(99, 360, 168)).toBe(192);
    expect(pageStart(-3, 360, 168)).toBe(0);
    expect(pageStart(5, 100, 168)).toBe(0);
  });

  it('a later window slices every series, accumulated totals included, from the same instants', () => {
    const f = forecast();
    const d = f.deterministic!.hourly;
    const w = hourlyWindow(f, FORECAST_HOURLY_DAYS, 24);
    expect(w.time).toEqual(d.time.slice(24, 24 + 168));
    expect(w.deterministic!.precip_accum_in).toEqual(d.precip_accum_in.slice(24, 192));
    expect(w.deterministic!.wind_dir_deg).toEqual(d.wind_dir_deg.slice(24, 192));
    expect(w.ensemble!.precip_accum_in.p50).toEqual(f.ensemble!.hourly.precip_accum_in.p50.slice(24, 192));
    expect(w.ensemble!.precip_in.p90).toEqual(f.ensemble!.hourly.precip_in.p90.slice(24, 192));
  });

  it('keeps null accumulated steps (before "now") as null in the slice', () => {
    const f = forecast();
    f.deterministic!.hourly.precip_accum_in = f.deterministic!.hourly.precip_accum_in.map((v, i) => (i < 10 ? null : v));
    const w = hourlyWindow(f, FORECAST_HOURLY_DAYS, 0);
    expect(w.deterministic!.precip_accum_in.slice(0, 10)).toEqual(Array(10).fill(null));
    expect(w.deterministic!.precip_accum_in[10]).toBe(f.deterministic!.hourly.precip_accum_in[10]);
  });

  it('arrowIndexes picks local hours divisible by 3 in the station zone, skipping unknown directions', () => {
    const times = denverHours('2026-07-10T06:00:00Z', 24); // local midnight (MDT) to 23:00
    const dirs = times.map((_, i) => (i === 3 ? null : 90));
    expect(arrowIndexes(times, DENVER, dirs)).toEqual([0, 6, 9, 12, 15, 18, 21]);
  });

  it('arrowRotation turns the arrow to where the wind blows (180 degrees from the reported direction)', () => {
    expect(arrowRotation(0)).toBe(180);
    expect(arrowRotation(90)).toBe(270);
    expect(arrowRotation(180)).toBe(0);
    expect(arrowRotation(270)).toBe(90);
    expect(arrowRotation(360)).toBe(180);
  });

  it('windCompass names the 16 points the wind comes from', () => {
    expect(windCompass(0)).toBe('N');
    expect(windCompass(359)).toBe('N');
    expect(windCompass(11)).toBe('N');
    expect(windCompass(12)).toBe('NNE');
    expect(windCompass(90)).toBe('E');
    expect(windCompass(225)).toBe('SW');
  });

  it('valueExtent ignores nulls and is null when there are no numbers', () => {
    expect(valueExtent([[null, 3, 1], [7, null]])).toEqual([1, 7]);
    expect(valueExtent([[null], []])).toBeNull();
  });
});

describe('axis domains (proposal 07 phase 4 review)', () => {
  it('niceRange rounds outward to the step and does not force zero', () => {
    expect(niceRange(12.3, 66.1, 5)).toEqual([10, 70]);
    expect(niceRange(40, 50, 5)).toEqual([40, 50]);
    expect(niceRange(31.9, 33, 5)).toEqual([30, 35]);
  });

  it('the hourly rain axis comes from the hourly values, so the bars are not flattened by the accumulated total', () => {
    const w = hourlyWindow(forecast(), FORECAST_HOURLY_DAYS * 2);
    const rain = hourlyRainDomain(w);
    const total = accumDomain(w);
    // fixture: hourly rain never above 0.04 in; the accumulated ensemble p90 passes 3 in
    expect(rain[0]).toBe(0);
    expect(rain[1]).toBeLessThan(0.5);
    expect(total[1]).toBeGreaterThan(3);
  });

  it('the temperature axis covers the data and the 32 F reference, rounded to 5 F, with no forced zero', () => {
    expect(temperatureDomain(hourlyWindow(forecast(), 15))).toEqual([30, 80]);
  });
});

describe('iconFor', () => {
  it('maps WMO codes to an icon kind and label', () => {
    expect(iconFor(0)).toEqual({ kind: 'clear', label: 'Clear' });
    expect(iconFor(2)).toEqual({ kind: 'partly', label: 'Partly cloudy' });
    expect(iconFor(3)).toEqual({ kind: 'cloudy', label: 'Overcast' });
    expect(iconFor(45)).toEqual({ kind: 'fog', label: 'Fog' });
    expect(iconFor(53)).toEqual({ kind: 'drizzle', label: 'Drizzle' });
    expect(iconFor(63)).toEqual({ kind: 'rain', label: 'Rain' });
    expect(iconFor(82)).toEqual({ kind: 'rain', label: 'Heavy showers' });
    expect(iconFor(73)).toEqual({ kind: 'snow', label: 'Snow' });
    expect(iconFor(86)).toEqual({ kind: 'snow', label: 'Snow showers' });
    expect(iconFor(95)).toEqual({ kind: 'storm', label: 'Thunderstorm' });
    expect(iconFor(99)).toEqual({ kind: 'storm', label: 'Thunderstorm, hail' });
  });
  it('returns null for no code and a neutral unknown icon for a code outside the table', () => {
    expect(iconFor(null)).toBeNull();
    expect(iconFor(undefined)).toBeNull();
    expect(iconFor(4)).toEqual({ kind: 'unknown', label: 'Unknown' });
  });
});
