import { goto } from '$app/navigation';
import { ApiError } from '$lib/api/errors';

/** Single mapping from an auth-failure ApiError to its route (also used by the (app) load guard). */
export function authRedirectTarget(err: unknown): string | null {
  if (!(err instanceof ApiError) || !err.kind) return null;
  return err.kind === 'session_expired' ? '/login?reason=expired' : '/account-inactive';
}

/** The only code that navigates for auth failures (besides the (app) layout load). */
export function handleAuthError(err: unknown): boolean {
  const target = authRedirectTarget(err);
  if (!target) return false;
  void goto(target, { replaceState: true });
  return true;
}

export type ActionResult<T> = { ok: true; value: T } | { ok: false; error: unknown; handled: boolean };

/** Wrapper for follows/connect handlers: auth failures navigate, everything else is returned for the caller to show. */
export async function callAction<T>(fn: () => Promise<T>): Promise<ActionResult<T>> {
  try {
    return { ok: true, value: await fn() };
  } catch (error) {
    return { ok: false, error, handled: handleAuthError(error) };
  }
}
