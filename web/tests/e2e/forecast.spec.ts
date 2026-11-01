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
  await page.route('**/api/v1/ytd*', (r) => json(r, fx.ytdFull()));
  await page.clock.install({ time: new Date(TODAY) });
  return hits;
}

test('mocked: /forecast redirects to the station forecast anchor; sections run forecast, changes, then Rain and GDD; no Forecast button', async ({ page }) => {
  await setup(page);
  await page.goto(`/stations/${ID}/forecast`);
  await expect(page).toHaveURL(new RegExp(`/stations/${ID}#forecast$`));
  await expect(page.getByTestId('forecast-stack')).toBeVisible();
  await expect(page.locator('.tabs a')).toHaveCount(0);
  const top = async (sel: string) => (await page.locator(sel).boundingBox())!.y;
  expect(await top('#forecast')).toBeLessThan(await top('#changes'));
  expect(await top('#changes')).toBeLessThan(await top('#season'));
  await page.getByRole('link', { name: 'Changes', exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/stations/${ID}#changes$`));
  await expect(page.getByTestId('changes-chart')).toBeVisible();
  await page.getByRole('link', { name: 'Rain and GDD', exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/stations/${ID}#season$`));
  await expect(page.getByTestId('station-ytd')).toBeVisible();
});

test('mocked: hourly stack draws the temperature panel, band, median, feels-like, dew point, 32 F line and day strip', async ({ page }) => {
  const hits = await setup(page);
  await page.goto(`/stations/${ID}`);
  const stack = page.getByTestId('forecast-stack');
  await expect(stack.getByTestId('panel-stack').locator('path.line-forecast')).toBeVisible();
  // temperature median, plus the accumulated-precipitation ensemble median in the precipitation panel
  await expect(stack.locator('path.line-median')).toHaveCount(2);
  await expect(stack.locator('path.band-outer')).toHaveCount(1);
  await expect(stack.locator('path.line-feels')).toHaveCount(1);
  await expect(stack.locator('path.line-dew')).toHaveCount(1);
  await expect(stack.locator('path.ref-freeze')).toHaveCount(1);
  // "now" is 2026-07-10 12:00 in Denver, inside the 7-day window; the day strip has one cell per local day
  await expect(stack.locator('line.now-line')).toHaveCount(1);
  await expect(stack.getByTestId('day-strip').locator('rect.day-cell')).toHaveCount(7);
  await expect(stack.getByTestId('day-strip').locator('text.day-hilo').first()).toHaveText('80 / 50 F');
  // only the hourly stack on this view: no temperature/precipitation switch
  await expect(page.getByRole('button', { name: 'Precipitation' })).toHaveCount(0);
  // axis labels are 12 px, not scaled up
  expect(await stack.locator('text.tick-y').first().evaluate((el) => getComputedStyle(el).fontSize)).toBe('12px');
  expect(hits.forecast).toBe(1);
});

test('mocked: four panels (temperature, precipitation, sky, wind) with the precipitation, cloud, pressure and wind series and one arrow every 3 hours', async ({ page }) => {
  await setup(page);
  await page.goto(`/stations/${ID}`);
  const stack = page.getByTestId('forecast-stack');
  await expect(stack.locator('rect.panel-frame')).toHaveCount(4);
  // precipitation: hourly bars, ensemble hourly range, accumulated total with its band and median, chance of precipitation
  await expect(stack.locator('path.bar-precip')).toHaveCount(1);
  await expect(stack.locator('path.range-precip')).toHaveCount(1);
  await expect(stack.locator('path.line-accum')).toHaveCount(1);
  await expect(stack.locator('path.band-accum')).toHaveCount(1);
  await expect(stack.locator('path.line-prob')).toHaveCount(1);
  // sky: cloud area, humidity, pressure on the right-hand inHg axis
  await expect(stack.locator('path.area-cloud')).toHaveCount(1);
  await expect(stack.locator('path.line-humidity')).toHaveCount(1);
  await expect(stack.locator('path.line-pressure')).toHaveCount(1);
  await expect(stack.locator('text.panel-unit', { hasText: 'inHg' })).toHaveCount(1);
  // wind: speed, gusts and one arrow every 3 local hours over the 7 visible days (8 a day)
  await expect(stack.locator('path.line-wind')).toHaveCount(1);
  await expect(stack.locator('path.line-gust')).toHaveCount(1);
  // 56 arrows on the wind panel, plus the one Direction swatch in its legend (the legend's <path> carries the class too)
  await expect(stack.getByTestId('legend-wind').locator('path.wind-arrow')).toHaveCount(1);
  await expect(stack.locator('path.wind-arrow')).toHaveCount(57);
  // model ranges: temperature and rain only (the one ribbon above is temperature)
  await expect(stack.locator('path.band-outer')).toHaveCount(1);
  // the fixture has no snowfall, so there is no snow axis
  await expect(stack.locator('text.panel-unit', { hasText: 'Snow (in)' })).toHaveCount(0);
  await expect(stack.locator('path.bar-snow')).toHaveCount(0);
  // separate labelled axes: hourly liquid on the left, the accumulated total on its own right-hand axis
  await expect(stack.locator('text.panel-unit', { hasText: 'Hourly (in)' })).toHaveCount(1);
  await expect(stack.locator('text.panel-unit', { hasText: 'Total (in)' })).toHaveCount(1);
});

test('mocked: a snow axis and snow bars appear only when the window has snowfall', async ({ page }) => {
  const f = fx.forecast();
  f.deterministic!.hourly.snow_in = f.deterministic!.hourly.snow_in.map((_, i) => (i % 24 === 5 ? 0.4 : 0));
  f.deterministic!.daily[0].snow_in = 0.4;
  await setup(page, { forecast: f });
  await page.goto(`/stations/${ID}`);
  const stack = page.getByTestId('forecast-stack');
  await expect(stack.locator('text.panel-unit', { hasText: 'Snow (in)' })).toHaveCount(1);
  await expect(stack.locator('path.bar-snow')).toHaveCount(1);
  // the day strip shows the day's snow total only where it is above zero
  await expect(stack.getByTestId('day-strip').locator('text.day-snow')).toHaveCount(1);
  await expect(stack.getByTestId('day-strip').locator('text.day-snow')).toHaveText('snow 0.40 in');
  // snow is hatched with the stack's own pattern (unique id), and its legend swatch uses the same pattern
  await expect(stack.getByTestId('legend-precipitation').locator('.bar-snow')).toHaveCount(1);
  const id = await stack.locator('path.bar-snow').getAttribute('fill');
  expect(id).toMatch(/^url\(#snow-hatch-\d+\)$/);
  await expect(stack.locator(`pattern[id="${id!.slice(5, -1)}"] .snow-hatch`)).toHaveCount(1);
  await expect(stack.getByTestId('legend-precipitation').locator('.bar-snow')).toHaveAttribute('fill', id!);
  // the swatch's pattern is in the same SVG as the legend (its own stack, not another one on the page)
  await expect(stack.getByTestId('panel-stack').locator(`pattern[id="${id!.slice(5, -1)}"]`)).toHaveCount(1);
});

test('mocked: each panel legend draws the series that panel draws, with the series class, and the captions carry no colour words', async ({ page }) => {
  await setup(page);
  await page.goto(`/stations/${ID}`);
  const stack = page.getByTestId('forecast-stack');
  const temp = stack.getByTestId('legend-temperature');
  await expect(temp).toHaveAttribute('aria-hidden', 'true');
  for (const cls of ['line-forecast', 'line-median', 'band-outer', 'line-feels', 'line-dew', 'ref-freeze']) {
    await expect(temp.locator(`.${cls}`), cls).toHaveCount(1);
  }
  await expect(temp.locator('text.legend-label')).toHaveText(['Forecast', 'Ensemble median', 'Range p10-p90', 'Feels like', 'Dew point', '32 F']);
  // the default fixture has no snowfall, so there is no snow swatch
  const precip = stack.getByTestId('legend-precipitation');
  await expect(precip.locator('.bar-snow')).toHaveCount(0);
  for (const cls of ['bar-precip', 'range-precip', 'line-accum', 'band-accum', 'line-median', 'line-prob']) {
    await expect(precip.locator(`.${cls}`), cls).toHaveCount(1);
  }
  await expect(stack.getByTestId('legend-sky').locator('.area-cloud, .line-humidity, .line-pressure')).toHaveCount(3);
  await expect(stack.getByTestId('legend-wind').locator('.line-wind, .line-gust, .wind-arrow')).toHaveCount(3);
  // the caption keeps model and attribution; the colour sentences are gone (the legends name the series)
  const provenance = page.getByTestId('provenance').first();
  await expect(provenance).not.toContainText('dashed');
  await expect(provenance).not.toContainText('purple');
  await expect(provenance).toContainText('Forecast: ');
});

test('mocked: the daily view draws high (solid gold) and low (dashed blue) with their own bands and a legend', async ({ page }) => {
  await setup(page);
  await page.goto(`/stations/${ID}`);
  await page.getByRole('button', { name: 'Daily (15 days)' }).click();
  const chart = page.getByTestId('forecast-chart');
  await expect(chart.locator('path.line-tmax')).toHaveCount(1);
  await expect(chart.locator('path.line-tmin')).toHaveCount(1);
  await expect(chart.locator('path.band-tmax')).toHaveCount(1);
  await expect(chart.locator('path.band-tmin')).toHaveCount(1);
  expect(await chart.locator('path.line-tmax').evaluate((el) => getComputedStyle(el).strokeDasharray)).toBe('none');
  expect(await chart.locator('path.line-tmin').evaluate((el) => getComputedStyle(el).strokeDasharray)).toBe('6px, 3px');
  const legend = page.getByTestId('legend-daily');
  await expect(legend.locator('.line-tmax')).toHaveCount(1);
  await expect(legend.locator('.line-tmin')).toHaveCount(1);
  await expect(legend.locator('text.legend-label')).toHaveText(['High', 'Low', 'High p10-p90', 'Low p10-p90', 'Median']);
  const plot = (await chart.boundingBox())!;
  for (const t of await legend.locator('text.legend-label').all()) {
    expect((await t.boundingBox())!.x + (await t.boundingBox())!.width).toBeLessThanOrEqual(plot.x + plot.width + 0.5);
  }
});

test('mocked: paging moves the window a day at a time; the y axes stay pinned; the day strip and now line follow the window', async ({ page }) => {
  await setup(page);
  await page.goto(`/stations/${ID}`);
  const stack = page.getByTestId('forecast-stack');
  await expect(stack.locator('text.tick-y').first()).toBeAttached();
  const ticks = await stack.locator('text.tick-y').allTextContents();
  expect(ticks.length).toBeGreaterThan(0);
  await expect(stack.getByTestId('page-range')).toHaveText('Showing Fri, 7/10 to Thu, 7/16');
  await expect(stack.getByRole('button', { name: 'Earlier day' })).toBeDisabled();
  await expect(stack.locator('line.now-line')).toHaveCount(1);

  await stack.getByRole('button', { name: 'Later day' }).click();
  await expect(stack.getByTestId('page-range')).toHaveText('Showing Sat, 7/11 to Fri, 7/17');
  await expect(stack.getByTestId('day-strip').locator('text.day-date').first()).toHaveText('Sat, 7/11');
  await expect(stack.getByTestId('day-strip').locator('rect.day-cell')).toHaveCount(7);
  // "now" (Jul 10, 12:00 local) is before this window
  await expect(stack.locator('line.now-line')).toHaveCount(0);
  expect(await stack.locator('text.tick-y').allTextContents()).toEqual(ticks);

  // the last page is days 8 to 14: Sat, 7/18 to Fri, 7/24, and Later is disabled there
  for (let i = 0; i < 7; i++) await stack.getByRole('button', { name: 'Later day' }).click();
  await expect(stack.getByTestId('page-range')).toHaveText('Showing Sat, 7/18 to Fri, 7/24');
  await expect(stack.getByRole('button', { name: 'Later day' })).toBeDisabled();
  expect(await stack.locator('text.tick-y').allTextContents()).toEqual(ticks);
});

test('mocked: dashboard rows keep the compact temperature-only panel: one panel, no paging, no precipitation view', async ({ page }) => {
  await setup(page);
  await page.goto('/');
  const row = page.getByTestId('forecast-row');
  await expect(row.getByTestId('forecast-stack')).toBeVisible();
  await expect(row.locator('rect.panel-frame')).toHaveCount(1);
  await expect(row.getByTestId('page-nav')).toHaveCount(0);
  await expect(row.locator('path.bar-precip')).toHaveCount(0);
});

test('mocked: hover and arrow keys read the values at an hour across the stack; daily keeps the precipitation view', async ({ page }) => {
  const hits = await setup(page);
  await page.goto(`/stations/${ID}`);
  const stack = page.getByTestId('forecast-stack');
  const slider = stack.getByTestId('panel-stack');
  await slider.focus();
  await page.keyboard.press('ArrowRight');
  await expect(slider).toHaveAttribute('aria-valuenow', '0');
  await page.keyboard.press('ArrowRight');
  await expect(slider).toHaveAttribute('aria-valuenow', '1');
  await expect(stack.locator('line.crosshair')).toHaveCount(1);
  // the second step is hour 1: 60 + 15 sin(1 / 3.8) = 63.9, shown rounded; the slider's value text names it
  await expect(slider).toHaveAttribute('aria-valuetext', /Forecast 64 F/);
  // every panel's values for the hovered hour
  await expect(slider).toHaveAttribute('aria-valuetext', /Hourly rain 0\.00 in, .*Chance of precipitation 20%/);
  await expect(slider).toHaveAttribute('aria-valuetext', /Cloud cover [^,]+, Humidity [^,]+, Pressure [\d.]+ inHg/);
  await expect(slider).toHaveAttribute('aria-valuetext', /Wind speed [^,]+, Gusts [^,]+, Wind from [NESW]{1,3}/);
  // no visible text: the readout lines are gone, and so is the pointer hint
  await expect(page.locator('.stack-readout, [data-testid="stack-readout"]')).toHaveCount(0);
  await page.keyboard.press('Escape');
  await expect(stack.locator('line.crosshair')).toHaveCount(0);

  await page.getByRole('button', { name: 'Daily (15 days)' }).click();
  const chart = page.getByTestId('forecast-chart');
  await expect(chart.locator('path.line-tmax')).toHaveCount(1);
  await expect(chart.locator('path.line-tmin')).toHaveCount(1);
  await page.getByRole('button', { name: 'Precipitation' }).click();
  await expect(chart.locator('text', { hasText: '35%' }).first()).toBeAttached();
  await expect(chart.locator('line.bar-range').first()).toBeAttached();
  await page.getByRole('button', { name: 'Temperature' }).click();
  await page.getByRole('button', { name: 'Hourly (7 days)' }).click();
  await expect(stack).toBeVisible();
  expect(hits.forecast).toBe(1);
});

test('mocked: each stack panel has its own tooltip for the hovered hour; arrows move them, a right-edge pointer flips them, Escape and leaving hide them', async ({ page }) => {
  await setup(page);
  await page.goto(`/stations/${ID}`);
  const stack = page.getByTestId('forecast-stack');
  const slider = stack.getByTestId('panel-stack');
  const panels = ['temperature', 'precipitation', 'sky', 'wind'];
  const tip = (name: string) => stack.getByTestId(`tip-${name}`);
  const tipX = async (name: string) => Number(/translate\(([-\d.]+) /.exec((await tip(name).getAttribute('transform')) ?? '')?.[1]);
  const tipY = async (name: string) => Number(/translate\([-\d.]+ ([-\d.]+)\)/.exec((await tip(name).getAttribute('transform')) ?? '')?.[1]);
  await expect(stack.locator('[data-testid^="tip-"]')).toHaveCount(0);

  // keyboard: two steps (hour 1, as in the value text test above)
  await slider.focus();
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('ArrowRight');
  for (const p of panels) await expect(tip(p)).toHaveCount(1);
  await expect(tip('temperature')).toContainText('Forecast');
  await expect(tip('temperature')).toContainText('Feels like');
  await expect(tip('temperature')).toContainText('64 F');
  await expect(tip('precipitation')).toContainText('Hourly rain');
  await expect(tip('precipitation')).toContainText('Chance');
  await expect(tip('precipitation')).toContainText('20%');
  await expect(tip('sky')).toContainText('Humidity');
  await expect(tip('sky')).toContainText('inHg');
  await expect(tip('wind')).toContainText('Wind speed');
  expect(await tip('wind').textContent()).toMatch(/Wind from[NESW]{1,3}/);
  // the slider's value text names the same hour and the same values
  await expect(slider).toHaveAttribute('aria-valuetext', /Chance of precipitation 20%/);

  // arrow keys move every box with the crosshair: the temperature box follows the value text at the new hour
  const valueTemp = async () => /Forecast (-?\d+) F/.exec((await slider.getAttribute('aria-valuetext')) ?? '')?.[1];
  const before = await tip('temperature').textContent();
  await page.keyboard.press('ArrowRight');
  await expect(slider).toHaveAttribute('aria-valuenow', '2');
  expect(await tip('temperature').textContent()).not.toBe(before);
  await expect(tip('temperature')).toContainText(`${await valueTemp()} F`);

  // a pointer in the middle of the plot puts every box to the right of the crosshair
  const crossX = async () => Number(await stack.locator('line.crosshair').getAttribute('x1'));
  await slider.hover({ position: { x: 300, y: 120 } });
  const mid = await crossX();
  for (const p of panels) expect(await tipX(p)).toBeGreaterThan(mid);

  // a pointer near the right edge of the plot (the axis labels sit in the last 192 px) flips every box to the left
  const box = (await slider.boundingBox())!;
  await slider.hover({ position: { x: box.width - 200, y: 120 } });
  const edge = await crossX();
  for (const p of panels) expect(await tipX(p)).toBeLessThan(edge);
  // every flipped box stays inside the plot (x) and inside its own panel (y)
  for (let i = 0; i < panels.length; i++) {
    const p = panels[i];
    const frame = stack.locator('rect.panel-frame').nth(i);
    const [fx, fy, fw, fh] = await Promise.all(['x', 'y', 'width', 'height'].map(async (a) => Number(await frame.getAttribute(a))));
    const bg = tip(p).locator('rect.tip-bg');
    const [w, h] = await Promise.all(['width', 'height'].map(async (a) => Number(await bg.getAttribute(a))));
    const tx = await tipX(p);
    const ty = await tipY(p);
    expect(tx).toBeGreaterThanOrEqual(fx);
    expect(tx + w).toBeLessThanOrEqual(fx + fw);
    expect(ty).toBeGreaterThanOrEqual(fy);
    expect(ty + h).toBeLessThanOrEqual(fy + fh);
  }

  // leaving the chart hides every box
  await page.mouse.move(0, 0);
  await expect(stack.locator('[data-testid^="tip-"]')).toHaveCount(0);
  await expect(stack.locator('line.crosshair')).toHaveCount(0);

  // Escape hides them too
  await slider.focus();
  await page.keyboard.press('ArrowRight');
  for (const p of panels) await expect(tip(p)).toHaveCount(1);
  await page.keyboard.press('Escape');
  await expect(stack.locator('[data-testid^="tip-"]')).toHaveCount(0);

  // after paging to the next day, the hovered hour's tooltip values match the slider's value text
  await page.getByRole('button', { name: 'Later day' }).click();
  await slider.focus();
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('ArrowRight');
  await expect(slider).toHaveAttribute('aria-valuenow', '1');
  await expect(tip('temperature')).toContainText(`${await valueTemp()} F`);
  const chance = /Chance of precipitation (\d+)%/.exec((await slider.getAttribute('aria-valuetext')) ?? '')?.[1];
  await expect(tip('precipitation')).toContainText(`${chance}%`);
});

test('mocked: daily view: a slider with value text, one tooltip per day (its high, low and ranges on Temperature; its rain on Precipitation), a date title, a right-edge flip, arrows and Escape', async ({ page }) => {
  await setup(page);
  await page.goto(`/stations/${ID}`);
  await page.getByRole('button', { name: 'Daily (15 days)' }).click();
  const chart = page.getByTestId('forecast-chart');
  const tip = page.getByTestId('tip-daily');
  // SVG text has no innerText, so read the text content
  const labels = () => tip.locator('text.tip-label').allTextContents();
  const values = () => tip.locator('text.tip-value').allTextContents();
  const tipX = async () => Number(/translate\(([-\d.]+) /.exec((await tip.getAttribute('transform')) ?? '')?.[1]);
  await expect(page.getByRole('slider', { name: 'Daily forecast chart' })).toBeVisible();
  await expect(tip).toHaveCount(0);
  // nothing is hovered: no value text (no pointer hint, no visible readout)
  await expect(chart).not.toHaveAttribute('aria-valuetext');
  await expect(page.getByTestId('daily-readout')).toHaveCount(0);

  // keyboard: the first arrow starts at the first day (Fri Jul 10: high 80 F, low 50 F, ensemble 74 to 86 and 44 to 55)
  await chart.focus();
  await page.keyboard.press('ArrowRight');
  await expect(tip.locator('text.tip-title')).toHaveText('Fri Jul 10');
  expect(await labels()).toEqual(['High', 'High p10-p90', 'Low', 'Low p10-p90', 'Median']);
  expect(await values()).toEqual(['80 F', '74 F to 86 F', '50 F', '44 F to 55 F', 'high 80 F, low 50 F']);
  await expect(chart).toHaveAttribute('aria-valuetext', 'Fri Jul 10: High 80 F, High p10-p90 74 F to 86 F, Low 50 F, Low p10-p90 44 F to 55 F, Median high 80 F, low 50 F.');
  await expect(chart).toHaveAttribute('aria-valuenow', '0');
  // each tooltip swatch is the legend's class for that series
  await expect(tip.locator('line.line-tmax')).toHaveCount(1);
  await expect(tip.locator('rect.band-tmin')).toHaveCount(1);
  await expect(tip.locator('line.line-median')).toHaveCount(1);

  await page.keyboard.press('ArrowRight');
  await expect(tip.locator('text.tip-title')).toHaveText('Sat Jul 11');
  await expect(tip.locator('text.tip-value').first()).toHaveText('79 F');
  await expect(chart).toHaveAttribute('aria-valuetext', /^Sat Jul 11: High 79 F/);

  // Precipitation: the ensemble range and median, then the chance of rain
  await page.getByRole('button', { name: 'Precipitation' }).click();
  await chart.focus();
  expect(await labels()).toEqual(['Range p10-p90', 'Median', 'Chance of rain']);
  expect(await values()).toEqual(['0.00 in to 0.30 in', '0.02 in', '35%']);
  await expect(tip.locator('rect.band-outer')).toHaveCount(1);
  await page.getByRole('button', { name: 'Temperature' }).click();
  await chart.focus();
  expect(await labels()).toEqual(['High', 'High p10-p90', 'Low', 'Low p10-p90', 'Median']);

  // a pointer at the right edge snaps to the last day, whose box would pass the plot's right edge: it flips left, inside the plot
  const width = Number(await chart.getAttribute('width'));
  const box = (await chart.boundingBox())!;
  await chart.hover({ position: { x: box.width - 4, y: 120 } });
  const cross = Number(await chart.locator('line.crosshair').getAttribute('x1'));
  const tx = await tipX();
  const tw = Number(await tip.locator('rect.tip-bg').getAttribute('width'));
  expect(tx).toBeLessThan(cross);
  expect(tx).toBeGreaterThanOrEqual(46); // the plot's left margin (M.l)
  expect(tx + tw).toBeLessThanOrEqual(width - 10); // the plot's right margin (M.r)

  // Escape clears the crosshair and the tooltip; leaving the chart does too
  await chart.focus();
  await page.keyboard.press('Escape');
  await expect(tip).toHaveCount(0);
  await expect(chart.locator('line.crosshair')).toHaveCount(0);
  await chart.hover({ position: { x: 200, y: 120 } });
  await expect(tip).toHaveCount(1);
  await page.mouse.move(0, 0);
  await expect(tip).toHaveCount(0);
});

// The changes grid (proposal 09): one panel per target day, titled by day. The fixture is made on 2026-07-12 (its
// generated_at), so Jul 10 is two days ago, Jul 11 yesterday, Jul 12 today, and Jul 13 to 16 are ahead.
const DATES = ['2026-07-10', '2026-07-11', '2026-07-12', '2026-07-13', '2026-07-14', '2026-07-15', '2026-07-16'];
const FACET_TITLES = ['Fri 10', 'Yesterday', 'Today', 'Tomorrow', 'Tue 14', 'Wed 15', 'Thu 16'];
const facet = (page: Page, date: string) => page.getByTestId(`facet-${date}`);

// The grid's shape from the panels' offsets: columns are the panels in the first row (top edge at y 0), rows are the
// distinct top edges. At the 1400 px project viewport the seven panels are one row.
async function gridShape(page: Page): Promise<{ cols: number; rows: number }> {
  const transforms = await page.locator('[data-testid^="facet-"]').evaluateAll((gs) => gs.map((g) => g.getAttribute('transform') ?? ''));
  const top = (t: string) => t.slice(t.lastIndexOf(' ') + 1, -1);
  return { cols: transforms.filter((t) => top(t) === '0').length, rows: new Set(transforms.map(top)).size };
}

test('mocked: changes grid has one panel per target day, titled by day, with the y labels in its first column only, and not-issued leads blank', async ({ page }) => {
  await setup(page);
  await page.goto(`/stations/${ID}`);
  const chart = page.getByTestId('changes-chart');
  await expect(chart).toBeVisible();
  await expect(page.locator('[data-testid^="facet-"]')).toHaveCount(7);
  for (let i = 0; i < DATES.length; i++) {
    await expect(facet(page, DATES[i]).locator('text.facet-title')).toHaveText(FACET_TITLES[i]);
    await expect(facet(page, DATES[i]).locator('text', { hasText: /^7d$/ })).toHaveCount(1);
    await expect(facet(page, DATES[i]).locator('text', { hasText: /^now$/ })).toHaveCount(1);
  }
  // the y labels are drawn only by the first column (the shared scale is labelled once per row)
  const panels = await page.locator('[data-testid^="facet-"]').evaluateAll((gs) =>
    gs.map((g) => ({ at: g.getAttribute('transform') ?? '', labels: g.querySelectorAll('text.tick-y').length }))
  );
  expect(panels.map((p) => p.labels > 0)).toEqual(panels.map((p) => p.at.startsWith('translate(0 ')));
  // a day k days ahead has k not-issued leads (1 to k), drawn as one shaded run; past and today have none
  for (let i = 0; i < DATES.length; i++) {
    await expect(facet(page, DATES[i]).locator('rect.facet-unissued')).toHaveCount(i > 2 ? 1 : 0);
  }
  // the table agrees: Jul 13 (one day ahead) has a dash at 1d, and Jul 16 (four days ahead) has dashes at 4d to 1d
  const table = page.getByTestId('changes-table');
  await expect(table.locator('thead th')).toHaveText(['Day', '7d', '6d', '5d', '4d', '3d', '2d', '1d', 'now', 'Range']);
  const rows = table.locator('tbody tr');
  await expect(rows).toHaveCount(7);
  await expect(rows.nth(3).locator('td')).toHaveText(['Jul 13', '0.10', '0.05', '0.00', '0.15', '0.10', '0.05', '–', '0.15', '0.00–0.15']);
  await expect(rows.nth(6).locator('td')).toHaveText(['Jul 16', '0.05', '0.00', '0.15', '–', '–', '–', '–', '0.10', '0.00–0.15']);
  // Jul 12's 4d lead is null in the API (no value), so it is a plain dash and not a not-issued dash
  await expect(rows.nth(2).locator('td').nth(4)).toHaveText('-');
});

test('mocked: a pointer over a panel shows one tooltip there (its day, the lead and now), a crosshair in every panel, and the same text as the value text', async ({ page }) => {
  await setup(page);
  await page.goto(`/stations/${ID}`);
  const chart = page.getByTestId('changes-chart');
  const tip = page.getByTestId('tip-changes');
  await expect(tip).toHaveCount(0);

  await chart.scrollIntoViewIfNeeded();
  const f = (await facet(page, '2026-07-13').boundingBox())!;
  await page.mouse.move(f.x + f.width * 0.5, f.y + 90);
  await expect(tip).toHaveCount(1);
  await expect(tip.locator('text.tip-title')).toHaveText('Tomorrow');
  // terse rows: the lead as "Nd before", then "now"; no sentences
  const labels = await tip.locator('text.tip-label').allTextContents();
  expect(labels).toHaveLength(2);
  expect(labels[0]).toMatch(/^\d+d before$/);
  expect(labels[1]).toBe('now');
  // the tooltip stays inside the hovered panel
  const tb = (await tip.boundingBox())!;
  expect(tb.x).toBeGreaterThanOrEqual(f.x - 0.5);
  expect(tb.x + tb.width).toBeLessThanOrEqual(f.x + f.width + 0.5);
  // one crosshair tick per panel, at the same lead
  await expect(chart.locator('line.crosshair')).toHaveCount(7);
  await expect(chart).toHaveAttribute('aria-valuetext', new RegExp(`^Tomorrow: ${labels[0]} `));

  // leaving the grid hides the tooltip, the crosshairs and the value text
  await page.mouse.move(0, 0);
  await expect(tip).toHaveCount(0);
  await expect(chart.locator('line.crosshair')).toHaveCount(0);
  await expect(chart).not.toHaveAttribute('aria-valuetext');
});

test('mocked: arrow keys move the lead, Up and Down move the panel a row, PageUp and PageDown a panel, Escape clears; a not-issued lead shows a dash', async ({ page }) => {
  await setup(page);
  await page.goto(`/stations/${ID}`);
  const chart = page.getByTestId('changes-chart');
  const tip = page.getByTestId('tip-changes');
  await chart.focus();
  await expect(chart).toHaveAttribute('aria-valuemin', '0');
  await expect(chart).toHaveAttribute('aria-valuemax', '7');

  // the first key press starts at the first panel and the oldest lead (7 days before): Fri 10 rain 0.15 in, now 0.00 in
  await page.keyboard.press('ArrowRight');
  await expect(tip.locator('text.tip-title')).toHaveText('Fri 10');
  expect(await tip.locator('text.tip-label').allTextContents()).toEqual(['7d before', 'now']);
  expect(await tip.locator('text.tip-value').allTextContents()).toEqual(['0.15 in', '0.00 in']);
  await expect(chart).toHaveAttribute('aria-valuenow', '0');
  await expect(chart).toHaveAttribute('aria-valuetext', 'Fri 10: 7d before 0.15 in, now 0.00 in.');

  // Right moves to the next lead (6 days before: 0.10 in); Left goes back
  await page.keyboard.press('ArrowRight');
  expect(await tip.locator('text.tip-label').allTextContents()).toEqual(['6d before', 'now']);
  expect(await tip.locator('text.tip-value').allTextContents()).toEqual(['0.10 in', '0.00 in']);
  await expect(chart).toHaveAttribute('aria-valuenow', '1');
  await page.keyboard.press('ArrowLeft');
  await expect(chart).toHaveAttribute('aria-valuenow', '0');

  // Down moves one row of the grid (cols panels on), or one panel when the grid is a single row; PageDown and PageUp
  // move one panel. Seven panels: the last index is 6, so every step is clamped to it.
  const { cols, rows } = await gridShape(page);
  const down = rows === 1 ? 1 : cols;
  await page.keyboard.press('ArrowDown');
  await expect(tip.locator('text.tip-title')).toHaveText(FACET_TITLES[down]);
  await page.keyboard.press('PageDown');
  await expect(tip.locator('text.tip-title')).toHaveText(FACET_TITLES[Math.min(down + 1, 6)]);
  await page.keyboard.press('PageUp');
  await expect(tip.locator('text.tip-title')).toHaveText(FACET_TITLES[down]);

  // Escape clears the tooltip, the crosshairs and the value text
  await page.keyboard.press('Escape');
  await expect(tip).toHaveCount(0);
  await expect(chart.locator('line.crosshair')).toHaveCount(0);
  await expect(chart).not.toHaveAttribute('aria-valuetext');
  await expect(chart).not.toHaveAttribute('aria-valuenow');

  // Tomorrow is one day ahead, so its 1d lead is not issued: a dash, not the copied value (0.00 in)
  for (const k of ['ArrowRight', 'PageDown', 'PageDown', 'PageDown']) await page.keyboard.press(k);
  await expect(tip.locator('text.tip-title')).toHaveText('Tomorrow');
  for (let i = 0; i < 6; i++) await page.keyboard.press('ArrowRight');
  expect(await tip.locator('text.tip-label').allTextContents()).toEqual(['1d before', 'now']);
  expect(await tip.locator('text.tip-value').allTextContents()).toEqual(['–', '0.15 in']);
  await expect(chart).toHaveAttribute('aria-valuetext', 'Tomorrow: 1d before –, now 0.15 in.');
});

test('mocked: a tooltip near the right edge of its panel flips inside it', async ({ page }) => {
  await setup(page);
  await page.goto(`/stations/${ID}`);
  const tip = page.getByTestId('tip-changes');
  await page.getByTestId('changes-chart').scrollIntoViewIfNeeded();
  const f = (await facet(page, '2026-07-14').boundingBox())!;
  await page.mouse.move(f.x + f.width - 4, f.y + 90);
  await expect(tip).toHaveCount(1);
  const tb = (await tip.boundingBox())!;
  expect(tb.x + tb.width).toBeLessThanOrEqual(f.x + f.width + 0.5);
  expect(tb.x).toBeGreaterThanOrEqual(f.x - 0.5);
  await page.keyboard.press('Escape');
  await expect(tip).toHaveCount(0);
});

test('mocked: ensemble null hides ribbons; stale shows the saved-at line', async ({ page }) => {
  await setup(page, { forecast: fx.forecast({ ensemble: null, stale: true }) });
  await page.goto(`/stations/${ID}`);
  await expect(page.getByTestId('no-ensemble')).toHaveText('Uncertainty range unavailable right now.');
  await expect(page.getByTestId('forecast-stack').locator('path.band-outer')).toHaveCount(0);
  await expect(page.getByTestId('stale-note')).toContainText('Showing the forecast saved at');
  await expect(page.getByTestId('stale-note')).toContainText('; refreshing failed.');
});

test('mocked: change view requests today..today+6, draws a panel per day with its legend, table, attribution and model', async ({ page }) => {
  const hits = await setup(page);
  await page.goto(`/stations/${ID}`);
  const chart = page.getByTestId('changes-chart');
  await expect(chart).toBeVisible();
  expect(hits.changes[0].searchParams.get('from')).toBe('2026-07-10');
  expect(hits.changes[0].searchParams.get('to')).toBe('2026-07-16');
  // the legend names the three marks the grid draws
  const legend = page.getByTestId('legend-changes');
  await expect(legend.locator('text.legend-label')).toHaveText(['Forecast', 'Latest', 'Not issued']);
  // no legend label runs past the plot's right edge
  const plotRight = (await chart.boundingBox())!.x + (await chart.boundingBox())!.width;
  for (const t of await legend.locator('text.legend-label').all()) {
    expect((await t.boundingBox())!.x + (await t.boundingBox())!.width).toBeLessThanOrEqual(plotRight + 0.5);
  }
  // the caption names the selected metric; rain draws one bar per issued lead, with the null lead (4 days before) left empty
  const caption = page.locator('figure', { has: chart }).locator('figcaption strong');
  await expect(caption).toHaveText('Rain');
  await expect(facet(page, '2026-07-12').locator('rect.bar-precip')).toHaveCount(7);
  // the table: one row per day, the range column, and the not-issued and null blanks
  const table = page.getByTestId('changes-table');
  await expect(table.locator('tbody tr')).toHaveCount(7);
  await expect(table.locator('tbody tr').first().locator('td').last()).toHaveText('0.00–0.15');
  await expect(table.locator('tbody tr').nth(2).locator('td').nth(4)).toHaveText('-');
  // High draws one solid line per panel, and its latest level is a dashed line of another dash
  await page.getByRole('button', { name: 'High', exact: true }).click();
  await expect(caption).toHaveText('High');
  await expect(chart.locator('path.line-facet')).toHaveCount(7);
  const dashes = await Promise.all([
    facet(page, '2026-07-12').locator('line.ref-latest').evaluate((el) => getComputedStyle(el).strokeDasharray),
    facet(page, '2026-07-12').locator('path.line-facet').evaluate((el) => getComputedStyle(el).strokeDasharray)
  ]);
  expect(dashes[0]).not.toBe('none');
  expect(dashes[1]).toBe('none');
  const captions = await page.getByTestId('provenance').allInnerTexts();
  expect(captions.join('\n')).toContain('Weather data by Open-Meteo.com (CC BY 4.0)');
  expect(captions.join('\n')).toContain('gfs_seamless');
  // footer attribution too
  await expect(page.getByRole('link', { name: 'Open-Meteo.com' })).toHaveAttribute('href', 'https://open-meteo.com/');
});

test('mocked: a span over 14 days shows "Pick up to 14 days." without a request, and a 422 shows the same text', async ({ page }) => {
  const hits = await setup(page);
  await page.goto(`/stations/${ID}`);
  await expect(page.getByTestId('changes-chart')).toBeVisible();
  const n = hits.changes.length;
  await page.getByLabel('To').fill('2026-07-28');
  await expect(page.getByText('Pick up to 14 days.')).toBeVisible();
  expect(hits.changes.length).toBe(n);

  const page2 = await page.context().newPage();
  const hits2 = await setup(page2, { changesStatus: 422, changes: { error: { code: 'validation_failed', message: 'x' } } });
  await page2.goto(`/stations/${ID}`);
  await expect(page2.getByText('Pick up to 14 days.')).toBeVisible();
  expect(hits2.changes.length).toBeGreaterThan(0);
});

test('mocked: span boundary is inclusive: 14 days is requested, 15 days is refused without a request', async ({ page }) => {
  const hits = await setup(page);
  await page.goto(`/stations/${ID}`);
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
  await page.goto(`/stations/${ID}`);
  await page.getByLabel('From').fill('2026-07-03');
  await page.getByLabel('To').fill('2026-07-09');
  await expect(page.getByText('Chance of rain is not available before October 2024.')).toBeVisible();
});

test('mocked: the browser only talks to the page origin', async ({ page }) => {
  const hits = await setup(page);
  await page.goto(`/stations/${ID}`);
  await expect(page.getByTestId('changes-chart')).toBeVisible();
  expect([...hits.origins].filter((o) => o !== 'http://localhost:8000')).toEqual([]);
});

for (const [name, status, code, text] of [
  ['502', 502, 'upstream_unavailable', 'The weather service is unavailable. Try again later.'],
  ['503', 503, 'upstream_busy', 'The weather service is busy. Try again in a moment.']
] as const) {
  test(`mocked: ${name} shows the error panel with Retry`, async ({ page }) => {
    await setup(page, { forecastStatus: status, forecast: { error: { code, message: 'x' } } });
    await page.goto(`/stations/${ID}`);
    await expect(page.getByRole('alert').getByText(text)).toBeVisible();
    await expect(page.getByRole('button', { name: 'Retry' }).first()).toBeVisible();
  });
}

test('mocked: 404 shows "That isn\'t available."', async ({ page }) => {
  await fakeSession(page);
  await page.route('**/api/v1/me', (r) => json(r, fx.me()));
  await page.route(`**/api/v1/stations/${ID}`, (r) => json(r, { error: { code: 'not_found', message: 'x' } }, 404));
  await page.goto(`/stations/${ID}`);
  await expect(page.getByText("That isn't available.")).toBeVisible();
});

test.describe('real smoke', () => {
  test.use({ storageState: `${AUTH_DIR}/alex.json` });
  test('both sections render for a seeded station', async ({ page }) => {
    await page.goto('/');
    await page.waitForSelector('[data-testid="forecast-row"]');
    const stations = await apiGet(page, '/api/v1/stations');
    await page.goto(`/stations/${stations.body.stations[0].id}`);
    await expect(page.getByTestId('forecast-stack')).toBeVisible();
    await expect(page.getByTestId('changes-table').locator('tbody tr').first()).toBeVisible();
  });
});
