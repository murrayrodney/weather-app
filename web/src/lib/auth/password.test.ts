import { AuthApiError, AuthRetryableFetchError } from '@supabase/supabase-js';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const m = vi.hoisted(() => ({ auth: { verifyOtp: vi.fn(), updateUser: vi.fn(), getSession: vi.fn(), signOut: vi.fn() } }));
vi.mock('./supabase', () => ({ supabase: { auth: m.auth } }));

import {
  MSG_LINK_EXPIRED,
  MSG_NETWORK,
  MSG_SAME,
  MSG_WEAK,
  changePassword,
  clearPreviousSession,
  createSetPasswordFlow,
  parseSetPasswordFragment,
  takeTokenFromLocation,
  validateNewPassword
} from './password';

const TOKEN = 'a'.repeat(64);
const PW = 'correct horse battery';

describe('parseSetPasswordFragment', () => {
  it('accepts a recovery fragment', () => {
    expect(parseSetPasswordFragment(`#token_hash=${TOKEN}&type=recovery`)).toBe(TOKEN);
  });
  it.each([
    '',
    '#',
    `#type=recovery`,
    `#token_hash=${TOKEN}&type=signup`,
    `#token_hash=${TOKEN}&type=magiclink`,
    `#token_hash=ab/cd${'e'.repeat(30)}&type=recovery`,
    `#token_hash=ab cd${'e'.repeat(30)}&type=recovery`,
    `#token_hash=${'a'.repeat(257)}&type=recovery`,
    `#token_hash=short&type=recovery`
  ])('rejects %j', (h) => {
    expect(parseSetPasswordFragment(h)).toBeNull();
  });
});

describe('validateNewPassword', () => {
  it('enforces length, bytes, match and email', () => {
    expect(validateNewPassword('a'.repeat(11), 'a'.repeat(11))).toBe(MSG_WEAK);
    expect(validateNewPassword('a'.repeat(12), 'a'.repeat(12))).toBeNull();
    expect(validateNewPassword('a'.repeat(73), 'a'.repeat(73))).not.toBeNull();
    const emoji = '\u{1F600}'.repeat(40); // 40 characters, 160 bytes
    expect(validateNewPassword(emoji, emoji)).not.toBeNull();
    expect(validateNewPassword(PW, PW + 'x')).not.toBeNull();
    expect(validateNewPassword('someone@example.test', 'someone@example.test', 'someone@example.test')).not.toBeNull();
  });
});

describe('set-password flow', () => {
  beforeEach(() => vi.clearAllMocks());

  it('redeems once per visit, then updates the password', async () => {
    m.auth.verifyOtp.mockResolvedValue({ data: {}, error: null });
    m.auth.updateUser.mockResolvedValueOnce({ data: {}, error: new AuthApiError('weak', 422, 'weak_password') });
    const flow = createSetPasswordFlow(TOKEN);
    expect(m.auth.verifyOtp).not.toHaveBeenCalled(); // not on load
    expect(await flow.submit(PW)).toEqual({ ok: false, message: MSG_WEAK });
    m.auth.updateUser.mockResolvedValueOnce({ data: {}, error: null });
    expect(await flow.submit(PW)).toEqual({ ok: true });
    expect(m.auth.verifyOtp).toHaveBeenCalledTimes(1);
    expect(m.auth.verifyOtp).toHaveBeenCalledWith({ token_hash: TOKEN, type: 'recovery' });
    expect(m.auth.updateUser).toHaveBeenCalledWith({ password: PW });
  });

  it.each([new AuthApiError('expired', 403, 'otp_expired'), new AuthApiError('bad', 422, 'validation_failed'), new AuthApiError('x', 400, 'bad_json')])(
    'maps a verifyOtp failure to the expired message',
    async (error) => {
      m.auth.verifyOtp.mockResolvedValue({ data: {}, error });
      expect(await createSetPasswordFlow(TOKEN).submit(PW)).toEqual({ ok: false, message: MSG_LINK_EXPIRED });
      expect(m.auth.updateUser).not.toHaveBeenCalled();
    }
  );

  it('same_password and network errors have fixed messages; the token stays for a retry', async () => {
    m.auth.verifyOtp.mockResolvedValueOnce({ data: {}, error: new AuthRetryableFetchError('offline', 0) });
    const flow = createSetPasswordFlow(TOKEN);
    expect(await flow.submit(PW)).toEqual({ ok: false, message: MSG_NETWORK });
    m.auth.verifyOtp.mockResolvedValueOnce({ data: {}, error: null });
    m.auth.updateUser.mockResolvedValueOnce({ data: {}, error: new AuthApiError('same', 422, 'same_password') });
    expect(await flow.submit(PW)).toEqual({ ok: false, message: MSG_SAME });
    expect(m.auth.verifyOtp).toHaveBeenCalledTimes(2);
  });

  it('never writes the token or password to storage or the console', async () => {
    const spies = (['log', 'info', 'warn', 'error'] as const).map((k) => vi.spyOn(console, k));
    m.auth.verifyOtp.mockResolvedValue({ data: {}, error: null });
    m.auth.updateUser.mockResolvedValue({ data: {}, error: null });
    await createSetPasswordFlow(TOKEN).submit(PW);
    const dump = JSON.stringify([...Object.entries(localStorage), ...Object.entries(sessionStorage), location.href]);
    expect(dump).not.toContain(TOKEN);
    expect(dump).not.toContain(PW);
    for (const s of spies) expect(JSON.stringify(s.mock.calls)).not.toContain(TOKEN);
  });
});

describe('takeTokenFromLocation', () => {
  it('returns the token and strips the fragment synchronously (no await in between)', () => {
    const calls: unknown[][] = [];
    const hist = { replaceState: (...a: unknown[]) => void calls.push(a) };
    const token = takeTokenFromLocation({ hash: `#token_hash=${TOKEN}&type=recovery`, pathname: '/set-password' }, hist as unknown as History);
    expect(token).toBe(TOKEN);
    expect(calls).toEqual([[null, '', '/set-password']]); // already done when the call returns: nothing awaited
  });
  it('strips the fragment even when it is invalid', () => {
    const hist = { replaceState: vi.fn() };
    expect(takeTokenFromLocation({ hash: '#token_hash=bad&type=signup', pathname: '/set-password' }, hist as unknown as History)).toBeNull();
    expect(hist.replaceState).toHaveBeenCalledWith(null, '', '/set-password');
  });
});

describe('clearPreviousSession', () => {
  beforeEach(() => vi.clearAllMocks());
  it('signs a signed-in user out locally before the link is redeemed', async () => {
    m.auth.getSession.mockResolvedValue({ data: { session: { access_token: 't' } }, error: null });
    m.auth.signOut.mockResolvedValue({ error: null });
    expect(await clearPreviousSession()).toBe(true);
    expect(m.auth.signOut).toHaveBeenCalledWith({ scope: 'local' });
  });
  it('does nothing without a session', async () => {
    m.auth.getSession.mockResolvedValue({ data: { session: null }, error: null });
    expect(await clearPreviousSession()).toBe(false);
    expect(m.auth.signOut).not.toHaveBeenCalled();
  });
});

describe('change password (account)', () => {
  it('calls updateUser({ password }) without asking for the current password', async () => {
    m.auth.updateUser.mockResolvedValue({ data: {}, error: null });
    expect(await changePassword(PW)).toEqual({ ok: true });
    expect(m.auth.updateUser).toHaveBeenCalledWith({ password: PW });
  });
});
