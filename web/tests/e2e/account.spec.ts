import { expect, test } from '@playwright/test';
import { NEW_PASSWORD } from '../helpers/env';
import { invite, setPassword, uniqueEmail, uniqueName } from '../helpers/invite';

// Uses its own invited user so the seed passwords stay untouched.
test('change password on /account, sign out, sign in with the new one', async ({ page }) => {
  test.setTimeout(120000);
  const email = uniqueEmail('acct');
  await setPassword(page, invite(email, uniqueName('Account')), NEW_PASSWORD);
  await expect(page.getByRole('heading', { name: 'Dashboard' })).toBeVisible();

  await page.getByRole('link', { name: 'Account' }).click();
  await expect(page.getByRole('heading', { name: 'Account' })).toBeVisible();
  await expect(page.getByText(email)).toBeVisible();
  await expect(page.getByLabel(/current password/i)).toHaveCount(0);

  const fresh = 'yet-another-passphrase-3';
  await page.getByLabel('New password').fill(fresh);
  await page.getByLabel('Confirm password').fill(fresh);
  await page.getByRole('button', { name: 'Change password' }).click();
  await expect(page.getByText('Password changed.')).toBeVisible();

  await page.getByRole('button', { name: 'Sign out', exact: true }).first().click();
  await expect(page).toHaveURL(/\/login/);
  await page.getByLabel('Email').fill(email);
  await page.getByLabel('Password').fill(NEW_PASSWORD);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('alert')).toHaveText("That email and password didn't work.");
  await page.getByLabel('Password').fill(fresh);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('heading', { name: 'Dashboard' })).toBeVisible();
});
