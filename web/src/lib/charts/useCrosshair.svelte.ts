// Shared crosshair for the hover and keyboard pattern (proposal 08 section 2.5): a pointer picks the step under it,
// the arrow keys step through the steps, Escape clears. The chart keeps the returned object and passes `onhover` and
// `onkey` to its PanelStack (or other slider), and reads `index` and `px` to draw the crosshair and its readout.
interface CrosshairOptions {
  count: () => number; // how many steps the index runs over (0 means no steps)
  stepAt: (px: number) => number; // the continuous step under a pixel x; the helper rounds and clamps it
  xAt: (step: number) => number; // pixel x of a step, for the crosshair line
}

export function useCrosshair(opts: CrosshairOptions) {
  let index = $state<number | null>(null);

  return {
    /** The step under the crosshair (pointer or keyboard), or null. */
    get index(): number | null {
      return index;
    },
    /** Pixel x of the crosshair, or null when there is none. */
    get px(): number | null {
      return index === null ? null : opts.xAt(index);
    },
    /** Hides the crosshair (for example when the window it refers to changes). */
    clear() {
      index = null;
    },
    /** Pointer position in pixels, or null when the pointer leaves. */
    onhover(px: number | null) {
      if (px === null) {
        index = null;
        return;
      }
      const n = opts.count();
      if (!n) {
        index = null;
        return;
      }
      index = Math.min(n - 1, Math.max(0, Math.round(opts.stepAt(px))));
    },
    /** Left and right arrows move one step (starting at the first step); Escape clears; other keys are left alone. */
    onkey(e: KeyboardEvent) {
      const n = opts.count();
      if (!n) return;
      if (e.key === 'ArrowRight') index = index === null ? 0 : Math.min(n - 1, index + 1);
      else if (e.key === 'ArrowLeft') index = index === null ? 0 : Math.max(0, index - 1);
      else if (e.key === 'Escape') index = null;
      else return;
      e.preventDefault();
    }
  };
}
