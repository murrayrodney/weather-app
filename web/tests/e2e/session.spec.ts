import { expect, test } from '@playwright/test';
import { DEMO_PASSWORD, USER_A_EMAIL } from '../helpers/env';
import { json, loginUi, mockForecasts } from '../helpers/api';

const sbKeys = (page: import('@playwright/test').Page) => page.evaluate(() => Object.keys(localStorage).filter((k) => k.startsWith('sb-')).length);

// These specs end sessions (sign-out and refresh rotate tokens), so each signs in on its own instead of using storageState.
test.describe('signed in', () => {
  test.beforeEach(async ({ page }) => {
    await mockForecasts(page); // keeps the per-user general limiter for the many sign-ins below
    await loginUi(page, USER_A_EMAIL, DEMO_PASSWORD);
  });

  test('sign-out removes sb- keys and / redirects to /login?next=%2F', async ({ page }) => {
    await expect(page.getByRole('heading', { name: 'Dashboard' })).toBeVisible();
    await page.getByRole('button', { name: 'Sign out' }).click();
    await expect(page).toHaveURL(/\/login/);
    expect(await sbKeys(page)).toBe(0);
    await page.goto('/');
    await expect(page).toHaveURL(/\/login\?next=%2F$/);
  });

  test('mocked /api/v1/me 401 twice ends on /login with the session expired text', async ({ page }) => {
    await page.route('**/api/v1/me', (r) => json(r, { error: { code: 'unauthorized', message: 'x' } }, 401));
    await page.goto('/');
    await expect(page).toHaveURL(/\/login\?reason=expired/);
    await expect(page.getByText('Your session expired. Sign in again.')).toBeVisible();
  });

  test('mocked /api/v1/me 503 auth_unavailable shows the busy message and keeps the session', async ({ page }) => {
    await page.route('**/api/v1/me', (r) => json(r, { error: { code: 'auth_unavailable', message: 'x' } }, 503, { 'Retry-After': '1' }));
    await page.goto('/');
    await expect(page.getByText('The service is busy right now. Try again in a moment.')).toBeVisible();
    expect(await sbKeys(page)).toBe(1);
    await expect(page).not.toHaveURL(/\/login/);
  });

  test('mid-session 403 account_inactive (client navigation) ends on /account-inactive, not /login (S11)', async ({ page }) => {
    await page.route('**/api/v1/follows', (r) => json(r, { error: { code: 'account_inactive', message: 'x' } }, 403));
    await page.getByRole('link', { name: 'Follows' }).click(); // client-side navigation: the dashboard stays loaded
    await expect(page).toHaveURL(/\/account-inactive$/);
    await expect(page.getByText('This account is inactive. Contact the administrator.')).toBeVisible();
    await page.waitForTimeout(500); // the SIGNED_OUT listener must not navigate afterwards
    await expect(page).toHaveURL(/\/account-inactive$/);
    expect(await sbKeys(page)).toBe(0);
  });

  test('"Sign out everywhere" tells the user when the global revoke fails (local session is dropped, login page says so)', async ({ page }) => {
    await page.getByRole('link', { name: 'Account' }).click();
    await page.route('**/auth/v1/logout*', (r) => json(r, { msg: 'down' }, 500));
    await page.getByRole('button', { name: 'Sign out everywhere' }).click();
    await expect(page).toHaveURL(/\/login\?reason=revoke_failed$/);
    await expect(page.getByTestId('revoke-failed')).toContainText('could not reach the server to end your other sessions');
    expect(await sbKeys(page)).toBe(0);
  });

  test('"Sign out everywhere" succeeds quietly when the revoke works', async ({ page }) => {
    await page.getByRole('link', { name: 'Account' }).click();
    await page.getByRole('button', { name: 'Sign out everywhere' }).click();
    await expect(page).toHaveURL(/\/login$/);
    await expect(page.getByTestId('revoke-failed')).toHaveCount(0);
    expect(await sbKeys(page)).toBe(0);
  });

  test('/stations/not-a-uuid says it is not available and requests no station data', async ({ page }) => {
    const hits: string[] = [];
    page.on('request', (r) => {
      const u = new URL(r.url());
      if (u.pathname.startsWith('/api/v1/') && u.pathname !== '/api/v1/me') hits.push(u.pathname);
    });
    await page.goto('/stations/not-a-uuid');
    await expect(page.getByText("That isn't available.")).toBeVisible();
    expect(hits).toEqual([]);
  });
});

for (const [label, next] of [
  ['//evil.com', '%2F%2Fevil.com'],
  ['/<tab>/evil.com', '%2F%09%2Fevil.com']
] as const) {
  test(`unsafe next ${label} ends on / after login`, async ({ page }) => {
    await page.goto(`/login?next=${next}`);
    await page.getByLabel('Email').fill(USER_A_EMAIL);
    await page.getByLabel('Password').fill(DEMO_PASSWORD);
    await page.getByRole('button', { name: 'Sign in' }).click();
    await expect(page.getByRole('heading', { name: 'Dashboard' })).toBeVisible();
    expect(new URL(page.url()).pathname).toBe('/');
    expect(new URL(page.url()).hostname).toBe('localhost');
  });
}
