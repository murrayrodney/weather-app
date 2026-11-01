<script lang="ts">
  import { applyTheme, nextTheme, readTheme, themeLabel, writeTheme, type ThemeChoice } from '$lib/theme/theme';

  // The saved choice was already applied before first paint (static/theme-init.js for an explicit choice); this only
  // reads it for the label. Each click moves System -> Light -> Dark -> System.
  let choice = $state<ThemeChoice>(readTheme());

  function cycle() {
    choice = nextTheme(choice);
    writeTheme(choice);
    applyTheme(choice);
  }
</script>

<!-- Icon only: sun = Light, moon = Dark, half-filled circle = System (follow the OS). The name and title carry the text. -->
<button
  type="button"
  class="secondary theme-toggle"
  onclick={cycle}
  aria-label="Theme: {themeLabel(choice)}. Change theme"
  title="Theme: {themeLabel(choice)}"
  data-testid="theme-toggle"
  data-choice={choice}
>
  <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true" focusable="false">
    {#if choice === 'light'}
      <circle cx="12" cy="12" r="4.5" />
      <path d="M12 2v2.5M12 19.5V22M2 12h2.5M19.5 12H22M4.9 4.9l1.8 1.8M17.3 17.3l1.8 1.8M4.9 19.1l1.8-1.8M17.3 6.7l1.8-1.8" />
    {:else if choice === 'dark'}
      <path d="M20 14.5A8.5 8.5 0 0 1 9.5 4a8.5 8.5 0 1 0 10.5 10.5z" />
    {:else}
      <circle cx="12" cy="12" r="8.5" />
      <path class="fill" d="M12 3.5a8.5 8.5 0 0 1 0 17z" />
    {/if}
  </svg>
</button>
