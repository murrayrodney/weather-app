import { test as setup, expect } from '@playwright/test';
import { AUTH_DIR, DEMO_PASSWORD, USER_A_EMAIL, USER_B_EMAIL, USER_DANA_EMAIL } from '../helpers/env';
import { mkdirSync } from 'node:fs';

mkdirSync(AUTH_DIR, { recursive: true });

for (const [name, email] of [
  ['alex', USER_A_EMAIL],
  ['pat', USER_B_EMAIL],
  ['dana', USER_DANA_EMAIL]
] as const) {
  setup(`sign in ${name}`, async ({ page }) => {
    await page.goto('/login');
    await page.getByLabel('Email').fill(email);
    await page.getByLabel('Password').fill(DEMO_PASSWORD);
    await page.getByRole('button', { name: 'Sign in' }).click();
    await expect(page).toHaveURL(/\/$/);
    await expect(page.getByRole('heading', { name: 'Dashboard' })).toBeVisible();
    await page.context().storageState({ path: `${AUTH_DIR}/${name}.json` });
  });
}
