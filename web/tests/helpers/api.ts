import type { Page, Route } from '@playwright/test';
import * as fx from '../../src/test/fixtures';
import { BASE_URL } from './env';

/** Reads the access token the page holds (tests are exempt from the fetch lint rule). */
export async function accessToken(page: Page): Promise<string> {
  return page.evaluate(() => {
    const k = Object.keys(localStorage).find((x) => x.startsWith('sb-'));
    return k ? (JSON.parse(localStorage.getItem(k)!).access_token as string) : '';
  });
}

export async function apiGet(page: Page, path: string, token?: string): Promise<{ status: number; body: any }> {
  const t = token ?? (await accessToken(page));
  const res = await page.request.get(`${BASE_URL}${path}`, { headers: { Authorization: `Bearer ${t}` } });
  return { status: res.status(), body: await res.json().catch(() => null) };
}

/** Puts a far-future fake Supabase session in storage so mocked specs never depend on the real clock or GoTrue. */
export async function fakeSession(page: Page) {
  await page.addInitScript(() => {
    const exp = 4102444800;
    localStorage.setItem(
      'sb-localhost-auth-token',
      JSON.stringify({
        access_token: 'fake.access.token',
        refresh_token: 'fake-refresh',
        token_type: 'bearer',
        expires_in: 3600,
        expires_at: exp,
        user: { id: '33333333-3333-4333-8333-333333333333', aud: 'authenticated', email: 'fake@example.test', app_metadata: {}, user_metadata: {}, created_at: '2026-01-01T00:00:00Z' }
      })
    );
  });
}

/** Fresh UI login (for destructive specs that must not burn the shared storageState sessions). */
export async function loginUi(page: Page, email: string, password: string) {
  await page.goto('/login');
  await page.getByLabel('Email').fill(email);
  await page.getByLabel('Password').fill(password);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await page.getByRole('heading', { name: 'Dashboard' }).waitFor();
  await page.waitForLoadState('networkidle');
}

export const json =(route: Route, body: unknown, status = 200, headers: Record<string, string> = {}) =>
  route.fulfill({ status, contentType: 'application/json', headers, body: JSON.stringify(body) });

export async function mockCore(page: Page, opts: { me?: unknown; stations?: unknown[] } = {}) {
  await page.route('**/api/v1/me', (r) => json(r, opts.me ?? fx.me()));
  await page.route('**/api/v1/stations', (r) => json(r, { stations: opts.stations ?? [fx.station()] }));
}

export { fx };
