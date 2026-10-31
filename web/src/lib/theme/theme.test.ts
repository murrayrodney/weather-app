import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { applyTheme, nextTheme, readTheme, THEME_KEY, themeLabel, writeTheme, type ThemeChoice, type ThemeStore } from './theme';

/** An in-memory store that can be made to refuse access, the way a blocked or full localStorage does. */
function store(opts: { refuse?: 'get' | 'set' | 'remove' | 'all' } = {}): ThemeStore & { data: Map<string, string> } {
  const data = new Map<string, string>();
  const boom = () => {
    throw new DOMException('blocked', 'SecurityError');
  };
  return {
    data,
    getItem: (k) => (opts.refuse === 'get' || opts.refuse === 'all' ? boom() : (data.get(k) ?? null)),
    setItem: (k, v) => (opts.refuse === 'set' || opts.refuse === 'all' ? boom() : void data.set(k, String(v))),
    removeItem: (k) => (opts.refuse === 'remove' || opts.refuse === 'all' ? boom() : void data.delete(k))
  };
}

const root = () => ({ dataset: {} as Record<string, string | undefined> });

describe('cycle order', () => {
  it('goes System, Light, Dark, then back to System', () => {
    expect(nextTheme('system')).toBe('light');
    expect(nextTheme('light')).toBe('dark');
    expect(nextTheme('dark')).toBe('system');
  });

  it('starts the cycle at System for an unknown value', () => {
    expect(nextTheme('purple' as ThemeChoice)).toBe('system');
  });

  it('three presses return to the start', () => {
    let c: ThemeChoice = 'light';
    for (let i = 0; i < 3; i++) c = nextTheme(c);
    expect(c).toBe('light');
  });

  it('labels each choice', () => {
    expect([themeLabel('system'), themeLabel('light'), themeLabel('dark')]).toEqual(['System', 'Light', 'Dark']);
  });
});

describe('readTheme', () => {
  it('returns a saved light or dark choice', () => {
    for (const v of ['light', 'dark'] as const) {
      const s = store();
      s.data.set(THEME_KEY, v);
      expect(readTheme(s)).toBe(v);
    }
  });

  it('is System with no entry, an unknown value, or no store', () => {
    expect(readTheme(store())).toBe('system');
    const s = store();
    s.data.set(THEME_KEY, 'purple');
    expect(readTheme(s)).toBe('system');
    expect(readTheme(null)).toBe('system');
  });

  it('is System when reading storage throws (blocked site data)', () => {
    expect(readTheme(store({ refuse: 'get' }))).toBe('system');
  });
});

describe('writeTheme', () => {
  it('saves light and dark under the theme key', () => {
    const s = store();
    expect(writeTheme('light', s)).toBe(true);
    expect(s.data.get(THEME_KEY)).toBe('light');
    expect(writeTheme('dark', s)).toBe(true);
    expect(s.data.get(THEME_KEY)).toBe('dark');
  });

  it('System removes the entry', () => {
    const s = store();
    s.data.set(THEME_KEY, 'dark');
    expect(writeTheme('system', s)).toBe(true);
    expect(s.data.has(THEME_KEY)).toBe(false);
  });

  it('returns false and does not throw when storage refuses (the choice still applies for the visit)', () => {
    expect(writeTheme('dark', store({ refuse: 'set' }))).toBe(false);
    expect(writeTheme('system', store({ refuse: 'remove' }))).toBe(false);
    expect(writeTheme('dark', null)).toBe(false);
  });

  it('round-trips through read', () => {
    const s = store();
    let c: ThemeChoice = 'system';
    for (let i = 0; i < 4; i++) {
      c = nextTheme(c);
      writeTheme(c, s);
      expect(readTheme(s)).toBe(c);
    }
  });
});

describe('applyTheme', () => {
  it('sets data-theme for an explicit choice', () => {
    const r = root();
    applyTheme('dark', r);
    expect(r.dataset.theme).toBe('dark');
    applyTheme('light', r);
    expect(r.dataset.theme).toBe('light');
  });

  it('removes data-theme for System so app.css follows the OS', () => {
    const r = root();
    applyTheme('dark', r);
    applyTheme('system', r);
    expect(r.dataset.theme).toBeUndefined();
    expect('theme' in r.dataset).toBe(false);
  });

  it('does nothing without a root (no document in this environment)', () => {
    expect(() => applyTheme('dark', null)).not.toThrow();
  });
});

describe('static/theme-init.js (runs before first paint)', () => {
  // The real file, run against fake browser globals. It must use the same key and values as theme.ts.
  const src = readFileSync(join(process.cwd(), 'static', 'theme-init.js'), 'utf8');
  const run = (localStorage: unknown) => {
    const doc = root();
    const win = {
      get localStorage() {
        if (localStorage === 'throw') throw new DOMException('blocked', 'SecurityError');
        return localStorage;
      }
    };
    new Function('window', 'document', src)(win, { documentElement: doc });
    return doc.dataset.theme;
  };

  it('applies a saved light or dark choice', () => {
    expect(run({ getItem: (k: string) => (k === THEME_KEY ? 'dark' : null) })).toBe('dark');
    expect(run({ getItem: (k: string) => (k === THEME_KEY ? 'light' : null) })).toBe('light');
  });

  it('sets nothing for System, an unknown value, or no storage', () => {
    expect(run({ getItem: () => null })).toBeUndefined();
    expect(run({ getItem: () => 'purple' })).toBeUndefined();
  });

  it('does not throw when storage is blocked', () => {
    expect(run('throw')).toBeUndefined();
    expect(run({ getItem: () => { throw new DOMException('blocked', 'SecurityError'); } })).toBeUndefined();
  });

  it('is loaded by app.html without async or defer (it must run before paint)', () => {
    const html = readFileSync(join(process.cwd(), 'src', 'app.html'), 'utf8');
    expect(html).toMatch(/<script src="%sveltekit\.assets%\/theme-init\.js"><\/script>/);
    expect(html.indexOf('theme-init.js')).toBeLessThan(html.indexOf('%sveltekit.head%'));
  });
});
