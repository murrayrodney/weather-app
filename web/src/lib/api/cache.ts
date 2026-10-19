// In-memory response cache (load reduction, not rate-limit avoidance). Cleared on sign-out.

interface Entry {
  value: unknown;
  expires: number;
}

const store = new Map<string, Entry>();
const inflight = new Map<string, Promise<unknown>>();
let generation = 0; // bumped by every clear

export const YTD_TTL_MS = 5 * 60 * 1000;
export const FORECAST_TTL_MS = 2 * 60 * 1000;

/**
 * Returns the cached value for `key` or runs `loader`, sharing one in-flight request per key.
 * `shouldCache` lets the caller refuse to cache a value (for example a stale forecast).
 */
export async function cached<T>(key: string, ttlMs: number, loader: () => Promise<T>, shouldCache: (v: T) => boolean = () => true): Promise<T> {
  const hit = store.get(key);
  if (hit && hit.expires > Date.now()) return hit.value as T;
  const running = inflight.get(key);
  if (running) return running as Promise<T>;
  const gen = generation;
  const holder: { p?: Promise<T> } = {};
  holder.p = (async () => {
    try {
      const value = await loader();
      // A clear (sign-out) while the loader was running makes this value stale: never write it back.
      if (gen === generation && shouldCache(value)) store.set(key, { value, expires: Date.now() + ttlMs });
      return value;
    } finally {
      if (inflight.get(key) === holder.p) inflight.delete(key);
    }
  })();
  inflight.set(key, holder.p);
  return holder.p;
}

function clearPrefix(prefixes: string[]) {
  generation++;
  for (const k of [...store.keys()]) if (prefixes.some((p) => k.startsWith(p))) store.delete(k);
}

export function clearYtdCache(): void {
  clearPrefix(['ytd|']);
}
export function clearForecastCache(): void {
  clearPrefix(['forecast|', 'forecast-changes|']);
}
export function clearAllCaches(): void {
  generation++;
  store.clear();
  inflight.clear();
}
