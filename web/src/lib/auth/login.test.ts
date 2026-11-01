import { AuthApiError, AuthRetryableFetchError } from '@supabase/supabase-js';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const m = vi.hoisted(() => ({
  auth: {
    signInWithPassword: vi.fn(),
    signInWithOtp: vi.fn(),
    signUp: vi.fn(),
    resetPasswordForEmail: vi.fn()
  }
}));
vi.mock('./supabase', () => ({ supabase: { auth: m.auth } }));

import { SIGN_IN_MESSAGES, signIn } from './login';

describe('signIn', () => {
  beforeEach(() => vi.clearAllMocks());

  it('calls signInWithPassword and nothing else; success is ok', async () => {
    m.auth.signInWithPassword.mockResolvedValue({ data: {}, error: null });
    expect(await signIn('a@example.test', 'pw-123456789012')).toBe('ok');
    expect(m.auth.signInWithPassword).toHaveBeenCalledWith({ email: 'a@example.test', password: 'pw-123456789012' });
    expect(m.auth.signInWithOtp).not.toHaveBeenCalled();
    expect(m.auth.signUp).not.toHaveBeenCalled();
    expect(m.auth.resetPasswordForEmail).not.toHaveBeenCalled();
  });

  it('gives one result for wrong password, unknown email, banned and unconfirmed users', async () => {
    const errors = [
      new AuthApiError('Invalid login credentials', 400, 'invalid_credentials'),
      new AuthApiError('User is banned', 403, 'user_banned'),
      new AuthApiError('Email not confirmed', 400, 'email_not_confirmed'),
      new AuthApiError('Unprocessable', 422, 'validation_failed')
    ];
    for (const error of errors) {
      m.auth.signInWithPassword.mockResolvedValueOnce({ data: {}, error });
      expect(await signIn('a@example.test', 'x')).toBe('failed');
    }
    expect(SIGN_IN_MESSAGES.failed).toBe("That email and password didn't work.");
  });

  it('429 is rate_limited; retryable fetch errors (returned or thrown) are network', async () => {
    m.auth.signInWithPassword.mockResolvedValueOnce({ data: {}, error: new AuthApiError('slow down', 429, 'over_request_rate_limit') });
    expect(await signIn('a@example.test', 'x')).toBe('rate_limited');
    m.auth.signInWithPassword.mockResolvedValueOnce({ data: {}, error: new AuthRetryableFetchError('offline', 0) });
    expect(await signIn('a@example.test', 'x')).toBe('network');
    m.auth.signInWithPassword.mockRejectedValueOnce(new AuthRetryableFetchError('offline', 0));
    expect(await signIn('a@example.test', 'x')).toBe('network');
    m.auth.signInWithPassword.mockRejectedValueOnce(new Error('boom'));
    expect(await signIn('a@example.test', 'x')).toBe('failed');
  });

  it('validates input without calling the service', async () => {
    expect(await signIn('', 'x')).toBe('invalid_input');
    expect(await signIn('noatsign', 'x')).toBe('invalid_input');
    expect(await signIn('a@example.test', '')).toBe('invalid_input');
    expect(m.auth.signInWithPassword).not.toHaveBeenCalled();
  });

  it('never leaks the password to console, URL or storage', async () => {
    const spies = (['log', 'info', 'warn', 'error', 'debug'] as const).map((k) => vi.spyOn(console, k));
    m.auth.signInWithPassword.mockResolvedValue({ data: {}, error: new AuthApiError('bad', 400, 'invalid_credentials') });
    await signIn('a@example.test', 'secret-password-xyz');
    for (const s of spies) expect(JSON.stringify(s.mock.calls)).not.toContain('secret-password-xyz');
    expect(location.href).not.toContain('secret-password-xyz');
    expect(JSON.stringify(Object.entries(localStorage))).not.toContain('secret-password-xyz');
    expect(localStorage.getItem('weather.next')).toBeNull();
  });
});
