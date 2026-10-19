<script lang="ts">
  let { lines }: { lines: string[] } = $props();
</script>

<ul class="provenance" data-testid="provenance">
  {#each lines as line (line)}
    <li>{line}</li>
  {/each}
</ul>
