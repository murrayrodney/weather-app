<script lang="ts">
  import { scaleLinear } from 'd3-scale';
  import type { DayLead } from '$lib/transforms/forecast';
  import type { Pt } from '$lib/transforms/series';
  import { FACET_PAD, panelX } from './facetLayout';
  import { linePath } from './scales';

  // One panel of the "How the forecast changed" grid (proposal 09 sections 2 to 5): one target day, drawn inside a
  // <g transform> of the chart's single SVG. x is the lead, oldest on the left and the latest run on the right; y is
  // the tab's shared domain, so panels compare by eye. A not-issued lead keeps its slot (shaded) and draws no mark.
  // The metric's colour comes from the tone class on the chart's SVG (--tone), so every mark here is classed, not styled.
  let {
    title,
    kind,
    points,
    domain,
    ticks,
    tickText,
    showAxis,
    cursor,
    w,
    h
  }: {
    title: string; // the day: "Today", "Tomorrow", "Mon 3"
    kind: 'bar' | 'line';
    points: DayLead[]; // oldest first (lead 7 to lead 0)
    domain: [number, number];
    ticks: number[];
    tickText: (v: number) => string;
    showAxis: boolean; // y tick labels, drawn in the left column only
    cursor: number | null; // the lead slot under the grid's cursor, drawn as a tick in every panel
    w: number; // the cell's size, px
    h: number;
  } = $props();

  const x = $derived(panelX(w, showAxis, points.length));
  const pl = $derived(x.pl);
  const pr = $derived(x.pr);
  const pt = FACET_PAD.top;
  const pb = $derived(h - FACET_PAD.bottom);
  const n = $derived(points.length);
  const slot = $derived(x.slot);
  const bar = $derived(slot * 0.6);

  const ys = $derived(scaleLinear().domain(domain).range([pb, pt]));
  const yAt = (v: number) => ys(v);
  const xAt = (i: number) => x.xAt(i);

  // The line breaks at null and at not-issued leads, so the latest point joins the older ones only by position.
  const linePts = $derived<(Pt | null)[]>(points.map((p, i) => (p.issued && p.value !== null ? { x: i, y: p.value } : null)));
  const d = $derived(linePath(linePts, xAt, yAt));

  // Runs of not-issued leads: one shaded band per run, so neighbouring slots do not show a seam.
  const gaps = $derived.by(() => {
    const out: { from: number; to: number }[] = [];
    points.forEach((p, i) => {
      if (p.issued) return;
      const last = out[out.length - 1];
      if (last && last.to === i - 1) last.to = i;
      else out.push({ from: i, to: i });
    });
    return out;
  });

  const latest = $derived(points[n - 1]);
  const latestY = $derived(latest && latest.issued && latest.value !== null ? yAt(latest.value) : null);
</script>

<g>
  <text class="facet-title" x={pl} y="14">{title}</text>
  {#each gaps as g (g.from)}
    <rect class="facet-unissued" x={pl + g.from * slot} y={pt} width={(g.to - g.from + 1) * slot} height={pb - pt} />
  {/each}
  {#each ticks as t (t)}
    <line class="grid-line" x1={pl} x2={pr} y1={yAt(t)} y2={yAt(t)} />
    {#if showAxis}<text class="tick-y" x={pl - 4} y={yAt(t) + 4} text-anchor="end">{tickText(t)}</text>{/if}
  {/each}
  {#if kind === 'bar'}
    {#each points as p, i (i)}
      {#if p.issued && p.value !== null}
        {@const top = p.value > 0 ? yAt(p.value) : pb - 2}
        <rect class={i === n - 1 ? 'bar-precip bar-latest' : 'bar-precip'} x={xAt(i) - bar / 2} y={top} width={bar} height={pb - top} />
      {/if}
    {/each}
  {:else}
    <path class="line-facet" d={d} />
  {/if}
  {#if latestY !== null}
    <line class="ref-latest" x1={pl} x2={pr} y1={latestY} y2={latestY} />
    <circle class="dot-latest" cx={xAt(n - 1)} cy={latestY} r="3.5" />
  {/if}
  {#if cursor !== null && cursor < n}
    <line class="crosshair" x1={xAt(cursor)} x2={xAt(cursor)} y1={pt} y2={pb} />
  {/if}
  <text x={pl} y={h - 7}>7d</text>
  <text x={pr} y={h - 7} text-anchor="end">now</text>
</g>
