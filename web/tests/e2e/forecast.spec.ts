import { expect, test, type Page } from '@playwright/test';
import { AUTH_DIR } from '../helpers/env';
import { apiGet, fakeSession, fx, json } from '../helpers/api';

const ID = fx.STATION_ID;
const TODAY = '2026-07-10T18:00:00Z';

interface Hits {
  forecast: number;
  changes: URL[];
  origins: Set<string>;
}

async function setup(page: Page, opts: { forecast?: unknown; forecastStatus?: number; changes?: unknown; changesStatus?: number } = {}): Promise<Hits> {
  await fakeSession(page);
  const hits: Hits = { forecast: 0, changes: [], origins: new Set() };
  page.on('request', (r) => hits.origins.add(new URL(r.url()).origin));
  await page.route('**/api/v1/me', (r) => json(r, fx.me()));
  await page.route('**/api/v1/stations', (r) => json(r, { stations: [fx.station()] }));
  await page.route(`**/api/v1/stations/${ID}`, (r) => json(r, fx.station()));
  await page.route(`**/api/v1/stations/${ID}/forecast`, (r) => {
    hits.forecast++;
    return json(r, opts.forecast ?? fx.forecast(), opts.forecastStatus ?? 200);
  });
  await page.route(`**/api/v1/stations/${ID}/forecast/changes*`, (r) => {
    hits.changes.push(new URL(r.request().url()));
    return json(r, opts.changes ?? fx.forecastChanges(), opts.changesStatus ?? 200);
  });
  await page.clock.install({ time: new Date(TODAY) });
  return hits;
}

test('mocked: live chart draws forecast line, median and ribbon; switches change the series with no extra request', async ({ page }) => {
  const hits = await setup(page);
  await page.goto(`/stations/${ID}/forecast`);
  const chart = page.getByTestId('forecast-chart');
  await expect(chart.locator('path.line-forecast').first()).toBeVisible();
  await expect(chart.locator('path.line-median').first()).toBeAttached();
  await expect(chart.locator('path.band-outer').first()).toBeAttached();
  const hourlyTemp = await chart.locator('path.line-forecast').first().getAttribute('d');
  await page.getByRole('button', { name: 'Precipitation' }).click();
  const hourlyPrecip = await chart.locator('path.line-forecast').first().getAttribute('d');
  expect(hourlyPrecip).not.toBe(hourlyTemp);
  await page.getByRole('button', { name: 'Daily (15 days)' }).click();
  await expect(chart.locator('text', { hasText: '35%' }).first()).toBeAttached();
  await expect(chart.locator('line.bar-range').first()).toBeAttached();
  await page.getByRole('button', { name: 'Temperature' }).click();
  expect(await chart.locator('path.line-forecast').count()).toBe(2); // tmax and tmin
  expect(hits.forecast).toBe(1);
});

test('mocked: ensemble null hides ribbons; stale shows the saved-at line', async ({ page }) => {
  await setup(page, { forecast: fx.forecast({ ensemble: null, stale: true }) });
  await page.goto(`/stations/${ID}/forecast`);
  await expect(page.getByTestId('no-ensemble')).toHaveText('Uncertainty range unavailable right now.');
  await expect(page.locator('path.band-outer')).toHaveCount(0);
  await expect(page.getByTestId('stale-note')).toContainText('Showing the forecast saved at');
  await expect(page.getByTestId('stale-note')).toContainText('; refreshing failed.');
});

test('mocked: change view requests today..today+6, draws 8 leads, a table, attribution and model', async ({ page }) => {
  const hits = await setup(page);
  await page.goto(`/stations/${ID}/forecast`);
  const chart = page.getByTestId('changes-chart');
  await expect(chart.locator('path.line-lead-0')).toBeAttached();
  for (let i = 0; i < 8; i++) await expect(chart.locator(`path.line-lead-${i}`)).toBeAttached();
  expect(hits.changes[0].searchParams.get('from')).toBe('2026-07-10');
  expect(hits.changes[0].searchParams.get('to')).toBe('2026-07-16');
  await expect(page.getByTestId('changes-table').locator('tbody tr')).toHaveCount(7);
  await expect(page.getByTestId('changes-table').locator('tbody tr').first()).toContainText('moved by');
  // a null point breaks the line (lead 4 on day 3 for rain)
  const d4 = await chart.locator('path.line-lead-4').getAttribute('d');
  expect((d4!.match(/M/g) ?? []).length).toBe(2);
  const captions = await page.getByTestId('provenance').allInnerTexts();
  expect(captions.join('\n')).toContain('Weather data by Open-Meteo.com (CC BY 4.0)');
  expect(captions.join('\n')).toContain('gfs_seamless');
  expect(captions.join('\n')).toContain('day offsets, not model runs');
  // footer attribution too
  await expect(page.getByRole('link', { name: 'Open-Meteo.com' })).toHaveAttribute('href', 'https://open-meteo.com/');
});

test('mocked: a span over 14 days shows "Pick up to 14 days." without a request, and a 422 shows the same text', async ({ page }) => {
  const hits = await setup(page);
  await page.goto(`/stations/${ID}/forecast`);
  await expect(page.getByTestId('changes-chart')).toBeVisible();
  const n = hits.changes.length;
  await page.getByLabel('To').fill('2026-07-28');
  await expect(page.getByText('Pick up to 14 days.')).toBeVisible();
  expect(hits.changes.length).toBe(n);

  const page2 = await page.context().newPage();
  const hits2 = await setup(page2, { changesStatus: 422, changes: { error: { code: 'validation_failed', message: 'x' } } });
  await page2.goto(`/stations/${ID}/forecast`);
  await expect(page2.getByText('Pick up to 14 days.')).toBeVisible();
  expect(hits2.changes.length).toBeGreaterThan(0);
});

test('mocked: span boundary is inclusive: 14 days is requested, 15 days is refused without a request', async ({ page }) => {
  const hits = await setup(page);
  await page.goto(`/stations/${ID}/forecast`);
  await expect(page.getByTestId('changes-chart')).toBeVisible();
  await page.getByLabel('To').fill('2026-07-23'); // 07-10 .. 07-23 = 14 days
  await expect.poll(() => hits.changes.length).toBe(2);
  expect(hits.changes[1].searchParams.get('to')).toBe('2026-07-23');
  await expect(page.getByText('Pick up to 14 days.')).toHaveCount(0);
  await page.getByLabel('To').fill('2026-07-24'); // 15 days
  await expect(page.getByText('Pick up to 14 days.')).toBeVisible();
  await page.waitForTimeout(300);
  expect(hits.changes.length).toBe(2);
});

test('mocked: past window with null probability explains the data gap', async ({ page }) => {
  const c = fx.forecastChanges();
  c.days = c.days.map((d, i) => ({ ...d, date: `2026-07-0${3 + i}`, prob_precip_max: d.prob_precip_max.map(() => null) }));
  await setup(page, { changes: c });
  await page.goto(`/stations/${ID}/forecast`);
  await page.getByLabel('From').fill('2026-07-03');
  await page.getByLabel('To').fill('2026-07-09');
  await expect(page.getByText('Chance of rain is not available before October 2024.')).toBeVisible();
});

test('mocked: the browser only talks to the page origin', async ({ page }) => {
  const hits = await setup(page);
  await page.goto(`/stations/${ID}/forecast`);
  await expect(page.getByTestId('changes-chart')).toBeVisible();
  expect([...hits.origins].filter((o) => o !== 'http://localhost:8000')).toEqual([]);
});

for (const [name, status, code, text] of [
  ['502', 502, 'upstream_unavailable', 'The weather service is unavailable. Try again later.'],
  ['503', 503, 'upstream_busy', 'The weather service is busy. Try again in a moment.']
] as const) {
  test(`mocked: ${name} shows the error panel with Retry`, async ({ page }) => {
    await setup(page, { forecastStatus: status, forecast: { error: { code, message: 'x' } } });
    await page.goto(`/stations/${ID}/forecast`);
    await expect(page.getByRole('alert').getByText(text)).toBeVisible();
    await expect(page.getByRole('button', { name: 'Retry' }).first()).toBeVisible();
  });
}

test('mocked: 404 shows "That isn\'t available."', async ({ page }) => {
  await fakeSession(page);
  await page.route('**/api/v1/me', (r) => json(r, fx.me()));
  await page.route(`**/api/v1/stations/${ID}`, (r) => json(r, { error: { code: 'not_found', message: 'x' } }, 404));
  await page.goto(`/stations/${ID}/forecast`);
  await expect(page.getByText("That isn't available.")).toBeVisible();
});

test.describe('real smoke', () => {
  test.use({ storageState: `${AUTH_DIR}/alex.json` });
  test('both sections render for a seeded station', async ({ page }) => {
    await page.goto('/');
    await page.waitForSelector('[data-testid="station-ytd"]');
    const stations = await apiGet(page, '/api/v1/stations');
    await page.goto(`/stations/${stations.body.stations[0].id}/forecast`);
    await expect(page.getByTestId('forecast-chart')).toBeVisible();
    await expect(page.getByTestId('changes-table').locator('tbody tr').first()).toBeVisible();
  });
});
