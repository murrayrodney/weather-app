<script lang="ts">
  import WeatherIcon from './WeatherIcon.svelte';
  import { STRIP_HEIGHT, type StripCell } from './stackLayout';

  // One cell per local day, aligned to the stack's time axis (same width and gutters).
  const HEIGHT = STRIP_HEIGHT;
  let { width, cells }: { width: number; cells: StripCell[] } = $props();

  const txt = (v: number | null, digits: number) => (v === null ? '--' : v.toFixed(digits));
</script>

<svg class="chart-svg plot strip" {width} height={HEIGHT} role="img" aria-label="Daily summary" data-testid="day-strip">
  {#each cells as c (c.key)}
    <rect class="day-cell" x={c.x0 + 1} y={1} width={Math.max(c.x1 - c.x0 - 2, 0)} height={HEIGHT - 2} rx="4" />
    <text class="day-date" x={c.cx} y={16} text-anchor="middle">{c.date}</text>
    <g transform={`translate(${c.cx - 8} 22)`}>
      {#if c.icon}<WeatherIcon kind={c.icon} />{/if}
    </g>
    <text class="day-text" x={c.cx} y={53} text-anchor="middle">{c.label ?? ''}</text>
    <text class="day-hilo" x={c.cx} y={68} text-anchor="middle">{txt(c.hi, 0)} / {txt(c.lo, 0)} F</text>
    <text class="day-precip" x={c.cx} y={81} text-anchor="middle">{txt(c.precip, 2)} in</text>
    {#if c.snow !== null && c.snow > 0}
      <text class="day-snow" x={c.cx} y={94} text-anchor="middle">snow {txt(c.snow, 2)} in</text>
    {/if}
  {/each}
</svg>
