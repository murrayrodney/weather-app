# 01 - Web (SvelteKit static SPA)

Audience: AI agents implementing and testing `web/`. Shared names, ports and env vars are defined in `00-conventions.md`; that file wins on any conflict. `02-api.md` is the authority for the HTTP contract. Decision IDs (S2, P2, ...) refer to `docs/Design Review Findings.md`.

Sources (verified = fetched 2026-10-02):
- SvelteKit SPA mode / adapter-static fallback (verified): https://svelte.dev/docs/kit/single-page-apps
- SvelteKit `kit.csp` (verified; prerendered/fallback pages use a `<meta>` CSP with hashes): https://svelte.dev/docs/kit/configuration#csp
- SvelteKit announcer inline style, known CSP issue (not fetched; confirm at implementation): https://github.com/sveltejs/kit/issues/11747 and https://github.com/sveltejs/kit/issues/11993
- Supabase `signInWithPassword`, `verifyOtp` (token hash), `updateUser` (not re-fetched; confirm at implementation): https://supabase.com/docs/reference/javascript/auth-signinwithpassword , https://supabase.com/docs/reference/javascript/auth-verifyotp , https://supabase.com/docs/reference/javascript/auth-updateuser
- Netlify `[[headers]]` (verified; custom headers do not apply to proxied content): https://docs.netlify.com/manage/routing/headers/
- Netlify redirects, rewrites and signed proxies (not fetched; confirm `force`, ordering, `signed`): https://docs.netlify.com/manage/routing/redirects/redirect-options/ , https://docs.netlify.com/manage/routing/redirects/rewrites-proxies/
- Supabase `signOut` scopes (not fetched; confirm): https://supabase.com/docs/reference/javascript/auth-signout
- Playwright authentication / `storageState` (not fetched): https://playwright.dev/docs/auth

---

## 1. Purpose and scope

`web/` is a SvelteKit app built with `@sveltejs/adapter-static`, `ssr = false`, SPA fallback `200.html`. It is login-only and renders everything from `/api/v1/*` JSON in the browser. The audience is about five family users on desktop; keep everything lean.

Owns:
- Supabase Auth client usage in the browser (email + password sign-in, the set-password page for hand-delivered links, change password, session, sign-out).
- One typed API client module that attaches the Bearer token and maps errors.
- Views: login, dashboard (side-by-side stations, with last season shown out of season), station detail (rain, GDD with a method/base/start selector, baseline panel), live forecast with ensemble bands and the day-by-day forecast-change view, account page (change password), follows (including finding people by name), connect-station flow (including choosing a baseline airport station), empty/error/inactive states.
- Chart components and the small transforms behind them.
- `web/security-headers.json` (single source of truth for headers/CSP), `web/netlify.toml`, header parity checks.
- Web tests: Vitest, Playwright, CSP-violation test, bundle-secrets test.

Does not own (reference these files):
- Endpoints, auth checks, error codes, rate limits: `02-api.md`.
- Ingest and reference data (including the ACIS client): `03-poller.md`. Forecasts and the forecast-change data: `02-api.md` 5.6 (nothing is stored). GDD is computed by the API at query time (`02-api.md` 5.5, `04-database.md` `app.gdd_day`); the web only sends the settings.
- Schema, Supabase project auth settings (sign-ups off, password rules, link expiry, no SMTP; the emailed-links growth path), the `admin invite`/`reset-link` commands that print the set-password link, seed data: `04-database.md`.
- Docker Compose, `ops/caddy/Caddyfile`, `make` targets, how `web/build` is produced and served locally: `05-local-dev.md`.
- Netlify site settings (base dir, production branch `production`, PR deploy previews on / branch deploys None, env vars), Fly URL, CI: `06-deploy-ops.md`. `netlify.toml` (owned here) carries the `[build] ignore` command that skips previews for `dependabot/*` branches.

## 2. Decisions applied

| ID | How it lands in the web guide |
| --- | --- |
| S1 | Web only forwards `session.access_token`; the anon key is never sent to `/api/*`. Bad-token tests live in 02; web tests cover the UI reaction to 401. |
| S2 | Supabase JS holds the session in browser storage; Bearer header on every API call. Compensating controls: strict CSP (4.8), no third-party scripts/fonts/CDNs, pinned dependencies and lockfile, no `{@html}`. No cookies. |
| S3 | Token on every route. The `/api/*` redirect carries a commented-out `signed = "NETLIFY_PROXY_SECRET"` line, enabled later together with the API secret (conventions "Web headers and proxy"). |
| S4 | Login is `signInWithPassword`; every GoTrue error with an HTTP response (wrong password, unknown email, banned user) shows the same message; the app never calls `signUp` or `signInWithOtp` (proposals/05). Sign-ups stay off in Supabase. |
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
| Owner, Oct 7 (follow discovery) | Follows page searches active users by name and sends a follow request (4.7). No suggestions. |
| Owner, Oct 7 (baseline) | After a station is saved, the connect flow offers the nearest airport station and nearby alternatives; the owner can change it later from the station page (4.7). |
| Owner, Oct 7 (off-season) | November to March the dashboard shows the most recent completed April-October season, clearly labelled (4.6, 4.7). |
| Owner, Oct 7 (GDD) | A GDD settings selector (method, base, start date) drives the `gdd_*` query parameters; defaults simple average, 50 F, April 1 (4.6, 4.7). |
| Design doc functional requirements | The live forecast chart (with p10-p90 ribbons) and the forecast-change view are v1 and are built (4.7, flag default on); both come from the API, which fetches Open-Meteo live and stores nothing (proposals/06). Open-Meteo CC BY 4.0 credit in the footer and on forecast charts. |
| Owner, Oct 7 (login) | Email + password; Rodney sends a one-time set-password link by hand; change password on the account page; no email, no magic link (proposals/05, b2). Emailed links are a documented growth path in 04 8.1a, not built. |

## 3. Cross-component contracts

### 3.1 Consumed from `00-conventions.md`

- Single origin: browser talks to `/api/*` (proxied) and to Supabase Auth. Locally the Supabase URL is `http://localhost:8000`; production: the Supabase project URL.
- Build-time env: only `PUBLIC_SUPABASE_URL` and `PUBLIC_SUPABASE_ANON_KEY`, passed as process env.
- Shared error codes: `unauthorized` (401), `account_inactive` (403), `forbidden` (403), `not_found` (404; "doesn't exist" and "not viewable" are identical), `rate_limited` (429 with `Retry-After`), `validation_failed` (422). Codes from 02: `invalid_key`, `unknown_device`, `not_a_candidate`, `timezone_required` (422), `device_already_registered`, `conflict` (409), `upstream_busy` (503), `upstream_unavailable` (502), `request_timeout` (408), `payload_too_large` (413), `overloaded`, `auth_unavailable` (503), `method_not_allowed` (405; generic message). The web keys only on these, never on other strings.
- Every API response sets `Cache-Control: no-store` and `X-Content-Type-Options: nosniff` itself (the e2e header test asserts it).
- GDD is computed by the API at query time from stored daily tmax/tmin (no stored GDD); the web chooses `gdd_method` (`simple_avg` default, `clamped_50_86`), `gdd_base` (default 50) and `gdd_start` (default `04-01`), the response echoes them in `gdd`, and station and baseline always use the same values; the web never computes GDD.
- Forecasts are not stored (proposals/06, option A, Oct 7): the API serves a live forecast with ensemble p10/p50/p90 bands (`/forecast`) and a day-by-day "forecast N days ago" view (`/forecast/changes`); the browser never calls Open-Meteo, so there is no Open-Meteo host in `connect-src`. Percentiles exist only in the live forecast chart; nothing keeps a percentile history. Forecast days are local wall-clock days.
- Sign-in is email + password (proposals/05, option b2, Oct 7): no magic link, no PKCE callback, no redirect allow-list, no email. The Supabase client makes only direct calls (`signInWithPassword`, `verifyOtp` with a `token_hash`, `updateUser`, `getSession`, `signOut`). People get a one-time set-password link from Rodney by hand (`admin invite` / `admin reset-link`, 04 10.3); the link is `https://<site>/set-password#token_hash=...&type=recovery`.
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
| `GET /api/v1/ytd` | `station_ids` (comma-separated, 1..5 UUIDs), `year` (REQUIRED, 2000..current), `start`, `end` (`MM-DD`), optional `gdd_method` (`simple_avg`\|`clamped_50_86`), `gdd_base` (integer 32..70), `gdd_start` (`MM-DD`); defaults are omitted from the URL | `YtdResponse` |
| `GET /api/v1/users/search` | `q` (2..40 characters after trimming) | `{ users: { id; display_name; follow_status: "none"\|"pending"\|"approved"\|"declined"\|"revoked" }[] }` (at most 10; never an email) |
| `GET /api/v1/stations/{station_id}/baseline/candidates` | - | `BaselineCandidates` (owner only; 502 `upstream_unavailable` when ACIS is down) |
| `PUT /api/v1/stations/{station_id}/baseline` | `{ reference_station_id }` | `Station` (owner only; 422 `not_a_candidate`) |
| `GET /api/v1/stations/{station_id}/forecast` | - (any query key is 422) | `Forecast` (cached 30 min by the API; `ensemble` or `deterministic` may be `null`; `stale: true` when served after a failed refresh; 502 `upstream_unavailable`, 503 `upstream_busy`) |
| `GET /api/v1/stations/{station_id}/forecast/changes` | `from`, `to` (local dates, span <= 14 days, `from >= 2024-01-01`, `to <= today + 15`) | `ForecastChanges` |
| `GET /api/v1/follows` | - | `{ following: FollowRow[]; followers: FollowRow[] }` |
| `POST /api/v1/follows` | `{ followee_id }` (an `id` from user search) | 201/200 `{ user; status }`; 404 `not_found` for an unknown or inactive id |
| `POST /api/v1/follows/{follower_id}/approve` \| `decline` \| `revoke` | empty | `FollowRow`; `{follower_id}` is the other user's `user.id` (there is no follow id); zero-row transitions are 404 `not_found` |
| `POST /api/v1/connect/validate` | `{ api_key }` | 200 `{ valid: true; devices: { mac; name; timezone: string \| null }[] }` or `{ valid: false }` |
| `POST /api/v1/connect/save` | `{ api_key; devices: { mac; name?; timezone? }[] }` (1..10; `timezone` is only a fallback when AWN gave none) | 201 `{ credential: { status: "ok" }; stations: Station[]; not_covered: { id: string; name: string }[] }` (`not_covered` = the caller's stations that stayed on the replaced credential; empty on first connect) |
| `GET /api/v1/connect/status` | - | `{ credential: null \| { status; last_ok_at: string \| null; needs_reconnecting: boolean } }` |

Not consumed in v1: `rollups/daily`, `observations`.

```ts
interface Baseline { reference_station_id: string; name: string; source: string; history_loaded: boolean }   // source e.g. "ghcnd_acis"; history_loaded false while the poller loads a newly chosen baseline
interface Station {
  id: string; name: string; is_own: boolean;
  owner: { id: string; display_name: string };
  lat: number; lon: number; coordinates_rounded: boolean;
  timezone: string;                          // IANA
  baseline: Baseline | null;
  latest_observation_at: string | null;
}
interface MdValue { md: string; v: number }               // md = "MM-DD"
interface GddSettings { method: 'simple_avg' | 'clamped_50_86'; base_f: number; start: string }   // start = "MM-DD"; echoed by the API
interface PerSeries<T> { rain: T; gdd: T }
interface YtdStation {
  station_id: string;
  baseline: Baseline | null;
  current: { first_date: string | null; last_date: string | null; last_complete_date: string | null;
             missing_days: PerSeries<number>;
             rain_cum_in: MdValue[]; gdd_cum: MdValue[] };
  prior_years: { year: number; missing_days: PerSeries<number>;
                 rain_cum_in: MdValue[] | null; gdd_cum: MdValue[] | null }[];     // null per series when that year is ineligible
  bands: { percentiles: number[]; years_included: PerSeries<number>;
           rain_cum_in: { md: string; p: number[] }[] | null;                    // null when < 10 eligible years for that series
           gdd_cum: { md: string; p: number[] }[] | null } | null;
}
interface YtdResponse { year: number; start: string; end: string; gdd: GddSettings; day_definition: string; stations: YtdStation[] }
interface BaselineCandidate { reference_station_id: string; name: string; airport_code: string | null; distance_mi: number;
                              elevation_ft: number | null; elevation_diff_ft: number | null; record_start: string; record_years: number; short_record: boolean }
interface BaselineCandidates { station_id: string; station_elevation_ft: number | null; radius_mi: number; current: string | null; suggested: string | null; candidates: BaselineCandidate[] }
interface UserHit { id: string; display_name: string; follow_status: 'none' | 'pending' | 'approved' | 'declined' | 'revoked' }
interface FollowRow { user: { id: string; display_name: string }; status: "pending"|"approved"|"declined"|"revoked"; requested_at: string; decided_at: string | null }
interface Band { p10: number | null; p50: number | null; p90: number | null }
interface BandSeries { p10: (number | null)[]; p50: (number | null)[]; p90: (number | null)[] }
interface Forecast {
  station_id: string; timezone: string; generated_at: string; stale: boolean; attribution: string;
  units: { temperature: 'F'; precipitation: 'in' };
  deterministic: { model: string;
    hourly: { time: string[]; temperature_f: (number | null)[]; precip_in: (number | null)[]; prob_precip: (number | null)[] };   // time = UTC ISO 8601
    daily: { date: string; tmax_f: number | null; tmin_f: number | null; precip_in: number | null; prob_precip_max: number | null }[] } | null;
  ensemble: { model: string; members: number;
    hourly: { time: string[]; temperature_f: BandSeries; precip_in: BandSeries };
    daily: { date: string; tmax_f: Band; tmin_f: Band; precip_in: Band; prob_precip: number | null }[] } | null;
}
interface ForecastChanges {
  station_id: string; timezone: string; model: string; generated_at: string; attribution: string;
  units: { temperature: 'F'; precipitation: 'in' };
  lead_days: number[];            // [0..7]; 0 = latest run, N = about N days earlier
  days: { date: string; precip_in: (number | null)[]; prob_precip_max: (number | null)[]; tmax_f: (number | null)[]; tmin_f: (number | null)[] }[];   // arrays indexed by lead_days
}
```

Contract facts the UI depends on:
- Cumulative sums, percentile bands, prior-year selection and eligibility are computed by the API; the web neither computes GDD, percentiles nor cumulative sums. Everything per-series is a `rain`/`gdd` pair or an array nullable per series: `current.missing_days` and `prior_years[].missing_days` are `{rain, gdd}`; `bands.years_included` is `{rain, gdd}`; `prior_years[].rain_cum_in`/`gdd_cum` are `null` for a year ineligible for that series (missing days > 7); `bands.rain_cum_in`/`gdd_cum` are each `null` when fewer than 10 years are eligible for that series; `bands` is `null` when both are. Candidate years include years later than `year` when `year` is historical (the key name `prior_years` is kept); the 5 most recent eligible years are returned, newest first.
- There is no GDD method-mismatch state: station and baseline are computed with the same `gdd` settings in one request. `baseline: null` (no baseline chosen yet) or `baseline.history_loaded === false` (chosen, history still loading) both mean `prior_years: []` and `bands: null` with `current` still returned; a GDD series is `null` for a prior year or band only when too few days exist (as for rain).
- GDD accumulates over `[gdd.start, end]` and rain over `[start, end]` in the same response, so switching between the Rain and GDD tabs never needs a second request; `current.first_date` is the first rain date.
- `current` starts at the station's first rollup date on or after `start` (`first_date`; earlier days are neither points nor missing, so a station installed mid-season has a partial total) and has points through `last_date` (the latest row in the window). `last_complete_date` marks the last complete day: a point after it (the open day) is a partial value. `current.missing_days.rain` / `.gdd` count NULL or absent days between `first_date` and `last_date`; they contributed 0 to the running sum. `is_complete = false` is not "missing".
- `first_date`, `last_date` and `last_complete_date` may each be `null` (no data in the window / no complete day yet); series arrays may be empty.
- `day_definition` and `baseline.source` are machine strings. Display map in `src/lib/labels.ts`: `local_standard_time_midnight_to_midnight` -> "Days are midnight to midnight in local standard time (no daylight-saving shift)."; `ghcnd_acis` -> "NOAA GHCN-Daily via ACIS". Unknown values print with underscores replaced by spaces and no added claims. GDD method names print through `labels.gddMethod`: `simple_avg` -> "Simple average", `clamped_50_86` -> "Clamped to the base and 86 F"; an unknown value prints with underscores replaced by spaces.
- No per-station connection field. Needs-reconnecting is known only for the caller's own credential (`/me`). Followers see staleness through `latest_observation_at` only.
- A hidden or unknown station, or one hidden id in a multi-station `ytd` call, is 404 `not_found` for the whole request.
- 02 limits (copy of 02 4.9, 4.6.2, 4.2): a per-user limiter, every request costs 1, 120 requests per minute, burst 40; user search additionally 30 per minute (burst 10) and baseline lookups 6 per minute (burst 3), both with the same 429 `rate_limited` and `Retry-After`; connect routes additionally 5 per minute and 20 per hour (durable). One overall 25 s request timeout on `/api/v1` (per-operation bounds inside), body limit 16 KiB. The web's client timeout is 30 s for every request (above the API's 25 s, so the API's `request_timeout` envelope arrives first).
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
- Auth flow for 05's smoke test: the web signs in with `signInWithPassword` (a direct `POST /auth/v1/token?grant_type=password`); there is no emailed link and no redirect.

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
│   │   ├── auth/{supabase.ts, session.svelte.ts, next.ts, redirect.ts, login.ts, password.ts}
│   │   ├── api/{client.ts, errors.ts, endpoints.ts, types.ts, ids.ts, cache.ts}
│   │   ├── data/resource.svelte.ts
│   │   ├── transforms/{dates.ts, season.ts, gddSettings.ts, slots.ts, series.ts, bands.ts, position.ts, provenance.ts, forecast.ts, *.test.ts}
│   │   ├── charts/{ChartFrame.svelte, CumulativeChart.svelte, ForecastChart.svelte, ForecastChangesChart.svelte, ProvenanceCaption.svelte, scales.ts}
│   │   └── components/{StationCard, StationPicker, SeasonToggle, GddSettings, BaselinePicker, UserSearch, EmptyState, ErrorPanel, LoadingBlock, NeedsReconnectBanner, FollowItem, Footer}.svelte
│   └── routes/
│       ├── +layout.ts                  # ssr=false, prerender=false, trailingSlash="never"
│       ├── +layout.svelte              # imports app.css, renders <Footer/>
│       ├── (public)/login/+page.svelte
│       ├── (public)/set-password/+page.svelte   # redeems the hand-delivered link (4.2)
│       ├── (public)/account-inactive/+page.svelte
│       └── (app)/{+layout.ts, +layout.svelte, +page.svelte, stations/[id]/+page.svelte, stations/[id]/forecast/+page.svelte, connect/+page.svelte, follows/+page.svelte, account/+page.svelte}
└── tests/
    ├── fixtures/{ytd-full.json, ytd-no-baseline.json, ytd-baseline-loading.json, ytd-gdd-clamped.json, ytd-partial-station.json, forecast.json, forecast-ensemble-null.json, forecast-changes.json, candidates-loveland.json, candidates-empty.json, user-search.json}   # committed API-shaped fixtures
    ├── e2e/{auth.setup.ts, *.spec.ts}
    ├── .auth/                          # storageState files, git-ignored
    └── helpers/{env.ts, csp.ts, api.ts, invite.ts}   # invite.ts runs `admin invite`/`admin reset-link` and returns the printed set-password link (no mail catcher)
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
  auth: { persistSession: true, autoRefreshToken: true, detectSessionInUrl: false }   // password sign-in and verifyOtp are direct calls: no PKCE, no URL session detection
});
```

Rules: one module-level client; only these auth calls exist in `src/`: `signInWithPassword`, `verifyOtp`, `updateUser({ password })`, `getSession`, `getUser`, `refreshSession`, `signOut`, `onAuthStateChange` (a lint rule forbids `signInWithOtp`, `signUp`, `resetPasswordForEmail`, `signInWithOAuth` and `exchangeCodeForSession`); the anon key is never sent to `/api/*`; the session lives in Supabase's default `localStorage` entry (`sb-*`), the accepted S2 exposure; do not call other async Supabase functions synchronously inside an `onAuthStateChange` callback (defer with `setTimeout(..., 0)`).

`session.svelte.ts` exports `auth` (`$state`: `status: 'loading'|'signed_out'|'signed_in'`), `initAuth()`, `getAccessToken()` (reads `supabase.auth.getSession()` each call; never cached in module state), and `signOut(scope?: 'local'|'global')` (default `'local'`; also clears the `ytd` cache and forecast cache).

**Single redirect owner (`auth/redirect.ts`).** `apiFetch` never navigates. When the session is unrecoverable it signs out (marking the sign-out as client-initiated: `session.svelte.ts` sets a module flag `signingOutFromApi = true` before calling `supabase.auth.signOut`, and clears it after the resulting `SIGNED_OUT` event) and throws `ApiError` with `kind: 'session_expired' | 'account_inactive'`. The only code that navigates for these kinds is `handleAuthError(err): boolean` in `redirect.ts` (`session_expired` -> `/login?reason=expired`; `account_inactive` -> `/account-inactive`), called from exactly three places: the `catch` in `createResource`, the `catch` in `callAction(fn)` (wrapper for follows/connect handlers), and `(app)/+layout.ts` (where it is expressed as `throw redirect(307, ...)`, see below). The `SIGNED_OUT` listener in `session.svelte.ts` navigates to `/login` only when the flag is not set (other-tab sign-out) and the current route is inside `(app)`; nav-initiated sign-out navigates itself. That way the listener never races `handleAuthError`.

**Retryable network errors never sign out.** supabase-js returns `AuthRetryableFetchError` (status 0) from `getSession()` and `refreshSession()` instead of throwing when it cannot reach the auth server. The rules:
- `getAccessToken()` returns the token if `getSession()` yields a session. If it yields no session and no error, the user is signed out (`session_expired`). If it yields an error with `isAuthRetryableFetchError(error)`, throw `ApiError(0, 'network')` and do not sign out.
- On a 401 from the API, call `refreshSession()` once. Sign out only when the refresh returns no session with a non-retryable error (`AuthApiError`, e.g. invalid refresh token) or the retried request is again 401. A retryable refresh error becomes `ApiError(0, 'network')`.

**Login (`/login`)** (proposals/05, option b2: email + password) via `login.ts: signIn(email, password)`:
1. Validate: email non-empty, contains `@`, <= 254 chars; password non-empty (no minimum on sign-in; people may have older rules).
2. Call `supabase.auth.signInWithPassword({ email, password })`. There is no `weather.next` storage and no second tab: the whole flow stays in one tab, so `?next=` is read from the current URL and passed through `safeNext` at the moment of success.
3. Success: `goto(safeNext(next), { replaceState: true })`.
4. Result mapping (S4 oracle rule): every GoTrue error that carries an HTTP response (wrong password, unknown email, a banned or deactivated user, unconfirmed user) shows exactly one message: "That email and password didn't work." Only a retryable fetch error (`isAuthRetryableFetchError(error)`, status 0) shows "Could not reach the sign-in service. Try again." A 429 from GoTrue is per IP, not per account, so it may show "Too many attempts. Wait a few minutes and try again." Disable the button while a request is in flight and for 30 s after 5 consecutive failures (client-side convenience only).
5. The page has no "forgot password" email link (nothing can send email). Under the form: "Forgot your password or don't have one yet? Ask Rodney for a new link." Fields use `autocomplete="username"` and `autocomplete="current-password"` so password managers work; the password is never put in the URL, a log, or `localStorage`.

**Set password (`/set-password`)**, a public route; reached only from a link that `admin invite` or `admin reset-link` printed (04 10.3, 10.3a): `https://<site>/set-password#token_hash=<hash>&type=recovery`. `password.ts` owns the logic:
1. On mount, before any `await`: parse `location.hash` with `URLSearchParams`; require `type === 'recovery'` and `token_hash` matching `^[A-Za-z0-9._-]{16,256}$`; keep the token only in a local variable (never component props shown in the DOM, never `localStorage`, never logged); then `history.replaceState(null, '', location.pathname)` to remove the fragment. (The fragment is never sent to Netlify or to the API; this just keeps the token out of history and the address bar.) No valid fragment: show "This page needs a link from Rodney. Ask for a new one." with a link to `/login`, and no form.
2. Form: "Choose a password" with new password and confirm (`autocomplete="new-password"`). Client checks, which are for UX only: length 12 to 72 bytes (UTF-8), the two fields equal, not equal to the email. The same minimum must also be set in Supabase (04 8.1); a client check alone is bypassable.
3. On submit (the token is **not** redeemed on page load, so abandoning the page leaves no passwordless session): call `supabase.auth.verifyOtp({ token_hash, type: 'recovery' })` (skip if already redeemed in this page visit), then `supabase.auth.updateUser({ password })`, then `clearForecastCache()`, `clearYtdCache()` and `goto('/', { replaceState: true })`.
4. Errors, each with one fixed message and no GoTrue text shown: `otp_expired`, 403/422 from `verifyOtp` or an unknown/used token -> "That link has expired or was already used. Ask Rodney for a new one."; `weak_password` (422) from `updateUser` -> "Choose a longer password (at least 12 characters)."; `same_password` -> "Choose a password you have not used here before."; a retryable fetch error -> "Could not reach the sign-in service. Try again." and the token stays in memory for the retry. If `verifyOtp` succeeded but `updateUser` failed, the page remembers `redeemed = true`, does not call `verifyOtp` again, and offers the form again; the session stays in Supabase's normal storage (`persistSession: true`, so the `sb-*` entry in `localStorage`, the accepted S2 exposure) as usual, so a successful retry works and an abandoned page leaves a signed-in session with no password (the person then needs a new link after signing out; acceptable).
5. If a different user is signed in on this browser, `verifyOtp` replaces the session; the page first calls `signOut('local')` only when `auth.status === 'signed_in'` at mount, so the previous user's caches are cleared.

**Change password (`/account`, in `(app)`)**: a small account page showing the display name (from `GET /me`) and the email (from `supabase.auth.getUser()`), "Change password" (new and confirm, same checks as above, `updateUser({ password })`, success message "Password changed."; it does not ask for the current password because the Supabase "require current password" setting is off, 04 8.1; a stolen-token attacker could therefore change it, which is the accepted S2 exposure), and "Sign out" / "Sign out everywhere". A nav link "Account" leads here.

**`safeNext(raw: string | null): string`** (`auth/next.ts`), returns `/` unless all hold:
- `raw` is a string of <= 200 chars that starts with a single `/`, does not start with `//`, contains no backslash and no control characters (code < 0x20 or 0x7f), including after `decodeURIComponent` (a decode failure rejects);
- `rawPath = raw` up to the first `?` or `#`; every `/`-separated segment after the leading empty one, once decoded, is not `.` or `..` and (except a trailing empty segment, i.e. a trailing slash) not empty;
- `const u = new URL(raw, location.origin)` has `u.origin === location.origin`, `u.pathname === rawPath` (no normalisation happened), and `u.pathname` does not start with `//`.
Returns `u.pathname + u.search` (never `u.hash`).

There is no `/auth/callback` route and no PKCE exchange (removed Oct 7); a request to `/auth/callback` is just an unknown route.

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
export async function apiFetch<T>(path: string, init?: { method?: 'GET'|'POST'|'PUT'; body?: unknown; signal?: AbortSignal }): Promise<T>;
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
| 405 `method_not_allowed` | "Something went wrong on our side. Try again." (client bug, never expected) |
| any other status or code (catch-all), `bad_response`, `unknown` | "Something went wrong on our side. Try again." |
| `session_expired` / `account_inactive` | handled by `handleAuthError` (login shows "Your session expired. Sign in again."; inactive page shows "This account is inactive. Contact the administrator.") |

### 4.4 Data loading and caching

`resource.svelte.ts`: `createResource<T>(fetcher: (signal) => Promise<T>)` returns a `$state` `{ status: 'loading'|'ready'|'error', data?, error?, reload() }`; aborts on reload/unmount; its `catch` calls `handleAuthError`. Every view renders `LoadingBlock`, `ErrorPanel` (message + Retry) or content.

`cache.ts`: in-memory `Map` for `/ytd` responses keyed by `year|start|end|gddMethod|gddBase|gddStart|sortedStationIds`, TTL 5 minutes, cleared on sign-out. Purpose: load reduction (a `/ytd` call runs window-function SQL over up to 5 stations and 30+ baseline years), not rate-limit avoidance (every request costs 1 of 120 per minute, burst 40, so toggling would not hit the limiter). Toggling Rain/GDD or Full year and back reuses cached responses; in-flight requests with the same key are shared. The key also includes the three GDD settings (`gdd_method|gdd_base|gdd_start`). Rain uses window `season` (default `04-01`..`10-31`) and GDD uses `gdd_start`..season end in the same response, so switching between Rain and GDD never needs an extra request; changing a GDD setting makes one new request. `/forecast` and `/forecast/changes` responses use the same module with a shorter TTL of 2 minutes (keys `forecast|stationId` and `forecast-changes|stationId|from|to`), only to absorb tab switches and date-picker toggling within a visit; the API's own 30-minute (`/forecast`) and 2-hour (`/forecast/changes`) in-memory caches (02 5.6) remain the real upstream protection, and a failed or `stale: true` response is not cached on the client. `clearForecastCache()` (called by `signOut`, and after the set-password flow) drops both kinds of keys; there is no `/forecast/history` route. Nothing else is cached.

### 4.5 Constants (`src/lib/config.ts`)

```ts
export const DEFAULT_SEASON = { start: '04-01', end: '10-31' } as const;     // F6
export const FULL_YEAR = { start: '01-01', end: '12-31' } as const;
export const GDD_DEFAULTS = { method: 'simple_avg', base_f: 50, start: '04-01' } as const;   // owner decision Oct 7; mirrors the API defaults
export const GDD_BASE_RANGE = { min: 32, max: 70 } as const;
export const WINTER_MONTHS = [11, 12, 1, 2, 3] as const;
export const YTD_MAX_STATIONS = 5;
export const STALE_AFTER_MIN = 30;                                           // Q9
export const FORECAST_CHANGES_DAYS = 7;                                      // default window of the change view, from today (API max span 14)
export const FORECAST_HOURLY_DAYS = 7;                                       // hourly chart horizon (the API returns up to 15 or 16 days)
export const FEATURES = { forecast: true } as const;                         // design doc: v1 requirement (forecast chart and change view)
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
- `season.ts: displaySeason(todayInZone: {year, month, day})` returns `{ year, offSeason: boolean }`: months 4 to 10 -> the current year, `offSeason: false`; months 11 and 12 -> the current year (its April-October season just ended), `offSeason: true`; months 1 to 3 -> the previous year, `offSeason: true`. The dashboard and station page use `year` for both the Apr-Oct and Full-year toggles, so November to March never shows an empty chart (owner decision, Oct 7).
- `gddSettings.ts`: `parseGddSettings(searchParams)` (invalid or out-of-range values fall back to `GDD_DEFAULTS`) and `toSearchParams(settings)` (writes only values that differ from the defaults). The settings live in the page URL (`?gdd_method=&gdd_base=&gdd_start=`), so they survive reload and can be shared; nothing is stored in `localStorage` or on the server.
- `forecast.ts` (no percentile maths: the API computes every p10/p50/p90): `bandPaths(ensemble.hourly | ensemble.daily, field)` -> outer ribbon (p10 to p90) and median line over the shared x positions (null steps break the path); `hourlyWindow(forecast, FORECAST_HOURLY_DAYS)` slices both series by time; `leadSeries(changes, metric)` -> one array of `({x,y}|null)` per lead (`metric` in `precip_in | prob_precip_max | tmax_f | tmin_f`), x = day index; `leadLabel(n)` ("latest" for 0, "1 day earlier" ... "7 days earlier"); `spreadAcrossLeads(day, metric)` = `{min, max}` ignoring nulls (`null` when fewer than 2 values), used for the "forecast moved by" note.
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
- GDD charts: `GDD: {label(gdd.method)}, base {gdd.base_f} F, counted from {gdd.start as "April 1"}. The airport baseline uses the same settings.` (values from the response's top-level `gdd`, never from UI state).
- Off-season (see 4.7): `Showing the {year} season (April-October), the most recent completed season. The {year+1} season starts April 1.`
- Rain charts only, when the window includes a winter month: `Shaded months: unheated rain gauges undercount snow, so totals read low against the baseline.` (never on GDD charts)
- Bands present: `Bands: {first}th-{last}th and {inner pair} percentile of prior years.` built from `bands.percentiles`.

### 4.7 Views

**Login**, **Set password** (`/set-password`), **Account** (`/account`, change password), **Account inactive** (`/account-inactive`, static text, link back to `/login`): section 4.2.

**Empty state** (`stations.length === 0`): "No stations yet." Actions: link "Connect your weather station" (`/connect`) and the text "Ask Rodney to share stations with you.", plus a link to `/follows` ("Find people to follow"), where a person can be searched by name and asked for access.

**Dashboard** `/`:
- Load `/me` (layout) and `/stations`. Selected stations default to all, capped at `YTD_MAX_STATIONS`; the picker limits selection to 5.
- Controls: metric tabs `Rain` (default) | `GDD`; season `Apr-Oct` (default) | `Full year`; "Same scale for all stations" (default off, Q7); `StationPicker`; on the GDD tab, `GddSettings` (below).
- Data: one `/ytd` call for all selected stations for the active window (4.4) with the current GDD settings; `year` = `displaySeason(today in the first selected station's timezone).year` (4.6), so from November to March the dashboard shows the most recent completed season instead of an empty state.
- Off-season label: when `displaySeason(...).offSeason`, a banner above the charts says "Showing the {year} season (April-October), the most recent completed season. The {year+1} season starts April 1." (the same line is in each chart caption, 4.6).
- Grid: one column per selected station using a fixed class `.cols-N` on the grid (`N = min(selected, 5)`), with horizontal scroll beyond the viewport. Each column: `StationCard` header (name, owner if not own, `latest_observation_at` in the station's timezone), the chart with caption, `describePosition` text.
- Season states (explicit, never a blank chart): if `current.last_date` is `null` (no data in the window) and the station-local date is before the window start: "Season not started. Showing prior years only." (bands and prior lines still drawn). If `last_date` is `null` and the date is inside the window: "No data yet this season." Out of season there is no separate "season ended" state: the completed season is shown with the off-season label above. A station with no data in the displayed season shows "No data for this station in the {year} season."
- **GDD settings** (`GddSettings.svelte`, shown on the GDD tab of the dashboard and station page): a method select ("Simple average" default | "Clamped to the base and 86 F"), a base-temperature number input (`GDD_BASE_RANGE`, default 50 F) and a start-date picker (month and day, default April 1). Changes update the page URL (`gddSettings.ts`) and trigger one new `/ytd` request; invalid input is not sent. The chart caption always shows the settings the response echoes.
- Baseline states (`baselineViewState(station)` in `provenance.ts`, one Vitest case each): `baseline: null` -> current line only, caption "No baseline yet." (own stations also show a "Choose a baseline" button, below); `baseline.history_loaded === false` -> current line only, caption "Baseline chosen: {name}. Its history is loading and should appear within the hour."; otherwise lines and bands. The same states apply to rain and GDD; there is no GDD-only comparison state.
- Needs reconnecting: `me.credential?.status === 'invalid'` -> own stations (`is_own`) show `NeedsReconnectBanner` ("Data collection paused: this station needs reconnecting. Last data: {time}.") with a "Reconnect" link to `/connect?reconnect=1`. Followed stations never show a reconnect action. `rate_limited`: neutral "Updates are delayed." on own stations. Stale (`latest_observation_at` older than `STALE_AFTER_MIN`, no banner): "No recent data." History still charts in every state.
- A `ytd` 404 reloads `stations` and shows `ErrorPanel` with Retry.

**Station detail** `/stations/{id}`: UUID check, `/stations/{id}` and `/ytd?station_ids={id}`, larger chart, tabs Rain | GDD | Forecast, season toggle (same off-season rule as the dashboard), `GddSettings` on the GDD tab, data table, and a **Baseline panel**: "Baseline: {name} ({airport_code or id}), {distance} mi away" for any viewer; for the owner (`is_own`) a "Change baseline" or, when none, "Choose a baseline" button that opens `BaselinePicker` (below). 404/403 -> "That isn't available." with a link to `/`.

**Forecast** `/stations/{id}/forecast` (v1, `FEATURES.forecast` default on; when forced off, the tab and route show "Forecasts are not enabled." for rollback). Two sections; nothing here is stored, and the browser calls only the two API routes (never Open-Meteo):
- **Live forecast.** `GET /stations/{id}/forecast` once per visit (the API caches 30 minutes). `ForecastChart.svelte` (SVG, same rules as 4.6): a switch `Temperature | Precipitation` and a switch `Hourly (7 days) | Daily (15 days)`. Hourly: x = UTC instants with axis labels in the station's timezone; `deterministic.hourly` as `line-forecast`, `ensemble.hourly` p50 as `line-median` and a `band-outer` ribbon p10 to p90, like a normal forecast site. Daily: temperature shows tmax and tmin each with its ribbon; precipitation shows a p50 marker with the p10-p90 range as a vertical bar and `prob_precip` (the ensemble's, else the deterministic `prob_precip_max`) as a percentage under each day. Null steps break lines and ribbons; `ensemble: null` hides the ribbons and adds "Uncertainty range unavailable right now."; `deterministic: null` hides the line likewise. `stale: true` shows "Showing the forecast saved at {generated_at}; refreshing failed." A 502/503 shows `ErrorPanel` with Retry. Percentiles appear in this chart only; no percentile history exists anywhere.
- **How the forecast changed.** `GET /stations/{id}/forecast/changes?from=<station-local today>&to=<from + FORECAST_CHANGES_DAYS - 1>` with a date picker (any window of up to 14 days from 2024-01-01 to 15 days ahead, the API rejects others with 422, shown as "Pick up to 14 days."). `ForecastChangesChart.svelte`: x = each target day; a metric switch `Rain | Chance of rain | High | Low`; one line per lead, `line-lead-0` (latest forecast, strongest) ... `line-lead-7` (7 days earlier, faintest), labelled with `leadLabel`; null points break the line. Below the chart, a table: one row per target day, columns = leads, plus "moved by {min} to {max}" from `spreadAcrossLeads`. Caption: `Forecast: {model} as it looked 0 to 7 days before each date (day offsets, not model runs). Days are local midnight to midnight.` plus `{attribution}` (Open-Meteo CC BY 4.0), in addition to the footer; when any `prob_precip_max` is `null` for a requested past day: "Chance of rain is not available before October 2024."
- Forecast-vs-gauge is post-v1: no route or component. It stays possible later because Open-Meteo's Previous Runs archive can be compared with `daily_rollups` without any stored forecast (04 5.8).

**Follows** `/follows`: `GET /follows`.
- "People who follow me" (`followers`): `pending` first with Approve / Decline buttons (`POST /follows/{user.id}/approve|decline`), then `approved` with Revoke (`/revoke`, inline confirm, not `window.confirm`), then `declined`/`revoked` collapsed. Text: "Approving shares all of your stations." Show `requested_at` (date) per row.
- "People I follow" (`following`): list with status badges. No cancel/unfollow control (02 has none).
- **Find people** (`UserSearch.svelte`, owner decision Oct 7; no suggestions or "people you may know"): a text input "Find a person by name". Nothing is requested for fewer than 2 characters (hint "Type at least 2 characters."); otherwise a 300 ms debounce, then `GET /users/search?q=` (the previous in-flight request is aborted). Results (at most 10) show the display name and the state: `none`, `declined` and `revoked` get a "Request to follow" button (`POST /follows {followee_id}`; on success the row shows "Pending" and `follows` reloads); `pending` shows "Request sent", `approved` shows "Following". Empty result: "No one found. Names are matched from the start; check the spelling." A 429 shows "Too many searches. Try again in {Retry-After} seconds." and disables the input until then; a 404 on the request shows "That person isn't available." and re-runs the search. The result area is an `aria-live="polite"` region; names render as escaped text; the query is never stored or logged.
- After each action reload `follows` and `stations`; a 404 shows "That request is no longer pending." and reloads.

**Connect station** `/connect` (4 steps, state in component memory only; step 4 is optional and never blocks a saved station):
1. **Key**: instructions (wording Q4); input `type="text"` with `autocomplete="off"`, `autocapitalize="off"`, `spellcheck="false"`, `data-1p-ignore`, `data-lpignore="true"` and class `secret-input` (`-webkit-text-security: disc`) so password managers do not offer to capture it as a login. Accepted risk (Q13): some browsers or extensions may still offer to save it, and a browser without `-webkit-text-security` shows the characters.
2. **Choose devices**: `POST /connect/validate`. `valid:false` -> exactly "That key couldn't be verified. Check it and try again." (one text for every invalid cause). 429/503/502 per 4.3. `valid:true` with `devices: []` -> "No devices found on that account." Otherwise a checklist (name + MAC, at least one required). Each device has `timezone: string | null`. The API derives the zone from AWN; **a zone picker is shown only for devices whose `timezone` is `null`** (a required `<select>` of IANA zones, `TIMEZONE_CHOICES` in `config.ts`, list content Q8), and the chosen value is sent as the fallback `timezone` in the save body. Devices with a non-null `timezone` show it as read-only text and send none.
3. **Save**: `POST /connect/save` with `{ api_key, devices: [{ mac, name?, timezone? }] }`. 201: clear the key, show "Connected. Your history will fill in over the next few minutes." with a link to `/`; reload `/me` and `/stations`. `not_covered` is always present (empty on first connect). When it is non-empty (stations the caller owns that the new key does not cover; they stay on the replaced credential and the poller skips them), additionally show "These stations are no longer on this account; ask Rodney to disable them:" followed by the names (escaped text). 422 `timezone_required` (a picker was missing or empty) -> keep step 2 and mark the unresolved devices with "Choose a time zone." 422 `invalid_key` -> the generic key message and back to step 1 with the key cleared. 409 `device_already_registered`/`conflict`, 422 `unknown_device`, 422 `validation_failed` per 4.3. The key stays in memory on recoverable errors.
4. **Choose a baseline** (`BaselinePicker.svelte`; also opened from the station page): for each station returned by `save` with `baseline: null` (and only the owner's), call `GET /stations/{id}/baseline/candidates`. Show the list nearest first with the suggested one preselected and marked "(suggested, nearest)". Each row: name and airport code, "{distance_mi} mi away", "{elevation_ft} ft" with "{|elevation_diff_ft|} ft higher/lower than your station" when known (a note "Different elevation may change rainfall and temperature comparisons" when the difference exceeds 1,000 ft), and "{record_years} years of records" (plus "short record" when `short_record`). Buttons: "Use this baseline" (`PUT /stations/{id}/baseline`; on success "Baseline set. Its history should appear within the hour.") and "Skip for now". Empty list: "No airport weather station with a long enough record was found within {radius_mi} miles. You can choose one later." 502/503: "We couldn't look up nearby airport stations right now. Your station is saved; you can choose a baseline later from its page." (never block or fail the connect). 422 `not_a_candidate` reloads the list.
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

`web/netlify.toml` (placeholders written literally: Fly app `famweather-api`, Node `26`; only `<project-ref>` remains, in `security-headers.json` and therefore in the generated CSP):

```toml
[build]
  command = "npm ci && npm run build && node scripts/check-build.mjs && node scripts/headers.mjs check && node scripts/check-bundle-secrets.mjs"
  publish = "build"
  # Skip Dependabot deploy previews: exit 0 = skip the build, exit 1 = build.
  # CONTEXT and HEAD (the pull request's head branch) are Netlify build variables; confirm the names in Netlify's "Environment variables" docs and a first Dependabot PR.
  ignore = 'test "$CONTEXT" = deploy-preview && case "$HEAD" in dependabot/*) exit 0;; esac; exit 1'

[build.environment]
  NODE_VERSION = "26"

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
4. **Supabase client, session, auth redirect owner** (4.2): `supabase.ts`, `session.svelte.ts`, `next.ts`, `login.ts`, `password.ts`, `redirect.ts`. Done when: `next.test.ts`, `login.test.ts`, `password.test.ts` pass (cases in 6.1).
5. **API client** (4.3): `ids.ts`, `types.ts` (exactly 3.2, re-checked against `02-api.md` section 5), `errors.ts`, `client.ts`, `endpoints.ts`, `cache.ts`. Done when: `client.test.ts` cases in 6.1 pass.
6. **Transforms** (4.6) except `forecast.ts`. Done when: transform tests in 6.1 pass.
7. **Login, set-password and account routes**, `(app)` guard, nav, sign-out, footer, account-inactive page. Done when (with the 05 stack up): `make invite EMAIL=<new address> NAME=<name>` prints a set-password link; opening it in a browser shows the set-password form with the fragment gone from the address bar; choosing a 12+ character password signs the person in and lands on `/`; signing out and signing in with that password works (and returns to `?next=` when present); a wrong password and an unknown email show the identical message; the same link used a second time (or after expiry) shows "That link has expired or was already used. Ask Rodney for a new one."; `/account` changes the password and the new one works; sign-out clears `sb-*` keys. Nothing is emailed and no mail catcher exists.
8. **Resource helper and shared components** (`createResource`, `callAction`, `LoadingBlock`, `ErrorPanel`, `EmptyState`, `NeedsReconnectBanner`, `StationCard`, `FollowItem`, `Footer`). Done when: `src/lib/data/resource.test.ts` passes (loading -> ready, error state, abort on reload, `handleAuthError` called once for `kind` errors and not for others) and `npm run check` is clean; component rendering itself is verified by the e2e specs of tasks 10-12 (no throwaway route).
9. **Chart components** (`CumulativeChart`, `ChartFrame`, `ProvenanceCaption`) fed by `tests/fixtures/ytd-full.json` (hand-written in the exact 02 §5.5 shape, then replaced by a real `/api/v1/ytd` capture once 02's Rollups and YTD task (§5.5, task 10) exists). Done when: rain and GDD render lines, bands, the current line with its partial-day segment, mid-season start note, season states and captions; browser console shows zero CSP violations against Caddy.
10. **Dashboard and station detail** (4.7), including off-season display, the GDD settings selector and the no-baseline / baseline-loading states. Done when: `dashboard.spec.ts` passes.
11. **Follows UI** including find-people search. Done when: `follows.spec.ts` passes.
12. **Connect flow** including the baseline picker (step 4 and the station page). Done when: `connect.spec.ts` passes including the key-never-persisted assertions.
13. **Headers parity and bundle scan.** `scripts/check-bundle-secrets.mjs` (including the production-context demo-value checks, 6.4); confirm `ops/caddy/Caddyfile` imports the generated file (05; report mismatches, do not edit). Done when: `npm run headers:check` and `npm run test:bundle` pass, `CONTEXT=production PUBLIC_SUPABASE_URL=http://localhost:8000 node scripts/check-bundle-secrets.mjs` fails, and `csp.spec.ts` passes against the stack.
14. **Forecast and forecast-change view** (last). `forecast.ts`, `ForecastChart.svelte`, `ForecastChangesChart.svelte`, route, tab, `forecast.spec.ts`; needs 02's forecast task (§5.6 and §5.6a, task 13; `fixtures/forecast.json`, `forecast-ensemble-null.json` and `forecast-changes.json` until then). Done when: `forecast.spec.ts` passes with the fixtures (and, once 02 ships, a real-data smoke against the replay stack that the page renders the chart and the change table).
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
- `baselineViewState`, one case each: ready (lines + bands); `baseline: null` ("No baseline yet", current only); `history_loaded: false` (loading caption, current only). `displaySeason`: 2026-04-01 and 2026-10-31 -> 2026 in season; 2026-11-01 and 2026-12-31 -> 2026 off-season; 2027-01-01 and 2027-03-31 -> 2026 off-season; 2027-04-01 -> 2027 in season; month boundaries are taken in the station's zone (stable under both `TZ` values). `gddSettings`: defaults produce an empty query string; a non-default method/base/start round-trips; `gdd_base=31`, `gdd_method=avg_50` and `gdd_start=13-40` fall back to the defaults. Metric selection: with `bands.rain_cum_in` non-null and `bands.gdd_cum` null, rain draws bands and GDD does not; `years_included`, `missing_days` and prior-year series are read from the active metric's field (a year with `rain_cum_in` null and `gdd_cum` non-null is drawn only on GDD). `splitCurrent`: a point after `last_complete_date` is partial; `last_complete_date: null` makes the whole series partial; a station with `first_date` after the window start produces no points before it and the "Data starts" note. `describePosition` returns `null` for `last_complete_date: null` and for a late `first_date`.
- `localYear`/`localMd`/`formatInZone` use the station's IANA zone and are stable under both `TZ` values.
- `forecast.ts`: `bandPaths` over a fixture with null steps breaks the ribbon and median at the nulls and draws p10 to p90; `hourlyWindow` keeps exactly the first `FORECAST_HOURLY_DAYS` days; `leadSeries` returns 8 series (leads 0 to 7) of the day count with nulls kept as `null`; `leadLabel(0)` is "latest", `leadLabel(1)` "1 day earlier", `leadLabel(7)` "7 days earlier"; `spreadAcrossLeads` ignores nulls and returns `null` for fewer than 2 values. No percentile is computed in the web (a source scan test finds no `quantile`/`percentile` computation outside `bands.ts` labels).

Auth/API:
- `safeNext` accepts `/stations/abc?x=1` and `/`; rejects `//evil.com`, `/\evil`, `/\t/evil.com` (tab), `/%5C%5Cevil`, `/%5Cevil.com`, `/.//evil.com`, `/a/..//evil.com`, `/%2e//evil.com`, `/a/../b`, `https://evil.com`, `javascript:alert(1)`, `/a\b`, control characters, empty, `null`, > 200 chars; never returns a fragment.
- `signIn` (mock supabase, `login.test.ts`): calls `signInWithPassword({ email, password })` and nothing else (spy: `signInWithOtp`, `signUp`, `resetPasswordForEmail` are never called); success returns `'ok'`; "Invalid login credentials" (400), a banned-user error (400/403), an unconfirmed-user error and an unknown email all return the same `'failed'` result and message; a 429 returns `'rate_limited'`; `'network'` only when `isAuthRetryableFetchError(error)` is true, whether returned or thrown (a thrown non-retryable error is `'failed'`); the password never appears in `console.*`, the URL or storage (spies); no `weather.next` key is ever written.
- Set-password logic (mock supabase, `password.test.ts`): `parseSetPasswordFragment` accepts `#token_hash=<64 hex>&type=recovery`, rejects a missing token, `type=signup`, `type=magiclink`, a token with a `/` or space, one longer than 256 characters, and an empty hash; the page code calls `history.replaceState` with no hash **before** the first `await`; `verifyOtp` is called only on submit, exactly once per visit (a second submit after a failed `updateUser` skips it), with `{ token_hash, type: 'recovery' }`; then `updateUser({ password })`; an `otp_expired`/403/422 from `verifyOtp` shows "That link has expired or was already used. Ask Rodney for a new one."; `weak_password` and `same_password` map to their fixed messages; a retryable fetch error keeps the token in memory for retry; the token is never written to `localStorage`/`sessionStorage`, logged, or placed in the URL; `validateNewPassword`: 11 characters fail, 12 pass, 73 bytes fail (a 40-character emoji string is over 72 bytes), mismatched confirm fails, equal-to-email fails.
- `account` change password (mock supabase): `updateUser({ password })` with the same validation; success message "Password changed."; the page never asks for the current password.
- `assertUuid`/path safety: accepts a UUID; rejects `../me`, `abc`, `<uuid>/../x`, `%2e%2e`, `a%2Fb`, `a\b`. Segment checks run on the raw pathname split on `/` (leading empty segment skipped): `/api/v1/stations/%2e%2e/x`, `/api/v1/stations/./x`, `/api/v1/stations/a/../b`, `/api/v1/stations/%2E/x`, `/api/v1/stations/a%5Cb`, `/api/v1/stations/a%2Fb`, `/api/v1/stations//x` and `/api/v1/stations/%zz` are rejected before any fetch, and the validation runs on the raw string (a test with a spied `URL` constructor shows it is not reached for these inputs). `apiFetch` rejects (no network call) `https://evil.test/api/v1/me`, `//evil.test`, `/api/v2/me`, `/api/v1/../me`, `/api/v1/%2e%2e/me`, `/api/v1//x`, and accepts `/api/v1/ytd?station_ids=<uuid>,<uuid>&year=2026&start=04-01&end=10-31`. A `getYtd` endpoint test asserts the exact request URL string for two ids (`...?station_ids=<a>,<b>&year=2026&start=04-01&end=10-31`, comma unencoded, ids in the given order).
- Auth error handling (mock supabase): `getSession()` returning `AuthRetryableFetchError` -> `ApiError(0, 'network')` and **no** `signOut`; no session and no error -> sign-out and `kind: 'session_expired'`; 401 then `refreshSession()` returning a retryable error -> `network`, no sign-out; refresh returning a non-retryable `AuthApiError` -> sign-out; the API-initiated sign-out sets the flag so the `SIGNED_OUT` listener does not navigate, while an unflagged `SIGNED_OUT` on an `(app)` route navigates to `/login` once; 503 `auth_unavailable`/`overloaded`, 408, 413 never sign out; `signIn` returns the network result only for `isAuthRetryableFetchError`.
- `apiFetch` (mock `fetch` and supabase): sends `Authorization: Bearer <access_token>` and never the anon key; `credentials: 'omit'`; 401 then refresh then 200 is exactly 2 fetches; 401 twice or refresh failure -> `signOut('local')` and `ApiError.kind === 'session_expired'` with no `goto` call from the client; 403 `account_inactive` -> sign-out and `kind === 'account_inactive'`; 403 `forbidden` and 404 do not sign out; 429 exposes `retryAfterSec`; network error -> `network`; non-JSON error body -> `unknown`; a server `message` containing `https://rt.ambientweather.net/...?apiKey=SECRET` never appears in `ApiError.message` or `userMessage`; request bodies with `api_key` never appear in errors or `console.*` calls (spy).
- `handleAuthError`: navigates exactly once for each kind and returns `false` for other errors.
- `cache.ts`: second call with the same key returns the cached value without a fetch; different `start` is a different key; TTL expiry refetches; sign-out clears; concurrent identical calls share one request.

### 6.2 Scan test

A Vitest test scans `src/**/*.svelte` and `src/app.html` **after stripping every `<style>...</style>` block** (component `<style>` blocks compile to external CSS and may contain anything) and fails on: `style=`, `style:`, `{@html`, `innerHTML`, `eval(`, `new Function`, `transition:`, `animate:`, and any `<style` tag in `app.html`.

### 6.3 Playwright e2e (against the Docker Compose stack)

Entry point: `make test-e2e CONFIRM=1` from the repo root (05 section 7.2). It is destructive for the working stack (Playwright needs `localhost:8000`), so `CONFIRM=1` is required; it chains `reset up web-build seed e2e-data` itself, then runs host `npm ci`, `npx playwright install chromium` and `npm run test:e2e` with `E2E_BASE_URL`. Host: Node matching `web/.nvmrc`. The stack runs `AWN_MODE=replay`. Seed users and defaults live only in `tests/helpers/env.ts` (04 section 12.1; nothing is restated in `.env.example`): `E2E_USER_A_EMAIL` (`alex@example.test`), `E2E_USER_B_EMAIL` (`pat@example.test`), `E2E_UNINVITED_EMAIL` (`nobody@example.test`), `E2E_DEMO_PASSWORD` (the local demo password `seed-dev` gives every seed user; a committed local-only value fingerprinted in `backend/demo-fingerprints.txt`, 05), `E2E_VALID_AWN_KEY` (`fixture-valid-key`), `E2E_INVALID_AWN_KEY` (`fixture-invalid-key`), each overridable from the environment. Other seed users are found by display name: `Dana` (`dana@example.test`; follow states Dana -> Alex pending, Dana -> Sam declined, Dana -> Rodney revoked) and `Casey` (`casey@example.test`, invited then deactivated). Casey enables an optional real `account_inactive` test (a password sign-in works at GoTrue unless banned, the first API call returns 403); when Casey cannot log in locally the spec is skipped and the mocked `/me` 403 test is the coverage.

`playwright.config.ts`: `baseURL = E2E_BASE_URL`; throws unless the host is exactly `localhost` or `127.0.0.1`; Chromium only; `workers: 1`; `retries: 0`; `trace: 'retain-on-failure'`; output dirs `web/test-results/` and `web/playwright-report/`. Projects: `setup` (`auth.setup.ts`) and `chromium` (`dependencies: ['setup']`). `auth.setup.ts` signs in once per user (alex, pat, dana) with `E2E_DEMO_PASSWORD` through the real login form, and saves `web/tests/.auth/<name>.json` (`storageState`). Specs use `test.use({ storageState })` except login/session/connect, which use fresh contexts. 05 must git-ignore `web/tests/.auth/`, `web/test-results/` and `web/playwright-report/` (Q12).

Helpers (all docker-compose invocations run with the repo root as working directory, resolved from the repo root even though Playwright runs in `web/`):
- `helpers/api.ts`: may use `fetch` with a token read from the page's `sb-*` localStorage entry (tests are exempt from the `fetch` lint rule).
- `helpers/invite.ts`: `invite(email, name)` runs `docker compose --profile tools run --rm -T admin admin invite --email <e> --display-name <n>` and `resetLink(email)` runs `... admin reset-link --email <e>`, both with `cwd` = repo root; each returns the single printed `http://localhost:8000/set-password#token_hash=...&type=recovery` line (the only stdout line starting with that origin) and refuses unless `E2E_BASE_URL` is localhost. `setPassword(page, link, password)` opens the link in the same context, fills the form and submits. There is no mail catcher: nothing is emailed.

Data strategy: chart, band, season-state and GDD-branch assertions use committed API-shaped fixtures (`tests/fixtures/ytd-full.json`, `ytd-no-baseline.json`, `ytd-baseline-loading.json`, `ytd-gdd-clamped.json` (echoes `clamped_50_86`, base 45, start 01-01), `ytd-partial-station.json` (first_date after start, open partial day), `forecast.json`, `forecast-ensemble-null.json`, `forecast-changes.json`, `candidates-loveland.json`, `candidates-empty.json`, `user-search.json`), all in the exact 02 §5.5/§5.6/§5.6a shapes (per-series `missing_days` and `years_included` objects, nullable per-series arrays), served with `page.route('**/api/v1/ytd*', ...)`; a Vitest fixture test validates each fixture against the TypeScript types and a runtime shape check (objects, not numbers, for `missing_days`/`years_included`; `current.first_date`/`last_date`). Real data (from `make e2e-data`) is exercised by one smoke assertion per area. Mocked specs say "mocked" in their title.

Specs:
1. `login.spec.ts` (fresh context; seed users have local demo passwords set by `seed-dev`, 05): alex signs in with the demo password and lands on the dashboard; an `sb-` key exists; reload keeps the session. `next`: visiting `/follows` signed out redirects to `/login?next=%2Ffollows`; after password sign-in (same tab) the app lands on `/follows`. Wrong password and an unknown email show the identical message. S4 oracle (mocked GoTrue `/auth/v1/token` 400, 403 banned and 422 give identical text; 429 shows the rate-limit text; only an aborted request shows the network message). Mocked: `/api/v1/me` 403 `account_inactive` ends on `/account-inactive` and removes `sb-` keys. Optional real: Casey (inactive) cannot get past `/account-inactive`.
1b. `set-password.spec.ts` (**settles proposal 05's unverified items for the browser against the local GoTrue**): `helpers/invite.ts` invites a fresh `setpw-<n>@example.test`; before the link is used, a password sign-in for that email fails; opening the link shows the form and the address bar has no fragment; a 6-character password is rejected by the form; a 12+ character password signs the person in on `/`; sign out, sign in with it succeeds; reloading the link URL (a saved copy) now shows "That link has expired or was already used. Ask Rodney for a new one." (records whether the token is single use; the spec asserts the observed behaviour and fails loudly if it changes); a link with a mangled token shows the same message; `resetLink(email)` for the same user, redeem, set a new password **without** entering the old one, old password no longer signs in, new one does (this proves the "require current password" switch is off in the local config); an inactive user's `reset-link` fails in the CLI. The page text and every request URL (except the POST bodies to GoTrue) never contain the token or the password; `localStorage` and `sessionStorage` do not contain the token.
1c. `account.spec.ts`: alex opens `/account`, changes the password to a new 12+ character value, signs out, signs in with the new value; the seed password is restored in `afterAll` (or the spec uses its own invited user).
2. `session.spec.ts`: sign-out removes `sb-*` keys and `/` redirects to `/login?next=%2F`; mocked `/api/v1/me` 401 twice ends on `/login` with the "session expired" text; mocked `/api/v1/me` 503 `auth_unavailable` shows the busy message and keeps the session (no sign-out); `next` values `//evil.com` and `/\t/evil.com` end on `/` after login; `/stations/not-a-uuid` shows "That isn't available." with no `/api/v1` request.
3. `dashboard.spec.ts`: real-data smoke (alex sees at least one chart whose caption starts with "Baseline:"). Mocked `ytd-full.json`: provenance lines; Apr-Oct label; Full year shows the winter note on Rain but not GDD; band, prior and current paths exist; data-table numbers equal fixture values; `current.missing_days[metric]` note (rain and GDD counts differ in the fixture); toggling Rain -> GDD -> Rain issues no repeat `ytd` request for a window already fetched (count route hits). `ytd-no-baseline.json`: "No baseline yet" caption, no band paths, GDD shows current only. `ytd-baseline-loading.json`: the loading caption. `ytd-gdd-clamped.json`: the GDD caption shows the echoed method, base and start; changing the method in `GddSettings` updates the URL and issues exactly one new `ytd` request with `gdd_method=clamped_50_86`, and switching Rain/GDD issues none. `ytd-partial-station.json`: the line starts at `first_date`, shows "Data starts ...", no position text, and the open-day segment has class `line-current-partial`. Season states via `page.clock`: on 2026-12-15 and on 2027-02-10 the request carries `year=2026` and the off-season banner and caption line appear (never an empty chart); on 2026-07-01 there is no banner; "No data yet this season." on April 2. Mocked `/me` `credential.status = invalid`: reconnect banner and link on own stations only. Stale `latest_observation_at`: "No recent data." `ytd` 404: error panel; Retry reloads `stations`. Mocked 500, 503 `overloaded` and 429 with `Retry-After: 7`: matching messages.
4. `follows.spec.ts` (alex's and dana's contexts; once per reset because it changes seeded states): alex opens `/follows`, sees Dana pending with a requested date, approves; Dana's `/api/v1/stations` now includes alex's stations and Dana's "People I follow" list shows badges **approved** for Alex, **declined** for Sam and **revoked** for Rodney (rows found by display name, not position); alex revokes via the inline confirm, Dana's list drops alex's stations immediately and Alex's badge becomes revoked. (Sam and Rodney are not logged in; only Dana's view is asserted for them.) **Find people (mocked `user-search.json` plus one real smoke):** typing 1 character sends nothing; 2 characters send one request after the debounce, and typing on aborts the earlier one; results show names and states, `none`/`declined`/`revoked` have a "Request to follow" button and `pending`/`approved` do not; clicking sends `POST /follows` with exactly `{"followee_id": ...}` and the row becomes "Pending"; a mocked 429 with `Retry-After: 7` shows the wait message and disables the input; the page text and every request never contain an email address (real smoke: pat searches "ra" and sees Rodney by display name only).
5. `empty-state.spec.ts`: pat (no stations, no follows) sees "No stations yet." with the Connect link and "Ask Rodney to share stations with you."
6. `connect.spec.ts`: setup creates a dedicated fresh invitee via `helpers/invite.ts` (random `connect-<n>@example.test`), sets a password through the printed link, and uses only that account. Steps: `E2E_INVALID_AWN_KEY` -> exactly the generic message, page text never contains `ambientweather`, `http`, `401`, `apiKey`; `fixture-ratelimited-key` -> the busy/rate-limit text; `E2E_VALID_AWN_KEY` -> the two fixture MACs `AA:BB:CC:00:01:01` and `AA:BB:CC:00:01:02`; the fixture devices carry AWN time zones, so no picker is shown; select both, save, success message; `/api/v1/stations` for that user lists two stations. Mocked variants: a validate response with `timezone: null` shows the picker for that device only and the save body carries the chosen zone; a save 422 `timezone_required` marks the device "Choose a time zone."; after save, step 4 lists mocked `candidates-loveland.json` with the nearest preselected, shows distance, elevation difference and years of record, and "Use this baseline" sends `PUT .../baseline` with exactly `{"reference_station_id": ...}`; `candidates-empty.json` shows the empty message; a mocked 502 on candidates shows the "saved; choose later" message and the station is still listed; "Skip for now" sends nothing; a save response with `not_covered` shows "These stations are no longer on this account; ask Rodney to disable them:" and the names. Then: `localStorage`, `sessionStorage` and `page.url()` do not contain the key; console messages and all request URLs do not contain it (it appears only in the POST bodies of `connect/validate` and `connect/save`); after navigating away and back the input is empty; the input has `type="text"`, `autocomplete="off"` and class `secret-input`. The real fixture MACs can be registered once per `make reset`; a second run yields 409 `device_already_registered`, which the spec reports as "run make reset && make up && make web-build && make seed && make e2e-data".
7. `forecast.spec.ts` (mocked `forecast.json`, `forecast-ensemble-null.json`, `forecast-changes.json`): the live chart draws `line-forecast`, `line-median` and a `band-outer` ribbon; the Temperature/Precipitation and Hourly/Daily switches change the plotted series with no extra request; `forecast-ensemble-null.json` hides the ribbons and shows "Uncertainty range unavailable right now."; `stale: true` shows the saved-at line; the change view requests `forecast/changes?from=<today>&to=<today+6>` (station-local date via `page.clock`), draws 8 lead lines with `line-lead-0` strongest, the table has one row per day with the "moved by" text, null points break lines, picking a 15-day span shows "Pick up to 14 days." without a request (and a mocked 422 shows the same text), "Chance of rain is not available before October 2024." appears for a past window with null probability; captions carry the response `attribution` and `model`; the browser makes no request to any host other than the page origin and Supabase (route log); a mocked 502 and 503 show `ErrorPanel` with Retry; mocked 404 shows "That isn't available.". Real smoke with the 02 replay stack: both sections render for a seeded station.

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

- [ ] Login uses `signInWithPassword` only (no `signInWithOtp`, `signUp`, `resetPasswordForEmail`, OAuth or code exchange anywhere: lint rule); wrong password, unknown email and banned user show one message (S4). `detectSessionInUrl` is false.
- [ ] `/set-password`: the token is read from the URL fragment and the fragment is removed before any `await`; the token is held only in memory, redeemed on submit, never stored, logged or sent anywhere except `verifyOtp`; expired or used links show the fixed message; password length is checked in the form and by the Supabase setting (04 8.1). The change-password form never asks for the current password.
- [ ] `next` is read from the current URL at the moment of login success (no storage) and validated by `safeNext` (same origin via `new URL`, `u.pathname` equals the raw path and does not start with `//`, no backslash/control/dot segments, no fragment); tests include `/\t/evil.com`, `/%5C%5Cevil`, `/.//evil.com`, `/a/..//evil.com`, `/%2e//evil.com`.
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
- [ ] Every chart shows the provenance caption (baseline, day definition, GDD settings for GDD, winter note only on rain, off-season label out of season) (F5, F6); the no-baseline and baseline-loading states draw no comparison. [ ] User search never shows or requests an email; the candidate list is rendered as escaped text.
- [ ] Open-Meteo CC BY 4.0 attribution in the footer and on the forecast chart.
- [ ] e2e config refuses non-localhost base URLs; `helpers/invite.ts` refuses non-local stacks.

---

## 8. Open questions / assumptions

- **Q1.** (Updated Oct 7, proposals/06.) The forecast chart and the day-by-day change view are built in v1 with `FEATURES.forecast = true`. Defaults to confirm: 7-day hourly horizon (`FORECAST_HOURLY_DAYS`), 7-day default change window (`FORECAST_CHANGES_DAYS`), no model selector (the API pins `gfs_seamless` and `ecmwf_ifs025`), forecast days in local wall-clock time. The change view shows day offsets (0 to 7 days earlier), not model runs, because that is what Open-Meteo's archive provides. Needs 02's task 13; nothing needs poller data.
- **Q2.** CSP shape: split between the SvelteKit meta tag (`script-src`) and the static header, with one sanctioned `style-src-attr` hash for the route announcer (conventions). Unverified: that `kit.csp` writes the meta tag into the adapter-static fallback page (task 3 has the fallback script), the exact announcer literal in the pinned SvelteKit (check-build fails loudly if it changes), and `netlify.toml` header behaviour for the fallback rewrite. A single header with `script-src 'self' 'sha256-...'` would be the cleaner end state if the Netlify build could emit it.
- **Q3.** (Closed, Oct 7) Follow discovery is name search plus a follow request; no suggestions.
- **Q4.** Wording and screenshots for "where to find your AWN API key". Needs a human to confirm the current Ambient UI.
- **Q5.** (Closed, Oct 7) GDD defaults are simple average, base 50 F, start April 1, all selectable; labels are in `labels.gddMethod`. Rodney did not name a start date; April 1 was chosen to match the April-October season (set `GDD_DEFAULTS.start` and the API default to `01-01` together to reverse).
- **Q6.** Band/prior-year rules (5 drawn years, all years in bands, `bands: null` under 10 years, `missing_days <= 7`) are 02's; the web draws whatever percentile list it receives.
- **Q7.** Presentation defaults not in the design doc: "same scale for all stations" off; all five prior years drawn faintly; no overlay of the airport's current year (02 does not return it).
- **Q8. New-station setup (product question for the owner).** The API derives each station's time zone from AWN data; the web shows a fallback zone picker only for devices whose `timezone` is null (list content `TIMEZONE_CHOICES`, undecided; may be a short US list or all IANA zones) and handles 422 `timezone_required`. Baseline assignment is decided (Oct 7): the connect flow's step 4 and the station page let the owner confirm the suggested airport station or pick another; until then a station charts with "No baseline yet." Reconnect has no dedicated route; it re-runs `connect/save` with the user's existing devices (02 replaces the credential and repoints the stations the new key covers; the rest come back in `not_covered`).
- **Q9.** `STALE_AFTER_MIN = 30` and the stale/delayed wording are placeholders.
- **Q10.** `<project-ref>` is a placeholder in `security-headers.json` (and thus in the generated production CSP in `netlify.toml`; the local Caddy CSP does not contain it). Rodney fills it with the Supabase project host and runs `npm run headers:generate` before the first deploy; the `CONTEXT=production` check failing on the placeholder is the intended guard. The signed-proxy line stays commented until the owner enables it with the API secret.
- **Q11.** (Closed, Oct 7) Off-season (November-March) shows the most recent completed April-October season with a label (`displaySeason`). Interpretation: in November and December that is the season that just ended (the same calendar year), in January to March it is last year's. The year is taken from the first selected station's local date.
- **Q12.** Requests to 05: `make test-web` should also run `npm run test:tz` and `npm run headers:check`; `.gitignore` entries `web/tests/.auth/`, `web/test-results/`, `web/playwright-report/`; the Caddyfile must exclude `/_app/*` from the SPA fallback (05 is adding it; `headers.mjs check` and the CSP spec assert it) while keeping the `@root` rewrite and `try_files {path} /200.html` for other paths; `make test-e2e CONFIRM=1` chain as in 05 section 7.2; Casey login behaviour if the optional inactive-account test is wanted.
- **Q13.** AWN key field: `type="text"` plus masking class avoids password-manager capture but is not guaranteed in every browser; accepted risk for a five-person app.
- **Q14.** (Closed, Oct 7) Magic links and their scanner/PKCE issues no longer apply (email + password, hand-delivered set-password link, proposals/05). Growth path to emailed links is 04 section 8.1a; if it is ever built, the `/set-password` page already accepts the `token_hash` shape that an email template can produce.
- **Q15.** Unverified assumptions to confirm during implementation (settled by the specs and tests named here, not by Rodney): GoTrue `verifyOtp` with a `recovery` token hash from `generate_link` returns a session with no SMTP configured; link lifetime and single use (`set-password.spec.ts`, 04 `invite_link_sets_password`); `updateUser({ password })` works without the old password once "require current password" and reauthentication are off (the hosted switches are checked in 06 section 7 step 5, fallback proposal 05 option b1); and the Netlify `signed` option behaves as 06 describes.
