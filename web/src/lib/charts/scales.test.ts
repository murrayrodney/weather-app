import { describe, expect, it } from 'vitest';
import { H, M, W, barPath, makeScales, rangePath } from './scales';

describe('makeScales', () => {
  it('defaults to the 520x280 station chart', () => {
    const s = makeScales(10, 5);
    expect(s.x.range()).toEqual([M.l, W - M.r]);
    expect(s.y.range()).toEqual([H - M.b, M.t]);
  });
  it('sizes the plot from width and height (dashboard forecast row)', () => {
    const s = makeScales(168, 5, 1400, 180);
    expect(s.x.range()).toEqual([M.l, 1400 - M.r]);
    expect(s.y.range()).toEqual([180 - M.b, M.t]);
  });
});

describe('barPath and rangePath (proposal 07 phase 4)', () => {
  const x = (i: number) => i * 10;
  const y = (v: number) => 100 - v;
  it('draws one bar per positive step and skips nulls and zeros', () => {
    expect(barPath([null, 0, 2, 5], x, y, 2)).toBe('M18 100V98H22V100ZM28 100V95H32V100Z');
  });
  it('draws a vertical range only where both bounds are known', () => {
    const lower = [{ x: 0, y: 1 }, null, { x: 2, y: 3 }];
    const upper = [{ x: 0, y: 4 }, { x: 1, y: 9 }, null];
    expect(rangePath(lower, upper, x, y)).toBe('M0 96V99');
  });
});
