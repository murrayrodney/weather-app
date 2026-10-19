import { expect, test } from '@playwright/test';
import { loginUi } from '../helpers/api';
import { DEMO_PASSWORD, NEW_PASSWORD, USER_A_EMAIL } from '../helpers/env';
import { invite, resetLink, resetLinkFails, setPassword, uniqueEmail, uniqueName } from '../helpers/invite';

test.describe.configure({ mode: 'serial' });

async function signInUi(page: import('@playwright/test').Page, email: string, password: string) {
  await page.goto('/login');
  await page.getByLabel('Email').fill(email);
  await page.getByLabel('Password').fill(password);
  await page.getByRole('button', { name: 'Sign in' }).click();
}

test('invite link: set a password, sign in with it, link is single use, reset-link needs no old password', async ({ page, browser }) => {
  test.setTimeout(180000);
  const email = uniqueEmail('setpw');
  const link = invite(email, uniqueName('SetPw'));
  const token = /token_hash=([^&]+)/.exec(link)![1];

  // Before the link is used there is no password.
  await signInUi(page, email, 'any-password-here-123');
  await expect(page.getByRole('alert')).toHaveText("That email and password didn't work.");

  // Requests and storage never carry the token (except the POST body to GoTrue).
  const urls: string[] = [];
  page.on('request', (r) => urls.push(r.url()));

  await page.goto(link);
  await expect(page.getByRole('heading', { name: 'Choose a password' })).toBeVisible();
  expect(page.url()).not.toContain('#');
  expect(page.url()).not.toContain(token);

  await page.getByLabel('New password').fill('short');
  await page.getByLabel('Confirm password').fill('short');
  await page.getByRole('button', { name: 'Set password' }).click();
  await expect(page.getByRole('alert')).toContainText('at least 12 characters');

  await page.getByLabel('New password').fill(NEW_PASSWORD);
  await page.getByLabel('Confirm password').fill(NEW_PASSWORD);
  await page.getByRole('button', { name: 'Set password' }).click();
  await expect(page.getByRole('heading', { name: 'Dashboard' })).toBeVisible();

  expect(urls.filter((u) => u.includes(token))).toEqual([]);
  expect(await page.locator('body').innerText()).not.toContain(token);
  const stored = await page.evaluate(() => JSON.stringify([...Object.entries(localStorage), ...Object.entries(sessionStorage)]));
  expect(stored).not.toContain(token);
  expect(stored).not.toContain(NEW_PASSWORD);

  await page.getByRole('button', { name: 'Sign out' }).click();
  await expect(page).toHaveURL(/\/login/);
  await signInUi(page, email, NEW_PASSWORD);
  await expect(page.getByRole('heading', { name: 'Dashboard' })).toBeVisible();

  // A saved copy of the link: record whether the token is single use (this spec asserts the observed behaviour).
  const ctx2 = await browser.newContext();
  const p2 = await ctx2.newPage();
  await setPassword(p2, link, NEW_PASSWORD + 'x');
  await expect(p2.getByRole('alert')).toHaveText('That link has expired or was already used. Ask Rodney for a new one.');
  await ctx2.close();

  // A mangled token shows the same message.
  const ctx3 = await browser.newContext();
  const p3 = await ctx3.newPage();
  await setPassword(p3, link.replace(token.slice(-6), 'zzzzzz'), NEW_PASSWORD + 'x');
  await expect(p3.getByRole('alert')).toHaveText('That link has expired or was already used. Ask Rodney for a new one.');
  await ctx3.close();

  // A reset link sets a new password without the old one; the old one stops working.
  const reset = resetLink(email);
  const ctx4 = await browser.newContext();
  const p4 = await ctx4.newPage();
  const second = 'another-new-passphrase-2';
  await setPassword(p4, reset, second);
  await expect(p4.getByRole('heading', { name: 'Dashboard' })).toBeVisible();
  await ctx4.close();
  const ctx5 = await browser.newContext();
  const p5 = await ctx5.newPage();
  await signInUi(p5, email, NEW_PASSWORD);
  await expect(p5.getByRole('alert')).toHaveText("That email and password didn't work.");
  await signInUi(p5, email, second);
  await expect(p5.getByRole('heading', { name: 'Dashboard' })).toBeVisible();
  await ctx5.close();
});

test('a page without a link shows the ask-Rodney text and no form', async ({ page }) => {
  await page.goto('/set-password');
  await expect(page.getByText('This page needs a link from Rodney. Ask for a new one.')).toBeVisible();
  await expect(page.getByLabel('New password')).toHaveCount(0);
});

test('reset-link fails in the CLI for an inactive user (Casey is seeded inactive)', async () => {
  expect(resetLinkFails('casey@example.test')).toBe(true);
});

test('a link opened while another user is signed in replaces that session (previous user is signed out first)', async ({ page }) => {
  test.setTimeout(120000);
  await loginUi(page, USER_A_EMAIL, DEMO_PASSWORD);
  const email = uniqueEmail('swap');
  await setPassword(page, invite(email, uniqueName('Swap')), NEW_PASSWORD);
  await expect(page.getByRole('heading', { name: 'Dashboard' })).toBeVisible();
  await page.getByRole('link', { name: 'Account' }).click();
  await expect(page.getByText(email)).toBeVisible();
  const stored = await page.evaluate(() => JSON.stringify(Object.entries(localStorage)));
  expect(stored).not.toContain(USER_A_EMAIL);
});
