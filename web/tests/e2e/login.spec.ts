import { expect, test } from '@playwright/test';
import { DEMO_PASSWORD, UNINVITED_EMAIL, USER_A_EMAIL } from '../helpers/env';
import { json, loginUi, mockForecasts } from '../helpers/api';

const FAILED = "That email and password didn't work.";

// Many UI sign-ins as alex in this file: keep the dashboard's per-station forecast calls off the general per-user limiter.
test.beforeEach(async ({ page }) => {
  await mockForecasts(page);
});

async function fill(page: import('@playwright/test').Page, email: string, password: string) {
  await page.getByLabel('Email').fill(email);
  await page.getByLabel('Password').fill(password);
  await page.getByRole('button', { name: 'Sign in' }).click();
}

test('alex signs in with the demo password, an sb- key exists and a reload keeps the session', async ({ page }) => {
  await page.goto('/login');
  await fill(page, USER_A_EMAIL, DEMO_PASSWORD);
  await expect(page.getByRole('heading', { name: 'Dashboard' })).toBeVisible();
  expect(await page.evaluate(() => Object.keys(localStorage).some((k) => k.startsWith('sb-')))).toBe(true);
  await page.reload();
  await expect(page.getByRole('heading', { name: 'Dashboard' })).toBeVisible();
});

test('next: signed-out /follows redirects to /login?next=%2Ffollows and lands back after sign-in', async ({ page }) => {
  await page.goto('/follows');
  await expect(page).toHaveURL(/\/login\?next=%2Ffollows$/);
  await fill(page, USER_A_EMAIL, DEMO_PASSWORD);
  await expect(page).toHaveURL(/\/follows$/);
  await expect(page.getByRole('heading', { name: 'Follows' })).toBeVisible();
});

test('wrong password and unknown email show the identical message', async ({ page }) => {
  await page.goto('/login');
  await fill(page, USER_A_EMAIL, 'definitely-the-wrong-password');
  await expect(page.getByRole('alert')).toHaveText(FAILED);
  await fill(page, UNINVITED_EMAIL, 'definitely-the-wrong-password');
  await expect(page.getByRole('alert')).toHaveText(FAILED);
});

test('S4 oracle (mocked GoTrue): 400, 403 banned and 422 give identical text; 429 differs; only an abort shows the network text', async ({ page }) => {
  await page.goto('/login');
  for (const [status, code] of [
    [400, 'invalid_credentials'],
    [403, 'user_banned'],
    [422, 'validation_failed']
  ] as const) {
    await page.route('**/auth/v1/token*', (r) => json(r, { code: status, error_code: code, msg: 'whatever the server says' }, status));
    await fill(page, USER_A_EMAIL, 'x');
    await expect(page.getByRole('alert')).toHaveText(FAILED);
  }
  await page.route('**/auth/v1/token*', (r) => json(r, { code: 429, error_code: 'over_request_rate_limit', msg: 'slow' }, 429));
  await fill(page, USER_A_EMAIL, 'x');
  await expect(page.getByRole('alert')).toHaveText('Too many attempts. Wait a few minutes and try again.');
  await page.route('**/auth/v1/token*', (r) => r.abort());
  await fill(page, USER_A_EMAIL, 'x');
  await expect(page.getByRole('alert')).toHaveText('Could not reach the sign-in service. Try again.');
});

test('five failures in a row disable the button for 30 seconds with visible text', async ({ page }) => {
  await page.clock.install();
  await page.route('**/auth/v1/token*', (r) => json(r, { code: 400, error_code: 'invalid_credentials', msg: 'x' }, 400));
  await page.goto('/login');
  const button = page.getByRole('button', { name: 'Sign in' });
  for (let i = 0; i < 5; i++) {
    await expect(button).toBeEnabled();
    await fill(page, USER_A_EMAIL, 'wrong-password-' + i);
    await expect(page.getByRole('alert')).toHaveText(FAILED);
  }
  await expect(button).toBeDisabled();
  await expect(page.getByTestId('login-locked')).toContainText('Wait 30 seconds');
  await page.clock.fastForward(31000);
  await expect(button).toBeEnabled();
  await expect(page.getByTestId('login-locked')).toHaveCount(0);
});

test.describe('inactive account', () => {
  test('mocked /me 403 account_inactive ends on /account-inactive and removes sb- keys', async ({ page }) => {
    await loginUi(page, USER_A_EMAIL, DEMO_PASSWORD); // own session: the sign-out below must not burn the shared one
    await page.route('**/api/v1/me', (r) => json(r, { error: { code: 'account_inactive', message: 'x' } }, 403));
    await page.goto('/');
    await expect(page).toHaveURL(/\/account-inactive$/);
    await expect(page.getByText('This account is inactive. Contact the administrator.')).toBeVisible();
    expect(await page.evaluate(() => Object.keys(localStorage).filter((k) => k.startsWith('sb-')).length)).toBe(0);
  });
});
