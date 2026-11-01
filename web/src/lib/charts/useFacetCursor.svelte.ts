// Shared cursor for the changes chart's facet grid (proposal 09 section 6): two indices, the panel (a target day) and
// the lead slot (oldest first, so the last slot is the latest run). It keeps useCrosshair's rules for the lead: the first
// key press starts at the first step, Left and Right move one step, Escape clears, and other keys are left alone. It
// adds the panel axis: Up and Down move one row of the grid (`cols` panels), or one panel when the grid is a single
// row; PageUp and PageDown always move one panel. It is a separate hook, not a second axis on useCrosshair, because
// the pointer picks both indices at once.
const NAV_KEYS = ['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'PageUp', 'PageDown'];

export interface FacetCursorOptions {
  panels: () => number; // how many panels the grid draws (0 means none)
  leads: () => number; // how many lead slots each panel has (0 means none)
  cols: () => number; // panels per row of the grid, for Up and Down
  rows: () => number; // rows in the grid: with one row, Up and Down step one panel
}

export function useFacetCursor(opts: FacetCursorOptions) {
  let panel = $state<number | null>(null);
  let lead = $state<number | null>(null);

  const clamp = (v: number, n: number) => Math.min(n - 1, Math.max(0, v));

  return {
    /** The panel under the cursor (pointer or keyboard), or null. */
    get panel(): number | null {
      return panel;
    },
    /** The lead slot under the cursor, oldest first, or null. */
    get lead(): number | null {
      return lead;
    },
    /** Hides the cursor (for example when the pointer leaves the grid). */
    clear() {
      panel = null;
      lead = null;
    },
    /** Pointer: the panel under it (null when none is) and the fractional lead slot in that panel's own x scale. */
    onhover(p: number | null, step: number) {
      const np = opts.panels();
      const nl = opts.leads();
      if (p === null || !np || !nl) {
        panel = null;
        lead = null;
        return;
      }
      panel = clamp(Math.round(p), np);
      lead = clamp(Math.round(step), nl);
    },
    /** Arrow keys and PageUp and PageDown move the cursor (starting at the first panel and the oldest lead); Escape clears. */
    onkey(e: KeyboardEvent) {
      const np = opts.panels();
      const nl = opts.leads();
      if (e.key === 'Escape') {
        panel = null;
        lead = null;
      } else if (NAV_KEYS.includes(e.key) && np && nl) {
        if (panel === null || lead === null) {
          panel = 0;
          lead = 0;
        } else {
          const row = opts.rows() > 1 ? Math.max(1, opts.cols()) : 1;
          if (e.key === 'ArrowRight') lead = clamp(lead + 1, nl);
          else if (e.key === 'ArrowLeft') lead = clamp(lead - 1, nl);
          else if (e.key === 'ArrowDown') panel = clamp(panel + row, np);
          else if (e.key === 'ArrowUp') panel = clamp(panel - row, np);
          else if (e.key === 'PageDown') panel = clamp(panel + 1, np);
          else panel = clamp(panel - 1, np);
        }
      } else {
        return;
      }
      e.preventDefault();
    }
  };
}
