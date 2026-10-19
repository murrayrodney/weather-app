export type AuthKind = 'session_expired' | 'account_inactive';

export class ApiError extends Error {
  constructor(
    public status: number,
    public code: string,
    message: string,
    public retryAfterSec?: number,
    public kind?: AuthKind
  ) {
    super(message);
    this.name = 'ApiError';
  }
}

/** Fixed user-facing text per error (01 section 4.3). The server message is never shown. */
export function userMessage(err: unknown): string {
  if (!(err instanceof ApiError)) return 'Something went wrong on our side. Try again.';
  const { status, code } = err;
  if (code === 'network') return "Can't reach the server. Check your connection and retry.";
  if (code === 'forbidden' || code === 'not_found' || code === 'bad_id' || code === 'bad_path') return "That isn't available.";
  if (code === 'rate_limited') {
    return err.retryAfterSec ? `Too many requests. Try again in ${err.retryAfterSec} seconds.` : 'Too many requests. Try again shortly.';
  }
  if (code === 'upstream_busy') return 'The weather service is busy. Try again in a moment.';
  if (code === 'overloaded' || code === 'auth_unavailable') return 'The service is busy right now. Try again in a moment.';
  if (code === 'request_timeout') return 'The request took too long. Try again.';
  if (code === 'payload_too_large') return 'That request was too large.';
  if (code === 'upstream_unavailable') return 'The weather service is unavailable. Try again later.';
  if (code === 'validation_failed') return "That request wasn't valid.";
  if (code === 'conflict') return 'That changed while you were working. Reload and try again.';
  if (code === 'device_already_registered') return 'That device is already registered.';
  if (code === 'unknown_device') return 'That device is not on this account. Check the device list and try again.';
  if (code === 'method_not_allowed') return 'Something went wrong on our side. Try again.';
  if (err.kind === 'session_expired') return 'Your session expired. Sign in again.';
  if (err.kind === 'account_inactive') return 'This account is inactive. Contact the administrator.';
  void status;
  return 'Something went wrong on our side. Try again.';
}
