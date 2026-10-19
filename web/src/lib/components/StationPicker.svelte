<script lang="ts">
  import type { Station } from '$lib/api/types';
  import { YTD_MAX_STATIONS } from '$lib/config';

  let { stations, selected, onchange }: { stations: Station[]; selected: string[]; onchange: (ids: string[]) => void } = $props();

  function toggle(id: string, on: boolean) {
    const next = on ? [...selected, id] : selected.filter((x) => x !== id);
    onchange(next);
  }
</script>

<fieldset class="picker">
  <legend class="small muted">Stations (up to {YTD_MAX_STATIONS})</legend>
  {#each stations as s (s.id)}
    <label class="small">
      <input
        type="checkbox"
        checked={selected.includes(s.id)}
        disabled={!selected.includes(s.id) && selected.length >= YTD_MAX_STATIONS}
        onchange={(e) => toggle(s.id, e.currentTarget.checked)}
      />
      {s.name}{#if !s.is_own} ({s.owner.display_name}){/if}
    </label>
  {/each}
</fieldset>
