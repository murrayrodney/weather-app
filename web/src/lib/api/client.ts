import { ApiError } from './errors';
import { getAccessToken, refreshAccessToken, signOutFromApi } from '$lib/auth/session.svelte';

export interface FetchInit {
  method?: 'GET' | 'POST' | 'PUT';
  body?: unknown;
  signal?: AbortSignal;
}

const TIMEOUT_MS = 30000;

function hasControl(s: string): boolean {
  for (let i = 0; i < s.length; i++) {
    const c = s.charCodeAt(i);
    if (c < 0x20 || c === 0x7f) return true;
  }
  return false;
}

/**
 * Validates the raw path string BEFORE any URL is constructed (the URL parser silently removes dot segments).
 * Returns the validated raw path (no query).
 */
export function validateApiPath(path: string, origin: string): string {
  const bad = () => new ApiError(0, 'bad_path', 'bad path');
  if (typeof path !== 'string' || path.includes('\\') || path.includes('#') || hasControl(path)) throw bad();
  const rawPath = path.split('?', 1)[0];
  if (!rawPath.startsWith('/api/v1/')) throw bad();
  const segments = rawPath.split('/').slice(1);
  for (const raw of segments) {
    if (raw === '') throw bad();
    let seg: string;
    try {
      seg = decodeURIComponent(raw);
    } catch {
      throw bad();
    }
    if (seg === '.' || seg === '..' || seg.includes('/') || seg.includes('\\') || hasControl(seg)) throw bad();
  }
  const u = new URL(path, origin);
  if (u.origin !== origin || u.pathname !== rawPath) throw bad();
  return rawPath;
}

function parseRetryAfter(res: Response): number | undefined {
  const v = res.headers.get('Retry-After');
  if (!v) return undefined;
  const n = parseInt(v, 10);
  return Number.isFinite(n) && n >= 0 ? n : undefined;
}

async function errorFrom(res: Response): Promise<ApiError> {
  let code = 'unknown';
  try {
    const body = (await res.json()) as { error?: { code?: unknown } };
    if (body && typeof body.error?.code === 'string') code = body.error.code;
  } catch {
    /* non-JSON error body */
  }
  // The server message is never copied into the error (it is never shown).
  return new ApiError(res.status, code, `request failed (${res.status})`, parseRetryAfter(res));
}

async function send(path: string, init: FetchInit, token: string): Promise<Response> {
  const signals = [init.signal, AbortSignal.timeout(TIMEOUT_MS)].filter((s): s is AbortSignal => !!s);
  const hasBody = init.body !== undefined;
  try {
    return await fetch(path, {
      method: init.method ?? 'GET',
      headers: {
        Authorization: 'Bearer ' + token,
        Accept: 'application/json',
        ...(hasBody ? { 'Content-Type': 'application/json' } : {})
      },
      body: hasBody ? JSON.stringify(init.body) : undefined,
      signal: AbortSignal.any(signals),
      credentials: 'omit',
      cache: 'no-store',
      referrerPolicy: 'no-referrer'
    });
  } catch (e) {
    if (init.signal?.aborted) throw e; // caller aborted: let the resource ignore it
    throw new ApiError(0, 'network', 'network');
  }
}

export async function apiFetch<T>(path: string, init: FetchInit = {}, origin: string = globalThis.location?.origin ?? ''): Promise<T> {
  validateApiPath(path, origin);
  let token = await getAccessToken();
  let res = await send(path, init, token);

  if (res.status === 401) {
    token = await refreshAccessToken(); // signs out and throws session_expired when it cannot
    res = await send(path, init, token);
    if (res.status === 401) {
      await signOutFromApi();
      throw new ApiError(401, 'unauthorized', 'session expired', undefined, 'session_expired');
    }
  }
  if (res.status === 403) {
    const err = await errorFrom(res);
    if (err.code === 'account_inactive') {
      await signOutFromApi(); // flagged, so the SIGNED_OUT listener does not race handleAuthError to /login
      throw new ApiError(403, 'account_inactive', 'inactive', undefined, 'account_inactive');
    }
    throw err;
  }
  if (!res.ok) throw await errorFrom(res);
  try {
    return (await res.json()) as T;
  } catch {
    throw new ApiError(502, 'bad_response', 'bad response');
  }
}
