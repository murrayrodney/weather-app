import { describe, expect, it } from 'vitest';
import { itemWidth, LEGEND_ROW, legendLayout, type LegendItem } from './legend';

const item = (label: string, cls = 'line-x'): LegendItem => ({ label, mark: 'line', cls });

describe('legendLayout', () => {
  it('has no rows and no height without items', () => {
    expect(legendLayout([], 500)).toEqual({ placed: [], rows: 0, height: 0 });
  });

  it('keeps every item in one row when the labels fit the width', () => {
    const items = [item('Forecast'), item('Dew point')];
    const l = legendLayout(items, 500);
    expect(l.rows).toBe(1);
    expect(l.height).toBe(LEGEND_ROW);
    expect(l.placed.map((p) => p.row)).toEqual([0, 0]);
    expect(l.placed[1].x).toBeGreaterThan(l.placed[0].x);
  });

  it('wraps an item that does not fit to a second row, starting again at x = 0', () => {
    const items = [item('Accumulated range'), item('Chance of precipitation'), item('Snow')];
    // The first two do not share a row at this width; "Snow" follows the second one only if it fits there.
    const width = itemWidth(items[0]) + 10;
    const l = legendLayout(items, width);
    expect(l.placed.map((p) => p.row)).toEqual([0, 1, 2]);
    expect(l.placed[1].x).toBe(0);
    expect(l.rows).toBe(3);
    expect(l.height).toBe(3 * LEGEND_ROW);
  });

  it('never leaves a single item without a row, even wider than the width', () => {
    const l = legendLayout([item('An extremely long legend label that cannot fit')], 40);
    expect(l.placed).toEqual([{ item: expect.anything(), x: 0, row: 0 }]);
    expect(l.rows).toBe(1);
  });

  it('no item runs past the width it is laid out in (when it fits a row by itself)', () => {
    const items = [item('Forecast'), item('Ensemble median'), item('Range p10-p90'), item('Feels like'), item('Dew point'), item('32 F')];
    for (const width of [200, 320, 500, 760]) {
      const l = legendLayout(items, width);
      for (const p of l.placed) expect(p.x + itemWidth(p.item), `${p.item.label} at width ${width}`).toBeLessThanOrEqual(width);
    }
  });

  it('places the swatch and label width from the label length', () => {
    expect(itemWidth(item('abcd'))).toBeGreaterThan(itemWidth(item('ab')));
  });
});
