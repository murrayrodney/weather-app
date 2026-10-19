import { isAuthRetryableFetchError } from '@supabase/supabase-js';
import { supabase } from './supabase';
import { clearAllCaches } from '$lib/api/cache';

export const MSG_LINK_EXPIRED = 'That link has expired or was already used. Ask Rodney for a new one.';
export const MSG_WEAK = 'Choose a longer password (at least 12 characters).';
export const MSG_SAME = 'Choose a password you have not used here before.';
export const MSG_NETWORK = 'Could not reach the sign-in service. Try again.';
export const MSG_GENERIC = 'Could not set the password. Try again.';

const TOKEN_RE = /^[A-Za-z0-9._-]{16,256}$/;

/** Parses `#token_hash=...&type=recovery`. Returns the token or null. */
export function parseSetPasswordFragment(hash: string): string | null {
  const raw = hash.startsWith('#') ? hash.slice(1) : hash;
  if (!raw) return null;
  const params = new URLSearchParams(raw);
  if (params.get('type') !== 'recovery') return null;
  const token = params.get('token_hash');
  return token && TOKEN_RE.test(token) ? token : null;
}

/**
 * Reads the token from the URL fragment and strips the fragment from the address bar and history, synchronously
 * (the set-password page calls this before its first await). SvelteKit's in-memory `page.url` keeps the hash until the
 * next navigation: it cannot be rewritten, nothing reads it, and it has the same exposure as the token variable itself
 * (same-origin script memory), so that copy is accepted.
 */
export function takeTokenFromLocation(
  loc: Pick<Location, 'hash' | 'pathname'> = globalThis.location,
  hist: Pick<History, 'replaceState'> = globalThis.history
): string | null {
  const token = parseSetPasswordFragment(loc.hash);
  hist.replaceState(null, '', loc.pathname);
  return token;
}

/**
 * A different user's session would be replaced by verifyOtp, so clear it (and its caches) first. Asks Supabase directly:
 * the `auth` state is still 'loading' when this page mounts (the root layout's onMount runs after the child's).
 */
export async function clearPreviousSession(): Promise<boolean> {
  try {
    const { data } = await supabase.auth.getSession();
    if (!data.session) return false;
    await supabase.auth.signOut({ scope: 'local' });
    clearAllCaches();
    return true;
  } catch {
    return false;
  }
}

/** UX-only checks; the same minimum is enforced by GoTrue. Returns an error message or null. */
export function validateNewPassword(password: string, confirm: string, email?: string | null): string | null {
  const bytes = new TextEncoder().encode(password).length;
  if ([...password].length < 12) return MSG_WEAK;
  if (bytes > 72) return 'Choose a password of at most 72 bytes.';
  if (password !== confirm) return 'The two passwords do not match.';
  if (email && password.toLowerCase() === email.toLowerCase()) return 'The password cannot be your email address.';
  return null;
}

interface AuthErr {
  code?: string;
  status?: number;
  message?: string;
}

function mapVerifyError(err: AuthErr): string {
  return isAuthRetryableFetchError(err) ? MSG_NETWORK : MSG_LINK_EXPIRED;
}

function mapUpdateError(err: AuthErr): string {
  if (isAuthRetryableFetchError(err)) return MSG_NETWORK;
  if (err.code === 'weak_password') return MSG_WEAK;
  if (err.code === 'same_password') return MSG_SAME;
  if (err.status === 422 && /different from the old/i.test(err.message ?? '')) return MSG_SAME;
  return MSG_GENERIC;
}

export type FlowResult = { ok: true } | { ok: false; message: string };

/** The set-password page flow. The token lives only in this closure; it is redeemed on submit, once per visit. */
export function createSetPasswordFlow(tokenHash: string) {
  let redeemed = false;
  return {
    async submit(password: string): Promise<FlowResult> {
      try {
        if (!redeemed) {
          const { error } = await supabase.auth.verifyOtp({ token_hash: tokenHash, type: 'recovery' });
          if (error) return { ok: false, message: mapVerifyError(error) };
          redeemed = true;
        }
        const { error } = await supabase.auth.updateUser({ password });
        if (error) return { ok: false, message: mapUpdateError(error) };
        clearAllCaches();
        return { ok: true };
      } catch (err) {
        return { ok: false, message: isAuthRetryableFetchError(err) ? MSG_NETWORK : MSG_GENERIC };
      }
    }
  };
}

/** Change password on /account (no current password: the Supabase switch is off, 04 8.1). */
export async function changePassword(password: string): Promise<FlowResult> {
  try {
    const { error } = await supabase.auth.updateUser({ password });
    if (error) return { ok: false, message: mapUpdateError(error) };
    return { ok: true };
  } catch (err) {
    return { ok: false, message: isAuthRetryableFetchError(err) ? MSG_NETWORK : MSG_GENERIC };
  }
}
