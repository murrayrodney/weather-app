<script lang="ts">
  let { label = 'Loading...' }: { label?: string } = $props();
</script>

<p class="state-note" role="status">{label}</p>
