import { describe, expect, it } from 'vitest';
import { facetLayout, facetTitle, FACET_GAP, FACET_PAD, panelX, sharedYDomain, type FacetPoint } from './facetLayout';

// Counts panels per row, e.g. 7 panels in 4 columns -> [4, 3].
const rowCounts = (width: number, n: number) => {
  const l = facetLayout(width, n);
  return Array.from({ length: l.rows }, (_, r) => l.cells.filter((c) => c.row === r).length);
};

describe('facetLayout columns', () => {
  it('gives 2 columns for 7 panels at phone width (390 px)', () => {
    const l = facetLayout(390, 7);
    expect(l.cols).toBe(2);
    expect(l.rows).toBe(4);
  });

  it('gives 4 columns for 7 panels at tablet width (768 px)', () => {
    const l = facetLayout(768, 7);
    expect(l.cols).toBe(4);
    expect(l.rows).toBe(2);
  });

  it('gives 4 columns for 7 panels at 1024 px', () => {
    const l = facetLayout(1024, 7);
    expect(l.cols).toBe(4);
    expect(l.rows).toBe(2);
  });

  it('gives 7 columns for 7 panels at 1400 px, all on one row', () => {
    const l = facetLayout(1400, 7);
    expect(l.cols).toBe(7);
    expect(l.rows).toBe(1);
  });

  it('gives 2 columns for 14 panels at 390 px and 7 per row at 1400 px', () => {
    expect(facetLayout(390, 14)).toMatchObject({ cols: 2, rows: 7 });
    expect(facetLayout(1400, 14)).toMatchObject({ cols: 7, rows: 2 });
  });

  it('gives 4 columns for 14 panels at 768 px and 5 at 1024 px', () => {
    expect(facetLayout(768, 14)).toMatchObject({ cols: 4, rows: 4 });
    expect(facetLayout(1024, 14)).toMatchObject({ cols: 5, rows: 3 });
  });

  it('keeps every cell at least the minimum width', () => {
    for (const w of [390, 768, 1024, 1400]) {
      const l = facetLayout(w, 14);
      expect(l.cellW).toBeGreaterThanOrEqual(150);
    }
  });
});

describe('facetLayout balance', () => {
  it('splits 7 panels as 4 + 3 when two rows are needed, not 6 + 1', () => {
    expect(rowCounts(768, 7)).toEqual([4, 3]);
    expect(rowCounts(1024, 7)).toEqual([4, 3]);
  });

  it('splits 7 panels as 2 + 2 + 2 + 1 at phone width', () => {
    expect(rowCounts(390, 7)).toEqual([2, 2, 2, 1]);
  });

  it('splits 14 panels at 1024 px as 5 + 5 + 4', () => {
    expect(rowCounts(1024, 14)).toEqual([5, 5, 4]);
  });

  it('fills row by row, so the cell index runs left to right then down', () => {
    const l = facetLayout(768, 7);
    expect(l.cells.map((c) => [c.row, c.col])).toEqual([
      [0, 0],
      [0, 1],
      [0, 2],
      [0, 3],
      [1, 0],
      [1, 1],
      [1, 2]
    ]);
  });

  it('places cells by column and row offsets with the gap', () => {
    const l = facetLayout(768, 7);
    expect(l.cells[1].x).toBe(l.cellW + FACET_GAP);
    expect(l.cells[4].x).toBe(0);
    expect(l.cells[4].y).toBe(l.cellH + FACET_GAP);
  });

  it('returns an empty layout for no panels', () => {
    expect(facetLayout(768, 0)).toEqual({ cols: 0, rows: 0, cellW: 0, cellH: 150, cells: [] });
  });
});

describe('facetTitle', () => {
  it('names today, tomorrow and yesterday, and otherwise the weekday and day', () => {
    expect(facetTitle('2026-11-01', '2026-11-01')).toBe('Today');
    expect(facetTitle('2026-11-02', '2026-11-01')).toBe('Tomorrow');
    expect(facetTitle('2026-10-31', '2026-11-01')).toBe('Yesterday');
    expect(facetTitle('2026-11-03', '2026-11-01')).toBe('Tue 3');
  });

  it('keeps the weekday right across a month boundary', () => {
    expect(facetTitle('2026-11-06', '2026-11-01')).toBe('Fri 6');
  });
});

describe('sharedYDomain', () => {
  const p = (value: number | null, issued = true): FacetPoint => ({ value, issued });

  it('uses 0 to 100 for chance of rain whatever the values', () => {
    expect(sharedYDomain('prob_precip_max', [p(30), p(55)])).toEqual([0, 100]);
    expect(sharedYDomain('prob_precip_max', [])).toEqual([0, 100]);
  });

  it('keeps rain zero-based and sized to the largest issued value', () => {
    const [lo, hi] = sharedYDomain('precip_in', [p(0.2), p(0.5), p(0)]);
    expect(lo).toBe(0);
    expect(hi).toBeGreaterThanOrEqual(0.55);
    expect(hi).toBeLessThan(1);
  });

  it('does not let a not-issued rain copy stretch the rain domain', () => {
    const [, hi] = sharedYDomain('precip_in', [p(0.2), p(3.0, false)]);
    expect(hi).toBeLessThan(1);
  });

  it('gives temperature one domain from the lowest to the highest issued value', () => {
    const [lo, hi] = sharedYDomain('tmax_f', [p(50), p(60), p(55)]);
    expect(lo).toBeLessThanOrEqual(48);
    expect(hi).toBeGreaterThanOrEqual(62);
    expect(lo).toBeGreaterThan(40);
    expect(hi).toBeLessThan(70);
  });

  it('leaves not-issued temperatures out of the domain', () => {
    const [lo, hi] = sharedYDomain('tmin_f', [p(40), p(45), p(20, false), p(90, false)]);
    expect(lo).toBeGreaterThan(20);
    expect(hi).toBeLessThan(90);
  });

  it('ignores null values', () => {
    const [lo, hi] = sharedYDomain('tmax_f', [p(null), p(50), p(60)]);
    expect(lo).toBeLessThanOrEqual(48);
    expect(hi).toBeGreaterThanOrEqual(62);
  });

  it('falls back to 0 to 1 when no value is issued', () => {
    expect(sharedYDomain('tmax_f', [p(50, false), p(null)])).toEqual([0, 1]);
    expect(sharedYDomain('precip_in', [])).toEqual([0, 1]);
  });
});

describe('panelX', () => {
  it('gives the first column the y-label margin and the other columns the bare margin', () => {
    expect(panelX(200, true, 8).pl).toBe(FACET_PAD.left);
    expect(panelX(200, false, 8).pl).toBe(FACET_PAD.bare);
    expect(panelX(200, true, 8).pr).toBe(200 - FACET_PAD.right);
  });

  it('spreads the lead slots evenly across the plot, oldest on the left', () => {
    const x = panelX(200, false, 8);
    expect(x.slot).toBeCloseTo((200 - FACET_PAD.right - FACET_PAD.bare) / 8, 6);
    expect(x.xAt(0)).toBeCloseTo(FACET_PAD.bare + x.slot / 2, 6);
    expect(x.xAt(7)).toBeGreaterThan(x.xAt(6));
    expect(x.xAt(7) + x.slot / 2).toBeCloseTo(x.pr, 6);
  });

  it('maps a panel-local x back to the slot under it (fractional, so the pointer can round)', () => {
    const x = panelX(200, true, 8);
    expect(x.stepAt(x.xAt(3))).toBeCloseTo(3, 6);
    expect(x.stepAt(x.xAt(3) + x.slot * 0.4)).toBeCloseTo(3.4, 6);
  });
});
