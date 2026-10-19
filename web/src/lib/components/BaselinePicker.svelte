<script lang="ts">
  import { ApiError, userMessage } from '$lib/api/errors';
  import { getBaselineCandidates, putBaseline } from '$lib/api/endpoints';
  import { callAction } from '$lib/auth/redirect';
  import { createResource } from '$lib/data/resource.svelte';
  import type { BaselineCandidate } from '$lib/api/types';
  import LoadingBlock from './LoadingBlock.svelte';

  let {
    stationId,
    stationName = '',
    ondone,
    onskip
  }: { stationId: string; stationName?: string; ondone?: () => void; onskip?: () => void } = $props();

  const cands = createResource((signal) => getBaselineCandidates(stationId, signal));
  let chosen = $state<string | null>(null);
  let busy = $state(false);
  let message = $state('');
  let saved = $state(false);

  const selectedId = $derived(chosen ?? cands.data?.suggested ?? cands.data?.candidates[0]?.reference_station_id ?? null);
  const unavailable = $derived(cands.status === 'error' && cands.error instanceof ApiError && [502, 503].includes(cands.error.status));

  function elevText(c: BaselineCandidate): string {
    if (c.elevation_ft === null) return '';
    let t = `${Math.round(c.elevation_ft)} ft`;
    if (c.elevation_diff_ft !== null) {
      const d = Math.round(Math.abs(c.elevation_diff_ft));
      t += c.elevation_diff_ft >= 0 ? `, ${d} ft higher than your station` : `, ${d} ft lower than your station`;
    }
    return t;
  }

  async function use() {
    if (!selectedId || busy) return;
    busy = true;
    message = '';
    const r = await callAction(() => putBaseline(stationId, selectedId));
    busy = false;
    if (r.ok) {
      saved = true;
      message = 'Baseline set. Its history should appear within the hour.';
      ondone?.();
      return;
    }
    if (r.handled) return;
    if (r.error instanceof ApiError && r.error.code === 'not_a_candidate') {
      message = userMessage(r.error);
      cands.reload();
      return;
    }
    message = userMessage(r.error);
  }
</script>

<div class="card baseline-picker">
  <h3>Choose a baseline{#if stationName} for {stationName}{/if}</h3>
  {#if cands.status === 'loading'}
    <LoadingBlock label="Looking up nearby airport stations..." />
  {:else if unavailable}
    <p class="muted">
      We couldn't look up nearby airport stations right now. Your station is saved; you can choose a baseline later from its page.
    </p>
    <button type="button" class="secondary" onclick={() => onskip?.()}>Skip for now</button>
  {:else if cands.status === 'error'}
    <p class="error-text">{userMessage(cands.error)}</p>
    <button type="button" class="secondary" onclick={() => cands.reload()}>Retry</button>
  {:else if cands.data && cands.data.candidates.length === 0}
    <p>
      No airport weather station with a long enough record was found within {cands.data.radius_mi} miles. You can choose one later.
    </p>
    <button type="button" class="secondary" onclick={() => onskip?.()}>Skip for now</button>
  {:else if cands.data}
    {#if saved}
      <p class="ok-text" role="status">{message}</p>
    {:else}
      <ul class="plain">
        {#each cands.data.candidates as c (c.reference_station_id)}
          <li>
            <label>
              <input type="radio" name={'baseline-' + stationId} value={c.reference_station_id} checked={selectedId === c.reference_station_id} onchange={() => (chosen = c.reference_station_id)} />
              <strong>{c.name}</strong>{#if c.airport_code} ({c.airport_code}){/if}
              {#if c.reference_station_id === cands.data.suggested} (suggested, nearest){/if}
              <br />
              <span class="small muted">
                {c.distance_mi} mi away
                {#if elevText(c)}; {elevText(c)}{/if}
                ; {c.record_years} years of records{#if c.short_record} (short record){/if}
              </span>
              {#if c.elevation_diff_ft !== null && Math.abs(c.elevation_diff_ft) > 1000}
                <br /><span class="small">Different elevation may change rainfall and temperature comparisons</span>
              {/if}
            </label>
          </li>
        {/each}
      </ul>
      <div class="inline-form">
        <button type="button" disabled={busy || !selectedId} onclick={use}>Use this baseline</button>
        <button type="button" class="secondary" onclick={() => onskip?.()}>Skip for now</button>
      </div>
      {#if message}<p class="error-text" role="alert">{message}</p>{/if}
    {/if}
  {/if}
</div>
