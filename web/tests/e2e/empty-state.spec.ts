import { expect, test } from '@playwright/test';
import { AUTH_DIR } from '../helpers/env';

test.use({ storageState: `${AUTH_DIR}/pat.json` });

test('pat (no stations, no follows) sees the empty state', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByText('No stations yet.')).toBeVisible();
  await expect(page.getByRole('link', { name: 'Connect your weather station' })).toBeVisible();
  await expect(page.getByText('Ask Rodney to share stations with you.')).toBeVisible();
  await expect(page.getByRole('link', { name: 'Find people to follow' })).toBeVisible();
});
