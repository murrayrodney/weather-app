import { AuthApiError, AuthRetryableFetchError } from '@supabase/supabase-js';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const m = vi.hoisted(() => ({
  auth: { getSession: vi.fn(), refreshSession: vi.fn(), signOut: vi.fn(), onAuthStateChange: vi.fn() }
}));
vi.mock('$lib/auth/supabase', () => ({ supabase: { auth: m.auth } }));

import { goto } from '$app/navigation';
import { handleAuthError } from '$lib/auth/redirect';
import { initAuth } from '$lib/auth/session.svelte';
import { apiFetch, validateApiPath } from './client';
import { ApiError, userMessage } from './errors';
import { getYtd, ytdPath } from './endpoints';
import { assertUuid } from './ids';

const O = 'http://localhost:8000';
const A = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';
const B = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb';

function json(body: unknown, status = 200, headers: Record<string, string> = {}) {
  return new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json', ...headers } });
}
const session = (t = 'tok-1') => ({ data: { session: { access_token: t } }, error: null });

const fetchMock = vi.fn();
vi.stubGlobal('fetch', fetchMock);

// initAuth registers its listener once per module; keep the callback so signOut can emit SIGNED_OUT like supabase-js does.
let listener: (e: string, s: unknown) => void = () => {};

beforeEach(() => {
  vi.clearAllMocks();
  m.auth.getSession.mockResolvedValue(session());
  m.auth.onAuthStateChange.mockImplementation((cb) => {
    listener = cb;
    return { data: { subscription: { unsubscribe() {} } } };
  });
  m.auth.signOut.mockImplementation(async () => {
    listener('SIGNED_OUT', null);
    return { error: null };
  });
  vi.mocked(goto).mockClear();
});

describe('assertUuid and path safety', () => {
  it('accepts a UUID and rejects tricks', () => {
    expect(assertUuid(A)).toBe(A);
    for (const bad of ['../me', 'abc', `${A}/../x`, '%2e%2e', 'a%2Fb', 'a\\b']) expect(() => assertUuid(bad)).toThrow(ApiError);
  });

  it.each([
    '/api/v1/stations/%2e%2e/x',
    '/api/v1/stations/./x',
    '/api/v1/stations/a/../b',
    '/api/v1/stations/%2E/x',
    '/api/v1/stations/a%5Cb',
    '/api/v1/stations/a%2Fb',
    '/api/v1/stations//x',
    '/api/v1/stations/%zz',
    'https://evil.test/api/v1/me',
    '//evil.test',
    '/api/v2/me',
    '/api/v1/../me',
    '/api/v1/%2e%2e/me',
    '/api/v1//x'
  ])('rejects %s before any fetch and without constructing a URL', async (p) => {
    const spy = vi.spyOn(globalThis, 'URL');
    await expect(apiFetch(p)).rejects.toMatchObject({ code: 'bad_path' });
    expect(spy).not.toHaveBeenCalled();
    spy.mockRestore();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('accepts a canonical ytd path', () => {
    expect(() => validateApiPath(`/api/v1/ytd?station_ids=${A},${B}&year=2026&start=04-01&end=10-31`, O)).not.toThrow();
  });

  it('getYtd builds the exact URL with an unencoded comma', async () => {
    fetchMock.mockResolvedValue(json({ year: 2026, stations: [] }));
    await getYtd({ stationIds: [A, B], year: 2026, start: '04-01', end: '10-31' });
    expect(fetchMock.mock.calls[0][0]).toBe(`/api/v1/ytd?station_ids=${A},${B}&year=2026&start=04-01&end=10-31`);
    expect(ytdPath({ stationIds: [A], year: 2026, start: '04-01', end: '10-31', gdd: { method: 'clamped_50_86', base_f: 45, start: '01-01' } })).toBe(
      `/api/v1/ytd?station_ids=${A}&year=2026&start=04-01&end=10-31&gdd_method=clamped_50_86&gdd_base=45&gdd_start=01-01`
    );
  });
});

describe('apiFetch', () => {
  it('sends the Bearer access token, omits credentials and never sends the anon key', async () => {
    fetchMock.mockResolvedValue(json({ id: 'x' }));
    await apiFetch('/api/v1/me');
    const [, init] = fetchMock.mock.calls[0];
    expect(init.headers.Authorization).toBe('Bearer tok-1');
    expect(init.credentials).toBe('omit');
    expect(JSON.stringify(init.headers)).not.toContain('dummy-anon-key-for-tests');
    expect(init.headers).not.toHaveProperty('apikey');
  });

  it('401 then refresh then 200 is exactly two fetches', async () => {
    fetchMock.mockResolvedValueOnce(json({ error: { code: 'unauthorized' } }, 401)).mockResolvedValueOnce(json({ ok: 1 }));
    m.auth.refreshSession.mockResolvedValue({ data: { session: { access_token: 'tok-2' } }, error: null });
    await expect(apiFetch('/api/v1/me')).resolves.toEqual({ ok: 1 });
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(fetchMock.mock.calls[1][1].headers.Authorization).toBe('Bearer tok-2');
  });

  it('401 twice signs out locally with session_expired and does not navigate', async () => {
    fetchMock.mockResolvedValue(json({ error: { code: 'unauthorized' } }, 401));
    m.auth.refreshSession.mockResolvedValue({ data: { session: { access_token: 'tok-2' } }, error: null });
    await expect(apiFetch('/api/v1/me')).rejects.toMatchObject({ kind: 'session_expired' });
    expect(m.auth.signOut).toHaveBeenCalledWith({ scope: 'local' });
    expect(goto).not.toHaveBeenCalled();
  });

  it('a non-retryable refresh failure signs out; a retryable one is a network error without sign-out', async () => {
    fetchMock.mockResolvedValue(json({ error: { code: 'unauthorized' } }, 401));
    m.auth.refreshSession.mockResolvedValueOnce({ data: { session: null }, error: new AuthApiError('bad refresh', 400, 'refresh_token_not_found') });
    await expect(apiFetch('/api/v1/me')).rejects.toMatchObject({ kind: 'session_expired' });
    expect(m.auth.signOut).toHaveBeenCalledTimes(1);
    m.auth.signOut.mockClear();
    m.auth.refreshSession.mockResolvedValueOnce({ data: { session: null }, error: new AuthRetryableFetchError('offline', 0) });
    await expect(apiFetch('/api/v1/me')).rejects.toMatchObject({ code: 'network' });
    expect(m.auth.signOut).not.toHaveBeenCalled();
  });

  it('getSession: retryable error is network with no sign-out; no session signs out', async () => {
    m.auth.getSession.mockResolvedValueOnce({ data: { session: null }, error: new AuthRetryableFetchError('offline', 0) });
    await expect(apiFetch('/api/v1/me')).rejects.toMatchObject({ code: 'network' });
    expect(m.auth.signOut).not.toHaveBeenCalled();
    m.auth.getSession.mockResolvedValueOnce({ data: { session: null }, error: null });
    await expect(apiFetch('/api/v1/me')).rejects.toMatchObject({ kind: 'session_expired' });
    expect(m.auth.signOut).toHaveBeenCalledTimes(1);
  });

  it('403 account_inactive signs out; forbidden and 404 do not', async () => {
    fetchMock.mockResolvedValueOnce(json({ error: { code: 'account_inactive' } }, 403));
    await expect(apiFetch('/api/v1/me')).rejects.toMatchObject({ kind: 'account_inactive' });
    expect(m.auth.signOut).toHaveBeenCalledTimes(1);
    m.auth.signOut.mockClear();
    fetchMock.mockResolvedValueOnce(json({ error: { code: 'forbidden' } }, 403));
    await expect(apiFetch('/api/v1/me')).rejects.toMatchObject({ status: 403, code: 'forbidden' });
    fetchMock.mockResolvedValueOnce(json({ error: { code: 'not_found' } }, 404));
    await expect(apiFetch('/api/v1/me')).rejects.toMatchObject({ status: 404 });
    expect(m.auth.signOut).not.toHaveBeenCalled();
  });

  it('503, 408 and 413 never sign out; 429 exposes retryAfterSec; method_not_allowed is a code', async () => {
    for (const [status, code] of [[503, 'auth_unavailable'], [503, 'overloaded'], [408, 'request_timeout'], [413, 'payload_too_large'], [405, 'method_not_allowed']] as const) {
      fetchMock.mockResolvedValueOnce(json({ error: { code } }, status));
      await expect(apiFetch('/api/v1/me')).rejects.toMatchObject({ status, code });
    }
    expect(m.auth.signOut).not.toHaveBeenCalled();
    fetchMock.mockResolvedValueOnce(json({ error: { code: 'rate_limited' } }, 429, { 'Retry-After': '7' }));
    await expect(apiFetch('/api/v1/me')).rejects.toMatchObject({ retryAfterSec: 7 });
    expect(userMessage(new ApiError(429, 'rate_limited', 'x', 7))).toBe('Too many requests. Try again in 7 seconds.');
  });

  it('network error, non-JSON error body, and server messages that must never surface', async () => {
    fetchMock.mockRejectedValueOnce(new TypeError('failed'));
    await expect(apiFetch('/api/v1/me')).rejects.toMatchObject({ code: 'network' });
    fetchMock.mockResolvedValueOnce(new Response('<html>', { status: 500 }));
    await expect(apiFetch('/api/v1/me')).rejects.toMatchObject({ code: 'unknown' });
    fetchMock.mockResolvedValueOnce(json({ error: { code: 'internal', message: 'https://rt.ambientweather.net/x?apiKey=SECRET' } }, 500));
    const err = (await apiFetch('/api/v1/me').catch((e) => e)) as ApiError;
    expect(err.message).not.toContain('SECRET');
    expect(userMessage(err)).not.toContain('SECRET');
  });

  it('request bodies with api_key never reach errors or the console', async () => {
    const spies = (['log', 'info', 'warn', 'error'] as const).map((k) => vi.spyOn(console, k));
    fetchMock.mockResolvedValueOnce(json({ error: { code: 'invalid_key' } }, 422));
    const err = (await apiFetch('/api/v1/connect/validate', { method: 'POST', body: { api_key: 'KEY-SECRET-123' } }).catch((e) => e)) as ApiError;
    expect(JSON.stringify([err.message, err.code])).not.toContain('KEY-SECRET-123');
    for (const s of spies) expect(JSON.stringify(s.mock.calls)).not.toContain('KEY-SECRET-123');
  });
});

describe('handleAuthError and the SIGNED_OUT listener', () => {
  it('navigates once per kind and returns false for other errors', () => {
    expect(handleAuthError(new ApiError(401, 'unauthorized', 'x', undefined, 'session_expired'))).toBe(true);
    expect(goto).toHaveBeenLastCalledWith('/login?reason=expired', { replaceState: true });
    expect(handleAuthError(new ApiError(403, 'account_inactive', 'x', undefined, 'account_inactive'))).toBe(true);
    expect(goto).toHaveBeenLastCalledWith('/account-inactive', { replaceState: true });
    expect(goto).toHaveBeenCalledTimes(2);
    expect(handleAuthError(new ApiError(500, 'internal', 'x'))).toBe(false);
    expect(handleAuthError(new Error('x'))).toBe(false);
    expect(goto).toHaveBeenCalledTimes(2);
  });

  it('API-initiated sign-out does not navigate from the listener; an unflagged one on an (app) route does once', async () => {
    vi.useFakeTimers();
    await initAuth();
    fetchMock.mockResolvedValue(json({ error: { code: 'unauthorized' } }, 401));
    m.auth.refreshSession.mockResolvedValue({ data: { session: null }, error: new AuthApiError('x', 400, 'refresh_token_not_found') });
    await expect(apiFetch('/api/v1/me')).rejects.toMatchObject({ kind: 'session_expired' });
    await vi.runAllTimersAsync();
    expect(goto).not.toHaveBeenCalled();
    listener('SIGNED_OUT', null);
    await vi.runAllTimersAsync();
    expect(goto).toHaveBeenCalledTimes(1);
    expect(goto).toHaveBeenCalledWith('/login');
    vi.useRealTimers();
  });

  it('403 account_inactive is an API-initiated sign-out: the listener does not navigate to /login', async () => {
    vi.useFakeTimers();
    await initAuth();
    fetchMock.mockResolvedValueOnce(json({ error: { code: 'account_inactive' } }, 403));
    await expect(apiFetch('/api/v1/me')).rejects.toMatchObject({ kind: 'account_inactive' });
    await vi.runAllTimersAsync();
    expect(goto).not.toHaveBeenCalledWith('/login');
    vi.useRealTimers();
  });
});
