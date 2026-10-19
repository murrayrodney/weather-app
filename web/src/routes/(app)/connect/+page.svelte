<script lang="ts">
  import { beforeNavigate, invalidateAll } from '$app/navigation';
  import { page } from '$app/state';
  import { onDestroy, onMount } from 'svelte';
  import { saveConnect, validateConnect } from '$lib/api/endpoints';
  import { ApiError, userMessage } from '$lib/api/errors';
  import type { ConnectDevice, SaveResponse, Station } from '$lib/api/types';
  import { callAction } from '$lib/auth/redirect';
  import BaselinePicker from '$lib/components/BaselinePicker.svelte';
  import { TIMEZONE_CHOICES } from '$lib/config';

  const reconnect = $derived(page.url.searchParams.get('reconnect') === '1');

  // The AWN key lives only in this variable. Never stored, logged or put in a URL.
  let apiKey = $state('');
  let step = $state<1 | 2 | 3 | 4>(1);
  let busy = $state(false);
  let message = $state('');
  let devices = $state<ConnectDevice[]>([]);
  let chosen = $state<Record<string, boolean>>({});
  let zones = $state<Record<string, string>>({});
  let missingZone = $state<string[]>([]);
  let result = $state<SaveResponse | null>(null);
  let queue = $state<Station[]>([]);

  function clearKey() {
    apiKey = '';
  }
  onMount(() => {
    window.addEventListener('pagehide', clearKey);
    return () => window.removeEventListener('pagehide', clearKey);
  });
  beforeNavigate(clearKey);
  onDestroy(clearKey);

  async function validate(e: SubmitEvent) {
    e.preventDefault();
    if (busy || !apiKey.trim()) return;
    busy = true;
    message = '';
    const r = await callAction(() => validateConnect(apiKey.trim()));
    busy = false;
    if (!r.ok) {
      if (!r.handled) message = errText(r.error);
      return;
    }
    if (!r.value.valid) {
      message = "That key couldn't be verified. Check it and try again.";
      return;
    }
    if (r.value.devices.length === 0) {
      message = 'No devices found on that account.';
      return;
    }
    devices = r.value.devices;
    chosen = Object.fromEntries(devices.map((d) => [d.mac, false]));
    zones = {};
    missingZone = [];
    step = 2;
  }

  function errText(err: unknown): string {
    if (err instanceof ApiError && err.code === 'invalid_key') return "That key couldn't be verified. Check it and try again.";
    return userMessage(err);
  }

  const picked = $derived(devices.filter((d) => chosen[d.mac]));

  async function save() {
    if (busy || picked.length === 0) return;
    const lacking = picked.filter((d) => d.timezone === null && !zones[d.mac]).map((d) => d.mac);
    if (lacking.length) {
      missingZone = lacking;
      return;
    }
    busy = true;
    message = '';
    const body = picked.map((d) => ({ mac: d.mac, name: d.name, ...(d.timezone === null ? { timezone: zones[d.mac] } : {}) }));
    const r = await callAction(() => saveConnect(apiKey.trim(), body));
    busy = false;
    if (r.ok) {
      clearKey();
      result = r.value;
      queue = r.value.stations.filter((s) => s.is_own && s.baseline === null);
      step = 3;
      void invalidateAll();
      return;
    }
    if (r.handled) return;
    const err = r.error;
    if (err instanceof ApiError && err.code === 'timezone_required') {
      missingZone = picked.filter((d) => d.timezone === null).map((d) => d.mac);
      message = '';
    } else if (err instanceof ApiError && err.code === 'invalid_key') {
      clearKey();
      step = 1;
      message = "That key couldn't be verified. Check it and try again.";
    } else {
      message = errText(err);
    }
  }

  function next() {
    queue = queue.slice(1);
    if (queue.length === 0) step = 3;
  }
</script>

<main class="page narrow">
  <h1>{reconnect ? 'Reconnect your station' : 'Connect your weather station'}</h1>

  {#if step === 1}
    <form onsubmit={validate} class="card">
      <p>Paste your Ambient Weather API key. It is checked once and kept only until you finish this page.</p>
      <label class="field">
        <span>API key</span>
        <input
          type="text"
          name="awn-key"
          class="secret-input"
          autocomplete="off"
          autocapitalize="off"
          spellcheck="false"
          data-1p-ignore
          data-lpignore="true"
          bind:value={apiKey}
        />
      </label>
      <button type="submit" disabled={busy}>Check key</button>
      {#if message}<p class="error-text" role="alert">{message}</p>{/if}
    </form>
  {:else if step === 2}
    <div class="card">
      <h2>Choose devices</h2>
      <ul class="plain">
        {#each devices as d (d.mac)}
          <li>
            <label>
              <input type="checkbox" bind:checked={chosen[d.mac]} />
              {d.name} <span class="muted small">{d.mac}</span>
            </label>
            {#if d.timezone === null}
              <select bind:value={zones[d.mac]} aria-label={'Time zone for ' + d.name}>
                <option value="">Choose a time zone</option>
                {#each TIMEZONE_CHOICES as z (z)}<option value={z}>{z}</option>{/each}
              </select>
              {#if missingZone.includes(d.mac)}<span class="error-text small">Choose a time zone.</span>{/if}
            {:else}
              <span class="muted small">{d.timezone}</span>
            {/if}
          </li>
        {/each}
      </ul>
      <button type="button" disabled={busy || picked.length === 0} onclick={save}>Save</button>
      {#if message}<p class="error-text" role="alert">{message}</p>{/if}
    </div>
  {:else if step === 3 && queue.length === 0}
    <div class="card">
      <p class="ok-text" role="status">Connected. Your history will fill in over the next few minutes.</p>
      {#if result && result.not_covered.length > 0}
        <p>These stations are no longer on this account; ask Rodney to disable them:</p>
        <ul>{#each result.not_covered as s (s.id)}<li>{s.name}</li>{/each}</ul>
      {/if}
      <p><a href="/">Go to the dashboard</a></p>
    </div>
  {:else}
    <div class="card"><p class="ok-text" role="status">Connected. Your history will fill in over the next few minutes.</p></div>
    {#key queue[0].id}
      <BaselinePicker stationId={queue[0].id} stationName={queue[0].name} ondone={() => setTimeout(next, 1200)} onskip={next} />
    {/key}
    {#if result && result.not_covered.length > 0}
      <p>These stations are no longer on this account; ask Rodney to disable them:</p>
      <ul>{#each result.not_covered as s (s.id)}<li>{s.name}</li>{/each}</ul>
    {/if}
  {/if}
</main>
