import { describe, expect, it } from 'vitest';
import { FORECAST_HOURLY_DAYS } from '$lib/config';
import { forecast, forecastChanges } from '../../test/fixtures';
import { forecastBand, hourlyWindow, leadLabel, leadSeries, spreadAcrossLeads } from './forecast';

describe('forecast transforms', () => {
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
