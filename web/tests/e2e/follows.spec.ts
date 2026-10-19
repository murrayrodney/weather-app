import { expect, test, type Page } from '@playwright/test';
import { AUTH_DIR } from '../helpers/env';
import { apiGet, fakeSession, fx, json } from '../helpers/api';

// Changes seeded follow states: run once per `make reset` (the e2e target resets first).
test('alex approves Dana, Dana sees the stations, alex revokes, Dana loses them', async ({ browser }) => {
  const alexCtx = await browser.newContext({ storageState: `${AUTH_DIR}/alex.json` });
  const danaCtx = await browser.newContext({ storageState: `${AUTH_DIR}/dana.json` });
  const alex = await alexCtx.newPage();
  const dana = await danaCtx.newPage();

  await alex.goto('/follows');
  const danaRow = alex.getByTestId('follow-row').filter({ hasText: 'Dana' });
  await expect(danaRow).toContainText('pending');
  await expect(danaRow).toContainText('Requested');
  await danaRow.getByRole('button', { name: 'Approve' }).click();
  await expect(alex.getByTestId('followers').getByTestId('follow-row').filter({ hasText: 'Dana' })).toContainText('approved');

  await dana.goto('/follows');
  const mine = dana.getByTestId('following');
  await expect(mine.getByTestId('follow-row').filter({ hasText: 'Alex' })).toContainText('approved');
  await expect(mine.getByTestId('follow-row').filter({ hasText: 'Sam' })).toContainText('declined');
  await expect(mine.getByTestId('follow-row').filter({ hasText: 'Rodney' })).toContainText('revoked');
  const withAlex = await apiGet(dana, '/api/v1/stations');
  expect(withAlex.body.stations.some((s: { owner: { display_name: string } }) => s.owner.display_name === 'Alex')).toBe(true);

  await alex.reload();
  const row = alex.getByTestId('follow-row').filter({ hasText: 'Dana' });
  await row.getByRole('button', { name: 'Revoke' }).click();
  await expect(row.getByText('Stop sharing your stations with Dana?')).toBeVisible(); // inline confirm, not window.confirm
  await row.getByRole('button', { name: 'Confirm revoke' }).click();
  await expect(alex.getByTestId('follow-row').filter({ hasText: 'Dana' }).first()).toContainText('revoked');

  const after = await apiGet(dana, '/api/v1/stations');
  expect(after.body.stations.some((s: { owner: { display_name: string } }) => s.owner.display_name === 'Alex')).toBe(false);
  await dana.reload();
  await expect(dana.getByTestId('following').getByTestId('follow-row').filter({ hasText: 'Alex' })).toContainText('revoked');
  await alexCtx.close();
  await danaCtx.close();
});

async function mockFollows(page: Page) {
  await fakeSession(page);
  await page.route('**/api/v1/me', (r) => json(r, fx.me()));
  await page.route('**/api/v1/follows', (r) => {
    if (r.request().method() === 'POST') return json(r, { user: { id: 'x', display_name: 'Dana' }, status: 'pending' }, 201);
    return json(r, { following: [], followers: [] });
  });
}

test('mocked: find people search, request button states and POST body', async ({ page }) => {
  await mockFollows(page);
  const searches: string[] = [];
  await page.route('**/api/v1/users/search*', (r) => {
    searches.push(new URL(r.request().url()).searchParams.get('q')!);
    return json(r, fx.userSearch());
  });
  const posts: string[] = [];
  page.on('request', (r) => {
    if (r.method() === 'POST' && r.url().endsWith('/api/v1/follows')) posts.push(r.postData() ?? '');
  });
  const urls: string[] = [];
  page.on('request', (r) => urls.push(r.url()));

  await page.goto('/follows');
  const box = page.getByLabel('Find a person by name');
  await box.fill('d');
  await page.waitForTimeout(500);
  expect(searches).toEqual([]);
  await expect(page.getByText('Type at least 2 characters.')).toBeVisible();

  await box.pressSequentially('an', { delay: 50 });
  await expect(page.getByTestId('search-results')).toBeVisible();
  expect(searches).toEqual(['dan']);

  const rows = page.getByTestId('search-results').locator('li');
  await expect(rows).toHaveCount(5);
  for (const [name, button] of [
    ['Dana', true],
    ['Danny', false],
    ['Daria', false],
    ['Dale', true],
    ['Dawn', true]
  ] as const) {
    await expect(rows.filter({ hasText: name }).getByRole('button', { name: 'Request to follow' })).toHaveCount(button ? 1 : 0);
  }
  await rows.filter({ hasText: 'Dana' }).getByRole('button', { name: 'Request to follow' }).click();
  await expect(rows.filter({ hasText: 'Dana' })).toContainText('Pending');
  expect(posts).toEqual(['{"followee_id":"44444444-4444-4444-8444-444444444441"}']);
  expect(await page.locator('body').innerText()).not.toContain('@');
  expect(urls.some((u) => u.includes('@') || u.includes('%40'))).toBe(false);
});

test('mocked: a 429 on search shows the wait message and disables the input', async ({ page }) => {
  await mockFollows(page);
  await page.route('**/api/v1/users/search*', (r) => json(r, { error: { code: 'rate_limited', message: 'x' } }, 429, { 'Retry-After': '7' }));
  await page.goto('/follows');
  await page.getByLabel('Find a person by name').fill('da');
  await expect(page.getByText('Too many searches. Try again in 7 seconds.')).toBeVisible();
  await expect(page.getByLabel('Find a person by name')).toBeDisabled();
});

test.describe('real smoke', () => {
  test.use({ storageState: `${AUTH_DIR}/pat.json` });
  test('pat searches "ro" and sees Rodney by display name only', async ({ page }) => {
    await page.goto('/follows');
    await page.getByLabel('Find a person by name').fill('ro');
    const results = page.getByTestId('search-results');
    await expect(results).toContainText('Rodney');
    expect(await results.innerText()).not.toContain('@');
  });
});
