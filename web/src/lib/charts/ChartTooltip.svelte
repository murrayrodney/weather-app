<script lang="ts">
  import { placeTooltip, TIP_LABEL_X, TIP_PAD, TIP_ROW, TIP_SWATCH, TIP_TITLE, tipHeight, tipWidth, type TipBox } from './tooltip';

  // One panel's tooltip, drawn inside the stack's SVG as one of its last children (proposal 08 sections 2.1 and 6.2).
  // It sits at the crosshair x and is centred vertically in its panel, clamped inside it. Classes and attributes only
  // (no style), and aria-hidden: the chart's slider aria-valuetext is the accessible channel.
  let {
    box,
    anchorX,
    plotLeft,
    plotRight,
    top,
    bottom,
    hatchId,
    testid
  }: {
    box: TipBox;
    anchorX: number; // crosshair x
    plotLeft: number; // left edge of the plot
    plotRight: number; // right edge of the plot
    top: number; // the panel's plot top and bottom
    bottom: number;
    hatchId?: string; // the chart's snow pattern, for a hatch swatch
    testid: string;
  } = $props();

  const w = $derived(tipWidth(box));
  const h = $derived(tipHeight(box.rows.length));
  const pos = $derived(placeTooltip({ x: anchorX, y: (top + bottom) / 2, w, h, left: plotLeft, plotW: plotRight, top, bottom }));
</script>

<g class="tip" transform={`translate(${pos.tx} ${pos.ty})`} aria-hidden="true" data-testid={testid}>
  <rect class="tip-bg" width={w} height={h} rx="3" />
  <text class="tip-title" x={TIP_PAD} y={TIP_PAD + 13}>{box.title}</text>
  {#each box.rows as r, k (k)}
    {@const cy = TIP_PAD + TIP_TITLE + k * TIP_ROW + TIP_ROW / 2}
    {#if r.mark === 'line' && r.cls}
      <line class={r.cls} x1={TIP_PAD} x2={TIP_PAD + TIP_SWATCH} y1={cy} y2={cy} />
    {:else if (r.mark === 'band' || r.mark === 'area') && r.cls}
      <rect class={r.cls} x={TIP_PAD} y={cy - 4} width={TIP_SWATCH} height="8" />
    {:else if r.mark === 'bar' && r.cls}
      <rect class={r.cls} x={TIP_PAD + 2} y={cy - 5} width="10" height="10" />
    {:else if r.mark === 'hatch' && r.cls}
      <rect class={r.cls} x={TIP_PAD + 2} y={cy - 5} width="10" height="10" fill={hatchId ? `url(#${hatchId})` : undefined} />
    {/if}
    <text class="tip-label" x={TIP_LABEL_X} y={cy + 4}>{r.label}</text>
    <text class="tip-value" x={w - TIP_PAD} y={cy + 4} text-anchor="end">{r.value}</text>
  {/each}
</g>
