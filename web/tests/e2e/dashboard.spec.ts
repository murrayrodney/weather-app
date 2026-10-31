import { expect, test, type Page, type Route } from '@playwright/test';
import { AUTH_DIR } from '../helpers/env';
import { fakeSession, fx, json, mockCore } from '../helpers/api';

// The plot's own paths. Legend swatches reuse the series classes (on <line> and <rect>), so tests match `path.`.

type YtdHandler = (route: Route, url: URL) => Promise<void> | void;

/** The dashboard opens on the Forecast tab (proposal 07); the column charts need the Rain tab. */
async function gotoRain(page: Page) {
  await page.goto('/');
  await page.getByRole('button', { name: 'Rain', exact: true }).click();
}

async function setup(page: Page, ytd: YtdHandler | unknown, core: { me?: unknown; stations?: unknown[] } = {}) {
  await fakeSession(page);
  await mockCore(page, core);
  await page.route('**/api/v1/stations/*/forecast', (r) => json(r, fx.forecast()));
  const urls: URL[] = [];
  await page.route('**/api/v1/ytd*', async (route) => {
    const url = new URL(route.request().url());
    urls.push(url);
    if (typeof ytd === 'function') await (ytd as YtdHandler)(route, url);
    else await json(route, ytd);
  });
  return urls;
}

test('mocked: Forecast is the default tab: one row and one /forecast call per selected station, no ytd call until Rain', async ({ page }) => {
  const urls = await setup(page, fx.ytdFull(), {
    stations: [fx.station(), fx.station({ id: fx.STATION_ID_2, name: 'Second station' })]
  });
  const forecastPaths: string[] = [];
  // registered after setup(), so this handler takes precedence over the generic forecast mock
  await page.route('**/api/v1/stations/*/forecast', (r) => {
    forecastPaths.push(new URL(r.request().url()).pathname);
    return json(r, fx.forecast());
  });
  await page.goto('/');
  await expect(page.getByTestId('forecast-row')).toHaveCount(2);
  await expect(page.getByRole('link', { name: 'Second station' })).toHaveAttribute('href', `/stations/${fx.STATION_ID_2}`);
  await expect(page.getByTestId('forecast-row').first().getByRole('button', { name: 'Precipitation' })).toHaveCount(0);
  // each row is the compact hourly stack: a day strip, the temperature panel and its ensemble band
  const row = page.getByTestId('forecast-row').first();
  await expect(row.getByTestId('forecast-stack')).toBeVisible();
  await expect(row.getByTestId('day-strip').locator('rect.day-cell')).toHaveCount(7);
  await expect(row.locator('path.band-outer')).toHaveCount(1);
  await expect(row.getByRole('slider')).toHaveCount(1);
  expect(forecastPaths.sort()).toEqual([`/api/v1/stations/${fx.STATION_ID}/forecast`, `/api/v1/stations/${fx.STATION_ID_2}/forecast`].sort());
  expect(urls.length).toBe(0);
  // two stacks on one page: each has its own snow hatch pattern id, so neither references the other's
  const patternIds = await page.locator('pattern[id^="snow-hatch-"]').evaluateAll((els) => els.map((e) => e.id));
  expect(patternIds).toHaveLength(2);
  expect(new Set(patternIds).size).toBe(2);

  // tab switches reuse the client cache (2 minutes): no second round of /forecast calls
  await page.getByRole('button', { name: 'Rain', exact: true }).click();
  await expect.poll(() => urls.length).toBe(1);
  await page.getByRole('button', { name: 'Forecast', exact: true }).click();
  await expect(page.getByTestId('forecast-row')).toHaveCount(2);
  expect(forecastPaths.length).toBe(2);
});

test('mocked: three Rain cards sit side by side inside a 1280 px window, and the cumulative legend is an SVG row', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  // one ytd station per selected station, so the grid has three cards
  const ytd = fx.ytdResponse([
    fx.ytdStation(),
    fx.ytdStation({ stationId: fx.STATION_ID_2 }),
    fx.ytdStation({ stationId: '33333333-3333-4333-8333-333333333333' })
  ]);
  await setup(page, ytd, {
    stations: [
      fx.station(),
      fx.station({ id: fx.STATION_ID_2, name: 'Second station' }),
      fx.station({ id: '33333333-3333-4333-8333-333333333333', name: 'Third station' })
    ]
  });
  await gotoRain(page);
  const grid = page.locator('.grid');
  const cards = grid.locator(':scope > *');
  await expect(cards).toHaveCount(3);
  // no card runs past the right edge of the grid (the old 420 px tracks clipped the third card at 1280 px)
  const gridBox = (await grid.boundingBox())!;
  const boxes = await cards.evaluateAll((els) => els.map((e) => e.getBoundingClientRect().toJSON()));
  for (const b of boxes) expect(b.x + b.width).toBeLessThanOrEqual(gridBox.x + gridBox.width + 0.5);
  // three columns on one row
  for (const b of boxes) expect(Math.abs(b.y - boxes[0].y)).toBeLessThan(1);
  // the cumulative plot is drawn at its measured width (no viewBox), so its labels are the plot size (12 px, as the
  // other .plot charts), not scaled with the card
  const chart = page.getByTestId('station-ytd').first().locator('svg.chart-svg:not(.legend-row)').first();
  await expect(chart).not.toHaveAttribute('viewBox', /.*/);
  expect(await chart.locator('text').first().evaluate((el) => getComputedStyle(el).fontSize)).toBe('12px');
  expect(await chart.getAttribute('width')).toBe(String(Math.min(1400, Math.round(await chart.evaluate((el) => el.getBoundingClientRect().width)))));
  // the legend is an SVG row (the HTML .legend is gone)
  await expect(page.locator('.legend')).toHaveCount(0);
  const legend = page.getByTestId('legend-cumulative').first();
  await expect(legend.locator('.line-current')).toHaveCount(1);
  await expect(legend.locator('.line-prior')).toHaveCount(1);
  await expect(legend.locator('text.legend-label').first()).toHaveText('This year');
  const plot = (await chart.boundingBox())!;
  for (const t of await legend.locator('text.legend-label').all()) {
    expect((await t.boundingBox())!.x + (await t.boundingBox())!.width).toBeLessThanOrEqual(plot.x + plot.width + 0.5);
  }
});

test.describe('real data smoke', () => {
  test.use({ storageState: `${AUTH_DIR}/alex.json` });
  test('alex sees at least one chart whose caption starts with "Baseline:"', async ({ page }) => {
    await page.goto('/');
    await page.getByRole('button', { name: 'Rain', exact: true }).click();
    await expect(page.getByTestId('provenance').first().locator('li').first()).toContainText(/^Baseline:/);
    await expect(page.locator('path.line-current').first()).toBeVisible();
  });
});

test('mocked: full fixture shows provenance, bands, priors, current, table values and one request across tab toggles', async ({ page }) => {
  const resp = fx.ytdFull();
  const urls = await setup(page, resp);
  await gotoRain(page);
  const prov = page.getByTestId('provenance').first();
  await expect(prov).toContainText('Baseline: Demo Airport (USW00099001), NOAA GHCN-Daily via ACIS; 34 years in the percentile bands, last 4 years drawn.');
  await expect(prov).toContainText('Bands: 10th-90th and 25th-75th percentile of prior years.');
  await expect(prov).toContainText('Days are midnight to midnight in local standard time');
  await expect(prov).not.toContainText('Shaded months');
  await expect(page.locator('path.band-outer').first()).toBeVisible();
  await expect(page.locator('path.band-inner').first()).toBeVisible();
  await expect(page.locator('path.line-prior').first()).toBeAttached();
  await expect(page.locator('path.line-current').first()).toBeVisible();
  await expect(page.locator('path.line-current-partial').first()).toBeAttached();
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

test('mocked: cumulative tooltip: this year, median, band and prior years at the hovered day, the same sentence as the slider value text, a right-edge flip, arrows and Escape', async ({ page }) => {
  await setup(page, fx.ytdFull());
  await gotoRain(page);
  const chart = page.getByTestId('station-ytd').first().locator('svg.chart-svg:not(.legend-row)').first();
  const tip = page.getByTestId('tip-cumulative').first();
  const valuetext = () => chart.getAttribute('aria-valuetext').then((v) => v ?? '');
  // label to value, in row order (the labels and values are two lists of the same length)
  const rows = async () => {
    const labels = await tip.locator('text.tip-label').allTextContents();
    const values = await tip.locator('text.tip-value').allTextContents();
    return Object.fromEntries(labels.map((l, i) => [l, values[i]]));
  };
  await expect(tip).toHaveCount(0);

  // x = 200 is inside the season (April 1 to July 10 is this year's line) and the prior years drawn are 2025 to 2022
  await chart.hover({ position: { x: 200, y: 100 } });
  await expect(tip).toBeVisible();
  // the tooltip and the slider's value text name the same day and the same this-year value
  const title = (await tip.locator('text.tip-title').textContent()) ?? '';
  expect(title).not.toBe('');
  await expect.poll(valuetext).toContain(`${title}: This year `);
  const r = await rows();
  expect(r['This year']).toMatch(/^-?\d+\.\d\d in$/);
  await expect.poll(valuetext).toContain(`This year ${r['This year']}`);
  expect(r['Median']).toMatch(/ in$/);
  expect(r['Percentile band']).toMatch(/ in to .* in$/);
  expect(Object.keys(r)).toEqual(expect.arrayContaining(['This year', 'Median', 'Percentile band', '2025', '2024', '2023', '2022']));
  expect(Object.keys(r)).not.toContain('2021'); // not drawn: too many missing days
  // the swatches carry the legend's classes
  await expect(tip.locator('line.line-current')).toHaveCount(1);
  await expect(tip.locator('rect.band-outer')).toHaveCount(1);
  await expect(tip.locator('line.line-prior')).toHaveCount(4);

  // a pointer near the right edge flips the box to the left of the crosshair
  const box = (await chart.boundingBox())!;
  await chart.hover({ position: { x: box.width - 120, y: 100 } });
  const cross = Number(await chart.locator('line.crosshair').getAttribute('x1'));
  const tx = Number(/translate\(([-\d.]+) /.exec((await tip.getAttribute('transform')) ?? '')?.[1]);
  expect(tx).toBeLessThan(cross);

  // arrows move the day; Escape clears
  await chart.focus();
  await page.keyboard.press('ArrowRight');
  // the value text follows the arrow: it names the tooltip's day (a day with no data has no "This year" row)
  await expect.poll(valuetext).not.toBe('');
  const moved = (await valuetext()).split(':')[0];
  expect(moved).toBe(((await tip.locator('text.tip-title').textContent()) ?? '').trim());
  await page.keyboard.press('Escape');
  await expect(tip).toHaveCount(0);
  await expect(chart).not.toHaveAttribute('aria-valuetext');
});

test('mocked: compact dashboard row: a temperature-only tooltip with forecast, ensemble median and range, feels like and dew point; a right-edge flip; arrows and Escape', async ({ page }) => {
  await setup(page, fx.ytdFull());
  await page.goto('/');
  const row = page.getByTestId('forecast-row').first();
  const slider = row.getByTestId('panel-stack');
  const tip = row.getByTestId('tip-temperature');
  const labels = () => tip.locator('text.tip-label').allTextContents();
  await expect(row.locator('[data-testid^="tip-"]')).toHaveCount(0);

  await slider.hover({ position: { x: 300, y: 60 } });
  await expect(tip).toBeVisible();
  // only the temperature panel has a tooltip in the compact row
  await expect(row.locator('[data-testid^="tip-"]')).toHaveCount(1);
  expect(await labels()).toEqual(['Forecast', 'Ensemble median', 'Range p10-p90', 'Feels like', 'Dew point']);
  // the box matches the row's slider value text at the same hour
  const valueTemp = /Forecast (-?\d+) F/.exec((await slider.getAttribute('aria-valuetext')) ?? '')?.[1];
  expect(await tip.locator('text.tip-value').first().textContent()).toBe(`${valueTemp} F`);

  const box = (await slider.boundingBox())!;
  await slider.hover({ position: { x: box.width - 120, y: 60 } });
  const cross = Number(await row.locator('line.crosshair').getAttribute('x1'));
  const tx = Number(/translate\(([-\d.]+) /.exec((await tip.getAttribute('transform')) ?? '')?.[1]);
  expect(tx).toBeLessThan(cross);

  // with the pointer gone, the first arrow starts at the first hour
  await page.mouse.move(0, 0);
  await expect(row.locator('[data-testid^="tip-"]')).toHaveCount(0);
  await slider.focus();
  await page.keyboard.press('ArrowRight');
  await expect(slider).toHaveAttribute('aria-valuenow', '0');
  await expect(tip).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(row.locator('[data-testid^="tip-"]')).toHaveCount(0);
});

test('mocked: no baseline shows the caption, no bands, GDD current only', async ({ page }) => {
  await setup(page, fx.ytdNoBaseline());
  await gotoRain(page);
  await expect(page.getByTestId('provenance').first()).toContainText('No baseline is set for this station yet.');
  await expect(page.locator('path.band-outer')).toHaveCount(0);
  await expect(page.locator('path.line-prior')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Choose a baseline' })).toBeVisible();
  await page.getByRole('button', { name: 'GDD', exact: true }).click();
  await expect(page.locator('path.line-current').first()).toBeVisible();
  await expect(page.locator('path.band-outer')).toHaveCount(0);
});

test('mocked: baseline still loading shows the loading caption', async ({ page }) => {
  await setup(page, fx.ytdBaselineLoading());
  await gotoRain(page);
  await expect(page.getByTestId('provenance').first()).toContainText('Baseline chosen: Demo Airport. Its history is loading and should appear within the hour.');
  await expect(page.locator('path.band-outer')).toHaveCount(0);
});

test('mocked: GDD settings echo, one request per setting change, none for tab switches', async ({ page }) => {
  const urls = await setup(page, fx.ytdGddClamped());
  await gotoRain(page);
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
  await gotoRain(page);
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
  await gotoRain(page);
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
    await gotoRain(page);
    await expect(page.locator('path.line-current').first()).toBeVisible();
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
  await gotoRain(page);
  await expect(page.getByTestId('season-note')).toHaveText('No data yet this season.');
});

test('mocked: reconnect banner and link only on own stations', async ({ page }) => {
  const a = fx.STATION_ID;
  const b = fx.STATION_ID_2;
  await setup(page, fx.ytdResponse([fx.ytdStation({ stationId: a }), fx.ytdStation({ stationId: b })]), {
    me: fx.me('invalid'),
    stations: [fx.station({ id: a, name: 'Mine', is_own: true }), fx.station({ id: b, name: 'Theirs', is_own: false })]
  });
  await gotoRain(page);
  await expect(page.getByText('Data collection paused: this station needs reconnecting.')).toHaveCount(1);
  await expect(page.getByRole('link', { name: 'Reconnect' })).toHaveAttribute('href', '/connect?reconnect=1');
  const mine = page.getByTestId('station-ytd').filter({ hasText: 'Mine' });
  await expect(mine.getByRole('link', { name: 'Reconnect' })).toHaveCount(1);
  await expect(page.getByTestId('station-ytd').filter({ hasText: 'Theirs' }).getByRole('link', { name: 'Reconnect' })).toHaveCount(0);
});

test('mocked: stale latest_observation_at shows "No recent data."', async ({ page }) => {
  await setup(page, fx.ytdFull(), { stations: [fx.station({ latest_observation_at: '2020-01-01T00:00:00Z' })] });
  await gotoRain(page);
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
  await page.route('**/api/v1/stations/*/forecast', (r) => json(r, fx.forecast()));
  await page.route('**/api/v1/ytd*', (r) => json(r, { error: { code: 'not_found', message: 'x' } }, 404));
  await gotoRain(page);
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
    await gotoRain(page);
    await expect(page.getByRole('alert').getByText(text)).toBeVisible();
  });
}
