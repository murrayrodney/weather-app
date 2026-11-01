import { expect, test, type Page } from '@playwright/test';
import { INVALID_AWN_KEY, NEW_PASSWORD, VALID_AWN_KEY } from '../helpers/env';
import { apiGet, fakeSession, fx, json } from '../helpers/api';
import { invite, setPassword, uniqueEmail, uniqueName } from '../helpers/invite';

const GENERIC = "That key couldn't be verified. Check it and try again.";

async function enterKey(page: Page, key: string) {
  await page.getByLabel('API key').fill(key);
  await page.getByRole('button', { name: 'Check key' }).click();
}

test('real: invalid key, rate-limited key, then the fixture devices connect', async ({ page }) => {
  test.setTimeout(120000);
  await setPassword(page, invite(uniqueEmail('connect'), uniqueName('Connect')), NEW_PASSWORD);
  await expect(page.getByRole('heading', { name: 'Dashboard' })).toBeVisible();
  const consoleText: string[] = [];
  page.on('console', (m) => consoleText.push(m.text()));
  const requestUrls: string[] = [];
  page.on('request', (r) => requestUrls.push(r.url()));

  await page.goto('/connect');
  const input = page.getByLabel('API key');
  await expect(input).toHaveAttribute('type', 'text');
  await expect(input).toHaveAttribute('autocomplete', 'off');
  await expect(input).toHaveClass(/secret-input/);

  await enterKey(page, INVALID_AWN_KEY);
  await expect(page.getByRole('alert')).toHaveText(GENERIC);
  const text = await page.locator('body').innerText();
  for (const bad of ['ambientweather', 'http', '401', 'apiKey']) expect(text).not.toContain(bad);

  await enterKey(page, 'fixture-ratelimited-key');
  await expect(page.getByRole('alert')).toContainText(/busy|Too many|try again/i);

  await enterKey(page, VALID_AWN_KEY);
  await expect(page.getByText('AA:BB:CC:00:01:01')).toBeVisible();
  await expect(page.getByText('AA:BB:CC:00:01:02')).toBeVisible();
  await expect(page.locator('select')).toHaveCount(0); // fixture devices carry AWN time zones: no picker
  for (const cb of await page.getByRole('checkbox').all()) await cb.check();
  await page.getByRole('button', { name: 'Save' }).click();
  await expect(page.getByText('Connected. Your history will fill in over the next few minutes.').first()).toBeVisible();
  const stations = await apiGet(page, '/api/v1/stations');
  expect(stations.body.stations).toHaveLength(2);

  // the key is nowhere
  const dump = JSON.stringify([...(await page.evaluate(() => [Object.entries(localStorage), Object.entries(sessionStorage)])), page.url()]);
  expect(dump).not.toContain(VALID_AWN_KEY);
  expect(consoleText.join('\n')).not.toContain(VALID_AWN_KEY);
  expect(requestUrls.some((u) => u.includes(VALID_AWN_KEY))).toBe(false);
  await page.goto('/connect');
  await expect(page.getByLabel('API key')).toHaveValue('');
});

async function mockConnect(page: Page, validate: unknown, save: { status?: number; body: unknown }) {
  await fakeSession(page);
  await page.route('**/api/v1/me', (r) => json(r, fx.me()));
  await page.route('**/api/v1/stations', (r) => json(r, { stations: [] }));
  await page.route('**/api/v1/connect/validate', (r) => json(r, validate));
  await page.route('**/api/v1/connect/save', (r) => json(r, save.body, save.status ?? 201));
}

const device = (mac: string, timezone: string | null) => ({ mac, name: 'Fixture ' + mac.slice(-2), timezone });
const saveOk = (extra: Record<string, unknown> = {}) => ({
  credential: { status: 'ok' },
  stations: [fx.station({ baseline: null })],
  not_covered: [],
  ...extra
});

test('mocked: a null timezone shows the picker for that device only and the save body carries the zone', async ({ page }) => {
  await mockConnect(page, { valid: true, devices: [device('AA:BB:CC:00:01:01', null), device('AA:BB:CC:00:01:02', 'America/Denver')] }, { body: saveOk() });
  await page.route('**/api/v1/stations/*/baseline/candidates', (r) => json(r, fx.candidatesEmpty()));
  await page.goto('/connect');
  await enterKey(page, 'mock-key-1');
  await expect(page.locator('select')).toHaveCount(1);
  for (const cb of await page.getByRole('checkbox').all()) await cb.check();
  await page.getByRole('button', { name: 'Save' }).click();
  await expect(page.getByText('Choose a time zone.')).toBeVisible();
  await page.locator('select').selectOption('America/Chicago');
  const bodyPromise = page.waitForRequest('**/api/v1/connect/save');
  await page.getByRole('button', { name: 'Save' }).click();
  const body = JSON.parse((await bodyPromise).postData()!);
  expect(body.devices).toEqual([
    { mac: 'AA:BB:CC:00:01:01', name: 'Fixture 01', timezone: 'America/Chicago' },
    { mac: 'AA:BB:CC:00:01:02', name: 'Fixture 02' }
  ]);
});

test('mocked: save 422 timezone_required marks the device', async ({ page }) => {
  await mockConnect(page, { valid: true, devices: [device('AA:BB:CC:00:01:01', null)] }, { status: 422, body: { error: { code: 'timezone_required', message: 'x' } } });
  await page.goto('/connect');
  await enterKey(page, 'mock-key-2');
  await page.getByRole('checkbox').check();
  await page.locator('select').selectOption('America/Denver');
  await page.getByRole('button', { name: 'Save' }).click();
  await expect(page.getByText('Choose a time zone.')).toBeVisible();
});

test('mocked: baseline picker lists candidates, preselects the nearest and PUTs exactly the reference id', async ({ page }) => {
  await mockConnect(page, { valid: true, devices: [device('AA:BB:CC:00:01:01', 'America/Denver')] }, { body: saveOk() });
  await page.route('**/api/v1/stations/*/baseline/candidates', (r) => json(r, fx.candidatesLoveland()));
  let put = '';
  await page.route('**/api/v1/stations/*/baseline', (r) => {
    put = r.request().postData() ?? '';
    return json(r, fx.station());
  });
  await page.goto('/connect');
  await enterKey(page, 'mock-key-3');
  await page.getByRole('checkbox').check();
  await page.getByRole('button', { name: 'Save' }).click();
  await expect(page.getByText('(suggested, nearest)')).toBeVisible();
  await expect(page.getByText('4.4 mi away')).toBeVisible();
  await expect(page.getByText('100 ft higher than your station')).toBeVisible();
  await expect(page.getByText('78 years of records')).toBeVisible();
  await expect(page.getByText('short record')).toBeVisible();
  await expect(page.getByText('Different elevation may change rainfall and temperature comparisons')).toBeVisible();
  await expect(page.getByRole('radio').first()).toBeChecked();
  await page.getByRole('button', { name: 'Use this baseline' }).click();
  await expect(page.getByText('Baseline set. Its history should appear within the hour.')).toBeVisible();
  expect(put).toBe('{"reference_station_id":"USW00099003"}');
});

test('mocked: empty candidates, 502 candidates, skip and not_covered', async ({ page }) => {
  await mockConnect(page, { valid: true, devices: [device('AA:BB:CC:00:01:01', 'America/Denver')] }, { body: saveOk({ not_covered: [{ id: 'z', name: 'Old Station' }] }) });
  await page.route('**/api/v1/stations/*/baseline/candidates', (r) => json(r, fx.candidatesEmpty()));
  await page.goto('/connect');
  await enterKey(page, 'mock-key-4');
  await page.getByRole('checkbox').check();
  await page.getByRole('button', { name: 'Save' }).click();
  await expect(page.getByText('No airport weather station with a long enough record was found within 60 miles. You can choose one later.')).toBeVisible();
  await expect(page.getByText('These stations are no longer on this account; ask Rodney to disable them:')).toBeVisible();
  await expect(page.getByText('Old Station')).toBeVisible();

  // 502 on candidates: saved message, never blocks
  await page.unroute('**/api/v1/stations/*/baseline/candidates');
  await page.route('**/api/v1/stations/*/baseline/candidates', (r) => json(r, { error: { code: 'upstream_unavailable', message: 'x' } }, 502));
  const puts: string[] = [];
  page.on('request', (r) => r.method() === 'PUT' && puts.push(r.url()));
  await page.goto('/connect');
  await enterKey(page, 'mock-key-5');
  await page.getByRole('checkbox').check();
  await page.getByRole('button', { name: 'Save' }).click();
  await expect(page.getByText("We couldn't look up nearby airport stations right now. Your station is saved")).toBeVisible();
  await page.getByRole('button', { name: 'Skip for now' }).click();
  expect(puts).toEqual([]);
});
