import { describe, expect, it } from 'vitest';
import { useCrosshair } from './useCrosshair.svelte';

// Ten steps, 10 px apart; a pixel x maps to step x / 10, as the stack's hours do.
function make(n = 10) {
  return useCrosshair({ count: () => n, stepAt: (px) => px / 10, xAt: (i) => i * 10 });
}

function key(k: string) {
  const e = { key: k, prevented: false, preventDefault() { this.prevented = true; } };
  return e as unknown as KeyboardEvent & { prevented: boolean };
}

describe('useCrosshair pointer', () => {
  it('picks the nearest step under the pointer and reports its pixel x', () => {
    const xh = make();
    xh.onhover(23);
    expect(xh.index).toBe(2);
    expect(xh.px).toBe(20);
  });

  it('clamps to the first and last step', () => {
    const xh = make();
    xh.onhover(-50);
    expect(xh.index).toBe(0);
    xh.onhover(999);
    expect(xh.index).toBe(9);
  });

  it('clears when the pointer leaves', () => {
    const xh = make();
    xh.onhover(40);
    xh.onhover(null);
    expect(xh.index).toBeNull();
    expect(xh.px).toBeNull();
  });
});

describe('useCrosshair keys', () => {
  it('ArrowRight starts at the first step, then moves one step and stops at the last', () => {
    const xh = make(3);
    xh.onkey(key('ArrowRight'));
    expect(xh.index).toBe(0);
    xh.onkey(key('ArrowRight'));
    xh.onkey(key('ArrowRight'));
    xh.onkey(key('ArrowRight'));
    expect(xh.index).toBe(2);
  });

  it('ArrowLeft starts at the first step and stops at zero', () => {
    const xh = make();
    xh.onkey(key('ArrowLeft'));
    expect(xh.index).toBe(0);
    xh.onkey(key('ArrowLeft'));
    expect(xh.index).toBe(0);
  });

  it('Escape clears; other keys are left alone and not prevented', () => {
    const xh = make();
    xh.onkey(key('ArrowRight'));
    const tab = key('Tab');
    xh.onkey(tab);
    expect(xh.index).toBe(0);
    expect(tab.prevented).toBe(false);
    const esc = key('Escape');
    xh.onkey(esc);
    expect(xh.index).toBeNull();
    expect(esc.prevented).toBe(true);
  });

  it('prevents the default for the keys it handles', () => {
    const xh = make();
    const e = key('ArrowRight');
    xh.onkey(e);
    expect(e.prevented).toBe(true);
  });
});

describe('useCrosshair clear', () => {
  it('hides the crosshair without a key or pointer event', () => {
    const xh = make();
    xh.onhover(50);
    xh.clear();
    expect(xh.index).toBeNull();
  });
});

describe('useCrosshair with no steps', () => {
  it('has no crosshair for a pointer or a key, rather than a step of -1', () => {
    const xh = make(0);
    xh.onhover(50);
    expect(xh.index).toBeNull();
    xh.onkey(key('ArrowRight'));
    expect(xh.index).toBeNull();
  });
});
