import { handleAuthError } from '$lib/auth/redirect';

export interface Resource<T> {
  status: 'loading' | 'ready' | 'error';
  data?: T;
  error?: unknown;
  reload(): void;
}

/**
 * Loads data with an AbortSignal. Aborts the previous request on reload and on unmount (the effect cleanup).
 * Must be created inside a component or `$effect.root`.
 */
export function createResource<T>(fetcher: (signal: AbortSignal) => Promise<T>): Resource<T> {
  const state = $state<{ status: 'loading' | 'ready' | 'error'; data?: T; error?: unknown; tick: number }>({ status: 'loading', tick: 0 });
  let controller: AbortController | null = null;

  async function run() {
    controller?.abort();
    const mine = new AbortController();
    controller = mine;
    state.status = 'loading';
    try {
      const data = await fetcher(mine.signal);
      if (mine.signal.aborted) return;
      state.data = data;
      state.error = undefined;
      state.status = 'ready';
    } catch (err) {
      if (mine.signal.aborted) return;
      if (handleAuthError(err)) return;
      state.error = err;
      state.status = 'error';
    }
  }

  $effect(() => {
    void state.tick;
    void run();
    return () => controller?.abort();
  });

  return {
    get status() {
      return state.status;
    },
    get data() {
      return state.data;
    },
    get error() {
      return state.error;
    },
    reload() {
      state.tick++;
    }
  };
}
