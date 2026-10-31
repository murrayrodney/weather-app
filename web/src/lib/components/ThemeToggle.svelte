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

<button
  type="button"
  class="secondary theme-toggle"
  onclick={cycle}
  aria-label="Theme: {themeLabel(choice)}. Change theme"
  data-testid="theme-toggle"
  data-choice={choice}
>
  Theme: {themeLabel(choice)}
</button>
