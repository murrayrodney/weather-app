<script lang="ts">
  import type { ForecastChanges } from '$lib/api/types';
  import { formatDateShort } from '$lib/transforms/dates';
  import { leadLabel, leadSeries, spreadAcrossLeads, type ChangeMetric } from '$lib/transforms/forecast';
  import ChartFrame from './ChartFrame.svelte';
  import { H, M, W, linePath, makeScales } from './scales';

  let { changes, pastNullProb = false }: { changes: ForecastChanges; pastNullProb?: boolean } = $props();

  let metric = $state<ChangeMetric>('precip_in');
  const METRICS: { id: ChangeMetric; label: string }[] = [
    { id: 'precip_in', label: 'Rain' },
    { id: 'prob_precip_max', label: 'Chance of rain' },
    { id: 'tmax_f', label: 'High' },
    { id: 'tmin_f', label: 'Low' }
  ];

  const series = $derived(leadSeries(changes, metric));
  const yRange = $derived.by(() => {
    let lo = Infinity;
    let hi = -Infinity;
    for (const s of series) {
      for (const p of s) {
        if (!p) continue;
        lo = Math.min(lo, p.y);
        hi = Math.max(hi, p.y);
      }
    }
    if (!Number.isFinite(lo)) return { lo: 0, hi: 1 };
    if (metric === 'precip_in' || metric === 'prob_precip_max') return { lo: 0, hi: Math.max(hi * 1.1, metric === 'precip_in' ? 0.05 : 10) };
    return { lo: Math.floor(lo - 2), hi: hi + 2 };
  });
  const sc = $derived.by(() => {
    const s = makeScales(changes.days.length, yRange.hi);
    s.y.domain([yRange.lo, yRange.hi]).nice();
    return s;
  });
  const xs = $derived((v: number) => sc.x(v));
  const ys = $derived((v: number) => sc.y(v));
  const yTicks = $derived(sc.y.ticks(5));
  const fmt = (v: number | null | undefined) => (v === null || v === undefined ? '-' : metric === 'precip_in' ? v.toFixed(2) : String(Math.round(v)));
  const caption = $derived([
    `Forecast: ${changes.model} as it looked 0 to 7 days before each date (day offsets, not model runs). Days are local midnight to midnight.`,
    changes.attribution,
    ...(pastNullProb ? ['Chance of rain is not available before October 2024.'] : [])
  ]);
</script>

<div class="controls">
  <div class="tabs" role="group" aria-label="Metric">
    {#each METRICS as m (m.id)}
      <button type="button" aria-pressed={metric === m.id} onclick={() => (metric = m.id)}>{m.label}</button>
    {/each}
  </div>
</div>
<ChartFrame title="How the forecast changed" provenance={caption}>
  <svg class="chart-svg" viewBox={`0 0 ${W} ${H}`} role="img" aria-label="Forecast changes by lead time" data-testid="changes-chart">
    {#each yTicks as t (t)}
      <line class="grid-line" x1={M.l} x2={W - M.r} y1={ys(t)} y2={ys(t)} />
      <text x={M.l - 6} y={ys(t) + 4} text-anchor="end">{t}</text>
    {/each}
    {#each changes.days as d, i (d.date)}<text x={xs(i)} y={H - 8} text-anchor="middle">{formatDateShort(d.date)}</text>{/each}
    {#each series as s, li (li)}
      <path class={'line-lead-' + changes.lead_days[li]} d={linePath(s, xs, ys)} />
    {/each}
  </svg>
  <div class="legend">
    {#each changes.lead_days as n (n)}<span>{leadLabel(n)}</span>{/each}
  </div>
  <div class="table-scroll">
    <table class="data" data-testid="changes-table">
      <thead>
        <tr>
          <th>Day</th>
          {#each changes.lead_days as n (n)}<th>{leadLabel(n)}</th>{/each}
          <th>Moved by</th>
        </tr>
      </thead>
      <tbody>
        {#each changes.days as d (d.date)}
          {@const sp = spreadAcrossLeads(d, metric)}
          <tr>
            <td>{formatDateShort(d.date)}</td>
            {#each d[metric] as v, i (i)}<td>{fmt(v)}</td>{/each}
            <td>{sp ? `moved by ${fmt(sp.min)} to ${fmt(sp.max)}` : '-'}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
</ChartFrame>
