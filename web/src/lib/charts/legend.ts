// Legend placement for ChartLegend and the stack's panel headers (proposal 08 section 1.1). Pure: no DOM, so the
// header height that stackLayout needs and the swatches ChartLegend draws come from the same numbers.

/** The mark a swatch imitates. Dash style comes from the series' CSS class, not from the mark. */
export type LegendMark = 'line' | 'band' | 'bar' | 'hatch' | 'area' | 'arrow';

/** One legend entry. `cls` is the series' own CSS class, so the swatch cannot drift from the mark it describes. */
export interface LegendItem {
  label: string;
  mark: LegendMark;
  cls: string;
}

export const LEGEND_ROW = 18; // px per legend row (12 px text)
export const LEGEND_SWATCH = 22; // px across the swatch
const SWATCH_GAP = 6; // px between the swatch and its label
const ITEM_GAP = 14; // px between items on one row
// Generous: a 12 px system-font character is about 6.5 px, so the estimate keeps a label clear of the next swatch.
const CHAR_PX = 7;

interface PlacedItem {
  item: LegendItem;
  x: number; // offset from the legend's left edge
  row: number; // 0-based
}

interface LegendLayout {
  placed: PlacedItem[];
  rows: number;
  height: number; // px
}

/** Width of one item (swatch, gap and label), from the label length. */
export function itemWidth(item: LegendItem): number {
  return LEGEND_SWATCH + SWATCH_GAP + item.label.length * CHAR_PX;
}

/** Places items left to right; an item that does not fit the width starts a new row. */
export function legendLayout(items: LegendItem[], width: number): LegendLayout {
  const placed: PlacedItem[] = [];
  let x = 0;
  let row = 0;
  for (const item of items) {
    const w = itemWidth(item);
    if (x > 0 && x + w > width) {
      row += 1;
      x = 0;
    }
    placed.push({ item, x, row });
    x += w + ITEM_GAP;
  }
  const rows = items.length ? row + 1 : 0;
  return { placed, rows, height: rows * LEGEND_ROW };
}
