import { execFileSync } from 'node:child_process';
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { expect, test } from '@playwright/test';
import { AUTH_DIR, REPO_ROOT } from '../helpers/env';
import { apiGet } from '../helpers/api';
import { watchCsp } from '../helpers/csp';

const cfg = JSON.parse(readFileSync(join(REPO_ROOT, 'web', 'security-headers.json'), 'utf8'));
const localCsp = Object.entries(cfg.csp as Record<string, string[]>)
  .map(([k, v]) => [k, ...v].join(' '))
  .join('; ');

test('no CSP violations on signed-out and signed-in pages, including client navigation', async ({ browser }) => {
  const anon = await browser.newContext();
  const ap = await anon.newPage();
  const aw = await watchCsp(ap);
  await ap.goto('/login');
  await expect(ap.getByRole('heading', { name: 'Sign in' })).toBeVisible();
  await aw.expectClean('/login (signed out)');
  await anon.close();

  const ctx = await browser.newContext({ storageState: `${AUTH_DIR}/alex.json` });
  const page = await ctx.newPage();
  const w = await watchCsp(page);
  await page.goto('/');
  await expect(page.getByTestId('forecast-row').first()).toBeVisible();
  await w.expectClean('/ (forecast tab)');
  await page.getByRole('button', { name: 'Rain', exact: true }).click();
  await page.waitForSelector('[data-testid="station-ytd"]');
  await w.expectClean('/');
  const stations = await apiGet(page, '/api/v1/stations');
  const sid = stations.body.stations[0].id as string;

  // season and metric toggles, cursor moves
  await page.getByRole('button', { name: 'Full year' }).click();
  await page.getByRole('button', { name: 'GDD', exact: true }).click();
  await page.getByRole('button', { name: 'Rain', exact: true }).click();
  // the plot, not the legend row above it (proposal 08 phase 2): 200 x 100 px is inside the plot's 280 px height
  const svg = page.locator('svg.chart-svg:not(.legend-row)').first();
  await svg.hover({ position: { x: 200, y: 100 } });
  await svg.press('ArrowRight');
  await expect(page.getByTestId('readout').first()).not.toHaveText('');
  await w.expectClean('toggles and cursor');

  // client-side navigation creates the route announcer (inline style attribute)
  await page.getByRole('link', { name: 'Follows' }).click();
  await expect(page.getByRole('heading', { name: 'Follows' })).toBeVisible();
  await w.expectClean('client nav to /follows');
  await page.getByRole('link', { name: 'Connect station' }).click();
  await expect(page.getByRole('heading', { name: 'Connect your weather station' })).toBeVisible();
  await w.expectClean('client nav to /connect');
  await page.getByRole('link', { name: 'Dashboard' }).click();
  await expect(page.getByTestId('forecast-row').first()).toBeVisible();
  // the compact row's temperature tooltip (proposal 08 phase 4) is covered by the listener too
  await page.getByTestId('forecast-row').first().getByTestId('panel-stack').hover({ position: { x: 300, y: 60 } });
  await expect(page.getByTestId('forecast-row').first().getByTestId('tip-temperature')).toBeVisible();
  await w.expectClean('compact row tooltip');
  await page.getByRole('button', { name: 'Rain', exact: true }).click();
  await page.waitForSelector('[data-testid="station-ytd"]');
  expect(await page.locator('#svelte-announcer').count()).toBe(1);
  await w.expectClean('client nav back to /');

  // the stack's per-panel tooltips draw on hover; the CSP listener covers them too (proposal 08 phase 3)
  await page.goto(`/stations/${sid}`);
  await page.getByTestId('panel-stack').waitFor();
  await page.getByTestId('panel-stack').hover({ position: { x: 300, y: 120 } });
  await expect(page.locator('[data-testid="tip-temperature"]')).toBeVisible();
  await w.expectClean('stack tooltips');

  // the daily and changes tooltips (proposal 08 phase 4) are covered by the same listener
  await page.getByRole('button', { name: 'Daily (15 days)' }).click();
  await page.getByTestId('forecast-chart').hover({ position: { x: 300, y: 120 } });
  await expect(page.getByTestId('tip-daily')).toBeVisible();
  await w.expectClean('daily tooltip');
  await page.getByTestId('changes-chart').hover({ position: { x: 300, y: 120 } });
  await expect(page.getByTestId('tip-changes')).toBeVisible();
  await w.expectClean('changes tooltip');

  // every full document load is checked before the next one replaces it
  for (const p of [`/stations/${sid}`, '/follows', '/connect']) {
    await page.goto(p);
    await page.waitForLoadState('networkidle');
    await w.expectClean(p);
  }
  // the retired forecast route redirects (client side) to the station page's forecast section
  await page.goto(`/stations/${sid}/forecast`);
  await expect(page).toHaveURL(new RegExp(`/stations/${sid}#forecast$`));
  await page.waitForLoadState('networkidle');
  await w.expectClean(`/stations/${sid}/forecast (redirect)`);
  await ctx.close();
});

test('negative controls: inline script, style element, foreign style attribute and a foreign image are blocked', async ({ page }) => {
  const w = await watchCsp(page);
  await page.goto('/login');
  await page.addScriptTag({ content: 'window.__x = 1' }).catch(() => {});
  expect(await page.evaluate(() => (window as unknown as { __x?: number }).__x)).toBeUndefined();
  await page.addStyleTag({ content: 'body{display:none}' }).catch(() => {});
  expect(await page.evaluate(() => getComputedStyle(document.body).display)).not.toBe('none');
  await page.evaluate(() => document.body.setAttribute('style', 'display:none'));
  await page.evaluate(() => {
    const i = document.createElement('img');
    i.src = 'https://example.com/x.png';
    document.body.appendChild(i);
  });
  await expect.poll(async () => (await w.violations()).join('|')).toContain('img-src');
  const v = (await w.violations()).join('|');
  expect(v).toContain('script-src');
  expect(v).toContain('style-src-elem');
  expect(v).toContain('style-src-attr');
});

test('headers on HTML and assets match security-headers.json; API responses carry their own', async ({ request }) => {
  const asset = readdirSync(join(REPO_ROOT, 'web', 'build', '_app', 'immutable', 'entry')).find((f) => f.endsWith('.js'))!;
  for (const path of ['/', '/200.html', '/stations/anything']) {
    const res = await request.get(path);
    expect(res.status()).toBe(200);
    const h = res.headers();
    expect(h['content-security-policy']).toBe(localCsp);
    for (const [k, v] of Object.entries(cfg.headers_all as Record<string, string>)) expect(h[k.toLowerCase()], k).toBe(v);
    expect(h['strict-transport-security']).toBeUndefined();
    expect(h['server']).toBeUndefined();
    expect(h['cache-control']).toBe(cfg.expected_html_cache_control.caddy);
    expect(await res.text()).toContain('sveltekit');
  }
  const res = await request.get(`/_app/immutable/entry/${asset}`);
  expect(res.status()).toBe(200);
  expect(res.headers()['cache-control']).toBe(cfg.cache_control['/_app/immutable/*']);
  expect(res.headers()['content-security-policy']).toBe(localCsp);

  const api = await request.get('/api/v1/me');
  expect(api.status()).toBe(401);
  expect(api.headers()['cache-control']).toBe('no-store');
  expect(api.headers()['x-content-type-options']).toBe('nosniff');
  expect(api.headers()['content-security-policy']).toBeUndefined();
  expect(api.headers()['x-frame-options']).toBeUndefined();
});

test('missing chunks 404 instead of falling back to the SPA page; version.txt is plain text', async ({ request }) => {
  for (const p of ['/_app/immutable/missing.js', '/_app/nope.json']) expect((await request.get(p)).status(), p).toBe(404);
  expect((await request.get('/_app/version.json')).status()).toBe(200);
  const v = await request.get('/version.txt');
  expect(v.status()).toBe(200);
  expect(v.headers()['content-type']).toContain('text/plain');
  expect(v.headers()['cache-control'] ?? '').not.toContain('immutable');
  expect(await v.text()).toMatch(/^([0-9a-f]{40}|local)$/);
});

test('headers parity check passes', () => {
  execFileSync('node', ['scripts/headers.mjs', 'check'], { cwd: join(REPO_ROOT, 'web'), stdio: 'pipe' });
  execFileSync('node', ['scripts/check-build.mjs'], { cwd: join(REPO_ROOT, 'web'), stdio: 'pipe' });
});
