import { describe, expect, it } from 'vitest';
import type { YtdResponse } from '$lib/api/types';
import { candidatesEmpty, candidatesLoveland, forecast, forecastChanges, forecastEnsembleNull, userSearch, ytdBaselineLoading, ytdFull, ytdGddClamped, ytdNoBaseline, ytdPartialStation } from './fixtures';

function checkYtd(r: YtdResponse) {
  expect(typeof r.year).toBe('number');
  expect(r.gdd).toHaveProperty('method');
  for (const s of r.stations) {
    expect(typeof s.current.missing_days).toBe('object');
    expect(s.current.missing_days).toHaveProperty('rain');
    expect(s.current).toHaveProperty('first_date');
    expect(s.current).toHaveProperty('last_date');
    for (const y of s.prior_years) expect(typeof y.missing_days).toBe('object');
    if (s.bands) expect(typeof s.bands.years_included).toBe('object');
  }
}

describe('fixtures have the API shapes', () => {
  it.each([ytdFull, ytdNoBaseline, ytdBaselineLoading, ytdGddClamped, ytdPartialStation])('ytd %#', (f) => checkYtd(f()));
  it('baseline states', () => {
    expect(ytdNoBaseline().stations[0].bands).toBeNull();
    expect(ytdBaselineLoading().stations[0].prior_years).toEqual([]);
    expect(ytdGddClamped().gdd).toEqual({ method: 'clamped_50_86', base_f: 45, start: '01-01' });
  });
  it('forecast fixtures', () => {
    expect(forecast().ensemble).not.toBeNull();
    expect(forecastEnsembleNull().ensemble).toBeNull();
    expect(forecastChanges().lead_days).toHaveLength(8);
    // Every hourly column is parallel to `time`, and sunrise and sunset are UTC instants.
    const det = forecast().deterministic!;
    for (const v of Object.values(det.hourly)) if (Array.isArray(v)) expect(v).toHaveLength(det.hourly.time.length);
    expect(forecast().ensemble!.hourly.precip_accum_in.p50).toHaveLength(forecast().ensemble!.hourly.time.length);
    expect(det.daily[0].sunrise).toMatch(/Z$/);
    expect(det.daily[0].sunset).toMatch(/Z$/);
    expect(candidatesLoveland().candidates.length).toBeGreaterThan(0);
    expect(candidatesEmpty().candidates).toEqual([]);
    expect(JSON.stringify(userSearch())).not.toContain('@');
  });
});
