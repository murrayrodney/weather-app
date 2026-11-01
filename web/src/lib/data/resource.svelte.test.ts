import { flushSync } from 'svelte';
import { describe, expect, it, vi } from 'vitest';

vi.mock('$lib/auth/redirect', () => ({ handleAuthError: vi.fn((e: unknown) => (e as { kind?: string })?.kind !== undefined) }));

import { handleAuthError } from '$lib/auth/redirect';
import { ApiError } from '$lib/api/errors';
import { createResource } from './resource.svelte';

const tick = () => new Promise((r) => setTimeout(r, 0));

describe('createResource', () => {
  it('goes loading -> ready', async () => {
    const cleanup = $effect.root(() => {
      const r = createResource(async () => 42);
      expect(r.status).toBe('loading');
      flushSync();
      void tick().then(() => {
        expect(r.status).toBe('ready');
        expect(r.data).toBe(42);
      });
    });
    await tick();
    await tick();
    cleanup();
  });

  it('error state, and handleAuthError is called once for kind errors but not for others', async () => {
    let res!: ReturnType<typeof createResource<number>>;
    const calls: unknown[] = [new Error('boom'), new ApiError(401, 'unauthorized', 'x', undefined, 'session_expired')];
    const cleanup = $effect.root(() => {
      res = createResource<number>(async () => {
        throw calls.shift();
      });
      flushSync();
    });
    await tick();
    expect(res.status).toBe('error');
    expect(handleAuthError).toHaveBeenCalledTimes(1);
    res.reload();
    flushSync();
    await tick();
    expect(handleAuthError).toHaveBeenCalledTimes(2);
    expect(res.status).not.toBe('error'); // auth errors navigate instead of showing an error panel
    cleanup();
  });

  it('aborts the previous request on reload and on unmount', async () => {
    const signals: AbortSignal[] = [];
    let res!: ReturnType<typeof createResource<number>>;
    const cleanup = $effect.root(() => {
      res = createResource<number>((signal) => {
        signals.push(signal);
        return new Promise<number>(() => {});
      });
      flushSync();
    });
    expect(signals).toHaveLength(1);
    res.reload();
    flushSync();
    expect(signals).toHaveLength(2);
    expect(signals[0].aborted).toBe(true);
    expect(signals[1].aborted).toBe(false);
    cleanup();
    expect(signals[1].aborted).toBe(true);
  });
});
