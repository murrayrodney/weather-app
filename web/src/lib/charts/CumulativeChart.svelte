<script lang="ts">
  import type { YtdStation } from '$lib/api/types';
  import { WINTER_MONTHS } from '$lib/config';
  import { mdLabel } from '$lib/labels';
  import { bandPaths } from '$lib/transforms/bands';
  import { monthOfMd } from '$lib/transforms/dates';
  import { splitCurrent, toPoints } from '$lib/transforms/series';
  import { buildSlots, fieldOf, type Metric } from '$lib/transforms/slots';
  import ChartFrame from './ChartFrame.svelte';
  import { H, M, W, linePath, makeScales, ribbonPath } from './scales';

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
  const sc = $derived(makeScales(slots.length, yMax));
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

  let cursor = $state<number | null>(null);
  function move(e: PointerEvent) {
    const svg = e.currentTarget as SVGSVGElement;
    const r = svg.getBoundingClientRect();
    if (r.width === 0 || slots.length === 0) return;
    const px = ((e.clientX - r.left) / r.width) * W;
    const idx = Math.round(sc.x.invert(px));
    cursor = Math.max(0, Math.min(slots.length - 1, idx));
  }
  function key(e: KeyboardEvent) {
    if (slots.length === 0) return;
    if (e.key === 'ArrowRight') cursor = Math.min(slots.length - 1, (cursor ?? -1) + 1);
    else if (e.key === 'ArrowLeft') cursor = Math.max(0, (cursor ?? slots.length) - 1);
    else return;
    e.preventDefault();
  }

  const fmt = (v: number | undefined | null) => (v === undefined || v === null ? '-' : v.toFixed(metric === 'rain' ? 2 : 0));
  const curByMd = $derived(new Map(ytd.current[field].map((p) => [p.md, p.v])));
  const bandRows = $derived(new Map((ytd.bands?.[field] ?? []).map((b) => [b.md, b.p])));
  const readout = $derived.by(() => {
    if (cursor === null || slots[cursor] === undefined) return '';
    const md = slots[cursor];
    const p = bandRows.get(md);
    const ps = ytd.bands?.percentiles ?? [];
    const med = p && ps.includes(50) ? p[ps.indexOf(50)] : undefined;
    const range = p && ps.length >= 2 ? `${fmt(p[0])} to ${fmt(p[p.length - 1])}` : '-';
    return `${mdLabel(md)}: this year ${fmt(curByMd.get(md))} ${valueLabel}; median ${fmt(med)}; band ${range}`;
  });
  const cursorX = $derived(cursor === null ? null : xs(cursor));
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
  <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
  <svg
    class="chart-svg"
    viewBox={`0 0 ${W} ${H}`}
    role="img"
    tabindex="0"
    aria-label={`${metric === 'rain' ? 'Rain' : 'Growing degree days'} for the season, cumulative`}
    onpointermove={move}
    onpointerleave={() => (cursor = null)}
    onkeydown={key}
  >
    {#each winterRects as r (r.x)}
      <rect class="winter" x={r.x} y={M.t} width={r.w} height={H - M.t - M.b} />
    {/each}
    {#each yTicks as t (t)}
      <line class="grid-line" x1={M.l} x2={W - M.r} y1={ys(t)} y2={ys(t)} />
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
    {#if cursorX !== null}<line class="cursor" x1={cursorX} x2={cursorX} y1={M.t} y2={H - M.b} />{/if}
  </svg>
  <div class="legend">
    <span><span class="swatch"></span>This year</span>
    {#if split.partial.length > 0}<span><span class="swatch partial"></span>Today (partial)</span>{/if}
    {#if layers.median}<span><span class="swatch median"></span>Median</span>{/if}
    {#if priors.some((p) => p.drawn)}<span><span class="swatch prior"></span>Prior years</span>{/if}
    {#if layers.outer}<span><span class="swatch band"></span>Percentile band</span>{/if}
  </div>
  <p class="readout" aria-live="polite" data-testid="readout">{readout}</p>
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
