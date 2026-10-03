# Family Weather App: Design Doc

Oct 2, 2026 · @Rodney

## Overview

A private, desktop-first dashboard that pulls in three family Ambient Weather (AWN) stations (Helena MT, Laramie WY, Loveland CO) and compares each one's rainfall and growing degree days (GDD) against prior years, alongside forecasts and how those forecasts have changed over recent model runs.

It is built for family use, not commercial use. The main daily viewer is a family member checking several stations on a desktop or laptop, so charts can be dense and mobile is deferred.

What it adds over weather.city and the AWN dashboard:

- **Year-to-date overlays:** cumulative rainfall and GDD for each station against prior years and percentile bands.
- **Forecast-change history:** how precipitation chance and totals shifted across the last several model runs, which weather.city shows only for temperature.
- **Forecast vs. gauge:** what the models said versus what each station caught.
- **Side-by-side stations:** compare every station you follow in one view.

Non-goals: public sign-up, mobile-first design, multi-region scale, running our own weather models, and any monetization.

## Architecture and data flow

One Rust binary on Fly.io polls stations and forecast sources into Supabase Postgres and serves chart data to a SvelteKit app on Netlify.

&#91;embedded content: data flow · 3 sources, Fly.io, Supabase, Netlify\]

The browser signs in with Supabase Auth directly and receives a short-lived JWT, which it sends with every API request. The API checks the token's signature against Auth's public keys, cached so Auth is not called per request, then applies the follow rules before reading Postgres. Whether the token lives in the browser or in a SvelteKit server-side session cookie is still to decide. Netlify proxies `/api/*` to Fly, which avoids CORS. GitHub Actions deploys the API, and Netlify builds the web app on each push.

## Requirements and assumptions

Version 1 succeeds if all family stations ingest reliably and the year-to-date rainfall and GDD comparisons can be trusted. Forecast-change history comes after that.

**Functional requirements**

- Poll each station every 5 minutes, upsert idempotently, and backfill gaps by paging `endDate`.
- Show cumulative rainfall per station against prior years and percentile bands from a reference record: the nearest long-record station (the airport for Helena and Laramie, Fort Collins USC00053005 for Loveland, PRISM where none is close), with the nearest airport's own totals shown alongside for comparison.
- Show GDD accumulation per station, computed the same way as the reference record.
- Snapshot forecasts every model run, storing precipitation probability and totals, and later attach what the gauge caught.
- Show how precipitation chance and totals changed across the last several runs for each location.
- Invite-only login; a user sees another user's stations only after that owner approves a follow request, and family members start as mutual approved follows.
- A guided connect-station flow that validates an AWN API key instantly.

**Assumptions to confirm or design around**

| Assumption | Why it matters | Mitigation |
| --- | --- | --- |
| A backyard gauge is comparable to a nearby reference gauge | Prior years come from a regional reference station (GHCN-Daily via ACIS), not the same instrument | Label the baseline source on every chart |
| Unheated tipping buckets undercatch snow | Winter cumulative rainfall will read low against the reference | Flag or season-limit winter totals |
| GDD uses one fixed method (50 °F simple average, or the 50/86 °F clamped variant) | A mismatch between station and reference makes the overlay lie | Decide once, apply to both |
| AWN history depth is undocumented | Backfill may be shorter than hoped | Test paging back on day one |
| Open-Meteo does not archive ensemble runs | A missed poll is a permanent gap | Run the snapshotter on a schedule and record run time per snapshot |
| Ambient's rolling rain counters can reset before capture | Counter-based totals may be wrong | Derive totals from stored 5-minute data using accumulation deltas, not integrated instantaneous rates, and reconcile each day against AWN's own daily total, flagging differences |
| Daily totals break at local midnight | DST shifts break UTC-day sums | Store UTC, convert at the edges using each station's own time zone |

**Constraints**

- Personal, non-commercial use, which the Open-Meteo free tier requires, with CC BY 4.0 attribution.
- AWN limits: 1 request per second per API key and 3 per second per application key.
- Desktop-first layout.

**Open questions**

- Does the Laramie owner also have a Mesa, AZ station on the same Ambient account, and which baseline should it use?
- Do the baselines fit each station's elevation: Helena airport USW00024144 (3,865 ft), Laramie airport USW00024022 (7,273 ft) and Fort Collins USC00053005 (about 5,004 ft)?

## Data model and credentials

Credentials live in their own table that only the poller reads, and visibility is a separate follow relationship, approved by the owner and checked in one function. That keeps API keys out of every query path and leaves room for broader sharing later.

| Table | Key columns | Notes |
| --- | --- | --- |
| users | id (auth user id), display name | Invite-only accounts |
| follows | follower\_id, followee\_id, status, decided\_at | Status is pending, approved, declined or revoked; family members start as mutual approved follows; approval shares all of the owner's stations; `can_view(user, station)` is the only access check |
| credentials | owner\_id, api\_key (encrypted), application\_key (nullable), status, last\_ok\_at | One row per Ambient account, which can expose several devices; status is ok, invalid or rate\_limited; never returned by the API |
| stations | id, MAC, name, owner\_id, credential\_id, lat, lon, timezone, reference\_station\_id | One row per AWN device, so an owner can have several stations |
| observations | station\_id, ts\_utc, sensor columns | 5-minute raw data, primary key (station\_id, ts\_utc), sparse nullable sensor columns |
| daily\_rollups | station\_id, local\_date, tmax, tmin, rain\_in, gdd | Precomputed from observations for fast charts |
| reference\_daily | reference\_station\_id, date, tmax, tmin, precip | Prior-year baseline from GHCN-Daily via ACIS |
| forecast\_snapshots | location\_id, fetched\_at, model, run\_time, valid\_time, variable, p10, p50, p90, prob\_precip | Quantiles per valid time, not every ensemble member |

**Credential handling**

- Encrypt API keys at rest with AES-GCM, master key in Fly secrets. Start with an `add-station` CLI command; the guided connect page replaces it later.
- Wrap keys in `secrecy::SecretString` and redact query strings from logs, since AWN sends keys in the URL.
- Enable row-level security with no policies on every table, or use a non-exposed schema, because Supabase exposes the public schema over its REST API. The Rust API is the only application database client; for development, browse data through the Supabase dashboard or a read-only database role.
- On a 401 or 403, mark the credential invalid, stop retrying and show "needs reconnecting" in the UI. Back off on 429.
- Take a scheduled offsite `pg_dump`; the forecast snapshots cannot be rebuilt.

## Build milestones

Build in five vertical slices, starting with ingestion so data accumulates while the charts are being built.

&#91;embedded content: roadmap · 5 phases, each ending in a gate\]

Forecast snapshots and station history cannot be rebuilt later, so the poller goes live in phase 2, before any chart. Each later phase adds one view on top of data that is already accumulating, and no phase starts until the previous gate passes.
