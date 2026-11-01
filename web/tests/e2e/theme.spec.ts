import { expect, test } from '@playwright/test';
import { AUTH_DIR } from '../helpers/env';
import { watchCsp } from '../helpers/csp';

// The header toggle cycles System -> Light -> Dark (src/lib/components/ThemeToggle.svelte), keeps an explicit choice in
// localStorage under "theme", and applies it before the page loads (static/theme-init.js). Colours are the --bg token of
// each block in app.css. Signed in from the stored session of pat (no sign-in form, no extra sign-in): session.spec.ts
// signs alex out everywhere earlier in the run, which revokes alex.json, so alex is not used here.

const LIGHT_BG = 'rgb(245, 246, 248)'; // --bg, light
const DARK_BG = 'rgb(18, 21, 29)'; // --bg, dark (the explicit and the OS-driven blocks)

test.use({ storageState: `${AUTH_DIR}/pat.json` });

test('the toggle cycles System, Light, Dark and the explicit choice survives a reload; no CSP violations', async ({ page }) => {
  const w = await watchCsp(page);
  await page.goto('/');
  const toggle = page.getByTestId('theme-toggle');
  await expect(toggle).toHaveAttribute('title', 'Theme: System');
  await expect(page.locator('html')).not.toHaveAttribute('data-theme');

  await toggle.click();
  await expect(toggle).toHaveAttribute('title', 'Theme: Light');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
  expect(await page.evaluate(() => localStorage.getItem('theme'))).toBe('light');

  await toggle.click();
  await expect(toggle).toHaveAttribute('title', 'Theme: Dark');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  expect(await page.evaluate(() => localStorage.getItem('theme'))).toBe('dark');

  await page.reload();
  await expect(toggle).toHaveAttribute('title', 'Theme: Dark');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');

  await toggle.click();
  await expect(toggle).toHaveAttribute('title', 'Theme: System');
  await expect(page.locator('html')).not.toHaveAttribute('data-theme');
  expect(await page.evaluate(() => localStorage.getItem('theme'))).toBeNull();
  await w.expectClean('theme toggle, reload and back to System');
});

test('a saved theme is applied before the page loads (no flash), with no CSP violations', async ({ page }) => {
  const w = await watchCsp(page);
  await page.goto('/login');
  await page.evaluate(() => localStorage.setItem('theme', 'dark'));
  // Read the attribute when the document is parsed (DOMContentLoaded). theme-init.js runs in <head>, before that.
  await page.addInitScript(() => {
    document.addEventListener('DOMContentLoaded', () => {
      (window as unknown as { __themeAtDOMContentLoaded: string | null }).__themeAtDOMContentLoaded = document.documentElement.dataset.theme ?? null;
    });
  });
  await page.reload();
  await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible();
  expect(await page.evaluate(() => (window as unknown as { __themeAtDOMContentLoaded: string | null }).__themeAtDOMContentLoaded)).toBe('dark');
  await w.expectClean('saved dark theme on /login');
});

test('System follows the OS, and an explicit choice wins over the OS in both directions', async ({ page }) => {
  const w = await watchCsp(page);
  const bg = () => page.evaluate(() => getComputedStyle(document.documentElement).backgroundColor);
  const scheme = () => page.evaluate(() => getComputedStyle(document.documentElement).colorScheme);

  await page.emulateMedia({ colorScheme: 'dark' });
  await page.goto('/');
  const toggle = page.getByTestId('theme-toggle');
  await expect(toggle).toHaveAttribute('title', 'Theme: System');
  expect(await bg()).toBe(DARK_BG);
  expect(await scheme()).toBe('dark');

  // explicit Light beats an OS that is dark
  await toggle.click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
  expect(await bg()).toBe(LIGHT_BG);
  expect(await scheme()).toBe('light');

  // explicit Dark, then the OS switches to light: the explicit choice still wins
  await toggle.click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await page.emulateMedia({ colorScheme: 'light' });
  expect(await bg()).toBe(DARK_BG);

  // back to System: the OS is light now, so light
  await toggle.click();
  await expect(toggle).toHaveAttribute('title', 'Theme: System');
  expect(await bg()).toBe(LIGHT_BG);
  await w.expectClean('OS dark and light with System and explicit choices');
});
