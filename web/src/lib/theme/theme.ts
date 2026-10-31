// Colour theme: System (follow the OS), Light or Dark. An explicit choice is kept in localStorage under THEME_KEY and
// applied as <html data-theme>. With no attribute, app.css follows prefers-color-scheme. Every storage access is
// wrapped: a blocked or full storage only means the choice lasts for this page visit.
// static/theme-init.js applies a saved choice before first paint. It reads the same key and values; keep them in step.

export type ThemeChoice = 'system' | 'light' | 'dark';

export const THEME_KEY = 'theme';

const ORDER: readonly ThemeChoice[] = ['system', 'light', 'dark'];
const LABEL: Record<ThemeChoice, string> = { system: 'System', light: 'Light', dark: 'Dark' };

/** The next choice in the cycle System -> Light -> Dark -> System. An unknown value starts the cycle at System. */
export function nextTheme(choice: ThemeChoice): ThemeChoice {
  return ORDER[(ORDER.indexOf(choice) + 1) % ORDER.length];
}

export function themeLabel(choice: ThemeChoice): string {
  return LABEL[choice];
}

export type ThemeStore = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>;

/** The browser's localStorage, or null when access is refused (reading the property itself can throw). */
export function defaultStore(): ThemeStore | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

/** The saved explicit choice. Anything other than "light" or "dark" (including no entry and a refused store) is System. */
export function readTheme(store: ThemeStore | null = defaultStore()): ThemeChoice {
  try {
    const saved = store?.getItem(THEME_KEY);
    return saved === 'light' || saved === 'dark' ? saved : 'system';
  } catch {
    return 'system';
  }
}

/** Saves an explicit choice; System removes the entry. Returns false when storage refused (the choice still applies). */
export function writeTheme(choice: ThemeChoice, store: ThemeStore | null = defaultStore()): boolean {
  try {
    if (!store) return false;
    if (choice === 'system') store.removeItem(THEME_KEY);
    else store.setItem(THEME_KEY, choice);
    return true;
  } catch {
    return false;
  }
}

export type ThemeRoot = { dataset: Record<string, string | undefined> };

/** Sets <html data-theme> for light or dark; System removes the attribute so app.css follows the OS. */
export function applyTheme(choice: ThemeChoice, root: ThemeRoot | null = globalThis.document?.documentElement ?? null): void {
  if (!root) return;
  if (choice === 'system') delete root.dataset.theme;
  else root.dataset.theme = choice;
}
