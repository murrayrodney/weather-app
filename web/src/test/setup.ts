import { vi } from 'vitest';

function memoryStorage(): Storage {
  const m = new Map<string, string>();
  return {
    get length() {
      return m.size;
    },
    clear: () => m.clear(),
    getItem: (k) => (m.has(k) ? m.get(k)! : null),
    key: (i) => [...m.keys()][i] ?? null,
    removeItem: (k) => void m.delete(k),
    setItem: (k, v) => void m.set(k, String(v))
  };
}

const loc = new URL('http://localhost:8000/');
const g = globalThis as unknown as Record<string, unknown>;
g.location = { origin: loc.origin, pathname: '/', search: '', hash: '', href: loc.href };
g.localStorage = memoryStorage();
g.sessionStorage = memoryStorage();

vi.mock('$app/navigation', () => ({ goto: vi.fn(), invalidateAll: vi.fn(), beforeNavigate: vi.fn() }));
vi.mock('$env/static/public', () => ({
  PUBLIC_SUPABASE_URL: 'http://localhost:8000',
  PUBLIC_SUPABASE_ANON_KEY: 'dummy-anon-key-for-tests'
}));
