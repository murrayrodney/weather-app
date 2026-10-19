<script lang="ts">
  import type { Snippet } from 'svelte';
  import ProvenanceCaption from './ProvenanceCaption.svelte';

  // provenance is required: every chart shows where its numbers come from (F5, F6).
  let { title, provenance, children }: { title?: string; provenance: string[]; children: Snippet } = $props();
</script>

<figure class="chart-frame">
  {#if title}<figcaption><strong>{title}</strong></figcaption>{/if}
  {@render children()}
  <ProvenanceCaption lines={provenance} />
</figure>
