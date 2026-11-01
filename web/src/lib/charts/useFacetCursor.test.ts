import { describe, expect, it } from 'vitest';
import { useFacetCursor } from './useFacetCursor.svelte';

// Seven panels in four columns (the 1200 px layout: 4 + 3) and eight lead slots, as the changes chart draws them.
function make(panels = 7, leads = 8, cols = 4, rows = 2) {
  return useFacetCursor({ panels: () => panels, leads: () => leads, cols: () => cols, rows: () => rows });
}

function key(k: string) {
  const e = { key: k, prevented: false, preventDefault() { this.prevented = true; } };
  return e as unknown as KeyboardEvent & { prevented: boolean };
}

describe('useFacetCursor pointer', () => {
  it('picks the panel under the pointer and the nearest lead slot in it', () => {
    const c = make();
    c.onhover(2, 5.4);
    expect(c.panel).toBe(2);
    expect(c.lead).toBe(5);
  });

  it('rounds the fractional lead and clamps both indices to the grid', () => {
    const c = make();
    c.onhover(3, 6.6);
    expect(c.lead).toBe(7);
    c.onhover(99, -4);
    expect(c.panel).toBe(6);
    expect(c.lead).toBe(0);
  });

  it('clears when the pointer leaves the grid (a null panel)', () => {
    const c = make();
    c.onhover(1, 2);
    c.onhover(null, 0);
    expect(c.panel).toBeNull();
    expect(c.lead).toBeNull();
  });

  it('clears when there are no panels or no leads', () => {
    const none = make(0, 8);
    none.onhover(0, 0);
    expect(none.panel).toBeNull();
    const noLeads = make(7, 0);
    noLeads.onhover(0, 0);
    expect(noLeads.lead).toBeNull();
  });
});

describe('useFacetCursor keys', () => {
  it('the first key press starts at the first panel and the oldest lead, as useCrosshair does', () => {
    const c = make();
    c.onkey(key('ArrowRight'));
    expect(c.panel).toBe(0);
    expect(c.lead).toBe(0);
  });

  it('Left and Right move the lead one slot and stop at the ends', () => {
    const c = make(7, 3);
    c.onkey(key('ArrowRight'));
    c.onkey(key('ArrowRight'));
    c.onkey(key('ArrowRight'));
    c.onkey(key('ArrowRight'));
    expect(c.lead).toBe(2);
    c.onkey(key('ArrowLeft'));
    expect(c.lead).toBe(1);
    c.onkey(key('ArrowLeft'));
    c.onkey(key('ArrowLeft'));
    expect(c.lead).toBe(0);
    expect(c.panel).toBe(0);
  });

  it('Down and Up move one row of the grid (cols panels), and stop at the ends', () => {
    const c = make(7, 8, 4);
    c.onkey(key('ArrowRight'));
    c.onkey(key('ArrowDown'));
    expect(c.panel).toBe(4);
    c.onkey(key('ArrowDown'));
    expect(c.panel).toBe(6);
    c.onkey(key('ArrowUp'));
    expect(c.panel).toBe(2);
    c.onkey(key('ArrowUp'));
    c.onkey(key('ArrowUp'));
    expect(c.panel).toBe(0);
  });

  it('with a single row, Down and Up step one panel, not to the ends', () => {
    const c = make(7, 8, 7, 1);
    c.onkey(key('ArrowRight'));
    c.onkey(key('ArrowDown'));
    expect(c.panel).toBe(1);
    c.onkey(key('ArrowDown'));
    expect(c.panel).toBe(2);
    c.onkey(key('ArrowUp'));
    expect(c.panel).toBe(1);
  });

  it('PageDown and PageUp move one panel in reading order', () => {
    const c = make(7, 8, 4);
    c.onkey(key('ArrowRight'));
    c.onkey(key('PageDown'));
    expect(c.panel).toBe(1);
    c.onkey(key('PageUp'));
    c.onkey(key('PageUp'));
    expect(c.panel).toBe(0);
  });

  it('keeps the lead when a key moves the panel, and the panel when a key moves the lead', () => {
    const c = make(7, 8, 4);
    c.onhover(5, 3);
    c.onkey(key('ArrowDown'));
    expect(c.lead).toBe(3);
    c.onkey(key('ArrowLeft'));
    expect(c.panel).toBe(6);
    expect(c.lead).toBe(2);
  });

  it('Escape clears; other keys are left alone and not prevented', () => {
    const c = make();
    c.onkey(key('ArrowRight'));
    const tab = key('Tab');
    c.onkey(tab);
    expect(c.panel).toBe(0);
    expect(tab.prevented).toBe(false);
    const esc = key('Escape');
    c.onkey(esc);
    expect(c.panel).toBeNull();
    expect(c.lead).toBeNull();
    expect(esc.prevented).toBe(true);
  });

  it('prevents the default for the keys it handles', () => {
    const c = make();
    const e = key('ArrowDown');
    c.onkey(e);
    expect(e.prevented).toBe(true);
  });

  it('ignores the arrow keys when the grid has no panels', () => {
    const c = make(0, 8);
    const e = key('ArrowRight');
    c.onkey(e);
    expect(c.panel).toBeNull();
    expect(e.prevented).toBe(false);
  });
});

describe('useFacetCursor clear', () => {
  it('hides the cursor without a key or pointer event', () => {
    const c = make();
    c.onhover(1, 1);
    c.clear();
    expect(c.panel).toBeNull();
    expect(c.lead).toBeNull();
  });
});
