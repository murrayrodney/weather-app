import { describe, expect, it } from 'vitest';
import { fmtRange, fmtValue, placeTooltip, TIP_GAP, tipBody, tipDate, tipHeight, tipHour, tipText, tipWidth, type TipBox } from './tooltip';

const base = { w: 200, h: 60, plotW: 800, left: 0, top: 0, bottom: 300 };

describe('placeTooltip', () => {
  it('sits to the right of the crosshair, centred vertically on the anchor', () => {
    expect(placeTooltip({ ...base, x: 100, y: 150 })).toEqual({ tx: 100 + TIP_GAP, ty: 120 });
  });

  it('never starts left of the plot: a flip in a narrow plot is clamped to the left edge', () => {
    // flipped would be 150 - 12 - 200 = -62, which is left of the plot edge at 52
    expect(placeTooltip({ ...base, left: 52, plotW: 300, x: 150, y: 150 })).toEqual({ tx: 52, ty: 120 });
  });

  it('flips to the left of the crosshair when the right-hand placement would pass the plot edge', () => {
    // 700 + 12 + 200 = 912 > 800, so the box ends 12 px left of the crosshair
    expect(placeTooltip({ ...base, x: 700, y: 150 })).toEqual({ tx: 700 - TIP_GAP - 200, ty: 120 });
  });

  it('stays on the right when it ends exactly at the plot edge', () => {
    expect(placeTooltip({ ...base, x: 588, y: 150 })).toEqual({ tx: 588 + TIP_GAP, ty: 120 });
  });

  it('clamps to the top of its panel', () => {
    expect(placeTooltip({ ...base, x: 100, y: 10 }).ty).toBe(0);
  });

  it('clamps to the bottom of its panel', () => {
    expect(placeTooltip({ ...base, x: 100, y: 290 }).ty).toBe(300 - 60);
  });

  it('keeps the top when the box is taller than its panel', () => {
    expect(placeTooltip({ ...base, h: 400, x: 100, y: 150 }).ty).toBe(0);
  });
});

describe('tooltip sizes', () => {
  const box = (title: string, rows: TipBox['rows']): TipBox => ({ title, rows });

  it('has a height from its title and row count', () => {
    expect(tipHeight(0)).toBe(8 * 2 + 20);
    expect(tipHeight(3)).toBe(8 * 2 + 20 + 3 * 16);
  });

  it('widens with the longest label and value', () => {
    const short = box('Tue Oct 27, 3 PM', [{ label: 'Dew', value: '1 F' }]);
    const long = box('Tue Oct 27, 3 PM', [{ label: 'Ensemble median', value: '58 F to 70 F' }]);
    expect(tipWidth(long)).toBeGreaterThan(tipWidth(short));
    expect(tipWidth(long)).toBe(28 + 15 * 7 + 12 + 12 * 7 + 8);
  });

  it('is at least as wide as its title', () => {
    expect(tipWidth(box('Tue Oct 27, 3 PM', []))).toBe(16 + 16 * 7);
  });
});

describe('row formatting', () => {
  it('prints a value with fixed decimals and a unit, or "no value" for a missing step', () => {
    expect(fmtValue(63.9, 0, ' F')).toBe('64 F');
    expect(fmtValue(0.004, 2, ' in')).toBe('0.00 in');
    expect(fmtValue(null, 0, '%')).toBe('no value');
    expect(fmtValue(undefined, 2, ' inHg')).toBe('no value');
  });

  it('prints a range as "low to high", the tooltip wording', () => {
    expect(fmtRange(58.4, 70.2, 0, ' F')).toBe('58 F to 70 F');
    expect(fmtRange(null, 0.5, 2, ' in')).toBe('no value to 0.50 in');
  });

  it('titles a box with the hour in the station zone', () => {
    // 21:00 UTC on Oct 27 is 3 PM in Denver (MDT, before DST ends on Nov 1)
    expect(tipHour(Date.parse('2026-10-27T21:00:00Z'), 'America/Denver')).toBe('Tue Oct 27, 3 PM');
    // midnight local is "12 AM", not "0 AM"
    expect(tipHour(Date.parse('2026-10-27T06:00:00Z'), 'America/Denver')).toBe('Tue Oct 27, 12 AM');
  });
});

describe('tipDate and tipText', () => {
  it('titles a day with its weekday, month and day, from the UTC date', () => {
    expect(tipDate('2026-07-10')).toBe('Fri Jul 10');
    expect(tipDate('2026-12-31')).toBe('Thu Dec 31');
  });

  it('reads a box as one sentence: title, then label and value pairs', () => {
    expect(tipText({ title: 'Fri Jul 10', rows: [{ label: 'High', value: '80 F' }, { label: 'Low', value: '50 F' }] })).toBe(
      'Fri Jul 10: High 80 F, Low 50 F.'
    );
  });

  it('says there are no values when a box has no rows', () => {
    expect(tipText({ title: 'Fri Jul 10', rows: [] })).toBe('Fri Jul 10: no values.');
  });
});

describe('tipBody', () => {
  it('reads the rows as one sentence without the title', () => {
    expect(tipBody({ title: 'Tue Oct 27, 3 PM', rows: [{ label: 'Hourly rain', value: '0.00 in' }, { label: 'Chance of precipitation', value: '20%' }] })).toBe(
      'Hourly rain 0.00 in, Chance of precipitation 20%.'
    );
  });

  it('says there are no values when a box has no rows, and tipText joins it after the title', () => {
    expect(tipBody({ title: 'Tue Oct 27, 3 PM', rows: [] })).toBe('no values.');
    const box = { title: 'Tue Oct 27, 3 PM', rows: [{ label: 'Gusts', value: '9 mph' }] };
    expect(tipText(box)).toBe(`Tue Oct 27, 3 PM: ${tipBody(box)}`);
  });
});
