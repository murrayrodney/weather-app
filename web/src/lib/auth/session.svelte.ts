import { isAuthRetryableFetchError } from '@supabase/supabase-js';
import { goto } from '$app/navigation';
import { supabase } from './supabase';
import { ApiError } from '$lib/api/errors';
import { clearAllCaches } from '$lib/api/cache';

export const auth = $state<{ status: 'loading' | 'signed_out' | 'signed_in' }>({ status: 'loading' });

let signingOutFromApi = false;
let initialised = false;

const PUBLIC_PATHS = ['/login', '/set-password', '/account-inactive'];

function onPublicRoute(): boolean {
  const p = globalThis.location?.pathname ?? '/';
  return PUBLIC_PATHS.some((x) => p === x || p.startsWith(x + '/'));
}

export async function initAuth(): Promise<void> {
  if (initialised) return;
  initialised = true;
  supabase.auth.onAuthStateChange((event, session) => {
    if (event === 'SIGNED_OUT') {
      auth.status = 'signed_out';
      clearAllCaches();
      if (!signingOutFromApi && !onPublicRoute()) setTimeout(() => void goto('/login'), 0);
    } else if (session) {
      auth.status = 'signed_in';
    }
  });
  try {
    const { data } = await supabase.auth.getSession();
    auth.status = data.session ? 'signed_in' : 'signed_out';
  } catch {
    auth.status = 'signed_out';
  }
}

function expired(): ApiError {
  return new ApiError(401, 'unauthorized', 'session expired', undefined, 'session_expired');
}

/** Sign out because the session is unrecoverable (marks the sign-out client-initiated so the listener stays quiet). */
export async function signOutFromApi(): Promise<void> {
  signingOutFromApi = true;
  try {
    await supabase.auth.signOut({ scope: 'local' });
  } catch {
    /* local state is cleared by supabase-js regardless */
  } finally {
    signingOutFromApi = false;
    auth.status = 'signed_out';
    clearAllCaches();
  }
}

/** Reads the session on every call; the token is never cached in module state. */
export async function getAccessToken(): Promise<string> {
  const { data, error } = await supabase.auth.getSession();
  if (error) {
    if (isAuthRetryableFetchError(error)) throw new ApiError(0, 'network', 'network');
    await signOutFromApi();
    throw expired();
  }
  if (!data.session) {
    await signOutFromApi();
    throw expired();
  }
  return data.session.access_token;
}

/** One refresh after a 401. A retryable error is a network error; anything else ends the session. */
export async function refreshAccessToken(): Promise<string> {
  const { data, error } = await supabase.auth.refreshSession();
  if (error && isAuthRetryableFetchError(error)) throw new ApiError(0, 'network', 'network');
  if (error || !data.session) {
    await signOutFromApi();
    throw expired();
  }
  return data.session.access_token;
}

/**
 * Nav sign-out; the caller navigates (so the SIGNED_OUT listener is told to stay quiet). Returns the supabase-js error,
 * if any, and whether the local session survived: supabase-js removes the local session even when the server revoke
 * fails (for example a global sign-out that could not reach GoTrue), except when it cannot read the session at all.
 * The caller must tell the user when `error` is set.
 */
export async function signOut(scope: 'local' | 'global' = 'local'): Promise<{ error: unknown | null; kept: boolean }> {
  let error: unknown | null;
  signingOutFromApi = true; // client-initiated: the caller navigates
  try {
    ({ error } = await supabase.auth.signOut({ scope }));
  } catch (e) {
    error = e;
  } finally {
    signingOutFromApi = false;
  }
  let kept = false;
  if (error) {
    try {
      kept = !!(await supabase.auth.getSession()).data.session;
    } catch {
      kept = false;
    }
  }
  if (!kept) {
    auth.status = 'signed_out';
    clearAllCaches();
  }
  return { error, kept };
}
