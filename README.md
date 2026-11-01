# Family Weather App

A private, invite-only weather dashboard for one family. It pulls in three Ambient Weather (AWN) backyard stations (Helena MT, Laramie WY, Loveland CO) and compares each one against prior years.

- **Year-to-date rain and growing degree days (GDD)** for each station, overlaid on prior years with percentile bands. The bands come from the nearest airport's long weather record (NOAA data via ACIS).
- **Live forecasts** for each station, hourly and daily, with an ensemble range band (10th to 90th percentile). Data is from Open-Meteo.
- **Forecast-change view:** day by day, how the forecast for a given day looked 0 to 7 days earlier (rain total, chance of rain, high and low).
- **Follow-based sharing:** you see another person's stations only after they approve your follow request. Family members start as mutual follows.

Non-goals: public sign-up, mobile-first design, multi-region scale, running our own weather models, any monetization. It is desktop-first.

This is a personal, non-commercial project. Forecast data is by [Open-Meteo.com](https://open-meteo.com/), licensed [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/); the app footer carries that attribution.

## How it fits together

| Piece | What it is | Where it runs |
| --- | --- | --- |
| Web | SvelteKit static single-page app (`web/`) | Netlify |
| API | Rust (Axum). Checks the login token, applies the follow rules, serves charts and forecasts | Fly.io app `famweather-api` |
| Poller | Rust. Polls each station every 5 minutes, syncs airport records, computes daily rollups. The only piece that can decrypt stored station keys | Fly.io app `famweather-poller` |
| Admin | Rust CLI (`admin`): invites, reset links, adding stations, migrations. No machines at rest | Fly.io app `famweather-admin` |
| Backup | Daily encrypted database dump to S3 | Fly.io app `famweather-backup` (not built yet) |
| Database and login | Postgres plus Supabase Auth (email + password, no sign-up) | Supabase |

The API, poller and admin are one Docker image (`backend/Dockerfile`) deployed as separate Fly apps, so the poller's decryption key never sits next to the API.

Locally, Docker Compose runs the same pieces on your machine. Caddy stands in for Netlify, and plain Postgres plus the open-source Supabase Auth server stand in for Supabase:

```
browser -> http://localhost:8000 (Caddy)
             /api/*      -> api    -> db (Postgres)
             /auth/v1/*  -> auth   -> db
             everything else -> web/build (static files)
           poller -> db     (idle unless you turn it on)
```

### Repo layout

| Path | Contents |
| --- | --- |
| `web/` | The SvelteKit app, its tests, `netlify.toml` and the security-header source (`security-headers.json`) |
| `backend/` | Rust workspace: `crates/{core,api,poller,admin}`, SQL `migrations/`, recorded `fixtures/` (AWN, ACIS, Open-Meteo), `Dockerfile` |
| `ops/db/` | Database roles and bootstrap SQL, seed data (`seed/seed.toml`), SQL tests |
| `ops/caddy/` | Local reverse proxy config and generated security headers |
| `ops/local/` | Helper scripts behind the `make` targets |
| `docs/` | Design doc, guides, decisions, proposals (see "Docs map") |
| `docker-compose.yml`, `docker-compose.test.yml`, `Makefile`, `.env.example` | The local stack and the commands to drive it |

Planned but not in the repo yet: `.github/` (CI workflows), `ops/fly/`, `ops/backup/`, `ops/ci/`.

## Prerequisites

macOS is the development platform.

- **Docker Desktop with Compose 2.24 or newer** (check with `docker compose version`; the test setup needs it). Give it about 6 GB of memory and 4 CPUs; the first Rust build is heavy.
- **make** (the macOS built-in is fine).
- **curl, jq, openssl, shasum, perl**, only for `make smoke`. Recent macOS includes them all; if `jq` is missing, `brew install jq`.
- **Node 26**, only for the host-side steps `make web-dev` and `make test-e2e`. `web/.nvmrc` pins major 26 (`web/package.json` allows `>=26 <27`). `ops/local/web-node.sh` uses your current `node` if it is 26, otherwise finds one under nvm, fnm or volta. It never downloads anything; if none matches it prints install instructions.
- **Rust is not needed on the host.** Everything Rust builds and tests inside Docker. Likewise `make web-build` and `make test-web` build the web app in a container.

## Run it locally

Run everything from the repo root. Nothing here needs a cloud account, and nothing is emailed.

**1. First-time setup.** Creates `.env` from `.env.example` (never overwrites an existing one):

```sh
make init
```

**2. Start the stack.** The first run builds the Rust image, which takes several minutes. Later runs are quick. It waits until everything is healthy:

```sh
make up
```

**3. Build the web app.** Caddy serves `web/build/`, which does not exist until you build it. Repeat this whenever you change `web/`:

```sh
make web-build
```

**4. Seed demo data.** Creates the fake demo users, follows, three demo stations and the demo airport (baseline) station rows. It loads no readings yet:

```sh
make seed
```

**5. Load demo readings.** Replays recorded station readings and the airport baseline history into the database (under a minute in replay: the poller's rate gates still run, but at a 20 ms spacing because replay never calls AWN; live mode keeps AWN's real limit). Needs step 4 first. Safe to re-run:

```sh
make e2e-data
```

**6. Open <http://localhost:8000>** and sign in.

Demo sign-in. These are local-only demo values, committed on purpose and refused by production:

| User | Email | Notes |
| --- | --- | --- |
| Rodney | `rodney@example.test` | admin, owns the Helena station |
| Alex | `alex@example.test` | owns Laramie |
| Sam | `sam@example.test` | owns Loveland |
| Pat | `pat@example.test` | follows nobody; sees an empty state |
| Dana | `dana@example.test` | has follow requests in pending, declined and revoked states |
| Casey | `casey@example.test` | inactive account (sign-in is refused) |

Password for all of them: `local-only-demo-pass-1234` (`LOCAL_DEMO_PASSWORD` in `.env.example`).

What to try:

- Sign in as Rodney, Alex or Sam: they follow each other, so each sees all three stations. Look at the rain and GDD charts and the forecast views.
- Sign in as Pat: no stations visible until a follow is approved. Search for a person by name and send a follow request.
- Sign in as Dana: see pending, declined and revoked follows.

The demo stations use fake coordinates and fake station IDs. In the default replay mode, forecasts come from recorded Open-Meteo files shifted so the forecast starts today.

### Stop, reset, look around

```sh
make down      # stop containers, keep your data
make up        # start again (data is still there)
make reset     # stop AND delete all local data (database volume); start over with make up, make seed, make e2e-data
make ps        # container status
make logs      # follow logs from all services (Ctrl-C to stop watching)
make logs-pretty            # follow the poller's logs, one readable line per job (SVC=api for another service; needs jq)
docker compose logs -f api     # one service: api, poller, auth, caddy, db
make psql      # SQL prompt on the local database (as the postgres superuser)
```

After `make reset`, `web/build/` is still there, so you only need `make up`, `make seed`, `make e2e-data`. If you changed the Rust code, `make up` rebuilds the image first (it runs `make build`).

### Invite someone / get a set-password link

Nothing in this app sends email. Inviting someone prints a one-time link, which you send by hand. Locally the link starts with `http://localhost:8000/set-password#token_hash=`:

```sh
make invite EMAIL=you@example.com NAME='Your Name' FOLLOW_ALL=1
make reset-link EMAIL=you@example.com     # new link for a forgotten password
```

`EMAIL` and `NAME` are both required. `FOLLOW_ALL=1` is optional; it makes the new user mutually follow the existing users. `make reset-link` also signs that user out everywhere. The stack must be running.

### Frontend work with hot reload

```sh
make web-dev
```

This installs web dependencies (`npm ci`) on the host with Node 26 and starts the Vite dev server at <http://localhost:5173>. The stack must be running (`make up`): the dev server forwards `/api` and `/auth/v1` to Caddy on port 8000. Sign in with a password at the 5173 address. Set-password links printed by `make invite` point at port 8000, so open those there. Check the strict security headers (CSP) only at port 8000; the dev server's differ.

### Poller on and off

By default the poller is idle (it logs one line and does nothing), so the database only changes when you run `make e2e-data`.

```sh
make poller-replay   # run the scheduler against recorded fixtures (shifted to now)
make poller-idle     # turn it back off
```

### Run against your own real station

This sends real traffic to the real services, from your machine, into your local database only.

1. In `.env`, set `AWN_APPLICATION_KEY` to your own *development* Ambient Weather application key (not a production key, not anyone else's).
2. Start clean without demo data, because the seeded fake credentials would be rejected by AWN and burn rate limit:

   ```sh
   make reset
   make up
   make web-build
   make invite EMAIL=you@example.com NAME='You'
   make live
   ```

3. Open the printed set-password link, choose a password, and sign in.
4. In the app's Connect station screen, paste your own station API key. The command-line alternative is below; it reads the key from a hidden prompt, never from the command line:

   ```sh
   make add-station ARGS='--owner "You" --mac AA:BB:CC:DD:EE:FF --name "My Station" --lat 46.6 --lon -112.0 --timezone America/Denver --std-offset-minutes -420'
   ```

   (`--owner` is a display name or user id. Optional: `--elevation-ft` and `--reference-station <GHCN id>`.)

`make live` waits 5 seconds (Ctrl-C to abort), then restarts `api` and `poller` in live mode. What goes live: the Ambient Weather API (your station only), ACIS (airport baseline lookups) and Open-Meteo (forecasts). To go back to demo mode, run `make reset` and start over (`make poller-idle` only stops the poller; the API stays in live mode until recreated). A later plain `make up`, or `make down` then `make up`, also recreates `api` and `poller` in replay mode from `.env`, so run `make live` again to go live. Never use another person's key or a production key here.

### Troubleshooting

| Symptom | Likely cause and fix |
| --- | --- |
| `Cannot connect to the Docker daemon` | Docker Desktop is not running. Start it and retry. |
| `port is already allocated` or `address already in use` on 8000 or 5432 | Something else uses the port (often a local Postgres on 5432). The stack publishes only `127.0.0.1:8000` (Caddy) and `127.0.0.1:5432` (Postgres). Stop the other program. |
| Page is blank or 404 at port 8000 | `web/build/` is missing. Run `make web-build`. |
| `make up` fails because `caddy` will not start, mentioning `security-headers.caddy` | `ops/caddy/security-headers.caddy` is missing (Caddy deliberately refuses to start without it). Docker may also have created an empty folder at that path, which shows up as "is a directory". It is a committed file: run `rmdir ops/caddy/security-headers.caddy` (only if it is a folder), then `git checkout ops/caddy/security-headers.caddy`. Regenerating it instead (`node web/scripts/headers.mjs generate` from the repo root) needs host Node 26. |
| Changes to `.env.example` have no effect | `make init` never overwrites `.env`. Compare the two (`diff .env .env.example`) and copy over what you need. Changing the Postgres and database-role passwords or their `*_VERIFIER` values (`POSTGRES_PASSWORD`, `SUPABASE_AUTH_ADMIN_*`, `APP_*_PASSWORD*`) also needs `make reset`, because they are set when the database is first created. Most other values (for example `RUST_LOG`, `AWN_APPLICATION_KEY`) take effect on the next `make up`. `PUBLIC_SUPABASE_*` and `WEB_NODE_IMAGE` need a `make web-build`. Changing `JWT_HS256_SECRET` also needs re-signed anon and service keys and a web rebuild. Changing `CREDENTIALS_*` makes stored station keys unreadable, so `make reset` and re-seed (or reconnect your station). A stale `WEB_NODE_IMAGE` in `.env` (it should be `node:26-bookworm-slim`) makes `make web-build` use the wrong Node. |
| `web needs Node 26` from `make web-dev` or `make test-e2e` | Your host Node is not major 26 and none was found. `nvm install 26` (or `fnm install 26`, `volta install node@26`), then re-run `make`. |
| Backend change not showing up | Run `make up` (it rebuilds first). `poller`, `migrate` and `admin` share the image `api` builds. |
| `docker compose` says a variable is required, or `.env` is missing | Run `make init`. |
| Sign-in works but charts are empty | You skipped `make seed` or `make e2e-data`. |

## Tests

```sh
make test                  # everything below except e2e and smoke
make check-local-only      # no production hostnames in the local config files
make test-db               # database roles, permissions and follow-rule SQL tests
make test-backend          # Rust tests (unit, API, poller, admin), run in a container
make test-web              # web lint, type check, unit tests (two time zones), production build, bundle-secrets and header checks
make smoke                 # checks the running local stack: health, headers, sign-up disabled, invite and login, token strictness, poller idle
make test-e2e CONFIRM=1    # browser tests with Playwright against the stack on localhost:8000
```

`make test` and its parts run in a separate throwaway Docker Compose project (`famweather-test`) with its own temporary database and no published ports, and tear it down afterwards. They do not touch your working stack or its data. They also scan the database and auth logs for leaked secrets.

Two exceptions:

- **`make test-e2e` is destructive.** The browser tests need the stack at `localhost:8000`, so it runs `make reset`, rebuilds, reseeds, reloads demo data (including the 3 minute load) and then runs Playwright. It refuses to run without `CONFIRM=1`. It needs host Node 26 and downloads a Chromium for Playwright.
- `make smoke` runs against your working stack and creates a throwaway `smoke-...@example.test` user, but deletes nothing. `make smoke SMOKE_ARGS=--clean` is destructive: it backs up `.env` to `.local/`, then overwrites `.env` with `.env.example` (so a development AWN key you put in `.env` is gone until you copy it back from the backup), resets the stack, and tears it down at the end.

## Deploying (future)

**Step-by-step runbook: [`docs/DEPLOY.md`](docs/DEPLOY.md).** The checklist below is the short version.

Not deployed yet. Deployment is a one-time, hands-on setup by Rodney. The full runbook is [`docs/guides/06-deploy-ops.md`](docs/guides/06-deploy-ops.md) section 15.1 (Supabase setup in section 7, Netlify in section 8, secrets in section 12). Expected cost is about $4 to $6 a month. This is the order. Every deploy step is Rodney's to run (credentials and approvals; the guides forbid agents from touching production), so "Rodney only" is not repeated on each line.

**Not built yet:** `ops/fly/site-url.txt` (the production origin, known only once the Netlify site exists). The Fly configs and one-off helper (`ops/fly/`), the backup image and script (`ops/backup/`, tested locally by `ops/backup/test.sh`) and the CI helper scripts (`ops/ci/`) are written. Not yet proven, because they need real accounts: `flyctl config validate` and the `--dockerfile` path (first deploy job), and the `run-oneoff.sh` dry run on Fly (guide 06 section 16, task 2). The GitHub Actions workflows and `.github/dependabot.yml` are written and pass `actionlint`, and `backend/deny.toml` passes `cargo deny check`, but no workflow has run on GitHub yet. Until those pass, treat the first-deploy runbook as unrehearsed.

Checklist:

1. **Pre-build checks.** Both passed on Oct 7, 2026 (see [`docs/Pre-build Checks.md`](docs/Pre-build%20Checks.md)).
2. **Accounts with two-factor on.** GitHub, Fly.io, Netlify, Supabase, AWS (backups only), healthchecks.io, plus password-manager entries.
3. **Netlify site.** Create it before Supabase, because Supabase needs its URL. Base directory `web`, production branch `production` (create the branch from `main` first if needed; its first build fails or is empty, which is expected), branch deploys off, deploy previews on for pull requests. Turn off post-processing and snippet injection (they break the strict content security policy). Do not deploy yet. Note the site URL. Later you add the variables `PUBLIC_SUPABASE_URL` and `PUBLIC_SUPABASE_ANON_KEY` (the publishable key).
4. **Supabase project.** Free plan. Set Site URL to the Netlify production URL. Turn sign-ups and anonymous sign-ins off, leave email on (this enables password sign-in), leave SMTP unconfigured, set the redirect allow-list empty, set the email OTP lifetime to 3600 seconds, set minimum password length 12, and turn off both "reauthentication on password change" and "require current password". If either of those two switches cannot be turned off, stop and tell the coding agent (there is a fallback design). Switch to asymmetric signing keys, disable the Data API, enforce SSL, and download the CA certificate and commit it as `backend/certs/supabase-ca.crt`. Then create the database roles with `ops/db/bootstrap/roles.sql` and run `ops/db/bootstrap/auth_access.sql` (the `app_auth` functions that let the app check sessions) from a trusted shell, never the dashboard SQL editor. Put the two Netlify variables from step 3 in place.
5. **AWS S3 bucket and age key for backups.** Upload-only IAM user, versioning, 90-day expiry; an offline `age` key pair for encrypting dumps.
6. **healthchecks.io checks** for ingest, backup and database size.
7. **Fly apps.** Create `famweather-api`, `famweather-poller`, `famweather-admin`, `famweather-backup`. Set secrets from a private file you delete afterwards, after checking it with `ops/ci/check-demo-values.sh`. Roughly, by app:
   - poller: its database login, the AWN application key, the **private** credential key, healthcheck URLs.
   - api (set later, at step 13): its database login, the AWN application key, the **public** credential key.
   - admin: the migrator database login, Supabase URL and secret key, the public credential key.
   - backup: the backup database login, AWS upload keys, the age public key, healthcheck URLs.

   Generate the credential key pair offline with `admin keygen`. The private key lives only on the poller (and offline in your password manager).
8. **GitHub settings and workflows.** Because the repo is public:
   - Turn on secret scanning and push protection **before the repo is made public**, plus Dependabot alerts and security updates, and private vulnerability reporting.
   - Under Actions settings, require approval for workflows from outside collaborators, and keep the default token read-only.
   - Create the `production` environment (deployment branch `main`, you as required reviewer) holding the Fly deploy tokens, `MIGRATOR_DATABASE_URL` and `EXPOSURE_DATABASE_URL`. Create the `ci-readonly` environment holding its own copy of `EXPOSURE_DATABASE_URL`.
   - Set the repo variable `SITE_URL` (from step 3). `DEPLOY_API` and `DEPLOY_WEB` stay unset until step 13.
   - Merge the workflows (they do not exist yet; see above).
   - The `production` branch ruleset comes later, at step 13.
9. **Poller goes live first.** Push to `main`: the workflow migrates the database and deploys only the poller. Do this early because Ambient's station history ages out and cannot be rebuilt later; every day without the poller is data lost.
10. **Backups.** Run the backup image once by hand, create the daily scheduled machine, and confirm the encrypted objects land in S3 and the healthcheck goes green.
11. **Invite yourself, then family.** `admin invite` run in an interactive one-off session prints a set-password link; you send it by hand. Add the airport baseline stations (`admin add-reference-station`) and the three weather stations.
12. **Watch ingest go green** in healthchecks.io.
13. **API and web.** Commit the real Supabase project ref into `web/security-headers.json` and regenerate `web/netlify.toml`, set the API secrets, set the repo variables `DEPLOY_API=true` and `DEPLOY_WEB=true`, add the branch ruleset on `production` (block deletion and force pushes only; no "restrict updates" rule), and re-run the deploy. The workflow publishes the web app by fast-forwarding `production`, and Netlify builds it. Check the site, then confirm `/api/v1/me` returns 200 with the new signing key, and only then disable Supabase's legacy keys.
14. **Restore drill.** Restore the latest backup into a local stack and then delete the data. Repeat quarterly.

## Security notes (public repo)

- No real secrets, station MAC addresses, home coordinates or family email addresses are committed. Seed data uses fake MACs (`AA:BB:CC:...`), approximate coordinates and `@example.test` addresses.
- Everything in `.env.example` is a throwaway demo value, public on purpose. Their SHA-256 fingerprints are listed in `backend/demo-fingerprints.txt`. The Rust services and the admin tool refuse to start in production with any of those values (this works today). The web production build also refuses them (`scripts/check-bundle-secrets.mjs`, run by `web/netlify.toml` in production builds; this works today). Only the pre-deploy secrets check in CI (`ops/ci/check-demo-values.sh`) is planned.
- `.env` is git-ignored. Real credentials never go in the repo; production secrets live in Fly, Netlify, GitHub environments and a password manager.
- Locally, only `127.0.0.1:8000` and `127.0.0.1:5432` are published, and sign-up is disabled.
- **Reporting a vulnerability:** please do not open a public issue. Use GitHub's private vulnerability reporting (the repository's Security tab, "Report a vulnerability").

## Docs map

| Doc | What it covers |
| --- | --- |
| [Design doc](docs/Family%20Weather%20App%20Design%20Doc.md) | What and why: requirements, architecture, assumptions |
| [00 conventions](docs/guides/00-conventions.md) | Shared names, layout, environment variables, binaries (read first) |
| [01 web](docs/guides/01-web.md) | The SvelteKit app, charts, security headers |
| [02 api](docs/guides/02-api.md) | HTTP API, auth checks, follow rules |
| [03 poller](docs/guides/03-poller.md) | Poller jobs, rate limits, alerting, credential encryption |
| [04 database](docs/guides/04-database.md) | Schema, roles, Supabase config, admin CLI, seed data |
| [05 local dev](docs/guides/05-local-dev.md) | The Docker Compose stack and `make` targets |
| [06 deploy and ops](docs/guides/06-deploy-ops.md) | Fly, Netlify, Supabase production, CI/CD, backups, runbooks, cost |
| [Guide review decisions](docs/Guide%20Review%20Decisions.md) | Decisions Rodney made, with dates |
| [Design review findings](docs/Design%20Review%20Findings.md), [Pre-build checks](docs/Pre-build%20Checks.md) | Review notes and the pre-deploy verifications |
| [Proposals](docs/proposals/) | Accepted design changes: credential encryption, Fly layout, Supabase auth schema access, web deploy gating, [login method](docs/proposals/05-login-method.md), [forecast data](docs/proposals/06-forecast-data.md) |

## License

There is no license file yet, so no license is granted: all rights reserved by the owner until one is added. Forecast data is Open-Meteo.com, CC BY 4.0.
