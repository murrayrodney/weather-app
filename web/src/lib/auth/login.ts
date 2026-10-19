import { isAuthRetryableFetchError } from '@supabase/supabase-js';
import { supabase } from './supabase';

export type SignInResult = 'ok' | 'failed' | 'rate_limited' | 'network' | 'invalid_input';

export const SIGN_IN_MESSAGES: Record<Exclude<SignInResult, 'ok'>, string> = {
  failed: "That email and password didn't work.",
  rate_limited: 'Too many attempts. Wait a few minutes and try again.',
  network: 'Could not reach the sign-in service. Try again.',
  invalid_input: 'Enter your email and password.'
};

export function validateSignInInput(email: string, password: string): boolean {
  return email.length > 0 && email.length <= 254 && email.includes('@') && password.length > 0;
}

/** Email + password sign-in. Every GoTrue error with an HTTP response collapses to one message (S4 oracle rule). */
export async function signIn(email: string, password: string): Promise<SignInResult> {
  const e = email.trim();
  if (!validateSignInInput(e, password)) return 'invalid_input';
  try {
    const { error } = await supabase.auth.signInWithPassword({ email: e, password });
    if (!error) return 'ok';
    if (isAuthRetryableFetchError(error)) return 'network';
    if (error.status === 429) return 'rate_limited';
    return 'failed';
  } catch (err) {
    return isAuthRetryableFetchError(err) ? 'network' : 'failed';
  }
}
