import { describe, expect, it } from 'vitest';
import type { DayLead } from '$lib/transforms/forecast';
import { cumulativeRows, dailyPrecipRows, dailyTempRows, facetRows, hourTempRows } from './tipRows';

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

describe('facetRows', () => {
  const at = (n: number, value: number | null, issued = true): DayLead => ({ lead: n, value, issued });

  it('lists the hovered lead then now, terse: "3d before 49°" and "now 49°"', () => {
    expect(facetRows('tmax_f', at(3, 49), at(0, 49))).toEqual([
      { label: '3d before', value: '49°', mark: 'line', cls: 'line-facet' },
      { label: 'now', value: '49°', mark: 'line', cls: 'line-facet' }
    ]);
  });

  it('formats rain in inches with two decimals and chance of rain as a whole percentage', () => {
    expect(facetRows('precip_in', at(1, 0.15), at(0, 0))[0]).toMatchObject({ value: '0.15 in', mark: 'bar', cls: 'bar-precip' });
    expect(facetRows('prob_precip_max', at(2, 40.4), at(0, 39.6))).toMatchObject([{ value: '40%' }, { value: '40%' }]);
  });

  it('lists only now at lead 0, since the hovered value is the latest', () => {
    expect(facetRows('tmin_f', at(0, 42), at(0, 42))).toEqual([{ label: 'now', value: '42°', mark: 'line', cls: 'line-facet' }]);
  });

  it('shows an en dash for a lead that is not issued and a hyphen for a null value, the table\'s own marks, without inventing one', () => {
    expect(facetRows('tmax_f', at(4, 50, false), at(0, 52))).toMatchObject([{ label: '4d before', value: '–' }, { label: 'now', value: '52°' }]);
    expect(facetRows('precip_in', at(2, null), at(0, 0.1))).toMatchObject([{ value: '-' }, { value: '0.10 in' }]);
    expect(facetRows('precip_in', at(1, 0.2), at(0, null))[1]).toMatchObject({ label: 'now', value: '-' });
  });
});
