# 01 - Web (SvelteKit static SPA)

Audience: AI agents implementing and testing `web/`. Shared names, ports and env vars are defined in `00-conventions.md`; that file wins on any conflict. `02-api.md` is the authority for the HTTP contract. Decision IDs (S2, P2, ...) refer to `docs/Design Review Findings.md`.

Sources (verified = fetched 2026-10-02):
- SvelteKit SPA mode / adapter-static fallback (verified): https://svelte.dev/docs/kit/single-page-apps
- SvelteKit `kit.csp` (verified; prerendered/fallback pages use a `<meta>` CSP with hashes): https://svelte.dev/docs/kit/configuration#csp
- SvelteKit announcer inline style, known CSP issue (not fetched; confirm at implementation): https://github.com/sveltejs/kit/issues/11747 and https://github.com/sveltejs/kit/issues/11993
- Supabase `signInWithOtp` (verified): https://supabase.com/docs/reference/javascript/auth-signinwithotp
- Netlify `[[headers]]` (verified; custom headers do not apply to proxied content): https://docs.netlify.com/manage/routing/headers/
- Netlify redirects, rewrites and signed proxies (not fetched; confirm `force`, ordering, `signed`): https://docs.netlify.com/manage/routing/redirects/redirect-options/ , https://docs.netlify.com/manage/routing/redirects/rewrites-proxies/
- Supabase `signOut` scopes, PKCE flow (not fetched; confirm): https://supabase.com/docs/reference/javascript/auth-signout , https://supabase.com/docs/guides/auth/sessions/pkce-flow
- Mailpit REST API (not fetched; same calls as 05's smoke script): https://mailpit.axllent.org/docs/api-v1/
- Playwright authentication / `storageState` (not fetched): https://playwright.dev/docs/auth

---

## 1. Purpose and scope

`web/` is a SvelteKit app built with `@sveltejs/adapter-static`, `ssr = false`, SPA fallback `200.html`. It is login-only and renders everything from `/api/v1/*` JSON in the browser. The audience is about five family users on desktop; keep everything lean.

Owns:
- Supabase Auth client usage in the browser (magic link, session, sign-out).
- One typed API client module that attaches the Bearer token and maps errors.
- Views: login, dashboard (side-by-side stations), station detail (rain, GDD), forecast-change history, follows, connect-station flow, empty/error/inactive states.
- Chart components and the small transforms behind them.
- `web/security-headers.json` (single source of truth for headers/CSP), `web/netlify.toml`, header parity checks.
- Web tests: Vitest, Playwright, CSP-violation test, bundle-secrets test.

Does not own (reference these files):
- Endpoints, auth checks, error codes, rate limits: `02-api.md`.
- GDD computation, ingest, reference data, forecast snapshots: `03-poller.md`.
- Schema, Supabase project auth settings (sign-ups off, redirect allow-list, email templates), seed data: `04-database.md`.
- Docker Compose, `ops/caddy/Caddyfile`, `make` targets, how `web/build` is produced and served locally: `05-local-dev.md`.
- Netlify site settings (base dir, production branch `production`, PR deploy previews on / branch deploys None, env vars), Fly URL, CI: `06-deploy-ops.md`. `netlify.toml` (owned here) carries the `[build] ignore` command that skips previews for `dependabot/*` branches.

## 2. Decisions applied

| ID | How it lands in the web guide |
| --- | --- |
| S1 | Web only forwards `session.access_token`; the anon key is never sent to `/api/*`. Bad-token tests live in 02; web tests cover the UI reaction to 401. |
| S2 | Supabase JS holds the session in browser storage; Bearer header on every API call. Compensating controls: strict CSP (4.8), no third-party scripts/fonts/CDNs, pinned dependencies and lockfile, no `{@html}`. No cookies. |
| S3 | Token on every route. The `/api/*` redirect carries a commented-out `signed = "NETLIFY_PROXY_SECRET"` line, enabled later together with the API secret (conventions "Web headers and proxy"). |
| S4 | `signInWithOtp` uses `shouldCreateUser: false`; every outcome except a network failure shows the same message (including GoTrue 429, which only happens for invited emails). |
| S9 | Connect flow shows only generic valid/invalid; no URL/host input; handles 429/503/502. |
| S10 | Only IDs from API responses are used, each validated as a UUID before use in a path; hidden or unknown stations render a generic "isn't available" state. |
| S11 | 403 `account_inactive` signs the user out locally and shows an inactive screen. |
| S14 | Pinned dependencies, lockfile, `npm ci`, install scripts disabled; only `PUBLIC_SUPABASE_URL` and `PUBLIC_SUPABASE_ANON_KEY` in the bundle; bundle-secrets test. |
| S15 | Headers and CSP from `security-headers.json` (4.8). |
| P1, P2, D3, D4 | Static adapter; one build served by Netlify and Caddy; Netlify proxies `/api/*`. |
| P5 | Full follow UI (pending/approved/declined/revoked). |
| P6 | Connect flow and percentile bands are v1. Forecast-vs-gauge is post-v1 (not built). |
| F5, F6 | Provenance caption on every chart; rain default April-October; winter flagged on rain charts. |
| G5 | Empty state until a station is connected or a follow is approved. |
| Design doc functional requirements | Forecast-change history is a v1 requirement and is built (4.7, flag default on). Open-Meteo CC BY 4.0 credit in the footer and on forecast charts. |

## 3. Cross-component contracts

### 3.1 Consumed from `00-conventions.md`

- Single origin: browser talks to `/api/*` (proxied) and to Supabase Auth. Locally the Supabase URL is `http://localhost:8000`; production: the Supabase project URL.
- Build-time env: only `PUBLIC_SUPABASE_URL` and `PUBLIC_SUPABASE_ANON_KEY`, passed as process env.
- Shared error codes: `unauthorized` (401), `account_inactive` (403), `forbidden` (403), `not_found` (404; "doesn't exist" and "not viewable" are identical), `rate_limited` (429 with `Retry-After`), `validation_failed` (422). Codes from 02: `invalid_key`, `unknown_device`, `timezone_required` (422), `device_already_registered`, `conflict` (409), `upstream_busy` (503), `upstream_unavailable` (502), `request_timeout` (408), `payload_too_large` (413), `overloaded`, `auth_unavailable` (503). The web keys only on these, never on other strings.
- Every API response sets `Cache-Control: no-store` and `X-Content-Type-Options: nosniff` itself (the e2e header test asserts it).
- GDD is computed by the poller and stored with `gdd_method`; the API refuses to overlay differing methods; the web never computes GDD.
- Forecast snapshots: only variable is `precip_6h_in` (inches), `prob_precip` is a 0-100 column, `valid_time` is the UTC start of a 6-hour window.
- Redirect allow-list entries are exact callback URLs (`http://localhost:8000/auth/callback`, `http://localhost:5173/auth/callback`, `https://<site>/auth/callback`) plus, in production, the one decided preview pattern `https://deploy-preview-*--<site>.netlify.app/auth/callback`: the web never adds a query string or fragment to `emailRedirectTo`.
- Caddy (05 section 5.3) mounts `./web`, serves `/srv/webroot/build` with SPA fallback to `200.html` (including `GET /`; `/_app/*` is excluded from the fallback so missing chunks 404), `/api/*` -> `api:8080`, `handle_path /auth/v1/*` -> `auth:9999`, and `import`s the generated `ops/caddy/security-headers.caddy`.
- Web deploys are gated (proposals/04, accepted): Netlify's production branch is `production`; `main` builds nothing; 06's `deploy.yml` fast-forwards `production` after the API deploy succeeds, and Netlify then builds and publishes it. The build writes `build/version.txt` (4.9) so the deploy can confirm the live site is the released commit.
- Demo values: `backend/demo-fingerprints.txt` has lines `<sha256-hex>  <NAME>` (e.g. `PUBLIC_SUPABASE_ANON_KEY`); the production web build hashes its candidate values against column 1 (6.4).
- Fixtures (03/04): replay keys `fixture-valid-key` (devices `AA:BB:CC:00:01:01` and `AA:BB:CC:00:01:02`, not seeded, with coordinates and tz), `fixture-invalid-key`, `fixture-ratelimited-key`. Seeded stations `AA:BB:CC:00:00:01..03`. Seeded users and their mutual follows: see 04's seed (this guide's e2e finds users by display name, 6.3).

### 3.2 HTTP contract consumed (copy of 02 section 5; update here when 02 changes)

All routes need `Authorization: Bearer <access token>`. Dates `YYYY-MM-DD`; timestamps RFC 3339 UTC; temperatures F, rain inches. Types in `src/lib/api/types.ts`; one function per endpoint in `endpoints.ts`.

| Method and path | Request | Response |
| --- | --- | --- |
| `GET /api/v1/me` | - | `{ id; display_name; is_admin; credential: { status: "ok"\|"invalid"\|"rate_limited" } \| null }` |
| `GET /api/v1/stations` | - | `{ stations: Station[] }` (may be empty) |
| `GET /api/v1/stations/{station_id}` | - | `Station` |
| `GET /api/v1/ytd` | `station_ids` (comma-separated, 1..5 UUIDs), `year` (REQUIRED, 2000..current), `start`, `end` (`MM-DD`) | `YtdResponse` |
| `GET /api/v1/stations/{station_id}/forecast/history` | `variable` (required, `precip_6h_in`), `model` (optional), `valid_from`, `valid_to` (RFC 3339, span <= 16 days), `runs` (1..12, default 6) | `ForecastHistory` |
| `GET /api/v1/follows` | - | `{ following: FollowRow[]; followers: FollowRow[] }` |
| `POST /api/v1/follows` | `{ followee_display_name }` | 201/200 `{ user; status }` (not used in v1 UI, Q3) |
| `POST /api/v1/follows/{follower_id}/approve` \| `decline` \| `revoke` | empty | `FollowRow`; `{follower_id}` is the other user's `user.id` (there is no follow id); zero-row transitions are 404 `not_found` |
| `POST /api/v1/connect/validate` | `{ api_key }` | 200 `{ valid: true; devices: { mac; name; timezone: string \| null }[] }` or `{ valid: false }` |
| `POST /api/v1/connect/save` | `{ api_key; devices: { mac; name?; timezone? }[] }` (1..10; `timezone` is only a fallback when AWN gave none) | 201 `{ credential: { status: "ok" }; stations: Station[]; not_covered: { id: string; name: string }[] }` (`not_covered` = the caller's stations that stayed on the replaced credential; empty on first connect) |
| `GET /api/v1/connect/status` | - | `{ credential: null \| { status; last_ok_at: string \| null; needs_reconnecting: boolean } }` |

Not consumed in v1: `rollups/daily`, `observations`.

```ts
interface Baseline { reference_station_id: string; name: string; source: string }   // source e.g. "ghcnd_acis"
interface Station {
  id: string; name: string; is_own: boolean;
  owner: { id: string; display_name: string };
  lat: number; lon: number; coordinates_rounded: boolean;
  timezone: string;                          // IANA
  baseline: Baseline | null;
  latest_observation_at: string | null;
}
interface MdValue { md: string; v: number }               // md = "MM-DD"
interface GddOverlay { available: boolean; reason?: 'no_baseline' | 'gdd_method_mismatch' | string; station_method: string | null; baseline_method: string | null }
interface PerSeries<T> { rain: T; gdd: T }
interface YtdStation {
  station_id: string;
  baseline: Baseline | null;
  gdd_overlay: GddOverlay;
  current: { first_date: string | null; last_date: string | null; last_complete_date: string | null;
             missing_days: PerSeries<number>;
             rain_cum_in: MdValue[]; gdd_cum: MdValue[] };
  prior_years: { year: number; missing_days: PerSeries<number>;
                 rain_cum_in: MdValue[] | null; gdd_cum: MdValue[] | null }[];     // null per series when that year is ineligible
  bands: { percentiles: number[]; years_included: PerSeries<number>;
           rain_cum_in: { md: string; p: number[] }[] | null;                    // null when < 10 eligible years for that series
           gdd_cum: { md: string; p: number[] }[] | null } | null;
}
interface YtdResponse { year: number; start: string; end: string; day_definition: string; stations: YtdStation[] }
interface FollowRow { user: { id: string; display_name: string }; status: "pending"|"approved"|"declined"|"revoked"; requested_at: string; decided_at: string | null }
interface ForecastHistory {
  station_id: string; variable: string; model: string; period_hours: number; attribution: string;
  runs: { run_time: string; fetched_at: string;
          points: { valid_time: string; p10: number | null; p50: number | null; p90: number | null; prob_precip: number | null }[] }[];   // newest run first
}
```

Contract facts the UI depends on:
- Cumulative sums, percentile bands, prior-year selection and eligibility are computed by the API; the web neither computes GDD, percentiles nor cumulative sums. Everything per-series is a `rain`/`gdd` pair or an array nullable per series: `current.missing_days` and `prior_years[].missing_days` are `{rain, gdd}`; `bands.years_included` is `{rain, gdd}`; `prior_years[].rain_cum_in`/`gdd_cum` are `null` for a year ineligible for that series (missing days > 7); `bands.rain_cum_in`/`gdd_cum` are each `null` when fewer than 10 years are eligible for that series; `bands` is `null` when both are. Candidate years include years later than `year` when `year` is historical (the key name `prior_years` is kept); the 5 most recent eligible years are returned, newest first.
- `gdd_overlay.available === false`: reason `gdd_method_mismatch` keeps `current.gdd_cum` but sets `prior_years[].gdd_cum` and `bands.gdd_cum` to `null` (rain unaffected, 200); reason `no_baseline` means `baseline: null`, `prior_years: []`, `bands: null`. Any other reason is treated generically (4.7).
- `current` starts at the station's first rollup date on or after `start` (`first_date`; earlier days are neither points nor missing, so a station installed mid-season has a partial total) and has points through `last_date` (the latest row in the window). `last_complete_date` marks the last complete day: a point after it (the open day) is a partial value. `current.missing_days.rain` / `.gdd` count NULL or absent days between `first_date` and `last_date`; they contributed 0 to the running sum. `is_complete = false` is not "missing".
- `first_date`, `last_date` and `last_complete_date` may each be `null` (no data in the window / no complete day yet); series arrays may be empty.
- `day_definition` and `baseline.source` are machine strings. Display map in `src/lib/labels.ts`: `local_standard_time_midnight_to_midnight` -> "Days are midnight to midnight in local standard time (no daylight-saving shift)."; `ghcnd_acis` -> "NOAA GHCN-Daily via ACIS". Unknown values print with underscores replaced by spaces and no added claims. GDD method names (`avg_50`, ...) print as given (display text: Q5).
- No per-station connection field. Needs-reconnecting is known only for the caller's own credential (`/me`). Followers see staleness through `latest_observation_at` only.
- A hidden or unknown station, or one hidden id in a multi-station `ytd` call, is 404 `not_found` for the whole request.
- 02 limits (copy of 02 4.9, 4.6.2, 4.2): one flat per-user limiter, every request costs 1, 120 requests per minute, burst 40; connect routes additionally 5 per minute and 20 per hour (durable). One overall 25 s request timeout on `/api/v1` (per-operation bounds inside), body limit 16 KiB. The web's client timeout is 30 s for every request (above the API's 25 s, so the API's `request_timeout` envelope arrives first).
- Extra error codes from 02: 408 `request_timeout`, 413 `payload_too_large`, 503 `overloaded` and `auth_unavailable`. 401 `unauthorized` always means sign out; **503 never signs out** (`auth_unavailable` means retry later).

### 3.3 Env vars

| Variable | Where | Rule |
| --- | --- | --- |
| `PUBLIC_SUPABASE_URL` | build-time, process env | Local `http://localhost:8000`; `make web-dev` uses `http://localhost:5173` with the Vite proxy in 4.1. Production `https://<project-ref>.supabase.co`. |
| `PUBLIC_SUPABASE_ANON_KEY` | build-time, process env | Anon/publishable key only (local: the demo anon key from `.env.example`). |

Read only through `$env/static/public`; `$env/static/private` and `$env/dynamic/*` are banned (ESLint `no-restricted-imports`). The only other variable relevant to Netlify is the runtime `NETLIFY_PROXY_SECRET` once signing is enabled (06); web code never reads it. For `check`, `lint` and `test`, CI has no `PUBLIC_*`: `web/.env.test` (committed, dummy values only, see 4.1) supplies them through `scripts/with-test-env.mjs`.

### 3.4 Files shared with other guides

- `web/security-headers.json`: 01 owns. `node scripts/headers.mjs generate` writes both the `web/netlify.toml` header block and the committed `ops/caddy/security-headers.caddy` (local variant: no HSTS, no Cache-Control, no `upgrade-insecure-requests`). 05's Caddyfile only `import`s that file (05 section 5.3). `headers.mjs check` verifies the generated outputs, the Caddyfile import, `-Server`, the cache matchers and the redirect invariants (4.8).
- `web/netlify.toml`: 01 owns headers/CSP and redirect rules; 06 owns site settings, the deploy gate and the hand-off of `<project-ref>` (4.9).
- npm scripts required by 05/06: `build` (output `web/build`, contains `200.html`), `lint`, `check`, `test`, `test:tz`, `test:e2e`, `test:bundle`, `headers:generate`, `headers:check`.
- 05 `make` targets 01 relies on: `make web-build`, `make web-dev`, `make test-web`, `make test-e2e` (prerequisites in 6.3). Requests to 05 (Q12): `test-web` should also run `npm run test:tz` and `npm run headers:check`; `.gitignore` must add `web/tests/.auth/`, `web/test-results/`, `web/playwright-report/`. Host prerequisite: `cd web && npm ci` with Node matching `.nvmrc` (= major of 05's `WEB_NODE_IMAGE`).
- Auth flow for 05's smoke test: the web uses PKCE; the magic link redirects to `/auth/callback?code=...` (not `#access_token=`).

---

## 4. Design

### 4.1 Project layout

```
web/
├── package.json              # exact versions; "engines": {"node": "<major of WEB_NODE_IMAGE>"}
├── package-lock.json         # committed
├── .npmrc                    # save-exact=true, ignore-scripts=true, engine-strict=true
├── .nvmrc                    # = major of 05's WEB_NODE_IMAGE (also Netlify NODE_VERSION)
├── .env.test                 # committed, dummy PUBLIC_SUPABASE_URL / PUBLIC_SUPABASE_ANON_KEY only
├── security-headers.json     # single source of truth for headers (4.8)
├── svelte.config.js, vite.config.ts, tsconfig.json, eslint.config.js, playwright.config.ts
├── netlify.toml              # build, redirects, GENERATED header block
├── static/{favicon.svg, robots.txt, 404.html}   # robots.txt: "User-agent: *\nDisallow: /"; 404.html: minimal static page for the /_app/* 404 rule
├── scripts/
│   ├── with-test-env.mjs     # fills missing PUBLIC_* from .env.test, then spawns the given command
│   ├── write-version.mjs     # writes build/version.txt: the full commit SHA (`COMMIT_REF`, else `GITHUB_SHA`, else the literal `local`), no trailing newline required
│   ├── check-build.mjs       # CSP meta with the single hashed bootstrap script, announcer hash, no other inline scripts/styles in build/200.html
│   ├── headers.mjs           # `generate` (netlify.toml block + ../ops/caddy/security-headers.caddy) and `check`; Node built-ins only
│   └── check-bundle-secrets.mjs
├── src/
│   ├── app.html              # no inline script/style/handlers; <meta name="robots" content="noindex">; wrapper <div class="contents">
│   ├── app.css               # all styling: tokens, layout classes (.cols-1..5), chart classes
│   ├── hooks.client.ts       # handleError: generic message only
│   ├── test/setup.ts         # Vitest setup: stubs globals, vi.mock('$app/navigation'), see 4.1
│   ├── lib/
│   │   ├── config.ts, labels.ts
│   │   ├── auth/{supabase.ts, session.svelte.ts, next.ts, redirect.ts, login.ts}
│   │   ├── api/{client.ts, errors.ts, endpoints.ts, types.ts, ids.ts, cache.ts}
│   │   ├── data/resource.svelte.ts
│   │   ├── transforms/{dates.ts, slots.ts, series.ts, bands.ts, position.ts, provenance.ts, forecast.ts, *.test.ts}
│   │   ├── charts/{ChartFrame.svelte, CumulativeChart.svelte, ForecastHistoryChart.svelte, ProvenanceCaption.svelte, scales.ts}
│   │   └── components/{StationCard, StationPicker, SeasonToggle, EmptyState, ErrorPanel, LoadingBlock, NeedsReconnectBanner, FollowItem, Footer}.svelte
│   └── routes/
│       ├── +layout.ts                  # ssr=false, prerender=false, trailingSlash="never"
│       ├── +layout.svelte              # imports app.css, renders <Footer/>
│       ├── (public)/login/+page.svelte
│       ├── (public)/auth/callback/+page.svelte
│       ├── (public)/account-inactive/+page.svelte
│       └── (app)/{+layout.ts, +layout.svelte, +page.svelte, stations/[id]/+page.svelte, stations/[id]/forecast/+page.svelte, connect/+page.svelte, follows/+page.svelte}
└── tests/
    ├── fixtures/{ytd-full.json, ytd-no-baseline.json, ytd-gdd-mismatch.json, ytd-gdd-unknown-reason.json, ytd-partial-station.json, forecast-history.json}   # committed API-shaped fixtures
    ├── e2e/{auth.setup.ts, *.spec.ts}
    ├── .auth/                          # storageState files, git-ignored
    └── helpers/{env.ts, mailpit.ts, csp.ts, api.ts, invite.ts}
```

`svelte.config.js`:

```js
import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

export default {
  preprocess: vitePreprocess(),
  kit: {
    adapter: adapter({ pages: 'build', assets: 'build', fallback: '200.html', precompress: false, strict: true }),
    output: { preloadStrategy: 'modulepreload' },
    csp: { mode: 'hash', directives: { 'script-src': ['self'] } }   // header carries every other directive (4.8)
  }
};
```

`vite.config.ts` (the `server.proxy` block is what 05's `make web-dev` needs; the browser stays single-origin at `:5173`):

```ts
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  plugins: [sveltekit()],
  server: {
    proxy: {
      '/api': { target: 'http://localhost:8000', changeOrigin: false },
      '/auth/v1': { target: 'http://localhost:8000', changeOrigin: false }
    }
  },
  resolve: process.env.VITEST ? { conditions: ['browser'] } : undefined,
  test: { include: ['src/**/*.test.ts'], environment: 'node', setupFiles: ['src/test/setup.ts'] }
});
```

`package.json` scripts (literal; `svelte-kit sync` must run first because `$env/static/public` types are generated, and CI has no `PUBLIC_*`):

```json
{
  "build": "vite build && node scripts/write-version.mjs",
  "dev": "vite dev",
  "lint": "node scripts/with-test-env.mjs svelte-kit sync && eslint .",
  "check": "node scripts/with-test-env.mjs svelte-kit sync && node scripts/with-test-env.mjs svelte-check --tsconfig ./tsconfig.json",
  "test": "node scripts/with-test-env.mjs svelte-kit sync && node scripts/with-test-env.mjs vitest run",
  "test:tz": "TZ=America/Denver npm test && TZ=UTC npm test",
  "test:e2e": "playwright test",
  "test:bundle": "node scripts/check-bundle-secrets.mjs",
  "headers:generate": "node scripts/headers.mjs generate",
  "headers:check": "node scripts/headers.mjs check"
}
```

Vitest runs in the `node` environment, so `src/test/setup.ts` stubs what the code under test reads: `globalThis.location = new URL('http://localhost:8000')`-like object (`origin`, `pathname`, `search`), an in-memory `localStorage`/`sessionStorage`, and `vi.mock('$app/navigation', () => ({ goto: vi.fn() }))` plus `vi.mock('$env/static/public', ...)` with the dummy values. Svelte rune modules (`*.svelte.ts`, e.g. `resource.svelte.ts`, `session.svelte.ts`) need Svelte's browser build, so `vite.config.ts` sets `resolve.conditions: ['browser']` under Vitest (without it `$effect` is unavailable in the node environment). Tests drive lifecycle with `$effect.root(() => { ... return cleanup })`: calling the returned cleanup is the "unmount" in the abort test. Pure modules (`next.ts`, `ids.ts`, `client.ts`) also accept `origin` and storage as injectable parameters with these globals as defaults.

`build` is run by Netlify and 05 with real `PUBLIC_*` in the process env and is never wrapped by `with-test-env`. `src/routes/+layout.ts`: `export const ssr = false; export const prerender = false; export const trailingSlash = 'never';`. Keep SvelteKit's default `inlineStyleThreshold` of 0.

### 4.2 Supabase client, session and auth redirects

`src/lib/auth/supabase.ts`:

```ts
import { createClient } from '@supabase/supabase-js';
import { PUBLIC_SUPABASE_URL, PUBLIC_SUPABASE_ANON_KEY } from '$env/static/public';
export const supabase = createClient(PUBLIC_SUPABASE_URL, PUBLIC_SUPABASE_ANON_KEY, {
  auth: { flowType: 'pkce', persistSession: true, autoRefreshToken: true, detectSessionInUrl: true }
});
```

Rules: one module-level client; the anon key is never sent to `/api/*`; the session lives in Supabase's default `localStorage` entry (`sb-*`), the accepted S2 exposure; do not call other async Supabase functions synchronously inside an `onAuthStateChange` callback (defer with `setTimeout(..., 0)`).

`session.svelte.ts` exports `auth` (`$state`: `status: 'loading'|'signed_out'|'signed_in'`), `initAuth()`, `getAccessToken()` (reads `supabase.auth.getSession()` each call; never cached in module state), and `signOut(scope?: 'local'|'global')` (default `'local'`; also clears the `ytd` cache and forecast cache).

**Single redirect owner (`auth/redirect.ts`).** `apiFetch` never navigates. When the session is unrecoverable it signs out (marking the sign-out as client-initiated: `session.svelte.ts` sets a module flag `signingOutFromApi = true` before calling `supabase.auth.signOut`, and clears it after the resulting `SIGNED_OUT` event) and throws `ApiError` with `kind: 'session_expired' | 'account_inactive'`. The only code that navigates for these kinds is `handleAuthError(err): boolean` in `redirect.ts` (`session_expired` -> `/login?reason=expired`; `account_inactive` -> `/account-inactive`), called from exactly three places: the `catch` in `createResource`, the `catch` in `callAction(fn)` (wrapper for follows/connect handlers), and `(app)/+layout.ts` (where it is expressed as `throw redirect(307, ...)`, see below). The `SIGNED_OUT` listener in `session.svelte.ts` navigates to `/login` only when the flag is not set (other-tab sign-out) and the current route is inside `(app)`; nav-initiated sign-out navigates itself. That way the listener never races `handleAuthError`.

**Retryable network errors never sign out.** supabase-js returns `AuthRetryableFetchError` (status 0) from `getSession()` and `refreshSession()` instead of throwing when it cannot reach the auth server. The rules:
- `getAccessToken()` returns the token if `getSession()` yields a session. If it yields no session and no error, the user is signed out (`session_expired`). If it yields an error with `isAuthRetryableFetchError(error)`, throw `ApiError(0, 'network')` and do not sign out.
- On a 401 from the API, call `refreshSession()` once. Sign out only when the refresh returns no session with a non-retryable error (`AuthApiError`, e.g. invalid refresh token) or the retried request is again 401. A retryable refresh error becomes `ApiError(0, 'network')`.

**Login (`/login`)** via `login.ts: requestMagicLink(email, next)`:
1. Validate: non-empty, contains `@`, <= 254 chars.
2. Store the validated `next` (from `?next=`, passed through `safeNext`) in `localStorage['weather.next']` as `{ "path": "<safeNext result>", "exp": <now + 15 min, epoch ms> }`. localStorage, not sessionStorage: the magic link opens in a new tab, which has empty sessionStorage but shares localStorage (the PKCE code verifier is stored there too, so the same browser profile is required; the page says so in one sentence).
3. Call `supabase.auth.signInWithOtp({ email, options: { shouldCreateUser: false, emailRedirectTo: location.origin + '/auth/callback' } })` (exact URL, no query).
4. Result mapping (S4 oracle rule): success, any GoTrue error including 429 and the "signups not allowed" error, all return `'sent'` and show exactly: "If that address is invited, a sign-in link is on its way. Open it in this browser." Only when the call returns or throws a retryable fetch error (`isAuthRetryableFetchError(error)`, status 0, no HTTP response from GoTrue) show "Could not reach the sign-in service. Try again." (A GoTrue 429 is only returned for invited emails; showing different text would reveal invitation status.) Disable the button 60 s after any send (client-side convenience).

**`safeNext(raw: string | null): string`** (`auth/next.ts`), returns `/` unless all hold:
- `raw` is a string of <= 200 chars that starts with a single `/`, does not start with `//`, contains no backslash and no control characters (code < 0x20 or 0x7f), including after `decodeURIComponent` (a decode failure rejects);
- `rawPath = raw` up to the first `?` or `#`; every `/`-separated segment after the leading empty one, once decoded, is not `.` or `..` and (except a trailing empty segment, i.e. a trailing slash) not empty;
- `const u = new URL(raw, location.origin)` has `u.origin === location.origin`, `u.pathname === rawPath` (no normalisation happened), and `u.pathname` does not start with `//`.
Returns `u.pathname + u.search` (never `u.hash`).

**Callback (`/auth/callback`)**: on mount `await supabase.auth.getSession()` (awaits initialisation, which exchanges `?code=`). If a session exists: read `localStorage['weather.next']` (ignore if expired or malformed; remove it) and `goto(safeNext(path), { replaceState: true })` (this replaces the callback URL, which carried the one-time code; no raw `history` calls). Else, or when the URL query/fragment has `error`/`error_code`, or after 10 s: show "That sign-in link is invalid or expired. Request a new one." with a link to `/login`. Never render URL parameters or `error_description`.

**Guard (`(app)/+layout.ts`)**: no session -> `throw redirect(307, '/login?next=' + encodeURIComponent(safeNext(url.pathname + url.search)))`; then `getMe()` inside a try/catch: for an `ApiError` with `kind`, `throw redirect(307, <target from handleAuthError's mapping>)` (the shared mapping function `authRedirectTarget(err)` is the single source; `handleAuthError` uses it with `goto`); other errors propagate to the layout error state. Load functions always use `throw redirect(...)`, never `goto`. Because `ssr = false` this runs only in the browser; it is UX only, the API enforces access.

**Sign-out**: nav "Sign out" = `signOut('local')` (not flagged as API-initiated) then `goto('/login')`; "Sign out everywhere" uses scope `'global'`. A 503 (including `auth_unavailable`) never signs out.

### 4.3 API client (single module)

All HTTP to the API goes through `apiFetch` (`src/lib/api/client.ts`). No other file calls `fetch` except `tests/` (ESLint rule). Paths are built only by `endpoints.ts`.

```ts
export type AuthKind = 'session_expired' | 'account_inactive';
export class ApiError extends Error {
  constructor(public status: number, public code: string, message: string,
              public retryAfterSec?: number, public kind?: AuthKind) { super(message); }
}
export async function apiFetch<T>(path: string, init?: { method?: 'GET'|'POST'; body?: unknown; signal?: AbortSignal }): Promise<T>;
```

Path safety (the Bearer token must never go to an arbitrary same-origin path):
- `endpoints.ts` builds the `station_ids` query value by joining the validated UUIDs with a literal `,` (never `URLSearchParams`, which would encode it as `%2C`), e.g. `?station_ids=<uuid>,<uuid>&year=2026&start=04-01&end=10-31`; other query keys are appended in that fixed order.
- `ids.ts: assertUuid(s)` matches `^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$` or throws `ApiError(0, 'bad_id')`. Every ID used in a path or in `station_ids` is checked, and path parameters are inserted with `encodeURIComponent`. Page route params (`stations/[id]`) are checked at page level; a non-UUID renders "That isn't available." without any request.
- `apiFetch` validates the caller's **raw path string before** constructing any `URL` (the URL parser silently removes `.`/`%2e` dot segments, so checking after parsing is too late). Steps: (1) reject if `path` contains `\`, `#`, or a control character (code < 0x20 or 0x7f); (2) `rawPath = path.split('?', 1)[0]`; require `rawPath.startsWith('/api/v1/')`; (3) split `rawPath` on `/`, skip the leading empty segment; every remaining segment must be non-empty, `decodeURIComponent`-able (a decode failure rejects), and its decoded form must not equal `.` or `..` or contain `/`, `\` or a control character; (4) only then `const u = new URL(path, location.origin)` and require `u.origin === location.origin` and `u.pathname === rawPath` (any normalisation by the parser means the input was not canonical, so reject). The query string is built only by `endpoints.ts` from validated values. Violations throw `ApiError(0, 'bad_path')` before any network call.

Algorithm:
1. `token = await getAccessToken()` (4.2 rules: no session and no error -> sign out and throw `kind: 'session_expired'`; retryable auth error -> `ApiError(0, 'network')`, no sign-out).
2. `fetch(u, { method, headers: { Authorization: 'Bearer ' + token, Accept: 'application/json', ...(body ? { 'Content-Type': 'application/json' } : {}) }, body: body ? JSON.stringify(body) : undefined, signal: AbortSignal.any([init?.signal, AbortSignal.timeout(30000)].filter(Boolean)), credentials: 'omit', cache: 'no-store', referrerPolicy: 'no-referrer' })`.
The client timeout is 30000 ms for every request (02's overall timeout is 25 s; the client waits slightly longer so a server `request_timeout` 408 envelope is seen first). A client timeout is `ApiError(0, 'network')`.
3. 401: refresh once per the rules in 4.2 and retry once. Sign out and throw `kind: 'session_expired'` only if the refresh fails non-retryably or the retry is 401.
4. 403 with `error.code === 'account_inactive'`: `signOut('local')`, throw `kind: 'account_inactive'`. Other 403 (`forbidden`): throw plain `ApiError(403, ...)`.
5. 429: throw with `retryAfterSec` from `Retry-After`; no automatic retry. 503 (`overloaded`, `auth_unavailable`, `upstream_busy`) and 408/413 throw plain `ApiError`s with `retryAfterSec` when `Retry-After` is present; they never sign out.
6. Other non-2xx: parse `{error:{code,message}}` if the body has that shape else `code = 'unknown'`. The server `message` is never shown, never rendered as HTML. Network failure/timeout -> `ApiError(0, 'network')`.
7. 2xx: minimal top-level shape check in `endpoints.ts` (required keys and array/boolean types); failure -> `ApiError(502, 'bad_response')`.
8. Bodies containing `api_key` are never logged, never copied into an `ApiError`; `apiFetch` has no debug logging.

`userMessage(err)`:

| Condition | Text |
| --- | --- |
| `network` | "Can't reach the server. Check your connection and retry." |
| 403 `forbidden`, 404 `not_found`, `bad_id`, `bad_path` | "That isn't available." |
| 429 `rate_limited` | "Too many requests. Try again in {n} seconds." (omit the number if no `Retry-After`) |
| 503 `upstream_busy` | "The weather service is busy. Try again in a moment." |
| 503 `overloaded`, 503 `auth_unavailable` | "The service is busy right now. Try again in a moment." (never signs out) |
| 408 `request_timeout` | "The request took too long. Try again." |
| 413 `payload_too_large` | "That request was too large." |
| 502 `upstream_unavailable` | "The weather service is unavailable. Try again later." |
| 422 `timezone_required` | (connect view shows the zone picker, 4.7) |
| 422 `validation_failed` | "That request wasn't valid." |
| 409 `conflict` | "That changed while you were working. Reload and try again." |
| 409 `device_already_registered` | "That device is already registered." |
| 422 `unknown_device` | "That device is not on this account. Check the device list and try again." |
| 422 `invalid_key` | (connect view shows the generic key message, 4.7) |
| any other status or code (catch-all), `bad_response`, `unknown` | "Something went wrong on our side. Try again." |
| `session_expired` / `account_inactive` | handled by `handleAuthError` (login shows "Your session expired. Sign in again."; inactive page shows "This account is inactive. Contact the administrator.") |

### 4.4 Data loading and caching

`resource.svelte.ts`: `createResource<T>(fetcher: (signal) => Promise<T>)` returns a `$state` `{ status: 'loading'|'ready'|'error', data?, error?, reload() }`; aborts on reload/unmount; its `catch` calls `handleAuthError`. Every view renders `LoadingBlock`, `ErrorPanel` (message + Retry) or content.

`cache.ts`: in-memory `Map` for `/ytd` responses keyed by `year|start|end|sortedStationIds`, TTL 5 minutes, cleared on sign-out. Purpose: load reduction (a `/ytd` call runs window-function SQL over up to 5 stations and 30+ baseline years), not rate-limit avoidance (every request costs 1 of 120 per minute, burst 40, so toggling would not hit the limiter). Toggling Rain/GDD or Full year and back reuses cached responses; in-flight requests with the same key are shared. Rain uses window `season` (default `04-01`..`10-31`); GDD uses `GDD_ACCUM_START`..season end, so the first GDD toggle makes at most one extra request. Forecast history responses are cached the same way (key `stationId|variable|from|to|runs`). Nothing else is cached.

### 4.5 Constants (`src/lib/config.ts`)

```ts
export const DEFAULT_SEASON = { start: '04-01', end: '10-31' } as const;     // F6
export const FULL_YEAR = { start: '01-01', end: '12-31' } as const;
export const GDD_ACCUM_START = '01-01';                                      // Q5
export const WINTER_MONTHS = [11, 12, 1, 2, 3] as const;
export const YTD_MAX_STATIONS = 5;
export const STALE_AFTER_MIN = 30;                                           // Q9
export const FORECAST_VARIABLE = 'precip_6h_in';
export const FORECAST_RUNS = 6;
export const FORECAST_HORIZON_DAYS = 7;                                      // Q1 (<= 16, API limit)
export const FEATURES = { forecastHistory: true } as const;                  // design doc: v1 requirement
export const TIMEZONE_CHOICES: string[] = [];                                // fallback zone picker list, content Q8
```

Units: rain inches, temperature F, GDD F degree-days. No unit toggle in v1.

### 4.6 Transforms and charts

Pure functions in `src/lib/transforms/`, unit-tested. Dates are `YYYY-MM-DD` / `MM-DD` strings; never `new Date('YYYY-MM-DD')` or local-time `Date` arithmetic.

- `slots.ts: buildSlots(ytd, metric)`: sorted union of `md` keys from `bands[field]` (when non-null), `current[field]` and every non-null prior-year `[field]`, where `field = metric === 'rain' ? 'rain_cum_in' : 'gdd_cum'` (lexicographic `MM-DD` order is chronological; the API drops Feb 29). X position = index.
- `series.ts: toPoints(values: MdValue[] | null, slots)`: `({x,y}|null)[]`; missing slots `null` (path breaks, no interpolation, no zero fill); `null` input -> `[]`. Callers pass the metric's field (`prior.rain_cum_in` etc.); a prior year whose field is `null` is not drawn. `splitCurrent(current, field)` returns `{ complete: MdValue[], partial: MdValue[] }`: points with `md` after the `md` of `last_complete_date` (or all points when `last_complete_date` is `null`) are `partial` (the open day), the rest `complete`; the partial segment is joined to the last complete point so the line is continuous.
- `bands.ts: bandPaths(bands, metric, slots)`: reads `bands[field]` for the metric (rain and GDD bands are independent and either may be `null`); finds percentile columns by value in `bands.percentiles`. Outer = lowest and highest listed; inner = the 25 and 75 entries if present, else omitted; median = 50 entry if present, else omitted. `bands`, or `bands[field]`, `null` -> no layers.
- `position.ts: describePosition(currentValueAtLastCompleteDay, bandsAtThatMd, percentiles, yearsIncluded: number)`: "below the 10th percentile" / "between the 25th and 50th percentile" / "above the 90th percentile" / "at about the 50th percentile", suffixed " of {yearsIncluded} years of airport record" where `yearsIncluded = bands.years_included[metric]`; `null` when that metric's bands are missing, when `last_complete_date` is `null`, or when `current.first_date` is later than the window start (a station that began mid-season is not comparable; the caption explains why, below). Uses the value at the `md` of `last_complete_date`, not the open day.
- `provenance.ts: buildProvenanceLines(ytd, station, metric, window)`.
- `forecast.ts`: `groupRuns(history)` -> per-run ordered points with run labels; `totalP50(run)` = sum of `p50` over points, ignoring nulls, with `approx: true`.
- `dates.ts`: `monthOfMd`, `formatInZone(isoUtc, ianaTz)` (`Intl.DateTimeFormat`), `localYear(ianaTz, now)`, `localMd(ianaTz, now)`.

Chart library: **Svelte-rendered SVG using `d3-scale`, `d3-shape`, `d3-array`** (ISC, modular, no DOM access). Output is Svelte-managed SVG built from data, no HTML strings or `innerHTML`; no `eval`/`new Function`; no injected `<style>`; styling by CSS classes works under the CSP; three small packages are easy to pin. Rejected: Apache ECharts (default tooltips are HTML strings with inline `style` attributes, injects styles, large), Plotly (large, historically needs `unsafe-eval`/inline styles), Chart.js (CSP-safe but custom plugins needed for bands and accessible tooltips, canvas has no DOM for tests). No other charting dependency.

`CumulativeChart.svelte` props: `ytd: YtdStation`, `metric: 'rain'|'gdd'`, `window`, `valueLabel`, `yDomain?`, `provenance` (required).
- Layers: winter shading (class `winter`, rain metric only), outer band (`band-outer`), inner band (`band-inner`), median (`line-median`), prior years (`line-prior`), current year (`line-current`). The current line is drawn through `current.last_date` (never extended beyond it): the complete part uses `line-current`, and the part after `last_complete_date` (the open, partial day) uses `line-current-partial` (lighter and dashed via CSS classes) so a partial value is not read as a full day. A station whose `first_date` is later than the window start begins its line at `first_date` (no earlier points, no zero fill) and the chart shows "Data starts {first_date}; earlier days are not included, so the total reads low against the baseline."
- Missing days use the active metric's counts: prior years with `missing_days[metric] > 0` appear in the data table ("3 missing days") with the note "Missing days are counted as zero, as supplied by the API."; `current.missing_days[metric] > 0` shows next to the chart title as "{n} days missing this season (counted as zero)" and in the data table. Prior years whose series for the metric is `null` are listed as "excluded: too many missing days" and not drawn.
- No `style=` attributes, no `style:` directives, no SVG `style`, no Svelte transitions/animations, no CSS-in-JS. Geometry uses SVG attributes (`d`, `x`, `width`, `transform`, `x1`, `x2`); variation uses classes. Dynamic layout counts use fixed classes (`.cols-1` ... `.cols-5`).
- Cursor: pointer move or keyboard left/right moves a `<line>`; a readout (escaped text) lists current, median and band values. A `<details>` "Show data table" renders the same numbers.
- `ChartFrame` always renders the provenance caption; `provenance` is a required prop.

Provenance caption lines (values from the API response):
- Baseline: `Baseline: {baseline.name} ({baseline.reference_station_id}), {label(baseline.source)}; {bands.years_included[metric]} years in the percentile bands, last {n} years drawn.` (`n` = prior years whose series for the metric is non-null; when that metric's bands are `null`: `Not enough years for percentile bands.`) No baseline: `No baseline is set for this station yet.` (no bands, no prior lines).
- Day definition: `labels.dayDefinition(ytd.day_definition)`.
- GDD charts: `GDD method: {gdd_overlay.station_method}.` plus, when `available`, `Airport method: {baseline_method}.` (`provenance.ts` takes the branch from `gdd_overlay.reason`, see 4.7).
- Rain charts only, when the window includes a winter month: `Shaded months: unheated rain gauges undercount snow, so totals read low against the baseline.` (never on GDD charts)
- Bands present: `Bands: {first}th-{last}th and {inner pair} percentile of prior years.` built from `bands.percentiles`.

### 4.7 Views

**Login**, **Account inactive** (`/account-inactive`, static text, link back to `/login`), **Callback**: section 4.2.

**Empty state** (`stations.length === 0`): "No stations yet." Actions: link "Connect your weather station" (`/connect`) and the text "Ask Rodney to share stations with you." (neutral until Q3 is decided).

**Dashboard** `/`:
- Load `/me` (layout) and `/stations`. Selected stations default to all, capped at `YTD_MAX_STATIONS`; the picker limits selection to 5.
- Controls: metric tabs `Rain` (default) | `GDD`; season `Apr-Oct` (default) | `Full year`; "Same scale for all stations" (default off, Q7); `StationPicker`.
- Data: one `/ytd` call for all selected stations for the active window (4.4); `year` = `localYear(first selected station's timezone, now)`.
- Grid: one column per selected station using a fixed class `.cols-N` on the grid (`N = min(selected, 5)`), with horizontal scroll beyond the viewport. Each column: `StationCard` header (name, owner if not own, `latest_observation_at` in the station's timezone), the chart with caption, `describePosition` text.
- Season states (explicit, never a blank chart): if `current.last_date` is `null` (no data in the window) and the station-local date is before the window start: "Season not started. Showing prior years only." (bands and prior lines still drawn). If `last_date` is `null` and the date is inside the window: "No data yet this season." If the station-local date is after the window end: caption "Season ended {end}."; the full season is shown. What the dashboard should default to between November and March is Q11.
- GDD view branches on `gdd_overlay` (`gddViewState(overlay, baseline)` in `provenance.ts`, one Vitest case each):
  - `available: true` -> current line, prior lines and bands.
  - `available: false`, `reason === 'no_baseline'` -> current line only, no prior lines or bands, caption "No baseline is set for this station yet."
  - `available: false`, `reason === 'gdd_method_mismatch'` -> current line only and the note "Gauge and airport GDD use different methods, so no comparison is drawn. Gauge: {station_method}. Airport: {baseline_method}." (a null method prints "unknown").
  - `available: false` with any other or missing `reason` -> current line only and the generic note "A comparison is not available for this station." (no claims about the cause).
- Needs reconnecting: `me.credential?.status === 'invalid'` -> own stations (`is_own`) show `NeedsReconnectBanner` ("Data collection paused: this station needs reconnecting. Last data: {time}.") with a "Reconnect" link to `/connect?reconnect=1`. Followed stations never show a reconnect action. `rate_limited`: neutral "Updates are delayed." on own stations. Stale (`latest_observation_at` older than `STALE_AFTER_MIN`, no banner): "No recent data." History still charts in every state.
- A `ytd` 404 reloads `stations` and shows `ErrorPanel` with Retry.

**Station detail** `/stations/{id}`: UUID check, `/stations/{id}` and `/ytd?station_ids={id}`, larger chart, tabs Rain | GDD | Forecast, season toggle, data table. 404/403 -> "That isn't available." with a link to `/`.

**Forecast-change history** `/stations/{id}/forecast` (v1, `FEATURES.forecastHistory` default on; when forced off, the tab and route show "Forecast history is not enabled." for rollback):
- Request: `GET /stations/{id}/forecast/history?variable=precip_6h_in&valid_from=<now floored to 6h UTC>&valid_to=<valid_from + FORECAST_HORIZON_DAYS>&runs=6` (no `model`: the API defaults to the newest run's model; the response's `model` is displayed, there is no selector because no endpoint lists models).
- `ForecastHistoryChart.svelte` (SVG, same rules as 4.6): x = `valid_time` (UTC instants, axis labels in the station's timezone), y = `p50` inches per `period_hours` window. One `line-run-0` (newest, strongest) ... `line-run-5` (oldest, faintest) per run; the newest run also draws a `band-outer` ribbon from `p10` to `p90`. A second panel plots `prob_precip` (0-100) per run on the same x axis. Runs arrive newest first; run labels "Run {run_time in station tz}". Null points break the line.
- Below the charts: a table with one row per run (run time, fetched at, approximate 7-day total = `totalP50`, labelled "sum of 6-hour medians, approximate") so a reader can see how the total changed run to run.
- Caption lines: `Forecast: {model}, {period_hours}-hour windows; ribbon is the p10-p90 spread of the newest run.` plus `{attribution}` from the response (Open-Meteo CC BY 4.0), in addition to the footer.
- Empty `runs: []`: "No forecast snapshots yet." Forecast-vs-gauge is post-v1: no route or component.

**Follows** `/follows`: `GET /follows`.
- "People who follow me" (`followers`): `pending` first with Approve / Decline buttons (`POST /follows/{user.id}/approve|decline`), then `approved` with Revoke (`/revoke`, inline confirm, not `window.confirm`), then `declined`/`revoked` collapsed. Text: "Approving shares all of your stations." Show `requested_at` (date) per row.
- "People I follow" (`following`): list with status badges. No cancel/unfollow control (02 has none).
- No "request to follow" form (Q3), although 02 exposes `POST /follows`.
- After each action reload `follows` and `stations`; a 404 shows "That request is no longer pending." and reloads.

**Connect station** `/connect` (3 steps, state in component memory only):
1. **Key**: instructions (wording Q4); input `type="text"` with `autocomplete="off"`, `autocapitalize="off"`, `spellcheck="false"`, `data-1p-ignore`, `data-lpignore="true"` and class `secret-input` (`-webkit-text-security: disc`) so password managers do not offer to capture it as a login. Accepted risk (Q13): some browsers or extensions may still offer to save it, and a browser without `-webkit-text-security` shows the characters.
2. **Choose devices**: `POST /connect/validate`. `valid:false` -> exactly "That key couldn't be verified. Check it and try again." (one text for every invalid cause). 429/503/502 per 4.3. `valid:true` with `devices: []` -> "No devices found on that account." Otherwise a checklist (name + MAC, at least one required). Each device has `timezone: string | null`. The API derives the zone from AWN; **a zone picker is shown only for devices whose `timezone` is `null`** (a required `<select>` of IANA zones, `TIMEZONE_CHOICES` in `config.ts`, list content Q8), and the chosen value is sent as the fallback `timezone` in the save body. Devices with a non-null `timezone` show it as read-only text and send none.
3. **Save**: `POST /connect/save` with `{ api_key, devices: [{ mac, name?, timezone? }] }`. 201: clear the key, show "Connected. Your history will fill in over the next few minutes." with a link to `/`; reload `/me` and `/stations`. `not_covered` is always present (empty on first connect). When it is non-empty (stations the caller owns that the new key does not cover; they stay on the replaced credential and the poller skips them), additionally show "These stations are no longer on this account; ask Rodney to disable them:" followed by the names (escaped text). 422 `timezone_required` (a picker was missing or empty) -> keep step 2 and mark the unresolved devices with "Choose a time zone." 422 `invalid_key` -> the generic key message and back to step 1 with the key cleared. 409 `device_already_registered`/`conflict`, 422 `unknown_device`, 422 `validation_failed` per 4.3. The key stays in memory on recoverable errors.
- Key handling: only a local variable bound to the input; never written to `localStorage`, `sessionStorage`, IndexedDB, URL, history state, shared stores, `console` or error objects. Cleared on success, on `beforeNavigate`/`onDestroy` and on `pagehide`. No prefill, no native form submit.
- Reconnect (`/connect?reconnect=1`, offered when `GET /connect/status` has `needs_reconnecting`): same flow, heading "Reconnect your station". 02 has no reconnect route; `connect/save` replaces the caller's credential (the old row becomes `replaced`) and repoints the caller's existing stations, so the user re-selects their existing devices (Q8).
- The UI never has an API URL/host field (S9).

**Footer** (root layout, all pages including login): "Forecast data by Open-Meteo.com, licensed CC BY 4.0." linking to `https://open-meteo.com/` and `https://creativecommons.org/licenses/by/4.0/` (`target="_blank" rel="noopener noreferrer"`); second line "Rain baselines: NOAA GHCN-Daily via ACIS."

### 4.8 Security headers and CSP (S2, S15)

**Source of truth: `web/security-headers.json`.** Schema (values below are the content; `<announcer-hash>` and `<project-ref>` are the only placeholders):

```json
{
  "csp": {
    "style-src": ["'self'"],
    "style-src-attr": ["'unsafe-hashes'", "'sha256-<announcer-hash>'"],
    "img-src": ["'self'"], "font-src": ["'self'"], "connect-src": ["'self'"],
    "manifest-src": ["'self'"], "media-src": ["'none'"], "worker-src": ["'none'"], "frame-src": ["'none'"],
    "object-src": ["'none'"], "base-uri": ["'none'"], "form-action": ["'self'"], "frame-ancestors": ["'none'"]
  },
  "csp_production_additions": {
    "connect-src": ["https://<project-ref>.supabase.co"],
    "upgrade-insecure-requests": []
  },
  "headers_all": {
    "X-Content-Type-Options": "nosniff",
    "X-Frame-Options": "DENY",
    "Referrer-Policy": "no-referrer",
    "Permissions-Policy": "camera=(), microphone=(), geolocation=(), payment=(), usb=(), interest-cohort=()",
    "Cross-Origin-Opener-Policy": "same-origin",
    "Cross-Origin-Resource-Policy": "same-origin",
    "X-Robots-Tag": "noindex, nofollow"
  },
  "headers_production_only": { "Strict-Transport-Security": "max-age=31536000; includeSubDomains" },
  "cache_control": { "/_app/immutable/*": "public, max-age=31536000, immutable" },
  "expected_html_cache_control": { "netlify": "public, max-age=0, must-revalidate", "caddy": "no-cache" }
}
```

How the policy works:
- Script policy is the SvelteKit `kit.csp` meta tag in `build/200.html`: `script-src 'self' 'sha256-...'` (hash of the single inline bootstrap script). Meta CSP cannot carry `frame-ancestors`, so the static header carries all other directives. The header has no `default-src` and no `script-src`: a header `default-src 'none'` would be intersected with the meta policy and block the hashed bootstrap.
- Sanctioned exception (conventions): SvelteKit's `#svelte-announcer` element is created with an inline `style` attribute, which `style-src 'self'` blocks (sveltejs/kit #11747, #11993). `style-src-attr 'unsafe-hashes' 'sha256-<announcer-hash>'` allows exactly that one attribute value. `style-src` stays `'self'` (elements/stylesheets). No other `unsafe-*` is allowed anywhere.
- `check-build.mjs` computes the hash from the built runtime: it scans `build/_app/**/*.js` for the announcer's style literal (the string starting `position: absolute; left: 0; top: 0; clip: rect(0 0 0 0)` up to its closing quote), takes SHA-256, base64, and fails if (a) the literal is not found (SvelteKit changed: re-evaluate), or (b) the hash differs from the one in `security-headers.json` (run `npm run headers:generate -- --update-announcer-hash`, review, commit). Pin exact Svelte and SvelteKit versions; any upgrade re-runs this check.
- There are no other dynamic inline styles: all layout/geometry variation uses classes and SVG attributes; `style=`, `style:` and Svelte transitions are banned (lint + scan test).

Header assembly (`scripts/headers.mjs`, Node built-ins only, no npm dependencies):
- Production CSP = directives from `csp` merged with `csp_production_additions`, each as `name value1 value2`, joined with `; `, in file key order. Local CSP = `csp` only (no `upgrade-insecure-requests`, no Supabase host: it is same-origin).
- `node scripts/headers.mjs generate` writes two outputs from the JSON: the block between `# BEGIN GENERATED HEADERS` and `# END GENERATED HEADERS` in `web/netlify.toml` (production CSP, `headers_all`, `headers_production_only`, and the `/_app/immutable/*` Cache-Control), and the committed `ops/caddy/security-headers.caddy` (a single `header { ... }` block with the local CSP and `headers_all`; no HSTS, no Cache-Control). It also supports `--update-announcer-hash` (4.8 above).
- `Cache-Control` is set only on `/_app/immutable/*` in `netlify.toml`, not on `/*`. HTML freshness relies on Netlify's default (`expected_html_cache_control.netlify`) and on 05's Caddy matcher (`no-cache` for everything except `/_app/immutable/*`).
- `node scripts/headers.mjs check` (run in the Netlify build command, in `npm run headers:check`, and by `make test-web` once 05 adds it) fails when any of these hold:
  1. the generated `netlify.toml` block or `ops/caddy/security-headers.caddy` differs from what `generate` would write;
  2. `ops/caddy/Caddyfile` (when present) fails exact-token checks against 05 section 5.3: contains `import /etc/caddy/security-headers.caddy` (in the static handlers); `header -Server`; a `handle /_app/*` block that appears before the catch-all `handle {` block (so missing chunks get `file_server`'s 404, not the fallback; no explicit `respond 404` is required); the `@immutable` and `@mutable` matchers; `rewrite @root /200.html`; and `try_files {path} /200.html` inside the catch-all handler. The "no header values of its own" rule means no `Content-Security-Policy`, `X-Frame-Options`, `Strict-Transport-Security` or other security-header string in the Caddyfile; `Cache-Control` is allowed there (05 sets it per matcher);
  3. redirect invariants in `netlify.toml`: the `/api/*` rule appears before the `/*` rule; the `/*` fallback does not apply to `/_app/*` (see 4.9); the `signed = "NETLIFY_PROXY_SECRET"` line is present but commented out (until the owner deliberately enables it and removes this assertion in the same change); no `*` host or scheme wildcard appears in any CSP source;
  4. with `CONTEXT=production`: the `connect-src` Supabase host differs from the origin of `PUBLIC_SUPABASE_URL`, or the string `<project-ref>` is still present anywhere (this failure on the placeholder is the intended guard: Rodney fills `<project-ref>` in `security-headers.json` and runs `npm run headers:generate` before the first deploy).

Consequences for code (enforced by the e2e CSP spec): no inline `<script>`, `on*=` handlers, `javascript:` URLs, `eval`, `new Function`, remote `import()`; no `{@html}`; no `style=`/`style:`/inline `<style>`; no Svelte `transition:`/`animate:`; no third-party scripts, styles, fonts, images, analytics or CDNs (system font stack); external links use `rel="noopener noreferrer"`.

Netlify does not apply `[[headers]]` to proxied content, so API responses carry their own `no-store`/`nosniff` (conventions).

### 4.9 `web/netlify.toml` and Caddy

`web/netlify.toml` (placeholders written literally: Fly app `famweather-api`, Node `24`; only `<project-ref>` remains, in `security-headers.json` and therefore in the generated CSP):

```toml
[build]
  command = "npm ci && npm run build && node scripts/check-build.mjs && node scripts/headers.mjs check && node scripts/check-bundle-secrets.mjs"
  publish = "build"

[build.environment]
  NODE_VERSION = "24"

# /api proxy first (status 200 = rewrite, force so it beats any static file).
# Signed proxy: commented out for the first deploy (conventions). Enable together with the API secret:
# uncomment the next line and set NETLIFY_PROXY_SECRET in Netlify (runtime scope) and on the API.
[[redirects]]
  from = "/api/*"
  to = "https://famweather-api.fly.dev/api/:splat"
  status = 200
  force = true
  # signed = "NETLIFY_PROXY_SECRET"

# Missing app chunks must 404, not return the SPA page as JavaScript.
[[redirects]]
  from = "/_app/*"
  to = "/404.html"
  status = 404

# SPA fallback last; real static files win because force is not set.
[[redirects]]
  from = "/*"
  to = "/200.html"
  status = 200

# BEGIN GENERATED HEADERS (scripts/headers.mjs generate; do not edit by hand)
# (production CSP, headers_all, Strict-Transport-Security, and the /_app/immutable/* Cache-Control rule,
#  exactly as in security-headers.json; see 4.8)
# END GENERATED HEADERS
```

Notes:
- `NODE_VERSION` equals the major of `.nvmrc` (`check-build.mjs` asserts they match). If the Fly app name differs, 06 changes the one `to =` line and tells 01.
- The `/_app/*` 404 rule sits before `/*`. Real files under `/_app/` are static and win. A `404.html` is not produced by the static adapter: add `static/404.html` (a minimal static page, no inline script/style) so the rule has a target.
- The generated block contains one `[[headers]] for = "/*"` section and one `[[headers]] for = "/_app/immutable/*"` section, with values exactly as in `security-headers.json`. The production CSP literal is: `style-src 'self'; style-src-attr 'unsafe-hashes' 'sha256-<announcer-hash>'; img-src 'self'; font-src 'self'; connect-src 'self' https://<project-ref>.supabase.co; manifest-src 'self'; media-src 'none'; worker-src 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'; upgrade-insecure-requests`.
- Netlify site settings (base directory `web`, production branch `production` moved only by 06's gated `deploy.yml`, deploy previews on for pull requests, branch deploys None) are owned by `06-deploy-ops.md`. `/version.txt` is a real static file (the SPA fallback does not shadow it), has no immutable caching, and is not covered by the CSP/headers special cases beyond the `/*` block; 06's deploy smoke step polls it until it equals the released SHA. The proxy has a request timeout (about 26 s, confirm); the API's overall request timeout is 25 s (02), below that.
- Uncommenting the signed line before the API deploys with `NETLIFY_PROXY_SECRET` would break the proxy.

Caddy: the literal Caddyfile belongs to `05-local-dev.md` (section 5.3). The web contract on it is: it imports the generated `ops/caddy/security-headers.caddy` and defines no security-header values itself (its own `Cache-Control` per matcher is allowed); `header -Server`; immutable `Cache-Control` on `/_app/immutable/*` and `no-cache` on everything else via a negated matcher; `GET /` rewritten to `/200.html`; `try_files {path} /200.html` for the SPA fallback, with a separate `handle /_app/*` block before the catch-all so a missing chunk is a 404 (as in 05 section 5.3); `/api/*` and `handle_path /auth/v1/*` proxied without the static headers.

### 4.10 Styling and layout

Desktop-first. Minimum supported width 1024 px; below that the page scrolls horizontally (mobile deferred). One `app.css` with CSS custom properties; chart tokens (`--c-current`, `--c-prior`, `--c-band-outer`, `--c-band-inner`, `--c-winter`) with WCAG AA contrast for the current line; meaning is never colour-only (labels, dash patterns via classes). System font stack. Layout variants are fixed classes (`.cols-1` to `.cols-5`, `.contents`, `.secret-input`). No dark mode in v1.

### 4.11 Dependencies (S2, S14)

Runtime: `@supabase/supabase-js`, `d3-scale`, `d3-shape`, `d3-array`. Dev: `svelte`, `@sveltejs/kit`, `@sveltejs/adapter-static`, `@sveltejs/vite-plugin-svelte`, `vite`, `typescript`, `svelte-check`, `vitest`, `@playwright/test`, `eslint`, `eslint-plugin-svelte`, `typescript-eslint`, `@types/d3-*`. No other packages without a one-line justification in the PR; none that load remote resources or need install scripts.

Pin exact versions at implementation time to the latest stable (`npm install --save-exact`; look versions up then). Commit `package-lock.json`; CI/Netlify use `npm ci` only; `.npmrc` has `ignore-scripts=true` (install Playwright browsers explicitly: `npx playwright install chromium`); run `npm audit --omit=dev --audit-level=high` and `npm audit signatures` in CI (06's `web-audit` job). Svelte/SvelteKit upgrades must re-run `check-build` (announcer hash).

---

## 5. Implementation tasks

Do them in order. Each "Done when" is a command or observable check.

1. **Scaffold.** `web/` with SvelteKit (TypeScript, Svelte 5), adapter-static, `.npmrc`, `.nvmrc`, `engines`, exact-pinned deps, `.env.test` (dummy values), `scripts/with-test-env.mjs`, the `package.json` scripts and `vite.config.ts` from 4.1, `static/robots.txt`, `src/app.html` (no inline script/style, wrapper `<div class="contents">`, noindex meta). Done when: `cd web && npm ci && PUBLIC_SUPABASE_URL=http://localhost:8000 PUBLIC_SUPABASE_ANON_KEY=x npm run build` produces `build/200.html`, and `npm run check` and `npm test` pass on a machine with no `PUBLIC_*` set.
2. **Lint gates.** ESLint with `svelte/no-at-html-tags: error`, `no-restricted-imports` for `$env/static/private`, `$env/dynamic/*`, and a rule banning `fetch(` outside `src/lib/api/client.ts` and `tests/`. Done when: a temporary `{@html x}`, `$env/static/private` import or stray `fetch(` fails `npm run lint`.
3. **CSP and headers pipeline.** `svelte.config.js` (4.1), `security-headers.json`, `scripts/check-build.mjs` (meta CSP with `script-src 'self' 'sha256-...'`; exactly one inline `<script>` (the bootstrap) and its hash present in the meta; no `unsafe-inline`/`unsafe-eval`, `<style`, `style=`, `on*=` in `200.html`; announcer hash matches the JSON; `build/version.txt` exists and is a 40-hex SHA or `local`), `scripts/write-version.mjs`, `scripts/headers.mjs` (`generate` writes the `netlify.toml` block and `ops/caddy/security-headers.caddy`; `check` per 4.8), initial `netlify.toml` (4.9), `static/404.html`. Done when: `npm run build && node scripts/check-build.mjs && node scripts/headers.mjs generate && git diff --exit-code web/netlify.toml ops/caddy/security-headers.caddy && node scripts/headers.mjs check` exits 0 (the `CONTEXT=production` variant is expected to fail while `<project-ref>` is a placeholder). If `kit.csp` does not emit the meta tag into the adapter-static fallback page, add `scripts/inject-csp-meta.mjs` (hash inline scripts, insert the meta) and record the deviation under Open questions.
4. **Supabase client, session, auth redirect owner** (4.2): `supabase.ts`, `session.svelte.ts`, `next.ts`, `login.ts`, `redirect.ts`. Done when: `next.test.ts`, `login.test.ts` pass (cases in 6.1).
5. **API client** (4.3): `ids.ts`, `types.ts` (exactly 3.2, re-checked against `02-api.md` section 5), `errors.ts`, `client.ts`, `endpoints.ts`, `cache.ts`. Done when: `client.test.ts` cases in 6.1 pass.
6. **Transforms** (4.6) except `forecast.ts`. Done when: transform tests in 6.1 pass.
7. **Login and callback routes**, `(app)` guard, nav, sign-out, footer, account-inactive page. Done when (with the 05 stack up): entering `alex@example.test` delivers a link to Mailpit (`http://localhost:8025`); opening it in a new tab lands on `/` (or the saved `next`); an unknown email shows the identical message and no mail arrives; sign-out clears `sb-*` keys.
8. **Resource helper and shared components** (`createResource`, `callAction`, `LoadingBlock`, `ErrorPanel`, `EmptyState`, `NeedsReconnectBanner`, `StationCard`, `FollowItem`, `Footer`). Done when: `src/lib/data/resource.test.ts` passes (loading -> ready, error state, abort on reload, `handleAuthError` called once for `kind` errors and not for others) and `npm run check` is clean; component rendering itself is verified by the e2e specs of tasks 10-12 (no throwaway route).
9. **Chart components** (`CumulativeChart`, `ChartFrame`, `ProvenanceCaption`) fed by `tests/fixtures/ytd-full.json` (hand-written in the exact 02 §5.5 shape, then replaced by a real `/api/v1/ytd` capture once 02's Rollups and YTD task (§5.5, task 10) exists). Done when: rain and GDD render lines, bands, the current line with its partial-day segment, mid-season start note, season states and captions; browser console shows zero CSP violations against Caddy.
10. **Dashboard and station detail** (4.7), including GDD mismatch and no-baseline states. Done when: `dashboard.spec.ts` passes.
11. **Follows UI.** Done when: `follows.spec.ts` passes.
12. **Connect flow.** Done when: `connect.spec.ts` passes including the key-never-persisted assertions.
13. **Headers parity and bundle scan.** `scripts/check-bundle-secrets.mjs` (including the production-context demo-value checks, 6.4); confirm `ops/caddy/Caddyfile` imports the generated file (05; report mismatches, do not edit). Done when: `npm run headers:check` and `npm run test:bundle` pass, `CONTEXT=production PUBLIC_SUPABASE_URL=http://localhost:8000 node scripts/check-bundle-secrets.mjs` fails, and `csp.spec.ts` passes against the stack.
14. **Forecast-change history** (last). `forecast.ts`, `ForecastHistoryChart.svelte`, route, tab, `forecast-history` spec; needs 02's forecast history task (§5.6, task 13) and poller snapshots (`fixtures/forecast-history.json` until then). Done when: `forecast.spec.ts` passes with the fixture (and, once seeded, a real-data smoke assertion that the page renders either runs or "No forecast snapshots yet.").
15. **Netlify/Caddy hand-off.** Done when: `node scripts/headers.mjs check` passes and the `/api/*` redirect in `netlify.toml` equals the rule in 06 section 8.2 (apart from the commented `signed` line); the extra `/_app/*` 404 and `/*` fallback rules are web-owned and 06 is told (PR note). `grep -n '<project-ref>' web/netlify.toml web/security-headers.json ops/caddy/security-headers.caddy` lists only the production CSP occurrences (Rodney fills them before the first deploy).

---

## 6. Testing

### 6.1 Vitest (`npm test`, and `npm run test:tz` which runs the suite under `TZ=America/Denver` and `TZ=UTC`)

The suite runs in the `node` environment with `src/test/setup.ts` (4.1): it stubs `location`, in-memory `localStorage`/`sessionStorage`, `vi.mock('$app/navigation')` and `vi.mock('$env/static/public')`; modules under test take `origin` and storage parameters with those globals as defaults.

Transforms (fixtures shaped like `YtdStation`):
- `buildSlots`: union and sorted; never `02-29`; Apr-Oct fixture gives 214 slots; empty input `[]`.
- `toPoints`: values land on the right index; missing slots `null` (no zero fill, no interpolation); `null` input `[]`.
- `bandPaths`: with `[10,25,50,75,90]` outer 10/90, inner 25/75, median 50; with `[10,50,90]` inner omitted; `bands: null` and `bands.gdd_cum: null` give no layers.
- `describePosition`: below p10, between p25 and p50, above p90, equal to a percentile; `null` without bands.
- `buildProvenanceLines`: baseline name/id/label/years; `baseline: null` text; recognised and unknown `day_definition`; GDD method lines only for gdd; winter note only for rain with a winter month in the window.
- `gddViewState`, one case each: `available: true` (lines + bands); `no_baseline` (current only, no-baseline caption); `gdd_method_mismatch` (current only, both method names, null prints "unknown"); an unknown or missing `reason` (current only, generic note). Metric selection: with `bands.rain_cum_in` non-null and `bands.gdd_cum` null, rain draws bands and GDD does not; `years_included`, `missing_days` and prior-year series are read from the active metric's field (a year with `rain_cum_in` null and `gdd_cum` non-null is drawn only on GDD). `splitCurrent`: a point after `last_complete_date` is partial; `last_complete_date: null` makes the whole series partial; a station with `first_date` after the window start produces no points before it and the "Data starts" note. `describePosition` returns `null` for `last_complete_date: null` and for a late `first_date`.
- `localYear`/`localMd`/`formatInZone` use the station's IANA zone and are stable under both `TZ` values.
- `forecast.ts`: `groupRuns` keeps newest-first order; `totalP50` ignores nulls and flags approximate.

Auth/API:
- `safeNext` accepts `/stations/abc?x=1` and `/`; rejects `//evil.com`, `/\evil`, `/\t/evil.com` (tab), `/%5C%5Cevil`, `/%5Cevil.com`, `/.//evil.com`, `/a/..//evil.com`, `/%2e//evil.com`, `/a/../b`, `https://evil.com`, `javascript:alert(1)`, `/a\b`, control characters, empty, `null`, > 200 chars; never returns a fragment.
- `requestMagicLink` (mock supabase): calls `signInWithOtp` with `shouldCreateUser: false` and exactly `emailRedirectTo = <origin>/auth/callback` (no query); returns `'sent'` for success, for "Signups not allowed for otp", for a 400 and for a 429; returns `'network'` only when `isAuthRetryableFetchError(error)` is true, whether the error is returned or thrown (a thrown non-retryable error is treated as a generic `'sent'` result); stores `weather.next` in localStorage with an `exp` 15 minutes ahead; the callback reader ignores expired/malformed entries.
- `assertUuid`/path safety: accepts a UUID; rejects `../me`, `abc`, `<uuid>/../x`, `%2e%2e`, `a%2Fb`, `a\b`. Segment checks run on the raw pathname split on `/` (leading empty segment skipped): `/api/v1/stations/%2e%2e/x`, `/api/v1/stations/./x`, `/api/v1/stations/a/../b`, `/api/v1/stations/%2E/x`, `/api/v1/stations/a%5Cb`, `/api/v1/stations/a%2Fb`, `/api/v1/stations//x` and `/api/v1/stations/%zz` are rejected before any fetch, and the validation runs on the raw string (a test with a spied `URL` constructor shows it is not reached for these inputs). `apiFetch` rejects (no network call) `https://evil.test/api/v1/me`, `//evil.test`, `/api/v2/me`, `/api/v1/../me`, `/api/v1/%2e%2e/me`, `/api/v1//x`, and accepts `/api/v1/ytd?station_ids=<uuid>,<uuid>&year=2026&start=04-01&end=10-31`. A `getYtd` endpoint test asserts the exact request URL string for two ids (`...?station_ids=<a>,<b>&year=2026&start=04-01&end=10-31`, comma unencoded, ids in the given order).
- Auth error handling (mock supabase): `getSession()` returning `AuthRetryableFetchError` -> `ApiError(0, 'network')` and **no** `signOut`; no session and no error -> sign-out and `kind: 'session_expired'`; 401 then `refreshSession()` returning a retryable error -> `network`, no sign-out; refresh returning a non-retryable `AuthApiError` -> sign-out; the API-initiated sign-out sets the flag so the `SIGNED_OUT` listener does not navigate, while an unflagged `SIGNED_OUT` on an `(app)` route navigates to `/login` once; 503 `auth_unavailable`/`overloaded`, 408, 413 never sign out; `requestMagicLink` returns the network result only for `isAuthRetryableFetchError`.
- `apiFetch` (mock `fetch` and supabase): sends `Authorization: Bearer <access_token>` and never the anon key; `credentials: 'omit'`; 401 then refresh then 200 is exactly 2 fetches; 401 twice or refresh failure -> `signOut('local')` and `ApiError.kind === 'session_expired'` with no `goto` call from the client; 403 `account_inactive` -> sign-out and `kind === 'account_inactive'`; 403 `forbidden` and 404 do not sign out; 429 exposes `retryAfterSec`; network error -> `network`; non-JSON error body -> `unknown`; a server `message` containing `https://rt.ambientweather.net/...?apiKey=SECRET` never appears in `ApiError.message` or `userMessage`; request bodies with `api_key` never appear in errors or `console.*` calls (spy).
- `handleAuthError`: navigates exactly once for each kind and returns `false` for other errors.
- `cache.ts`: second call with the same key returns the cached value without a fetch; different `start` is a different key; TTL expiry refetches; sign-out clears; concurrent identical calls share one request.

### 6.2 Scan test

A Vitest test scans `src/**/*.svelte` and `src/app.html` **after stripping every `<style>...</style>` block** (component `<style>` blocks compile to external CSS and may contain anything) and fails on: `style=`, `style:`, `{@html`, `innerHTML`, `eval(`, `new Function`, `transition:`, `animate:`, and any `<style` tag in `app.html`.

### 6.3 Playwright e2e (against the Docker Compose stack)

Entry point: `make test-e2e CONFIRM=1` from the repo root (05 section 7.2). It is destructive for the working stack (Playwright needs `localhost:8000`), so `CONFIRM=1` is required; it chains `reset up web-build seed e2e-data` itself, then runs host `npm ci`, `npx playwright install chromium` and `npm run test:e2e` with `E2E_BASE_URL` and `E2E_MAILPIT_URL`. Host: Node matching `web/.nvmrc`. The stack runs `AWN_MODE=replay`. Seed users and defaults live only in `tests/helpers/env.ts` (04 section 12.1; nothing is restated in `.env.example`): `E2E_USER_A_EMAIL` (`alex@example.test`), `E2E_USER_B_EMAIL` (`pat@example.test`), `E2E_UNINVITED_EMAIL` (`nobody@example.test`), `E2E_VALID_AWN_KEY` (`fixture-valid-key`), `E2E_INVALID_AWN_KEY` (`fixture-invalid-key`), each overridable from the environment. Other seed users are found by display name: `Dana` (`dana@example.test`; follow states Dana -> Alex pending, Dana -> Sam declined, Dana -> Rodney revoked) and `Casey` (`casey@example.test`, invited then deactivated). Casey enables an optional real `account_inactive` test (a link request works at GoTrue, the first API call returns 403); when Casey cannot log in locally the spec is skipped and the mocked `/me` 403 test is the coverage.

`playwright.config.ts`: `baseURL = E2E_BASE_URL`; throws unless the host is exactly `localhost` or `127.0.0.1`; Chromium only; `workers: 1`; `retries: 0`; `trace: 'retain-on-failure'`; output dirs `web/test-results/` and `web/playwright-report/`. Projects: `setup` (`auth.setup.ts`) and `chromium` (`dependencies: ['setup']`). `auth.setup.ts` logs in once per user (alex, pat, dana) using the Mailpit helper, opening the link in a new page of the same context, and saves `web/tests/.auth/<name>.json` (`storageState`). Specs use `test.use({ storageState })` except login/session/connect, which use fresh contexts. 05 must git-ignore `web/tests/.auth/`, `web/test-results/` and `web/playwright-report/` (Q12).

Helpers (all docker-compose invocations run with the repo root as working directory, resolved from the repo root even though Playwright runs in `web/`):
- `helpers/mailpit.ts`: `clearMailbox()` (`DELETE {E2E_MAILPIT_URL}/api/v1/messages`); `waitForLink(email, timeoutMs = 15000)` polls `GET /api/v1/search?query=to:<email>`, fetches `GET /api/v1/message/{ID}`, and extracts the first `http://localhost:8000/auth/v1/verify...` URL from the message's `HTML` field (HTML-decode `&amp;`), falling back to the `Text` field when `HTML` is empty or has no match. Links are opened with `context.newPage()` (a new tab, as a real mail client does).
- `helpers/api.ts`: may use `fetch` with a token read from the page's `sb-*` localStorage entry (tests are exempt from the `fetch` lint rule).
- `helpers/invite.ts`: `docker compose --profile tools run --rm -T admin admin invite --email <e> --display-name <n>` with `cwd` = repo root; refuses unless `E2E_BASE_URL` is localhost.

Data strategy: chart, band, season-state and GDD-branch assertions use committed API-shaped fixtures (`tests/fixtures/ytd-full.json`, `ytd-no-baseline.json`, `ytd-gdd-mismatch.json`, `ytd-gdd-unknown-reason.json`, `ytd-partial-station.json` (first_date after start, open partial day), `forecast-history.json`), all in the exact 02 §5.5/§5.6 shapes (per-series `missing_days` and `years_included` objects, nullable per-series arrays), served with `page.route('**/api/v1/ytd*', ...)`; a Vitest fixture test validates each fixture against the TypeScript types and a runtime shape check (objects, not numbers, for `missing_days`/`years_included`; `current.first_date`/`last_date`). Real data (from `make e2e-data`) is exercised by one smoke assertion per area. Mocked specs say "mocked" in their title.

Specs:
1. `login.spec.ts` (fresh context): alex requests a link; the link opened in a new page lands on the dashboard; an `sb-` key exists; reload keeps the session. `next`: visiting `/follows` signed out redirects to `/login?next=%2Ffollows`; after the link (new tab) the app lands on `/follows`. Uninvited email: identical neutral message to alex's, and Mailpit receives no message within 5 s. S4 oracle (mocked GoTrue `/auth/v1/otp` 429, 400 and 200 give identical text; only an aborted request shows the network message). Mocked: `/api/v1/me` 403 `account_inactive` ends on `/account-inactive` and removes `sb-` keys. Optional real: Casey.
2. `session.spec.ts`: sign-out removes `sb-*` keys and `/` redirects to `/login?next=%2F`; mocked `/api/v1/me` 401 twice ends on `/login` with the "session expired" text; mocked `/api/v1/me` 503 `auth_unavailable` shows the busy message and keeps the session (no sign-out); `next` values `//evil.com` and `/\t/evil.com` end on `/` after login; `/stations/not-a-uuid` shows "That isn't available." with no `/api/v1` request.
3. `dashboard.spec.ts`: real-data smoke (alex sees at least one chart whose caption starts with "Baseline:"). Mocked `ytd-full.json`: provenance lines; Apr-Oct label; Full year shows the winter note on Rain but not GDD; band, prior and current paths exist; data-table numbers equal fixture values; `current.missing_days[metric]` note (rain and GDD counts differ in the fixture); toggling Rain -> GDD -> Rain issues no repeat `ytd` request for a window already fetched (count route hits). `ytd-no-baseline.json`: no-baseline caption, no band paths, GDD shows current only. `ytd-gdd-mismatch.json`: GDD tab current line only plus the mismatch note with both method names; Rain unaffected. `ytd-gdd-unknown-reason.json`: generic note. `ytd-partial-station.json`: the line starts at `first_date`, shows "Data starts ...", no position text, and the open-day segment has class `line-current-partial`. Season states via `page.clock`: "Season not started" and "Season ended". Mocked `/me` `credential.status = invalid`: reconnect banner and link on own stations only. Stale `latest_observation_at`: "No recent data." `ytd` 404: error panel; Retry reloads `stations`. Mocked 500, 503 `overloaded` and 429 with `Retry-After: 7`: matching messages.
4. `follows.spec.ts` (alex's and dana's contexts; once per reset because it changes seeded states): alex opens `/follows`, sees Dana pending with a requested date, approves; Dana's `/api/v1/stations` now includes alex's stations and Dana's "People I follow" list shows badges **approved** for Alex, **declined** for Sam and **revoked** for Rodney (rows found by display name, not position); alex revokes via the inline confirm, Dana's list drops alex's stations immediately and Alex's badge becomes revoked. (Sam and Rodney are not logged in; only Dana's view is asserted for them.)
5. `empty-state.spec.ts`: pat (no stations, no follows) sees "No stations yet." with the Connect link and "Ask Rodney to share stations with you."
6. `connect.spec.ts`: setup creates a dedicated fresh invitee via `helpers/invite.ts` (random `connect-<n>@example.test`), logs in through Mailpit, and uses only that account. Steps: `E2E_INVALID_AWN_KEY` -> exactly the generic message, page text never contains `ambientweather`, `http`, `401`, `apiKey`; `fixture-ratelimited-key` -> the busy/rate-limit text; `E2E_VALID_AWN_KEY` -> the two fixture MACs `AA:BB:CC:00:01:01` and `AA:BB:CC:00:01:02`; the fixture devices carry AWN time zones, so no picker is shown; select both, save, success message; `/api/v1/stations` for that user lists two stations. Mocked variants: a validate response with `timezone: null` shows the picker for that device only and the save body carries the chosen zone; a save 422 `timezone_required` marks the device "Choose a time zone."; a save response with `not_covered` shows "These stations are no longer on this account; ask Rodney to disable them:" and the names. Then: `localStorage`, `sessionStorage` and `page.url()` do not contain the key; console messages and all request URLs do not contain it (it appears only in the POST bodies of `connect/validate` and `connect/save`); after navigating away and back the input is empty; the input has `type="text"`, `autocomplete="off"` and class `secret-input`. The real fixture MACs can be registered once per `make reset`; a second run yields 409 `device_already_registered`, which the spec reports as "run make reset && make up && make web-build && make seed && make e2e-data".
7. `forecast.spec.ts` (mocked `forecast-history.json`): one run line per run, newest first and strongest class; ribbon only on the newest run; `prob_precip` panel; run table with approximate totals; caption has the response `attribution` and `model`; request has `variable=precip_6h_in`, `runs=6` and a span <= 16 days; `{runs: []}` shows "No forecast snapshots yet."; mocked 404 shows "That isn't available.".

### 6.4 CSP, headers and bundle tests

`csp.spec.ts` (against Caddy at `:8000`):
- Before navigation `page.addInitScript` records `securitypolicyviolation` events into `window.__csp`; the test also listens to `page.on('console')` for "Content Security Policy". As alex visit `/login` (signed out), `/`, `/stations/{id}`, `/stations/{id}/forecast`, `/follows`, `/connect`; toggle season and metric, move the chart cursor, and trigger a client-side navigation (so the route announcer is created). Expect `window.__csp` empty and no CSP console messages, with the announcer's inline style allowed only through the sanctioned `style-src-attr` hash.
- Negative controls: `page.addScriptTag({ content: 'window.__x = 1' })` fails and records a script violation; `page.addStyleTag` is blocked (`style-src-elem`); `document.body.setAttribute('style', 'display:none')` records `style-src-attr` (a different value than the announcer's is blocked); an `<img src="https://example.com/x.png">` records `img-src`.
- Headers (local set from `security-headers.json`) on `GET /`, `GET /200.html`, `GET /stations/anything` (all 200 with the fallback body, so `/` is covered) and a real `/_app/immutable/...` asset: every header name/value equals the JSON; CSP equals the local CSP exactly; no `Server` header; `Cache-Control` on HTML paths equals `expected_html_cache_control.caddy` (`no-cache`); on the immutable asset equals the `cache_control` entry and is not duplicated; `GET /api/v1/me` without a token has `Cache-Control: no-store` and `X-Content-Type-Options: nosniff` and none of the static-handler headers.
- Parity: `node scripts/headers.mjs check` passes (netlify.toml block, Caddyfile and JSON agree).
- Missing chunk: `GET /_app/immutable/missing.js` and `GET /_app/nope.json` return 404 (not the SPA page, not 200). (`/_app/version.json` exists in the build and returns 200, so it is not used as the missing example.)
- `build/200.html` document check as in `check-build.mjs`. `GET /version.txt` on the local stack returns 200 with `text/plain`, the content `local` (or the build's SHA), and no `Cache-Control: immutable`.

`scripts/check-bundle-secrets.mjs` (`npm run test:bundle`, also in the Netlify build command) reads every file under `build/` and fails on:
- Any JWT-shaped string (`eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+`) whose decoded payload `role` is not `anon` or that contains `service_role`; at most one distinct JWT; `sb_publishable_...` allowed; any `sb_secret_` fails.
- The strings `service_role`, `sb_secret_`, `BEGIN PRIVATE KEY`, `postgres://`, `postgresql://`, `AWN_APPLICATION_KEY`, `CREDENTIALS_PRIVATE_KEY`, `CREDENTIALS_PUBLIC_KEY`, `JWT_HS256_SECRET`, `SUPABASE_SERVICE_ROLE_KEY`, `NETLIFY_PROXY_SECRET`, `HEALTHCHECK_URL`, `AWS_SECRET`.
- With `CONTEXT=production` (Netlify sets it; locally `CONTEXT=production node scripts/check-bundle-secrets.mjs`): the SHA-256 of `PUBLIC_SUPABASE_ANON_KEY` must not appear in column 1 of `../backend/demo-fingerprints.txt` (format `<sha256>  <NAME>`; when the file is missing the production-context check fails), and `PUBLIC_SUPABASE_URL` must not have host `localhost`, `127.0.0.1` or `::1`. Outside production context these two checks are skipped.
- If a repo-root `.env` exists: only variables whose names look secret (end with `_PASSWORD`, `_SECRET`, `_KEY`, `_TOKEN`, or whose value is a URL containing `user:pass@`) and do not start with `PUBLIC_` are considered, and only values of length >= 12; none may appear in `build/`. (Non-secret settings such as ports and hostnames are ignored to avoid false positives.)
- Absolute `http(s)://` hosts outside the allow-list: `localhost`, the host of `PUBLIC_SUPABASE_URL`, `open-meteo.com`, `creativecommons.org`, `www.w3.org`, `svelte.dev`, plus strings that supabase-js and its dependencies are known to embed: `supabase.com`, `github.com`. Extend only with justification.
- Positive check: the build contains the value of `PUBLIC_SUPABASE_URL`.

Run all web checks: `cd web && npm run lint && npm run check && npm run test:tz && npm run build && npm run headers:check && npm run test:bundle`, then `make test-e2e` from the repo root.

---

## 7. Security checklist (verify before merge)

- [ ] `signInWithOtp` uses `shouldCreateUser: false`; login shows one message for success, unknown email and GoTrue 429; `emailRedirectTo` is the exact callback URL with no query (S4).
- [ ] `next` is carried in localStorage with a 15-minute TTL and validated by `safeNext` (same origin via `new URL`, `u.pathname` equals the raw path and does not start with `//`, no backslash/control/dot segments, no fragment); tests include `/\t/evil.com`, `/%5C%5Cevil`, `/.//evil.com`, `/a/..//evil.com`, `/%2e//evil.com`.
- [ ] Only `session.access_token` is sent to `/api/*`, never the anon key; `apiFetch` only reaches `/api/v1/` paths on the same origin, rejects dot segments and encoded separators, and every ID is a validated UUID (S1, S10).
- [ ] `apiFetch` is the only caller of `fetch`; `credentials: 'omit'`; no cookies set by the app (S2).
- [ ] All auth-failure navigation goes through `handleAuthError` in `auth/redirect.ts`.
- [ ] No `{@html}`, `innerHTML`, `eval`, `new Function`, inline `<script>`, `<style>`, `style=`, `style:`, Svelte transitions (lint + scan test).
- [ ] `security-headers.json` is the only place header values are edited; `netlify.toml` block, Caddyfile and live responses match it (`headers:check`, `csp.spec.ts`); the only `unsafe-*` token is `'unsafe-hashes'` for `style-src-attr` with the verified announcer hash (S15).
- [ ] Cache-Control is set only on `/_app/immutable/*` in `netlify.toml`; Caddy sets `no-cache` only on non-immutable paths and removes `Server`; `GET /` returns the SPA page with 200.
- [ ] HSTS (production), nosniff, DENY, Referrer-Policy, Permissions-Policy, COOP, CORP, noindex present.
- [ ] No third-party scripts, fonts, styles, images, analytics or CDNs (bundle allow-list).
- [ ] Only `PUBLIC_SUPABASE_URL` and `PUBLIC_SUPABASE_ANON_KEY` are read from env; private/dynamic env imports banned; bundle-secrets test green (S14).
- [ ] `package-lock.json` committed; exact versions; `npm ci`; `ignore-scripts=true`; `npm audit --omit=dev --audit-level=high` clean; Svelte/SvelteKit pinned (S2, S14).
- [ ] `netlify.toml` `/api/*` rule precedes `/*`, a `/_app/*` 404 rule precedes `/*`, `headers:check` passes; the `signed = "NETLIFY_PROXY_SECRET"` line is commented out until the API secret is deployed.
- [ ] AWN key: only in component memory; not in storage, URL, history state, console, errors or request URLs except the two POST bodies; clear on success/navigation; input is `type="text"` with `autocomplete="off"` and masking class (S7 web side, S9).
- [ ] Connect UI never shows upstream error text, status codes, URLs or hosts; no URL/host input (S9).
- [ ] A retryable supabase-js error (`AuthRetryableFetchError`) and any 503/408/413 never sign the user out; only 401 after a non-retryable refresh failure, no session, or 403 `account_inactive` do.
- [ ] The production build rejects the demo anon key (fingerprint) and a localhost `PUBLIC_SUPABASE_URL`; `<project-ref>` placeholder fails `CONTEXT=production`.
- [ ] 403 `account_inactive` signs out locally and shows the inactive page (S11); 404/403 elsewhere show "That isn't available." (S10).
- [ ] Coordinates are shown only as returned by the API (S10).
- [ ] Every chart shows the provenance caption (baseline, day definition, GDD method for GDD, winter note only on rain) (F5, F6); the GDD-mismatch and no-baseline states draw no comparison.
- [ ] Open-Meteo CC BY 4.0 attribution in the footer and on the forecast chart.
- [ ] e2e config refuses non-localhost base URLs; `helpers/invite.ts` refuses non-local stacks.

---

## 8. Open questions / assumptions

- **Q1.** Forecast-change history is built in v1 (design doc functional requirement) with `FEATURES.forecastHistory = true`. Defaults to confirm: 7-day horizon (`FORECAST_HORIZON_DAYS`), 6 runs, no model selector (no endpoint lists models), and showing a "sum of 6-hour medians" per run (an approximation, since a sum of medians is not the median of a sum). Needs poller data (03) to look real; the page shows "No forecast snapshots yet." until then.
- **Q2.** CSP shape: split between the SvelteKit meta tag (`script-src`) and the static header, with one sanctioned `style-src-attr` hash for the route announcer (conventions). Unverified: that `kit.csp` writes the meta tag into the adapter-static fallback page (task 3 has the fallback script), the exact announcer literal in the pinned SvelteKit (check-build fails loudly if it changes), and `netlify.toml` header behaviour for the fallback rewrite. A single header with `script-src 'self' 'sha256-...'` would be the cleaner end state if the Netlify build could emit it.
- **Q3. Follow discovery (product question for the owner).** The design does not say how a user finds someone to follow. 02 exposes `POST /follows` by exact display name, but whether the UI offers it (versus follows created only by `admin invite --follow-all`) is undecided. No request form is built; the empty state says "Ask Rodney to share stations with you."
- **Q4.** Wording and screenshots for "where to find your AWN API key". Needs a human to confirm the current Ambient UI.
- **Q5.** GDD: accumulation start (`01-01` assumed), default window, and display text for method names (`avg_50`, ...). Needs 02/03 to confirm `ytd` works for windows starting `01-01`.
- **Q6.** Band/prior-year rules (5 drawn years, all years in bands, `bands: null` under 10 years, `missing_days <= 7`) are 02's; the web draws whatever percentile list it receives.
- **Q7.** Presentation defaults not in the design doc: "same scale for all stations" off; all five prior years drawn faintly; no overlay of the airport's current year (02 does not return it).
- **Q8. New-station setup (product question for the owner).** The API derives each station's time zone from AWN data; the web shows a fallback zone picker only for devices whose `timezone` is null (list content `TIMEZONE_CHOICES`, undecided; may be a short US list or all IANA zones) and handles 422 `timezone_required`. How a baseline airport is assigned to a new station is undecided; until then a new station charts with "No baseline is set for this station yet." Reconnect has no dedicated route; it re-runs `connect/save` with the user's existing devices (02 replaces the credential and repoints the stations the new key covers; the rest come back in `not_covered`).
- **Q9.** `STALE_AFTER_MIN = 30` and the stale/delayed wording are placeholders.
- **Q10.** `<project-ref>` is a placeholder in `security-headers.json` (and thus in the generated production CSP in `netlify.toml`; the local Caddy CSP does not contain it). Rodney fills it with the Supabase project host and runs `npm run headers:generate` before the first deploy; the `CONTEXT=production` check failing on the placeholder is the intended guard. The signed-proxy line stays commented until the owner enables it with the API secret.
- **Q11. Off-season dashboard (product question).** What the dashboard should show November-March is undecided. The web shows explicit "Season not started" / "Season ended" states for the Apr-Oct window and a Full-year toggle; it makes no further choice. Year is taken from the first selected station's local date.
- **Q12.** Requests to 05: `make test-web` should also run `npm run test:tz` and `npm run headers:check`; `.gitignore` entries `web/tests/.auth/`, `web/test-results/`, `web/playwright-report/`; the Caddyfile must exclude `/_app/*` from the SPA fallback (05 is adding it; `headers.mjs check` and the CSP spec assert it) while keeping the `@root` rewrite and `try_files {path} /200.html` for other paths; `make test-e2e CONFIRM=1` chain as in 05 section 7.2; Casey login behaviour if the optional inactive-account test is wanted.
- **Q13.** AWN key field: `type="text"` plus masking class avoids password-manager capture but is not guaranteed in every browser; accepted risk for a five-person app.
- **Q14.** Possible OTP-code fallback for magic links (email scanners can consume single-use links; PKCE needs the same browser profile). Would need an email template change in 04.
- **Q15.** Unverified assumptions to confirm during implementation: GoTrue matches `redirect_to` against the exact allow-list URLs (the web never adds a query), PKCE magic links opened in a new tab of the same profile succeed because the code verifier is in localStorage, and the Netlify `signed` option behaves as 06 describes.
