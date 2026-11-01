import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cached, clearAllCaches, clearForecastCache, clearYtdCache } from './cache';

describe('cache', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    clearAllCaches();
  });
  afterEach(() => vi.useRealTimers());

  it('serves the second call from the cache', async () => {
    const loader = vi.fn().mockResolvedValue(1);
    await cached('ytd|a', 1000, loader);
    await cached('ytd|a', 1000, loader);
    expect(loader).toHaveBeenCalledTimes(1);
  });
  it('different key refetches', async () => {
    const loader = vi.fn().mockResolvedValue(1);
    await cached('ytd|2026|04-01', 1000, loader);
    await cached('ytd|2026|01-01', 1000, loader);
    expect(loader).toHaveBeenCalledTimes(2);
  });
  it('refetches after the TTL', async () => {
    const loader = vi.fn().mockResolvedValue(1);
    await cached('k', 1000, loader);
    vi.advanceTimersByTime(1500);
    await cached('k', 1000, loader);
    expect(loader).toHaveBeenCalledTimes(2);
  });
  it('clearing drops entries', async () => {
    const loader = vi.fn().mockResolvedValue(1);
    await cached('ytd|a', 1000, loader);
    await cached('forecast|a', 1000, loader);
    clearYtdCache();
    await cached('ytd|a', 1000, loader);
    await cached('forecast|a', 1000, loader);
    expect(loader).toHaveBeenCalledTimes(3);
    clearForecastCache();
    await cached('forecast|a', 1000, loader);
    expect(loader).toHaveBeenCalledTimes(4);
    clearAllCaches();
    await cached('ytd|a', 1000, loader);
    expect(loader).toHaveBeenCalledTimes(5);
  });
  it('concurrent identical calls share one request', async () => {
    let resolve!: (v: number) => void;
    const loader = vi.fn(() => new Promise<number>((r) => (resolve = r)));
    const a = cached('k', 1000, loader);
    const b = cached('k', 1000, loader);
    resolve(5);
    expect(await a).toBe(5);
    expect(await b).toBe(5);
    expect(loader).toHaveBeenCalledTimes(1);
  });
  it('a loader that resolves after a clear does not write into the store', async () => {
    let resolve!: (v: string) => void;
    const slow = vi.fn(() => new Promise<string>((r) => (resolve = r)));
    const pending = cached('ytd|late', 60000, slow);
    clearAllCaches(); // sign-out while the request is in flight
    resolve('previous user data');
    expect(await pending).toBe('previous user data'); // the caller that asked still gets its answer
    const fresh = vi.fn().mockResolvedValue('next');
    expect(await cached('ytd|late', 60000, fresh)).toBe('next'); // but nothing was cached
    expect(fresh).toHaveBeenCalledTimes(1);
  });
  it('does not cache values the caller refuses (stale forecasts)', async () => {
    const loader = vi.fn<() => Promise<{ stale: boolean }>>().mockResolvedValue({ stale: true });
    await cached('forecast|x', 1000, loader, (v) => !v.stale);
    await cached('forecast|x', 1000, loader, (v) => !v.stale);
    expect(loader).toHaveBeenCalledTimes(2);
  });
});
