<script lang="ts">
  import type { GddSettings } from '$lib/api/types';
  import { GDD_BASE_RANGE } from '$lib/config';
  import { labels } from '$lib/labels';
  import { mdLabel } from '$lib/labels';
  import { startAllowed, validBase } from '$lib/transforms/gddSettings';

  let { value, onchange, windowEnd = '12-31' }: { value: GddSettings; onchange: (v: GddSettings) => void; windowEnd?: string } = $props();
  let startProblem = $state('');

  const months = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December'];
  const daysIn = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

  // svelte-ignore state_referenced_locally
  let baseText = $state(String(value.base_f));
  const month = $derived(Number(value.start.slice(0, 2)));
  const day = $derived(Number(value.start.slice(3)));
  const p2 = (n: number) => String(n).padStart(2, '0');

  function setStart(m: number, d: number) {
    const dd = Math.min(d, daysIn[m - 1]);
    const md = `${p2(m)}-${p2(dd)}`;
    if (!startAllowed(md, windowEnd)) {
      // Invalid input is never sent: the API rejects a start after the window end.
      startProblem = `The start date must be on or before ${mdLabel(windowEnd)}.`;
      return;
    }
    startProblem = '';
    onchange({ ...value, start: md });
  }
  function onBase(text: string) {
    baseText = text;
    const n = Number(text);
    if (/^\d+$/.test(text) && validBase(n)) onchange({ ...value, base_f: n });
  }
</script>

<div class="controls gdd-settings" aria-label="GDD settings">
  <label class="small">
    Method
    <select value={value.method} onchange={(e) => onchange({ ...value, method: e.currentTarget.value as GddSettings['method'] })}>
      <option value="simple_avg">{labels.gddMethod('simple_avg')}</option>
      <option value="clamped_50_86">{labels.gddMethod('clamped_50_86')}</option>
    </select>
  </label>
  <label class="small">
    Base (F)
    <input
      type="number"
      min={GDD_BASE_RANGE.min}
      max={GDD_BASE_RANGE.max}
      step="1"
      value={baseText}
      oninput={(e) => onBase(e.currentTarget.value)}
    />
  </label>
  <label class="small">
    Start month
    <select value={String(month)} onchange={(e) => setStart(Number(e.currentTarget.value), day)}>
      {#each months as m, i (m)}<option value={String(i + 1)}>{m}</option>{/each}
    </select>
  </label>
  <label class="small">
    Day
    <input type="number" min="1" max={daysIn[month - 1]} step="1" value={day} onchange={(e) => setStart(month, Number(e.currentTarget.value) || 1)} />
  </label>
  {#if startProblem}<span class="error-text small" role="alert" data-testid="gdd-start-problem">{startProblem}</span>{/if}
</div>
