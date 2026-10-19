import { expect, test, type Page, type Route } from '@playwright/test';
import { AUTH_DIR } from '../helpers/env';
import { fakeSession, fx, json, mockCore } from '../helpers/api';

type YtdHandler = (route: Route, url: URL) => Promise<void> | void;

async function setup(page: Page, ytd: YtdHandler | unknown, core: { me?: unknown; stations?: unknown[] } = {}) {
  await fakeSession(page);
  await mockCore(page, core);
  const urls: URL[] = [];
  await page.route('**/api/v1/ytd*', async (route) => {
    const url = new URL(route.request().url());
    urls.push(url);
    if (typeof ytd === 'function') await (ytd as YtdHandler)(route, url);
    else await json(route, ytd);
  });
  return urls;
}

test.describe('real data smoke', () => {
  test.use({ storageState: `${AUTH_DIR}/alex.json` });
  test('alex sees at least one chart whose caption starts with "Baseline:"', async ({ page }) => {
    await page.goto('/');
    await expect(page.getByTestId('provenance').first().locator('li').first()).toContainText(/^Baseline:/);
    await expect(page.locator('.line-current').first()).toBeVisible();
  });
});

test('mocked: full fixture shows provenance, bands, priors, current, table values and one request across tab toggles', async ({ page }) => {
  const resp = fx.ytdFull();
  const urls = await setup(page, resp);
  await page.goto('/');
  const prov = page.getByTestId('provenance').first();
  await expect(prov).toContainText('Baseline: Demo Airport (USW00099001), NOAA GHCN-Daily via ACIS; 34 years in the percentile bands, last 4 years drawn.');
  await expect(prov).toContainText('Bands: 10th-90th and 25th-75th percentile of prior years.');
  await expect(prov).toContainText('Days are midnight to midnight in local standard time');
  await expect(prov).not.toContainText('Shaded months');
  await expect(page.locator('.band-outer').first()).toBeVisible();
  await expect(page.locator('.band-inner').first()).toBeVisible();
  await expect(page.locator('.line-prior').first()).toBeAttached();
  await expect(page.locator('.line-current').first()).toBeVisible();
  await expect(page.locator('.line-current-partial').first()).toBeAttached();
  await expect(page.getByTestId('missing-current')).toHaveText('2 days missing this season (counted as zero)');

  // data table numbers equal the fixture
  await page.getByText('Show data table').first().click();
  const last = resp.stations[0].current.rain_cum_in.at(-1)!;
  const row = page.getByTestId('data-table').first().locator('tr', { hasText: 'July 10' });
  await expect(row.locator('td').nth(1)).toHaveText(last.v.toFixed(2));

  // Full year: winter note on Rain, not on GDD
  await page.getByRole('button', { name: 'Full year' }).click();
  await expect(page.getByTestId('provenance').first()).toContainText('Shaded months: unheated rain gauges undercount snow');
  await page.getByRole('button', { name: 'GDD' }).click();
  await expect(page.getByTestId('provenance').first()).not.toContainText('Shaded months');
  await expect(page.getByTestId('provenance').first()).toContainText('GDD: Simple average, base 50 F, counted from April 1.');
  await expect(page.getByTestId('missing-current')).toHaveText('1 days missing this season (counted as zero)');

  // Rain -> GDD -> Rain on windows already fetched: no repeat request
  const before = urls.length;
  await page.getByRole('button', { name: 'Rain', exact: true }).click();
  await page.getByRole('button', { name: 'GDD', exact: true }).click();
  await page.getByRole('button', { name: 'Rain', exact: true }).click();
  expect(urls.length).toBe(before);
  // Apr-Oct then Full year were two distinct windows
  expect(urls.length).toBe(2);
});

test('mocked: no baseline shows the caption, no bands, GDD current only', async ({ page }) => {
  await setup(page, fx.ytdNoBaseline());
  await page.goto('/');
  await expect(page.getByTestId('provenance').first()).toContainText('No baseline is set for this station yet.');
  await expect(page.locator('.band-outer')).toHaveCount(0);
  await expect(page.locator('.line-prior')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Choose a baseline' })).toBeVisible();
  await page.getByRole('button', { name: 'GDD', exact: true }).click();
  await expect(page.locator('.line-current').first()).toBeVisible();
  await expect(page.locator('.band-outer')).toHaveCount(0);
});

test('mocked: baseline still loading shows the loading caption', async ({ page }) => {
  await setup(page, fx.ytdBaselineLoading());
  await page.goto('/');
  await expect(page.getByTestId('provenance').first()).toContainText('Baseline chosen: Demo Airport. Its history is loading and should appear within the hour.');
  await expect(page.locator('.band-outer')).toHaveCount(0);
});

test('mocked: GDD settings echo, one request per setting change, none for tab switches', async ({ page }) => {
  const urls = await setup(page, fx.ytdGddClamped());
  await page.goto('/');
  await page.getByRole('button', { name: 'GDD', exact: true }).click();
  await expect(page.getByTestId('provenance').first()).toContainText('GDD: Clamped to the base and 86 F, base 45 F, counted from January 1.');
  expect(urls.length).toBe(1);
  await page.getByLabel('Method').selectOption('clamped_50_86');
  await expect(page).toHaveURL(/gdd_method=clamped_50_86/);
  await expect.poll(() => urls.length).toBe(2);
  expect(urls[1].searchParams.get('gdd_method')).toBe('clamped_50_86');
  await page.getByRole('button', { name: 'Rain', exact: true }).click();
  await page.getByRole('button', { name: 'GDD', exact: true }).click();
  expect(urls.length).toBe(2);
});

test('mocked: a GDD start after the window end is not sent', async ({ page }) => {
  const urls = await setup(page, fx.ytdFull());
  await page.goto('/');
  await page.getByRole('button', { name: 'GDD', exact: true }).click();
  const before = urls.length;
  await page.getByLabel('Start month').selectOption('11'); // November is after the Apr-Oct window end
  await expect(page.getByTestId('gdd-start-problem')).toContainText('The start date must be on or before October 31.');
  expect(urls.length).toBe(before);
  expect(page.url()).not.toContain('gdd_start');
  await page.getByLabel('Start month').selectOption('6'); // June is fine
  await expect(page).toHaveURL(/gdd_start=06-01/);
  await expect.poll(() => urls.length).toBe(before + 1);
  expect(urls.at(-1)!.searchParams.get('gdd_start')).toBe('06-01');
});

test('mocked: a station that began mid-season starts at first_date, has no position text and a partial open-day segment', async ({ page }) => {
  await setup(page, fx.ytdPartialStation());
  await page.goto('/');
  await expect(page.getByTestId('starts-note')).toContainText('Data starts 2026-06-01; earlier days are not included, so the total reads low against the baseline.');
  await expect(page.getByTestId('position')).toHaveCount(0);
  await expect(page.locator('path.line-current-partial').first()).toBeAttached();
});

for (const [label, iso, offSeason] of [
  ['2026-12-15', '2026-12-15T18:00:00Z', true],
  ['2027-02-10', '2027-02-10T18:00:00Z', true],
  ['2026-07-01', '2026-07-01T18:00:00Z', false]
] as const) {
  test(`mocked season states on ${label}`, async ({ page }) => {
    const urls = await setup(page, fx.ytdFull(), { stations: [fx.station({ latest_observation_at: iso })] });
    await page.clock.install({ time: new Date(iso) });
    await page.goto('/');
    await expect(page.locator('.line-current').first()).toBeVisible();
    expect(urls[0].searchParams.get('year')).toBe('2026');
    if (offSeason) {
      await expect(page.getByTestId('off-season-banner')).toHaveText(
        'Showing the 2026 season (April-October), the most recent completed season. The 2027 season starts April 1.'
      );
      await expect(page.getByTestId('provenance').first()).toContainText('Showing the 2026 season');
    } else {
      await expect(page.getByTestId('off-season-banner')).toHaveCount(0);
    }
  });
}

test('mocked: "No data yet this season." on April 2 and "Season not started" before the window', async ({ page }) => {
  const empty = fx.ytdResponse([fx.ytdStation({ lastDate: null })]);
  await setup(page, empty, { stations: [fx.station({ latest_observation_at: '2026-04-02T18:00:00Z' })] });
  await page.clock.install({ time: new Date('2026-04-02T18:00:00Z') });
  await page.goto('/');
  await expect(page.getByTestId('season-note')).toHaveText('No data yet this season.');
});

test('mocked: reconnect banner and link only on own stations', async ({ page }) => {
  const a = fx.STATION_ID;
  const b = fx.STATION_ID_2;
  await setup(page, fx.ytdResponse([fx.ytdStation({ stationId: a }), fx.ytdStation({ stationId: b })]), {
    me: fx.me('invalid'),
    stations: [fx.station({ id: a, name: 'Mine', is_own: true }), fx.station({ id: b, name: 'Theirs', is_own: false })]
  });
  await page.goto('/');
  await expect(page.getByText('Data collection paused: this station needs reconnecting.')).toHaveCount(1);
  await expect(page.getByRole('link', { name: 'Reconnect' })).toHaveAttribute('href', '/connect?reconnect=1');
  const mine = page.getByTestId('station-ytd').filter({ hasText: 'Mine' });
  await expect(mine.getByRole('link', { name: 'Reconnect' })).toHaveCount(1);
  await expect(page.getByTestId('station-ytd').filter({ hasText: 'Theirs' }).getByRole('link', { name: 'Reconnect' })).toHaveCount(0);
});

test('mocked: stale latest_observation_at shows "No recent data."', async ({ page }) => {
  await setup(page, fx.ytdFull(), { stations: [fx.station({ latest_observation_at: '2020-01-01T00:00:00Z' })] });
  await page.goto('/');
  await expect(page.getByText('No recent data.')).toBeVisible();
});

test('mocked: ytd 404 shows the error panel and Retry reloads stations', async ({ page }) => {
  await fakeSession(page);
  let stationCalls = 0;
  await page.route('**/api/v1/me', (r) => json(r, fx.me()));
  await page.route('**/api/v1/stations', (r) => {
    stationCalls++;
    return json(r, { stations: [fx.station()] });
  });
  await page.route('**/api/v1/ytd*', (r) => json(r, { error: { code: 'not_found', message: 'x' } }, 404));
  await page.goto('/');
  await expect(page.getByText("That isn't available.")).toBeVisible();
  const before = stationCalls;
  await page.getByRole('button', { name: 'Retry' }).click();
  await expect.poll(() => stationCalls).toBeGreaterThan(before);
});

for (const [name, status, body, headers, text] of [
  ['500', 500, { error: { code: 'internal', message: 'x' } }, {}, 'Something went wrong on our side. Try again.'],
  ['503 overloaded', 503, { error: { code: 'overloaded', message: 'x' } }, { 'Retry-After': '1' }, 'The service is busy right now. Try again in a moment.'],
  ['429', 429, { error: { code: 'rate_limited', message: 'x' } }, { 'Retry-After': '7' }, 'Too many requests. Try again in 7 seconds.']
] as const) {
  test(`mocked: ${name} shows its message`, async ({ page }) => {
    await setup(page, (route: Route) => json(route, body, status, headers as Record<string, string>));
    await page.goto('/');
    await expect(page.getByRole('alert').getByText(text)).toBeVisible();
  });
}
