import { describe, expect, it } from 'vitest';
import type { ForecastChanges } from '$lib/api/types';
import { changeRows, cumulativeRows, dailyPrecipRows, dailyTempRows, hourTempRows } from './tipRows';

const band = (p10: number, p50: number, p90: number) => ({ p10, p50, p90 });
const det = { tmax_f: 80, tmin_f: 50, precip_in: 0.1, prob_precip_max: 30 };
const ens = {
  tmax_f: band(74, 80, 86),
  tmin_f: band(44, 50, 55),
  precip_in: band(0, 0.02, 0.3),
  prob_precip: 35
};

describe('dailyTempRows', () => {
  it('lists High, High range, Low, Low range and Median in the legend order, with the legend classes', () => {
    expect(dailyTempRows(det, ens)).toEqual([
      { label: 'High', value: '80 F', mark: 'line', cls: 'line-tmax' },
      { label: 'High p10-p90', value: '74 F to 86 F', mark: 'band', cls: 'band-tmax' },
      { label: 'Low', value: '50 F', mark: 'line', cls: 'line-tmin' },
      { label: 'Low p10-p90', value: '44 F to 55 F', mark: 'band', cls: 'band-tmin' },
      { label: 'Median', value: 'high 80 F, low 50 F', mark: 'line', cls: 'line-median' }
    ]);
  });

  it('leaves out the ranges and the median when there is no ensemble', () => {
    expect(dailyTempRows(det, undefined).map((r) => r.label)).toEqual(['High', 'Low']);
  });

  it('leaves out the deterministic lines when there is no deterministic row', () => {
    expect(dailyTempRows(undefined, ens).map((r) => r.label)).toEqual(['High p10-p90', 'Low p10-p90', 'Median']);
  });

  it('reads a missing value as "no value" and keeps the row', () => {
    const rows = dailyTempRows({ ...det, tmax_f: null }, undefined);
    expect(rows[0]).toMatchObject({ label: 'High', value: 'no value' });
  });
});

describe('dailyPrecipRows', () => {
  it('lists the ensemble range and median, then the chance of rain', () => {
    expect(dailyPrecipRows(det, ens)).toEqual([
      { label: 'Range p10-p90', value: '0.00 in to 0.30 in', mark: 'band', cls: 'band-outer' },
      { label: 'Median', value: '0.02 in', mark: 'line', cls: 'line-median' },
      { label: 'Chance of rain', value: '35%' }
    ]);
  });

  it('shows the deterministic amount when there is no ensemble, and the daily maximum chance', () => {
    expect(dailyPrecipRows(det, undefined)).toEqual([
      { label: 'Precipitation', value: '0.10 in', mark: 'line', cls: 'line-forecast' },
      { label: 'Chance of rain', value: '30%' }
    ]);
  });
});

const changes: ForecastChanges = {
  station_id: 's',
  timezone: 'America/Denver',
  model: 'gfs_seamless',
  generated_at: '2026-07-10T12:00:00Z',
  attribution: 'a',
  units: { temperature: 'F', precipitation: 'in' },
  lead_days: [0, 1, 7],
  days: [{ date: '2026-07-10', precip_in: [0.1, null, 0.25], prob_precip_max: [null, 20, 40], tmax_f: [80, 79.4, 77], tmin_f: [50, 49, 48] }]
};

describe('changeRows', () => {
  it('lists one row per lead, labelled as the legend, with the lead class for the swatch', () => {
    expect(changeRows(changes.lead_days, 'precip_in', changes.days[0])).toEqual([
      { label: 'latest', value: '0.10 in', mark: 'line', cls: 'line-lead-0' },
      { label: '1 day earlier', value: 'no value', mark: 'line', cls: 'line-lead-1' },
      { label: '7 days earlier', value: '0.25 in', mark: 'line', cls: 'line-lead-7' }
    ]);
  });

  it('formats each metric in its own unit', () => {
    expect(changeRows(changes.lead_days, 'prob_precip_max', changes.days[0]).map((r) => r.value)).toEqual(['no value', '20%', '40%']);
    expect(changeRows(changes.lead_days, 'tmax_f', changes.days[0]).map((r) => r.value)).toEqual(['80 F', '79 F', '77 F']);
    expect(changeRows(changes.lead_days, 'tmin_f', changes.days[0]).map((r) => r.value)).toEqual(['50 F', '49 F', '48 F']);
  });
});

describe('cumulativeRows', () => {
  const full = {
    thisYear: 1.2,
    partial: 0.3,
    median: 1.5,
    bandLo: 0.9,
    bandHi: 2.4,
    priors: [
      { year: 2025, v: 1.1 },
      { year: 2024, v: null }
    ]
  };

  it('lists this year, today, median, the band and each prior year drawn, with the legend classes', () => {
    expect(cumulativeRows(full, 2, ' in')).toEqual([
      { label: 'This year', value: '1.20 in', mark: 'line', cls: 'line-current' },
      { label: 'Today (partial)', value: '0.30 in', mark: 'line', cls: 'line-current-partial' },
      { label: 'Median', value: '1.50 in', mark: 'line', cls: 'line-median' },
      { label: 'Percentile band', value: '0.90 in to 2.40 in', mark: 'band', cls: 'band-outer' },
      { label: '2025', value: '1.10 in', mark: 'line', cls: 'line-prior' }
    ]);
  });

  it('leaves out the rows that have no value at this step', () => {
    const rows = cumulativeRows({ ...full, thisYear: null, partial: null, median: null, bandLo: null, bandHi: null, priors: [] }, 0, ' GDD');
    expect(rows).toEqual([]);
  });
});

describe('hourTempRows', () => {
  const hourly = {
    temperature_f: [64, 65],
    apparent_temp_f: [60, 61],
    dew_point_f: [40, 41]
  };
  const ensHourly = { temperature_f: { p10: [58, 59], p50: [63, 64], p90: [70, 71] } };

  it('lists forecast, ensemble median, range, feels like and dew point at the step', () => {
    expect(hourTempRows(hourly, ensHourly, 1).map((r) => [r.label, r.value])).toEqual([
      ['Forecast', '65 F'],
      ['Ensemble median', '64 F'],
      ['Range p10-p90', '59 F to 71 F'],
      ['Feels like', '61 F'],
      ['Dew point', '41 F']
    ]);
  });

  it('leaves out the ensemble rows without an ensemble', () => {
    expect(hourTempRows(hourly, null, 0).map((r) => r.label)).toEqual(['Forecast', 'Feels like', 'Dew point']);
  });
});
