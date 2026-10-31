<script lang="ts">
  import type { IconKind } from '$lib/transforms/forecast';

  // A 16 x 16 glyph drawn with SVG shapes; colours come from classes in app.css (no inline styles, CSP-safe).
  // Rendered inside an outer <g> placed by the day strip.
  let { kind }: { kind: IconKind } = $props();

  const CLOUD = 'M4.5 13.5h7.2a3 3 0 0 0 0-6 3.6 3.6 0 0 0-6.8-1A2.6 2.6 0 0 0 4.5 13.5z';
</script>

{#if kind === 'clear'}
  <circle class="icon-sun" cx="8" cy="8" r="3.5" />
  <path class="icon-sun-ray" d="M8 1v2M8 13v2M1 8h2M13 8h2M3.1 3.1l1.4 1.4M11.5 11.5l1.4 1.4M3.1 12.9l1.4-1.4M11.5 4.5l1.4-1.4" />
{:else if kind === 'partly'}
  <circle class="icon-sun" cx="5.5" cy="5.5" r="3" />
  <path class="icon-sun-ray" d="M5.5 0.8v1.2M0.8 5.5h1.2M2.1 2.1l0.8 0.8M8.9 2.1l-0.8 0.8" />
  <path class="icon-cloud" d={CLOUD} />
{:else if kind === 'cloudy'}
  <path class="icon-cloud" d={CLOUD} />
{:else if kind === 'fog'}
  <path class="icon-fog" d="M2 5.5h12M3.5 8.5h9M2 11.5h12M4 14.5h8" />
{:else if kind === 'drizzle'}
  <path class="icon-cloud" d={CLOUD} />
  <circle class="icon-rain-dot" cx="5" cy="14.8" r="0.9" />
  <circle class="icon-rain-dot" cx="8.5" cy="14.8" r="0.9" />
  <circle class="icon-rain-dot" cx="12" cy="14.8" r="0.9" />
{:else if kind === 'rain'}
  <path class="icon-cloud" d={CLOUD} />
  <path class="icon-rain" d="M5 12.5l-0.8 2.5M8.5 12.5l-0.8 2.5M12 12.5l-0.8 2.5" />
{:else if kind === 'snow'}
  <path class="icon-cloud" d={CLOUD} />
  <path class="icon-snow" d="M5 13v3M3.6 14.5h2.8M12 13v3M10.6 14.5h2.8" />
{:else if kind === 'storm'}
  <path class="icon-cloud" d={CLOUD} />
  <path class="icon-bolt" d="M8.8 10.2L6.6 13.6h2.2l-1 2.4 2.6-3.6H8.2z" />
{:else}
  <circle class="icon-unknown" cx="8" cy="8" r="5" />
{/if}
