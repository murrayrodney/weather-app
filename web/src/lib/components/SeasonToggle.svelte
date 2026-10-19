<script lang="ts">
  let { value, onchange }: { value: 'season' | 'full'; onchange: (v: 'season' | 'full') => void } = $props();
</script>

<div class="tabs" role="group" aria-label="Date range">
  <button type="button" aria-pressed={value === 'season'} onclick={() => onchange('season')}>Apr-Oct</button>
  <button type="button" aria-pressed={value === 'full'} onclick={() => onchange('full')}>Full year</button>
</div>
