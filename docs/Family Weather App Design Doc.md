# Family Weather App: Design Doc

Oct 2, 2026 · @Rodney

## Overview

A private, desktop-first dashboard that pulls in three family Ambient Weather (AWN) stations (Helena MT, Laramie WY, Loveland CO) and compares each one's rainfall and growing degree days (GDD) against prior years, alongside forecasts and how those forecasts have changed over recent model runs.

It is built for family use, not commercial use. The main daily viewer is a family member checking several stations on a desktop or laptop, so charts can be dense and mobile is deferred. Rainfall is the primary view.

What it adds over weather.city and the AWN dashboard:

- **Year-to-date overlays:** cumulative rainfall and GDD for each station against prior years and percentile bands.
- **Forecast-change history:** how precipitation chance and totals shifted across the last several model runs, which weather.city shows only for temperature.
- **Forecast vs. gauge (after v1):** what the models said versus what each station caught.
- **Side-by-side stations:** compare every station you follow in one view.

Non-goals: public sign-up, mobile-first design, multi-region scale, running our own weather models, and any monetization.

## Architecture and data flow

The web app is a static single-page app on Netlify. The API and the poller are separate Fly.io apps built from one codebase, with exactly one machine each, so there is no high-availability pair. Fly gives every machine in an app the same secrets, so keeping the decryption key away from the API takes separate apps: `famweather-api`, `famweather-poller`, `famweather-admin` (no machines at rest; used only for one-off admin runs) and `famweather-backup` (the daily backup). The poller is always on and fully independent of the API. Both talk to Supabase Postgres, and `sqlx` migrations are the only schema tool.

&#91;embedded content: data flow · 3 sources, Fly.io, Supabase, Netlify\]

The browser signs in with Supabase Auth directly. Supabase's library holds the short-lived token in the browser and the app sends it to the API as a Bearer header on every request. This is protected by a strict content security policy, no third-party scripts and pinned npm packages. If users grow beyond family, or outside scripts or user content are added, we move to an API-owned cookie session.

The API checks the token strictly: signing method, audience, issuer, expiry and user ID, with public keys cached so Auth is not called per request. It then confirms the user is active and their Supabase session still exists (through a small locked-down database function, since the app's database login cannot read Supabase's login tables), and applies the follow rules before reading Postgres. Netlify proxies `/api/*` to Fly, which avoids CORS. The Fly address is still public, so every route requires login and limits are per user, not per IP. GitHub Actions deploys the poller and API, then publishes the web app by moving a `production` branch forward; Netlify publishes only that branch, so the web never goes live before the API. Netlify also builds previews for pull requests, which never publish to the real site.

## Requirements and assumptions

Version 1 succeeds if all family stations ingest reliably and the year-to-date rainfall and GDD comparisons can be trusted. The guided connect flow and percentile bands are both in v1. Forecast-vs-gauge comes after.

**Functional requirements**

- Poll each station every 5 minutes, upsert idempotently, and backfill gaps by paging `endDate`. Every write is safe to repeat.
- Show cumulative rainfall per station against prior years and percentile bands from a reference record: the station's nearest airport weather station (PRISM is deferred). The default view is April to October, with winter months flagged.
- Show GDD accumulation per station, computed the same way as the reference record.
- Snapshot forecasts as 6-hourly aggregates (precipitation probability and totals), fetched only when a new model run is published, and later attach what the gauge caught.
- Show how precipitation chance and totals changed across the last several runs for each location.
- Invite-only login; a user sees another user's stations only after that owner approves a follow request. The full follow model is kept so new people can join without a rewrite, and family members are seeded as mutual approved follows.
- Rodney is the only admin and sends every invite. One admin command creates the Supabase login, the app user row and the starting follows. New users see an empty state until they add a station or a follow is approved.
- A guided connect-station flow that validates an AWN API key instantly. A command-line `add-station` is only a stopgap for the first stations.

**Assumptions to confirm or design around**

| Assumption | Why it matters | Mitigation |
| --- | --- | --- |
| A backyard gauge is comparable to a nearby airport gauge | Prior years come from the nearest airport station (GHCN-Daily via ACIS), not the same instrument | Label the baseline source on every chart |
| Unheated tipping buckets undercatch snow | Winter cumulative rainfall will read low against the reference | Default to April to October and flag winter months |
| GDD uses one fixed method (50 °F simple average, or the 50/86 °F clamped variant) | A mismatch between station and reference makes the overlay lie | Decide once, apply to both |
| Airport days run midnight to midnight in local standard time | Daylight-saving shifts or other day cut-offs skew comparisons | Compute GDD and rain days for the backyard stations in local standard time too, and note the day definition on the chart |
| AWN history depth is undocumented | Backfill may be shorter than hoped | Run a one-time backfill per station and see how far it goes |
| Open-Meteo does not archive ensemble runs | A missed run is a permanent gap | Fetch when each run is published, catch up on start, and record run time per snapshot |
| Ambient's rolling rain counters can reset before capture | Counter-based totals may be wrong | Derive totals from stored 5-minute counter changes, treat drops as resets, cap impossible jumps, and reconcile each day against AWN's own daily total, flagging differences |
| Daily totals break at local midnight | DST shifts break UTC-day sums | Store UTC, convert at the edges using each station's own time zone (standard time for baseline comparisons) |

**Constraints**

- Personal, non-commercial use, which the Open-Meteo free tier requires, with CC BY 4.0 attribution.
- AWN limits: 1 request per second per API key and 3 per second per application key. The poller uses at most 2 of the application key's 3, leaving room for the connect flow, and both back off on "too many requests".
- Desktop-first layout.
- Cost plan: Netlify free, Fly about $3 to $5 a month, Supabase free. Supabase pauses only quiet projects and the poller writes every 5 minutes. Move to Pro if the database nears 500 MB or a pause warning arrives. A single-server Docker Compose setup is a fallback only.

**Open questions**

- Which airport station is the Loveland baseline? It replaces the Fort Collins volunteer station USC00053005 and still needs confirming.
- What does Open-Meteo's archive ("previous runs") cover, and how far back? Research it and snapshot only what is not archived.
- Does the Laramie owner also have a Mesa, AZ station on the same Ambient account, and which baseline should it use?
- Do the baselines fit each station's elevation: Helena airport USW00024144 (3,865 ft) and Laramie airport USW00024022 (7,273 ft), plus the Loveland airport once chosen?

## Data model and credentials

Credentials live in their own table that only the poller can decrypt, and visibility is a separate follow relationship, approved by the owner and checked in one function. That keeps API keys out of every query path and leaves room for broader sharing later. All tables live in a private `app` schema, with random UUID ids.

| Table | Key columns | Notes |
| --- | --- | --- |
| users | id (auth user id), display name, active flag, admin flag | Invite-only accounts; the active flag is checked on every request; only Rodney is admin; no database link to Supabase's user table, so the admin commands keep the two in step |
| follows | follower\_id, followee\_id, status, decided\_at | Status is pending, approved, declined or revoked; family members start as mutual approved follows; approval shares all of the owner's stations; `can_view(user, station)` is the only access check, run on every endpoint |
| credentials | owner\_id, encrypted api\_key, key\_version, status, last\_ok\_at | One row per Ambient account, which can expose several devices; status is ok, invalid or rate\_limited; never returned by the API |
| stations | id, MAC, name, owner\_id, credential\_id, lat, lon, timezone, reference\_station\_id | One row per AWN device; coordinates are rounded in API responses |
| observations | station\_id, ts\_utc, sensor columns | 5-minute raw data, primary key (station\_id, ts\_utc), sparse nullable sensor columns |
| daily\_rollups | station\_id, local\_date, tmax, tmin, rain\_in, gdd | Precomputed from observations in local standard time |
| reference\_daily | reference\_station\_id, date, tmax, tmin, precip | Prior-year baseline from airport stations via GHCN-Daily and ACIS |
| forecast\_locations | id, station\_id, rounded lat, lon | One per station; access follows the station through `can_view` |
| forecast\_snapshots | location\_id, fetched\_at, model, run\_time, valid\_time, variable, p10, p50, p90, prob\_precip | 6-hourly aggregates per valid time, about 100 MB a year |
| job\_runs | job, started\_at, finished\_at, status, detail | One row per poller job run, used for catch-up and alerting |

There is one AWN application key for the whole app, held as a Fly secret and used by the poller and the connect flow. Each station owner has one AWN API key, stored encrypted in `credentials`.

**Credential handling**

- Encrypt API keys with public-key (HPKE) encryption, so the API can encrypt a new key from the connect flow but only the poller, which holds the private key, can decrypt it. The API holds only the public key; a stored key can be read only by the poller, but the API sees a key in plain text while checking it during connect. Each ciphertext is tied to its row and stamped with a key version so keys can be rotated.
- Wrap keys in `secrecy::SecretString`, strip URLs from errors and redact query strings from logs, since AWN sends keys in the URL. The command-line tool reads the key from a prompt, and a test checks logs for the key.
- Keep app tables in the private `app` schema, hidden from Supabase's REST API, with a CI check that nothing sits in `public`.
- Use least-privilege database roles with verified encrypted connections, never the `postgres` admin login: the API cannot read credentials, the poller can, and backups have their own read-only role. Developers browse data locally, not from production.
- The connect-flow key check is rate-limited per user, returns only valid or invalid, and never accepts a URL or host.
- On a 401 or 403, mark the credential invalid, stop retrying and show "needs reconnecting" in the UI. Back off on 429.
- A scheduled machine in its own Fly app (`famweather-backup`) takes a daily `pg_dump`, encrypts it before upload and stores it in AWS S3, because forecast snapshots cannot be rebuilt.

**Access and sign-up**

- Invite-only is enforced twice: open and anonymous sign-up are off in Supabase settings, and the API also requires the user to be on the app's allow-list (active users).
- Removing a user means setting them inactive, which takes effect on the next request rather than when their token expires. The admin command also signs them out everywhere and bans the login in Supabase.
- The app's database logins never read Supabase's login tables. Two small database functions do it: one answers "is this session still valid?" and one signs a user out everywhere.

## Local development

Docker Compose runs plain Postgres, Supabase's auth server, a mail catcher for magic links, Caddy, the API and the poller. Caddy serves the static web build and forwards `/api/*` to the API, mirroring Netlify. The API's token check works with both local and hosted signing, and production refuses local demo keys.

- Local never points at production. Secrets live in a git-ignored `.env`, with a committed `.env.example`.
- The poller is off by default locally and replays recorded responses. Real AWN calls use only your own station key and a separate dev application key.
- Backfills are tested locally, then the same repeat-safe job is run once against production from a one-off Fly machine that shares the poller's lock and rate limit.
- The production poller goes live early, because forecast snapshots cannot be backfilled.

## Operations

- **Testing:** automated tests use recorded AWN, Open-Meteo and ACIS responses. Security tests try the anon key, expired and unsigned tokens, and each endpoint as a stranger, a pending follower and a revoked follower.
- **Logging:** keys and URLs are redacted; logs are kept for a short, fixed period.
- **Scheduling:** each poller job catches up from its last success on start, takes a database lock so two copies never write at once, logs to `job_runs` and pings a free dead-man's-switch service (such as healthchecks.io) that emails when expected runs stop. Backups ping the same service.
- **Backups:** daily encrypted dumps to S3, kept apart from the encryption keys, with a retention period and a restore drill each quarter.
- **Key rotation:** the HPKE key pair (the poller re-encrypts rows to the new version, then the old private key is retired), the AWN application key, and the Supabase keys.
- **Attribution:** Open-Meteo CC BY 4.0 credit appears in the app footer and on forecast charts.
- **Audit log:** logins, follow decisions and credential changes, with a defined rule for what deleting an account removes.
- **Dashboards:** two-factor login on Supabase, Fly, Netlify and AWS.
- **Connecting Fly to Supabase:** use the direct IPv6 connection or the pooler in session mode, never transaction mode, in the same region.

## Security model

Accepted decisions (review items S1 to S16), in short:

- Every API route requires a strictly checked login, an active-user check and `can_view`, with per-user rate limits.
- The API and poller are separate Fly apps with separate database logins and secrets; only the poller app holds the private key that decrypts AWN keys. Admin runs and backups have their own apps and secrets too.
- App data is hidden from Supabase's public REST API, and database access is least-privilege over encrypted connections.
- The browser holds the token, so the app ships a strict content security policy, HTTPS and anti-framing headers, no third-party scripts, pinned dependencies and no secrets in `PUBLIC_` variables.
- Each Fly app that CI deploys (API, poller, backup) has its own deploy token limited to that app, and no Netlify token exists in GitHub. GitHub Actions versions are pinned. Netlify publishes only the `production` branch, which only the approved deploy workflow is meant to move (a GitHub rule blocks deletion and force pushes, but anyone with repo write access, only Rodney, could still push it by hand; GitHub rules also do not apply to private repos on the free plan), and it builds previews for pull requests (branch deploys off, Dependabot pull requests skipped). Previews use the production backend, so only sign in on previews of changes you've read.
- Backups are encrypted and stored away from their keys.

## Build milestones

Before building, Rodney runs two quick checks: a scratch Supabase project to confirm the session functions work on hosted Supabase, and a throwaway Netlify site to confirm that the deploy workflow's push to `production` publishes. Build in five vertical slices, starting with ingestion so data accumulates while the charts are being built.

&#91;embedded content: roadmap · 5 phases, each ending in a gate\]

Forecast snapshots and station history cannot be rebuilt later, so the poller goes live in phase 2, before any chart. Each later phase adds one view on top of data that is already accumulating, and no phase starts until the previous gate passes.
