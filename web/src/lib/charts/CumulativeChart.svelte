<script lang="ts">
  import type { YtdStation } from '$lib/api/types';
  import { WINTER_MONTHS } from '$lib/config';
  import { mdLabel } from '$lib/labels';
  import { bandPaths } from '$lib/transforms/bands';
  import { monthOfMd } from '$lib/transforms/dates';
  import { splitCurrent, toPoints } from '$lib/transforms/series';
  import { buildSlots, fieldOf, type Metric } from '$lib/transforms/slots';
  import ChartFrame from './ChartFrame.svelte';
  import ChartLegend from './ChartLegend.svelte';
  import ChartTooltip from './ChartTooltip.svelte';
  import { legendLayout, type LegendItem } from './legend';
  import { H, M, linePath, makeScales, ribbonPath } from './scales';
  import { cumulativeRows } from './tipRows';
  import { useCrosshair } from './useCrosshair.svelte';
  import { tipText, type TipBox } from './tooltip';

  // Measured width, drawn at 1:1 pixels (like the other charts), so the 12 px text is not scaled with the card.
  const MAX_WIDTH = 1400;
  const DEFAULT_WIDTH = 900;
  let measured = $state(0);
  const width = $derived(Math.min(MAX_WIDTH, measured || DEFAULT_WIDTH));

  let {
    ytd,
    metric,
    window: win,
    valueLabel,
    yDomain,
    provenance,
    title
  }: {
    ytd: YtdStation;
    metric: Metric;
    window: { start: string; end: string };
    valueLabel: string;
    yDomain?: number;
    provenance: string[];
    title?: string;
  } = $props();

  const field = $derived(fieldOf(metric));
  const slots = $derived(buildSlots(ytd, metric));
  const layers = $derived(bandPaths(ytd.bands, metric, slots));
  const split = $derived(splitCurrent(ytd.current, field));
  const completePts = $derived(toPoints(split.complete, slots));
  const partialPts = $derived(toPoints(split.partial, slots));
  const priors = $derived(
    ytd.prior_years.map((y) => ({ year: y.year, pts: toPoints(y[field], slots), drawn: y[field] !== null, missing: y.missing_days[metric] }))
  );

  const yMax = $derived.by(() => {
    if (yDomain !== undefined) return yDomain;
    let m = 0;
    const bump = (p: ({ y: number } | null)[]) => {
      for (const q of p) if (q && q.y > m) m = q.y;
    };
    bump(completePts);
    bump(partialPts);
    for (const p of priors) bump(p.pts);
    if (layers.outer) bump(layers.outer.upper);
    return m * 1.05;
  });
  const sc = $derived(makeScales(slots.length, yMax, width, H));
  const xs = $derived((v: number) => sc.x(v));
  const ys = $derived((v: number) => sc.y(v));

  const monthTicks = $derived.by(() => {
    const out: { x: number; label: string }[] = [];
    let prev = -1;
    slots.forEach((md, i) => {
      const m = monthOfMd(md);
      if (m !== prev) {
        out.push({ x: i, label: mdLabel(md).split(' ')[0].slice(0, 3) });
        prev = m;
      }
    });
    return out;
  });
  const yTicks = $derived(sc.y.ticks(5));

  const winterRects = $derived.by(() => {
    if (metric !== 'rain') return [] as { x: number; w: number }[];
    const out: { x: number; w: number }[] = [];
    let start = -1;
    slots.forEach((md, i) => {
      const w = (WINTER_MONTHS as readonly number[]).includes(monthOfMd(md));
      if (w && start < 0) start = i;
      if ((!w || i === slots.length - 1) && start >= 0) {
        const end = w ? i : i - 1;
        out.push({ x: xs(start), w: Math.max(xs(end) - xs(start), 1) });
        start = -1;
      }
    });
    return out;
  });

  const startsLate = $derived(ytd.current.first_date !== null && ytd.current.first_date.slice(5) > win.start);
  const missingCurrent = $derived(ytd.current.missing_days[metric]);
  const excluded = $derived(priors.filter((p) => !p.drawn));

  // Legend rows above the plot: only the series this chart draws.
  const legendItems = $derived.by((): LegendItem[] => {
    const out: LegendItem[] = [{ label: 'This year', mark: 'line', cls: 'line-current' }];
    if (split.partial.length > 0) out.push({ label: 'Today (partial)', mark: 'line', cls: 'line-current-partial' });
    if (layers.median) out.push({ label: 'Median', mark: 'line', cls: 'line-median' });
    if (priors.some((p) => p.drawn)) out.push({ label: 'Prior years', mark: 'line', cls: 'line-prior' });
    if (layers.outer) out.push({ label: 'Percentile band', mark: 'band', cls: 'band-outer' });
    return out;
  });
  // The legend spans the plot (between its left and right margins), so no item runs past the plot's right edge.
  const legendWidth = $derived(width - M.l - M.r);
  const legendHeight = $derived(legendLayout(legendItems, legendWidth).height);

  // Hover or arrow keys pick one day (proposal 08 section 2.5). The tooltip names the values; the slider's aria-valuetext is the same box.
  const xh = useCrosshair({ count: () => slots.length, stepAt: (px) => sc.x.invert(px), xAt: (i) => sc.x(i) });
  const cursor = $derived(xh.index);
  const cursorX = $derived(xh.px);
  function pointerPx(e: PointerEvent): number {
    const box = (e.currentTarget as SVGSVGElement).getBoundingClientRect();
    return ((e.clientX - box.left) * width) / box.width;
  }

  const fmt = (v: number | undefined | null) => (v === undefined || v === null ? '-' : v.toFixed(metric === 'rain' ? 2 : 0));
  const curByMd = $derived(new Map(ytd.current[field].map((p) => [p.md, p.v])));
  const bandRows = $derived(new Map((ytd.bands?.[field] ?? []).map((b) => [b.md, b.p])));
  // The tooltip: this year (complete days), today (the open day), median, band range and each prior year drawn.
  const completeByMd = $derived(new Map(split.complete.map((p) => [p.md, p.v])));
  const partialByMd = $derived(new Map(split.partial.map((p) => [p.md, p.v])));
  const tip = $derived.by((): TipBox | null => {
    const i = cursor;
    if (i === null || slots[i] === undefined) return null;
    const md = slots[i];
    const p = bandRows.get(md);
    const ps = ytd.bands?.percentiles ?? [];
    const medIdx = ps.indexOf(50);
    const rows = cumulativeRows(
      {
        thisYear: completeByMd.get(md) ?? null,
        // the partial segment is joined to the last complete day, so that shared day is not repeated as "today"
        partial: completeByMd.has(md) ? null : (partialByMd.get(md) ?? null),
        median: p && medIdx >= 0 ? p[medIdx] : null,
        bandLo: p && ps.length >= 2 ? p[0] : null,
        bandHi: p && ps.length >= 2 ? p[p.length - 1] : null,
        priors: priors.filter((q) => q.drawn).map((q) => ({ year: q.year, v: q.pts[i]?.y ?? null }))
      },
      metric === 'rain' ? 2 : 0,
      ` ${valueLabel}`
    );
    return { title: mdLabel(md), rows };
  });
</script>

<ChartFrame {title} {provenance}>
  <p class="small muted chart-notes">
    {#if missingCurrent > 0}<span data-testid="missing-current">{missingCurrent} days missing this season (counted as zero)</span>{/if}
  </p>
  {#if startsLate}
    <p class="small muted" data-testid="starts-note">
      Data starts {ytd.current.first_date}; earlier days are not included, so the total reads low against the baseline.
    </p>
  {/if}
  <div class="fit" bind:clientWidth={measured}>
    {#if legendHeight > 0}
      <svg class="chart-svg plot legend-row" {width} height={legendHeight} aria-hidden="true">
        <ChartLegend items={legendItems} x={M.l} y={0} width={legendWidth} testid="legend-cumulative" />
      </svg>
    {/if}
    <svg
      class="chart-svg plot"
      {width}
      height={H}
      role="slider"
      tabindex="0"
      aria-label={`${metric === 'rain' ? 'Rain' : 'Growing degree days'} for the season, cumulative`}
      aria-valuemin={0}
      aria-valuemax={Math.max(slots.length - 1, 0)}
      aria-valuenow={cursor ?? undefined}
      aria-valuetext={tip ? tipText(tip) : undefined}
      onpointermove={(e) => xh.onhover(pointerPx(e))}
      onpointerleave={() => xh.onhover(null)}
      onkeydown={xh.onkey}
    >
      {#each winterRects as r (r.x)}
        <rect class="winter" x={r.x} y={M.t} width={r.w} height={H - M.t - M.b} />
      {/each}
      {#each yTicks as t (t)}
        <line class="grid-line" x1={M.l} x2={width - M.r} y1={ys(t)} y2={ys(t)} />
        <text x={M.l - 6} y={ys(t) + 4} text-anchor="end">{t}</text>
      {/each}
      {#each monthTicks as t (t.x)}
        <text x={xs(t.x)} y={H - 8}>{t.label}</text>
      {/each}
      {#if layers.outer}<path class="band-outer" d={ribbonPath(layers.outer.lower, layers.outer.upper, xs, ys)} />{/if}
      {#if layers.inner}<path class="band-inner" d={ribbonPath(layers.inner.lower, layers.inner.upper, xs, ys)} />{/if}
      {#if layers.median}<path class="line-median" d={linePath(layers.median, xs, ys)} />{/if}
      {#each priors as p (p.year)}
        {#if p.drawn}<path class="line-prior" data-year={p.year} d={linePath(p.pts, xs, ys)} />{/if}
      {/each}
      <path class="line-current" d={linePath(completePts, xs, ys)} />
      {#if split.partial.length > 0}<path class="line-current-partial" d={linePath(partialPts, xs, ys)} />{/if}
      {#if cursorX !== null}<line class="crosshair" x1={cursorX} x2={cursorX} y1={M.t} y2={H - M.b} />{/if}
      {#if cursorX !== null && tip}
        <ChartTooltip box={tip} anchorX={cursorX} plotLeft={M.l} plotRight={width - M.r} top={M.t} bottom={H - M.b} testid="tip-cumulative" />
      {/if}
    </svg>
  </div>
  <details class="data-table">
    <summary>Show data table</summary>
    <div class="table-scroll">
      <table class="data" data-testid="data-table">
        <thead>
          <tr>
            <th>Date</th><th>This year</th><th>Median</th>
            {#each priors.filter((p) => p.drawn) as p (p.year)}<th>{p.year}</th>{/each}
          </tr>
        </thead>
        <tbody>
          {#each slots as md, i (md)}
            {@const bp = bandRows.get(md)}
            <tr>
              <td>{mdLabel(md)}</td>
              <td>{fmt(curByMd.get(md))}</td>
              <td>{fmt(bp && ytd.bands?.percentiles.includes(50) ? bp[ytd.bands.percentiles.indexOf(50)] : undefined)}</td>
              {#each priors.filter((q) => q.drawn) as p (p.year)}<td>{fmt(p.pts[i]?.y)}</td>{/each}
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
    {#if priors.some((p) => p.missing > 0) || excluded.length > 0}
      <ul class="small muted">
        {#each priors as p (p.year)}
          {#if !p.drawn}
            <li>{p.year}: excluded: too many missing days</li>
          {:else if p.missing > 0}
            <li>{p.year}: {p.missing} missing days</li>
          {/if}
        {/each}
      </ul>
      <p class="small muted">Missing days are counted as zero, as supplied by the API.</p>
    {/if}
  </details>
</ChartFrame>
