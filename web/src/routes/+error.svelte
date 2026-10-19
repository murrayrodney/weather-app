<script lang="ts">
  import { page } from '$app/state';
</script>

<main class="page narrow">
  <h1>Something went wrong</h1>
  <p class="error-text" role="alert">{page.status === 404 ? "That isn't available." : page.error?.message}</p>
  <p><a href="/">Back to the dashboard</a></p>
</main>
