# 06 — Deploy and Operations

Production deployment (Fly.io, Netlify, Supabase), CI/CD, backups to S3, monitoring, secrets, runbooks and cost. Read `00-conventions.md` first; names here follow it exactly. **Accepted decisions (owner, Oct 5, 2026):** four Fly apps from one codebase (`docs/proposals/02-fly-app-layout.md`); `app_auth` definer functions for Supabase's `auth` schema (`docs/proposals/03-supabase-auth-schema-access.md`), whose scratch-project checklist is a required pre-build verification and a gate in the first-deploy runbook; web published only from the `production` branch moved by `deploy.yml` (`docs/proposals/04-web-deploy-gating.md`), whose throwaway-Netlify-site test is a required pre-build verification.

## 1. Purpose and scope

Owns:
- Fly.io configuration (`ops/fly/*.toml`, `ops/backup/fly.toml`), app layout, secrets placement, one-off machines (`ops/fly/run-oneoff.sh`).
- Netlify site settings and the 06-owned parts of `web/netlify.toml` (placeholders and the commented signed-proxy line). Headers, CSP and redirects come from 01-web (`web/security-headers.json`).
- Supabase production setup checklist (SQL owned by 04-database).
- GitHub Actions (`.github/workflows/*`, `.github/dependabot.yml`), `ops/ci/*`.
- `ops/backup/` image and scripts, AWS S3/IAM, restore procedure.
- Monitoring, secrets inventory and rotation, runbooks, cost.

Does not own: Rust code, `backend/Dockerfile` (03, core milestone), SQL for roles/schema/exposure check (04), the Compose stack and `make` targets (05), the SPA, CSP source lists and `security-headers.json` (01).

**Rule for implementing agents** (conventions, "Agents and production"): write files and test locally in Compose. Never run production-changing commands (Fly secrets or deploys, Supabase auth probes, admin CLI, restores against production) and never hold production secrets. Every "Rodney" step in sections 7, 8, 9.5, 15 is a human action. Agents verify with `actionlint`, `shellcheck`, `docker build`, `flyctl config validate` where it needs no token, and `ops/backup/test.sh`.

## 2. Decisions applied

| ID | How it is applied here |
| --- | --- |
| P1 | Netlify (static + `/api/*` proxy), Fly (Rust image), Supabase (Postgres + Auth). |
| P3 | Exactly one machine per long-running service, `--ha=false`, poller always on with no services, auto-stop only for API; the DB advisory lock (03) is the second line of defence. |
| P4 | healthchecks.io check per poller job and backup; section 10. |
| P6 | The `admin` CLI is a stopgap; production stations can be added through the web connect flow instead (section 5). |
| S3 | Optional Netlify-signed proxy (`x-nf-sign`), enabled after the first deploy; every route still needs a Bearer token (02). |
| S4 | Supabase sign-up and anonymous sign-in disabled (section 7). |
| S5, S8, G2 | `CREDENTIALS_PRIVATE_KEY` only on the poller app. Fly secrets are app-wide, hence separate apps (4 apps). |
| S6, G3 | Data API disabled; `ops/db/checks/exposure.sh` in CI and after every production deploy, run as `app_ci_check`. |
| S12 | One `DATABASE_URL` per role, `sslmode=verify-full`, never `postgres` in apps. |
| S13, C5 | `ops/backup/` image, age encryption, upload-only IAM, no-overwrite writes, versioning; the encrypted backup includes the output of `app_auth.backup_users()` (9.1, decided Oct 7). S3 retention is decided: versioning plus 90-day expiry, no Object Lock (9.5). |
| Owner, Oct 7 (Fly API auto-stop) | Allowed: the API machine sleeps when idle and wakes in about 1 s; `ops/fly/api.toml` has `auto_stop_machines = "stop"`, `min_machines_running = 0` and the cost table (14) uses the auto-stopping figure. No external uptime probe is set up for the API (10). |
| Owner, Oct 7 (repository) | **Public** repo on GitHub Free. A private Free repo has no deployment environments (required-reviewer approval and environment-scoped secrets would not work); public keeps environments, an enforced `production` branch ruleset and unlimited Actions minutes. Nothing secret lives in the repo by design (11.1 public-repo hygiene). Workflows keep path filters and caches for speed only. |
| Owner, Oct 7 (restore drill) | The quarterly restore drill may run on Rodney's laptop: an explicit, accepted exception to S12, with the data deleted after each drill (15.6). |
| S14 | App-scoped Fly deploy tokens, protected GitHub environment, actions pinned to SHAs, Netlify deploy previews on for pull requests only (branch deploys off, Dependabot branches skipped), only `PUBLIC_*` in the bundle. |
| S15 | Headers from `web/security-headers.json` via `netlify.toml`. |
| S16 | MFA on Supabase, Fly, Netlify, AWS, GitHub, healthchecks. |
| C1, C2 | Free tiers; Supabase Pro trigger rules; cost table. |
| C3 | Direct IPv6 from Fly, session pooler only for IPv4-only clients (CI, operator laptop). Same region. |
| D6 | One-off machines for `admin` and `awn_backfill`. |
| D4 | Static build served by Netlify. |

## 3. Cross-component contracts

### 3.1 Fly apps (proposals/02, accepted)

Fly secrets are **app-wide**: "An app's secrets are available as environment variables at runtime on every Machine belonging to that Fly App" ([Fly secrets](https://fly.io/docs/apps/secrets/)); `fly.toml` has no per-process-group secret scoping ([community thread](https://community.fly.io/t/different-secrets-for-each-process-group/18005)). A Machines-API-only workaround (`ignore_app_secrets`, [thread](https://community.fly.io/t/hide-secrets-from-some-machines/21206)) is not driven by `fly deploy` and is not used. A single app with two process groups would put `CREDENTIALS_PRIVATE_KEY` and the poller DB login into the API machine, defeating S5. **Accepted layout (conventions "Fly layout"): four Fly apps built from the same `backend` image.** Rationale and rejected options: `docs/proposals/02-fly-app-layout.md`. Process names `api` and `poller` are kept so the layout could collapse later.

| Fly app | Process group | Machines | Holds | Config file |
| --- | --- | --- | --- | --- |
| `integrated-awn-weather-api` | `api` | 1 (`auto_stop` allowed) | `app_api` DB URL, `CREDENTIALS_PUBLIC_KEY`, `CREDENTIALS_KEY_VERSION`, `AWN_APPLICATION_KEY`, `NETLIFY_PROXY_SECRET` | `ops/fly/api.toml` (outbound HTTPS to `api.open-meteo.com`, `ensemble-api.open-meteo.com` and `previous-runs-api.open-meteo.com` for live forecasts; keyless, nothing to configure; Fly allows outbound by default) |
| `integrated-awn-weather-poller` | `poller` | 1, always on, no services | `app_poller` DB URL, `CREDENTIALS_PRIVATE_KEY`, `CREDENTIALS_KEY_VERSION`, `AWN_APPLICATION_KEY`, `HEALTHCHECK_URL_*` | `ops/fly/poller.toml` |
| `integrated-awn-weather-admin` | none (zero machines at rest) | 0 | `app_migrator` DB URL, `SUPABASE_URL`, `SUPABASE_SERVICE_ROLE_KEY`, `CREDENTIALS_PUBLIC_KEY`, `CREDENTIALS_KEY_VERSION` | none |
| `integrated-awn-weather-backup` | none (machine image runs the script) | 1 scheduled, `--restart no` | `app_backup` DB URL, `AWS_*`, `BACKUP_S3_BUCKET`, `BACKUP_AGE_RECIPIENT`, `HEALTHCHECK_URL_BACKUP`, `HEALTHCHECK_URL_DBSIZE` | `ops/backup/fly.toml` |

The admin app deliberately does not hold `AWN_APPLICATION_KEY` (least privilege; `add-station` must not require it: closed as Open question 7).

App names are literal strings. They appear in `ops/fly/*.toml` (`app = ...`), `ops/backup/fly.toml`, the `<fly-app>` fill in `web/netlify.toml` and the workflows. Everything else derives the API app name from `ops/fly/api.toml` (`ops/ci/smoke.sh` and `deploy.yml` read it). If `famweather-*` is taken on Fly, replace it in exactly those places with one `grep -rn famweather-`.

### 3.2 Contracts consumed from other guides

| From | Contract used here |
| --- | --- |
| 03 (Dockerfile, core milestone) | Final image distroless nonroot (no shell, no `sh -c`); binaries `/usr/local/bin/{api,poller,admin}`; Docker build context `backend/`. **CA (conventions):** the Dockerfile does `COPY certs/ /etc/ssl/app-certs/` from the `backend/` context, `backend/certs/.keep` is committed so builds work before the CA exists, and production config refuses to start if `/etc/ssl/app-certs/supabase-ca.crt` is missing. 06 supplies the file (runbook step 1b: download and commit `backend/certs/supabase-ca.crt`). |
| 02 | `GET /api/health` unauthenticated and exempt from the signature check. If `NETLIFY_PROXY_SECRET` is set, other requests need a valid `x-nf-sign`. Responses set `Cache-Control: no-store`/`nosniff`. `API_BIND_ADDR` default `0.0.0.0:8080`. |
| 03 | No inbound port; healthcheck pings per 03 section 3.5; `poller run-job awn_backfill --station <uuid>`; `poller run-job credential_reencrypt`; one-offs take the same advisory locks and the `awn:app:poller` / `awn:key:<credential_id>` rate gates. |
| 04 | `ops/db/bootstrap/roles.sql` (passwords via env + `\getenv`), `ops/db/bootstrap/auth_access.sql` (as `postgres`, once, after GoTrue created `auth`; proposals/03, accepted), `ops/db/checks/exposure.sh <url>` (run as `app_ci_check`), `ops/db/checks/size.sql` (400 MB threshold; also used by backup), `ops/db/checks/supabase_smoke.sh`, `ops/db/tests/run.sh`. `app_backup` has an explicit `SELECT` on `app._sqlx_migrations`. Role connection limits: `app_api` 20, `app_poller` 15, `app_migrator` 10, `app_backup` 3, `app_ci_check` 2. |
| 04 | CLI: `admin migrate`, `admin keygen [--check]`, `admin invite --email --display-name [--follow-all]` (prints a one-time set-password link; needs `SITE_URL`, which `run-oneoff.sh` passes from the committed `ops/fly/site-url.txt`; shell mode only), `admin reset-link --email`, `add-station`, `add-reference-station`, `deactivate-user`, `delete-user`, `list-users`; `admin wait --minutes <n>` for interactive one-offs (conventions). `admin migrate` needs only `APP_ENV`, `DATABASE_URL`. |
| 01 | `web/security-headers.json` is the single source of header truth; `web/netlify.toml` headers are generated from it by `node web/scripts/headers.mjs generate` and verified by `node web/scripts/headers.mjs check` (06 never hand-edits or duplicates the header values); 06 only supplies `<fly-app>`, `<project-ref>` and Netlify site settings. Build command and guard scripts are 01's; `.nvmrc`. |
| 05 | Make targets `check-local-only test-db test-backend test-web` (CI runs these); `make init` creates `.env`. No local `backup` compose service (conventions "Backups (local)"): `ops/backup/test.sh` owns backup testing. |

### 3.3 Env vars set in production (exhaustive)

| App | `[env]` (toml) | Secrets (`fly secrets`) |
| --- | --- | --- |
| api | `APP_ENV=production`, `AWN_MODE=live`, `RUST_LOG=info`, `SUPABASE_URL`, `SUPABASE_JWKS_URL` | `DATABASE_URL`, `AWN_APPLICATION_KEY`, `CREDENTIALS_PUBLIC_KEY`, `CREDENTIALS_KEY_VERSION`, `NETLIFY_PROXY_SECRET` (after enabling) |
| poller | `APP_ENV=production`, `AWN_MODE=live`, `POLLER_ENABLED=true`, `RUST_LOG=info` | `DATABASE_URL`, `AWN_APPLICATION_KEY`, `CREDENTIALS_PRIVATE_KEY`, `CREDENTIALS_KEY_VERSION`, `HEALTHCHECK_URL_AWN_INGEST` at first go-live (add `_DAILY_ROLLUP`, `_REFERENCE_SYNC` when those jobs ship); rotation only: `CREDENTIALS_PRIVATE_KEY_PREVIOUS`, `CREDENTIALS_KEY_VERSION_PREVIOUS` |
| admin | (set by `run-oneoff.sh`: `APP_ENV=production` and `SITE_URL`, read from the committed `ops/fly/site-url.txt`, one line: the production origin, https, no path; update it with the custom domain) | `DATABASE_URL` (`app_migrator`), `SUPABASE_URL`, `SUPABASE_SERVICE_ROLE_KEY` (`sb_secret_...`), `CREDENTIALS_PUBLIC_KEY`, `CREDENTIALS_KEY_VERSION` |
| backup | `AWS_DEFAULT_REGION`, `BACKUP_S3_PREFIX=weather` (passed with `-e` on `fly machine run`, not via toml) | `BACKUP_DATABASE_URL`, `BACKUP_S3_BUCKET`, `BACKUP_AGE_RECIPIENT`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `HEALTHCHECK_URL_BACKUP`, `HEALTHCHECK_URL_DBSIZE` |
| Netlify | `PUBLIC_SUPABASE_URL`, `PUBLIC_SUPABASE_ANON_KEY` (= the `sb_publishable_...` key), `NODE_VERSION` (toml); optional runtime-only `NETLIFY_PROXY_SECRET` (8.1) | n/a |

Never set `JWT_HS256_SECRET`, `AWN_MODE=replay`, `FIXTURES_DIR` or a `postgres` URL in production (processes refuse to start). The API must not receive `CREDENTIALS_PRIVATE_KEY` or `SUPABASE_SERVICE_ROLE_KEY`.

## 4. Fly.io design

### 4.1 Region and networking

- Region (decided Nov 1, 2026): Supabase **specific region** `us-west-1` (West US, North California), Fly `sjc`. The dashboard's general "Americas" choice places projects in `us-east-1`, so pick the specific region. Fly has no Oregon region (checked [docs.fly.io/reference/regions](https://docs.fly.io/reference/regions)), which rules out `us-west-2`. Fly cannot target an AWS AZ; measure RTT after deploy (target under 10 ms) from the backup image with `psql -c 'select 1'` and `\timing`.
- DB connection (C3): **direct** `db.<project-ref>.supabase.co:5432` (IPv6 only) from Fly. Rationale: session-level connections for advisory locks and `core::ratelimit` (03), prepared statements, no pooler hop, no $4/month IPv4 add-on. Username is the plain role name.
- **Never** the transaction pooler (port 6543): breaks prepared statements and advisory locks ([Supabase connecting](https://supabase.com/docs/guides/database/connecting-to-postgres)).
- Session pooler (`<pooler-host>:5432`, username `<role>.<project-ref>`; copy the pooler hostname from the dashboard Connect panel, it is region specific) is IPv4-capable and is used for CI migrations and exposure checks (GitHub runners are IPv4-only), for the operator's bootstrap `psql` (a home network may be IPv4-only; the direct host is IPv6-only) and as emergency fallback.
- Connection budget: roles have limits (3.2); the poller uses query pool max 4 plus one connection per held job lock (03), well under 15. A CI run uses `app_migrator` (limit 10) and `app_ci_check` (2). Do not run backfill, backup and an admin shell at the same moment as a deploy migration.
- TLS: `sslmode=verify-full` with `sslrootcert=/etc/ssl/app-certs/supabase-ca.crt`. Verify hostname matching on the first connectivity test (Open question 11); never downgrade to `require` without the owner's approval.
- Fly IPs: API uses shared IPv4 + IPv6 (default); no dedicated IPv4. Other apps have no services, hence no public IPs.

### 4.2 `ops/fly/api.toml`

No `[build]` section: build inputs are passed on the command line (4.7).

```toml
app = "integrated-awn-weather-api"
primary_region = "sjc"          # co-locate with Supabase (us-west-1); 06 section 4.1
kill_signal = "SIGTERM"
kill_timeout = "30s"

[deploy]
  strategy = "rolling"          # one machine: brief restart; bluegreen would briefly run two

[processes]
  api = "/usr/local/bin/api"

[env]
  APP_ENV = "production"
  AWN_MODE = "live"
  RUST_LOG = "info"
  SUPABASE_URL = "https://<project-ref>.supabase.co"
  SUPABASE_JWKS_URL = "https://<project-ref>.supabase.co/auth/v1/.well-known/jwks.json"

[http_service]
  processes = ["api"]
  internal_port = 8080
  force_https = true
  auto_stop_machines = "stop"    # DECIDED Oct 7: allowed for api only (tradeoff below)
  auto_start_machines = true
  min_machines_running = 0

  [[http_service.checks]]
    method = "GET"
    path = "/api/health"
    interval = "30s"
    timeout = "5s"
    grace_period = "15s"

[[vm]]
  processes = ["api"]
  size = "shared-cpu-1x"
  memory = "512mb"
```

`auto_stop_machines` (**decided by Rodney, Oct 7, 2026: allowed**): the API is stateless and independent of ingestion, so stopping when idle costs about a 1 s cold start on the first request after idle (the JWKS fetch at startup has a 3 s timeout, 02 4.3.1, and the in-memory limiters and baseline cache start empty) and saves about $1 to $2 per month; it is safe only because the poller is a different app. Any external uptime probe would wake it, so none is configured (section 10). The cost table (14) matches this. `--ha=false` prevents Fly's default two machines.

### 4.3 `ops/fly/poller.toml`

```toml
app = "integrated-awn-weather-poller"
primary_region = "sjc"
kill_signal = "SIGTERM"
kill_timeout = "30s"

[deploy]
  strategy = "rolling"          # replace in place; catch-up on start covers the gap

[processes]
  poller = "/usr/local/bin/poller"

[env]
  APP_ENV = "production"
  AWN_MODE = "live"
  POLLER_ENABLED = "true"
  RUST_LOG = "info"

# No [http_service] and no [[services]]: the poller is not reachable from the network.
# No auto_stop_machines: it applies only to services, and the poller must never stop
# (missed AWN polls cannot be backfilled once they age out of Ambient's history: F3, P3; forecasts are fetched live by the API, proposals/06).

# SIGTERM: poller grace is 25 s (03); kill_timeout above (30 s) must stay larger.

[[restart]]
  processes = ["poller"]
  policy = "always"

[[vm]]
  processes = ["poller"]
  size = "shared-cpu-1x"
  memory = "256mb"
```

Health: no HTTP port (03), so no Fly check. `poller healthcheck` (heartbeat file) is for Compose; Fly cannot run exec checks. Liveness is the dead-man's-switch (section 10) plus `restart always`.

### 4.4 Migrations: CI step running `admin migrate`, not `release_command` (decision)

`release_command` would need `app_migrator` credentials as a secret in the API or poller app (readable by every machine), violating S5/S12. Conventions: "Migrations run from CI in a protected environment". The deploy job builds `admin` (`cargo run --release -p admin -- migrate`) with `APP_ENV=production` and `DATABASE_URL` = `app_migrator` over the **session pooler**, `sslmode=verify-full`, `sslrootcert=<runner path to backend/certs/supabase-ca.crt>`, before deploying binaries.
- Migrations must be backward compatible with the previous release (expand then contract): old binaries run until redeploy completes. Down migrations refuse to run in production (04).
- Bootstrap is manual and one-time: `roles.sql`, then `auth_access.sql` (section 7). `admin migrate` assumes both are done.
- Fallback if CI cannot reach the DB: Rodney runs `ops/fly/run-oneoff.sh batch integrated-awn-weather-admin integrated-awn-weather-poller poller -- /usr/local/bin/admin migrate`.

### 4.5 Single-app layout (rejected)

A single app with two process groups is rejected: Fly secrets are app-wide, so the API machine would hold `CREDENTIALS_PRIVATE_KEY` and the poller's DB URL (proposals/02, options (b) and (c)). Not documented here.

### 4.6 Secrets handling commands (Rodney; never in agent sessions)

```bash
# never pass values as arguments or heredocs (shell history); import from a private file
umask 077
${EDITOR:-nano} ~/fw-secrets.env                      # KEY=VALUE lines for one app only, mode 0600, OUTSIDE the repo
ops/ci/check-demo-values.sh ~/fw-secrets.env          # must exit 0 (no local demo values); use an absolute path: the script cd's to the repo root
fly secrets import -a integrated-awn-weather-poller --stage < ~/fw-secrets.env
rm -f ~/fw-secrets.env                                # delete it immediately (macOS has no shred; FileVault covers the disk)
fly secrets list -a integrated-awn-weather-poller                 # names and digests only
```
`--stage` defers the restart to the next deploy. Verify `fly secrets list -a integrated-awn-weather-api` does not contain `CREDENTIALS_PRIVATE_KEY`.

### 4.7 Build and deploy invocation (build order: poller first)

Run from the **repo root**; the working-directory argument is the Docker build context (`backend`), and `--dockerfile` is relative to the shell's working directory:

```bash
flyctl deploy backend --config ../ops/fly/poller.toml --dockerfile Dockerfile --remote-only --ha=false --image-label sha-<sha12>
flyctl deploy backend --config ../ops/fly/api.toml --dockerfile Dockerfile --remote-only --ha=false --image-label sha-<sha12>
```
**Path resolution (verified Nov 8, 2026, flyctl 0.5.5):** `flyctl deploy backend` changes into `backend/` before resolving `--config` and `--dockerfile`, so both are relative to `backend/` (hence `../ops/fly/...` and `Dockerfile`); the build context stays `backend/`. Backup builds (`fly deploy .`) are unaffected. **Default (proposals/02):** each app does its own `--remote-only` build (layers are cached). Optional later improvement, only if someone verifies that an app-scoped deploy token can pull a sibling app's image (proposal 02 fact 6): build once with `flyctl deploy ... --build-only --push` and deploy the same image to the other app with `--image`. Every deploy, rollback and `flyctl config validate` always passes `--config`:
`flyctl deploy backend --config ../ops/fly/api.toml --dockerfile Dockerfile --image registry.fly.io/integrated-awn-weather-api:sha-<previous> --ha=false`.

## 5. One-off machines (D6)

`fly machine run` has no `-it`. Interactive commands use `admin wait`: the one-off's main process is `admin wait --minutes 15` (conventions); the operator runs the real command through `fly ssh console --pty -C`; the machine is then destroyed. **Verify in a dry run on distroless** before relying on it: (1) `fly ssh console -C "/usr/local/bin/admin list-users"` works without a shell; (2) the ssh session sees the machine's secrets and env; (3) the machine is destroyed afterwards. **Fallback** if it does not work: add production stations through the web connect flow after Stage B (the CLI is only a stopgap, P6) (there is no batch-mode fallback for `admin invite` or `admin reset-link`: `run-oneoff.sh` refuses them so the link never reaches Fly's log store; if shell mode cannot work, stop and ask the coding agent for a different delivery method).

`ops/fly/run-oneoff.sh`:

```bash
#!/usr/bin/env bash
# usage: run-oneoff.sh batch <app> <image-src-app> <image-src-group> -- <abs-binary> [args...]
#        run-oneoff.sh shell <app> <image-src-app> <image-src-group> [minutes]
set -euo pipefail
usage() { sed -n '2,3p' "$0" >&2; exit 2; }
[ $# -ge 4 ] || usage
mode=$1; app=$2; src=$3; grp=$4; shift 4
case "$mode" in
  batch) [ "${1:-}" = "--" ] || usage; shift
         [[ "${1:-}" == /usr/local/bin/* ]] || { echo "command must be an absolute /usr/local/bin/... path (distroless)" >&2; exit 2; }
         # batch output is read back through `fly logs`, so a set-password link would land in Fly's log store: refuse
         case "${2:-}" in invite|reset-link) echo "admin ${2} prints a set-password link: run it in shell mode (fly ssh console -C), never batch" >&2; exit 2 ;; esac ;;
  shell) minutes="${1:-15}" ;;
  *) usage ;;
esac
# image source = the single running machine of the source process group (not a stray one-off)
cfg="$(fly machines list -a "$src" --json | jq -c --arg g "$grp" \
  '[.[] | select(.config.metadata.fly_process_group == $g)] | if length == 1 then .[0] else error("expected exactly one machine in group " + $g) end')"
image="$(jq -r .config.image <<<"$cfg")"
region="$(jq -r .region <<<"$cfg")"
envargs=()
while IFS= read -r kv; do envargs+=(-e "$kv"); done < <(jq -r '.config.env // {} | to_entries[] | "\(.key)=\(.value)"' <<<"$cfg")  # mirrors POLLER_ENABLED, RUST_LOG, AWN_MODE, APP_ENV
name="oneoff-$(date -u +%Y%m%d-%H%M%S)"
common=(-a "$app" --name "$name" --region "$region" --vm-size shared-cpu-1x --vm-memory 256 --restart no "${envargs[@]}" -e APP_ENV=production)
if [ "$app" = integrated-awn-weather-admin ]; then
  # integrated-awn-weather-admin has no config file, so the non-secret SITE_URL (origin printed in set-password links) comes from a committed constant
  site_url="$(tr -d '[:space:]' < "$(dirname "$0")/site-url.txt")"
  [[ "$site_url" =~ ^https://[^/]+$ ]] || { echo "ops/fly/site-url.txt must hold the production origin (https, no path)" >&2; exit 2; }
  common+=(-e "SITE_URL=$site_url")
fi
machine_id() { fly machines list -a "$app" --json | jq -r --arg n "$name" '.[] | select(.name == $n) | .id'; }
cleanup() { local id; id="$(machine_id 2>/dev/null || true)"; [ -z "$id" ] || fly machine destroy "$id" -a "$app" --force || true; }
trap cleanup EXIT                      # armed BEFORE the machine exists: a failed or interrupted run never leaks one

if [ "$mode" = batch ]; then
  fly machine run "$image" "${common[@]}" -- "$@"
  id="$(machine_id)"; [ -n "$id" ] || { echo "machine $name not found after run" >&2; exit 1; }
  fly machine wait "$id" -a "$app" --state stopped --wait-timeout 1h
  fly logs -a "$app" --machine "$id" --no-tail || true
  code="$(fly machine status "$id" -a "$app" --json | jq -r '[.events[]? | select(.type == "exit") | .request.exit_event.exit_code] | last // 1')"
  echo "exit code: $code"
  exit "$code"                         # the trap destroys the machine
else
  fly machine run "$image" "${common[@]}" -- /usr/local/bin/admin wait --minutes "$minutes"
  id="$(machine_id)"; [ -n "$id" ] || { echo "machine $name not found after run" >&2; exit 1; }
  echo "machine $id is up for $minutes min. In another terminal:"
  echo "  fly ssh console -a $app --machine $id --pty -C \"/usr/local/bin/admin <subcommand> ...\""
  read -r -p "Press Enter when finished (the machine is destroyed on exit) " _
fi
```

`fly machine wait` and the shape of the exit event in `fly machine status --json` are unverified: confirm in the dry run (task 2) and adjust the `jq`.

Uses (flag names per 04; `admin --help`):
- Admin shell (uses the poller's image, which contains `admin`; secrets come from `integrated-awn-weather-admin`): `ops/fly/run-oneoff.sh shell integrated-awn-weather-admin integrated-awn-weather-poller poller 15`, then `fly ssh console ... -C "/usr/local/bin/admin add-station"` (API key via hidden prompt only, S7).
- Invite and reset (shell mode only): `ops/fly/run-oneoff.sh shell integrated-awn-weather-admin integrated-awn-weather-poller poller 15`, then in another terminal `fly ssh console -a integrated-awn-weather-admin --machine <id> --pty -C "/usr/local/bin/admin invite --email <addr> --display-name \"<name>\" --follow-all"` (and `admin reset-link --email <addr>` for a forgotten password). The link is printed once, over the ssh session, so it appears only on Rodney's terminal and never in the machine's stdout/stderr, hence never in `fly logs`; `run-oneoff.sh batch` refuses these two subcommands for that reason. Copy the link straight to the person, never into a ticket or a file. It expires (Email OTP expiration, 3600 s by default), so run the command when the person is ready. Batch mode is fine for `deactivate-user`, `delete-user`, `list-users`, `add-reference-station`, `migrate` (their output holds no secrets).
- Backfill: `ops/fly/run-oneoff.sh batch integrated-awn-weather-poller integrated-awn-weather-poller poller -- /usr/local/bin/poller run-job awn_backfill --station <uuid>`. Safe to repeat (F3); takes `job:awn_backfill` and the `awn:app:poller` and `awn:key:<credential_id>` rate gates shared with the live poller. The command's own exit code is the script's exit code. Do not run during a poller deploy. Key rotation re-encrypt: same with `run-job credential_reencrypt`.
- Logs: `fly logs -a <app>` during the run (redacted by code, S7; do not paste into chats). Destroying a stray: `fly machine destroy <id> -a <app> --force`. A leftover machine in `integrated-awn-weather-poller` that runs a scheduler is a duplicate poller; the advisory lock prevents double writes but destroy it.

## 6. Scheduled backup machine (Fly)

`ops/backup/fly.toml` (no `[processes]`, no `[[restart]]`, no `[env]`: the image `ENTRYPOINT` is the script, the machine is created with `--restart no`, and a toml `[env]` is **not** applied to machines created with `fly machine run`, so env is passed with `-e` below):

```toml
app = "integrated-awn-weather-backup"
primary_region = "sjc"
```

Build context is the **repo root** (the image copies `ops/backup/backup.sh`, `ops/db/checks/size.sql` and `backend/certs/`; one CA source, no duplicate copy). A root `.dockerignore` must therefore exclude everything else (it keeps `.env`, `target/`, `node_modules/`, `.git` out of the context): `*` then `!ops/backup/backup.sh`, `!ops/db/checks/size.sql`, `!backend/certs/`. Backend builds use `backend/` as context with their own `.dockerignore` and are unaffected.

Create once (Rodney), after the app exists (runbook step 5 creates all four apps; do not create it again). Machine `schedule` is a machine property, granularity `hourly|daily|weekly|monthly`, started roughly that interval after creation (not a fixed clock time), so create at a quiet hour (e.g. 09:00 UTC):

```bash
flyctl deploy . --config ops/backup/fly.toml --dockerfile ops/backup/Dockerfile --build-only --push --image-label sha-<sha12>
fly machine run registry.fly.io/integrated-awn-weather-backup:sha-<sha12> -a integrated-awn-weather-backup \
  --name backup-daily --region sjc --schedule daily --restart no --vm-size shared-cpu-1x --vm-memory 256 \
  -e AWS_DEFAULT_REGION=us-west-2 -e BACKUP_S3_PREFIX=weather
```

Update on change (CI does this, 11.4, and **skips it when no machine exists yet**, i.e. on the first deploy): `fly machine update <id> --image registry.fly.io/integrated-awn-weather-backup:sha-<sha12> -a integrated-awn-weather-backup --yes` (verify that it keeps the schedule and the `-e` env; Open question 10). On demand: `fly machine start <id> -a integrated-awn-weather-backup`. Docs: [task scheduling](https://fly.io/docs/blueprints/task-scheduling/), [fly machine run](https://fly.io/docs/machines/flyctl/fly-machine-run/).

## 7. Supabase production checklist (Rodney)

Reference 04 for SQL and 04's hosted-verification items. Tick each in order.

1. Create the project (free plan) with **Specific region → West US (North California), `us-west-1`** (section 4.1; not the general "Americas" choice). Generate a long DB password; store it in the password manager only. The `postgres` password is used only for steps 8 and 9 and emergencies; never put it on Fly or GitHub.
1b. **CA certificate:** Database → Settings → SSL Configuration → download the CA certificate; commit it as `backend/certs/supabase-ca.crt` (public certificate). 02's Dockerfile copies `backend/certs/` to `/etc/ssl/app-certs/` (so the file lands at `/etc/ssl/app-certs/supabase-ca.crt`); the backup image copies the same directory. Builds work before this step because `backend/certs/.keep` is committed, but production config refuses to start without the file, so do it before the first production deploy.
2. Supabase account MFA (S16; GitHub 2FA too) ([Supabase MFA](https://supabase.com/docs/guides/platform/multi-factor-authentication)).
3. Auth → Sign In / Providers: **disable "Allow new users to sign up"**; **disable anonymous sign-ins**; Email provider **on** (this is what enables password sign-in; login is email + password, proposals/05); all social and phone providers off; "Confirm email" on (S4). Set **Email OTP expiration** to 3600 seconds (it is the lifetime of the set-password and reset links; up to 86400 is allowed if family members need longer).
4. Auth → URL Configuration: Site URL = the Netlify production URL (the Netlify site already exists, runbook step 2). **Redirect URLs: leave empty** (decided Oct 7: no flow uses a redirect; password sign-in and `verifyOtp` are direct calls, so Netlify deploy previews sign in without any allow-list entry and the earlier preview wildcard is gone).
5. **Password settings check (required, proposal 05; you set these while creating the project, nothing is verified beforehand):** in the dashboard find and set each of these, and write what you found in the setup notes (04 section 8.1 lists the same intent):
   - **Require reauthentication when changing password: Off** (older name "Secure password change"; Email provider settings, [password security](https://supabase.com/docs/guides/auth/password-security)). If it is on, a signed-in person with a session older than 24 hours cannot change their password without an emailed nonce, and nothing can send email.
   - **Require current password when changing password: Off.** If it is on, a password change from a normal signed-in session (the `/account` form) is refused with `current_password_required` (it does not affect set-password links, verified on GoTrue v2.197.0). If it cannot be turned off, the fallback is that the `/account` change-password flow must send the current password (01 must then add that field), not proposal 05 option (b1).
   - **Minimum password length: 12** (Auth → Sign In / Providers → Email, or Password security), and note whether leaked-password protection is available (Pro only; expected not available).
   - **If either switch cannot be turned off on the free plan, stop and tell the coding agent:** the fallback is proposal 05 option (b1) (a temporary password handed over by Rodney and a "change your password" prompt; `admin invite` then sets an initial random password instead of printing a link). If the minimum length cannot be raised, the web form and `admin` keep the 12-character rule and the gap is written into the security notes (04 section 8.1).
   - Auth → SMTP: **leave custom SMTP unconfigured** (decided Oct 7: nothing in the product sends email; family members get set-password links from `admin invite` / `admin reset-link`, sent by hand). Do not add an email provider or SES now. **Growth path (documented, not built):** if the family grows beyond a few people, configure custom SMTP (Amazon SES is acceptable then: a verified sender domain with SPF/DKIM and production access out of the SES sandbox, [Supabase SMTP](https://supabase.com/docs/guides/auth/auth-smtp)), point the recovery and invite email templates at `https://<site>/set-password#token_hash={{ .TokenHash }}&type=recovery`, and ask the coding agent for the small code paths (04 section 8.1a). The built-in sender only delivers to organization members at about 2 messages per hour, so it is not an option for family.
6. Project Settings → JWT Signing Keys ([signing keys](https://supabase.com/docs/guides/auth/signing-keys)): projects created now already sign with an ES256 (P-256) key, so there is nothing to migrate (verified Nov 1, 2026: the project's JWKS lists one `EC`/`ES256` key; the "Migrate JWT secret" flow in the docs applies only to older projects). Just confirm the current key is the elliptic-curve one. **Keep the legacy HS256 secret until the Stage B JWKS check passes** (runbook step 14). Revocation order then ([API keys](https://supabase.com/docs/guides/api/api-keys)): first **disable the legacy `anon`/`service_role` API keys** (API Keys, legacy tab), move the legacy secret to "Previously used", wait at least 1 h 15 min (token lifetime plus margin), then revoke it, then re-run smoke. Key changes are throttled (about 5 minutes between changes). Access-token expiry 3600 s. `SUPABASE_JWKS_URL = ${SUPABASE_URL}/auth/v1/.well-known/jwks.json`.
7. Integrations → Data API (S6, [hardening the Data API](https://supabase.com/docs/guides/database/hardening-data-api)): turn **off** "Enable Data API" (the app never uses PostgREST). If it cannot be disabled, expose no schemas except an empty `public`, and confirm `app` and `app_auth` are not exposed.
8. **API keys:** use the new-style keys. `PUBLIC_SUPABASE_ANON_KEY` = the `sb_publishable_...` key (public by design). `SUPABASE_SERVICE_ROLE_KEY` = the `sb_secret_...` key, sent as the `apikey` header by the admin client; it goes **only** into `integrated-awn-weather-admin` secrets, never GitHub or Netlify. It must be sent only in the `apikey` header, never as an `Authorization: Bearer` token (it is not a JWT; 04 must make the admin client do this). The legacy anon/service_role JWT keys are disabled, then the legacy secret revoked, per step 6.
9. **Roles (04):** from a trusted shell. Connection: 04 specifies the direct connection for `roles.sql`; the direct host is IPv6-only, so on an IPv4-only network use the **session pooler** instead (same SQL; user `postgres.<ref>`; copy the pooler hostname from the dashboard Connect panel). 04 should mention the pooler alternative (see "Changes other guides must make").
   - Generate five passwords (`openssl rand -hex 24`, URL-safe). Plaintext appears only inside the role-specific `DATABASE_URL`s you will store as secrets; the bootstrap receives **SCRAM verifiers**:
     `export APP_MIGRATOR_PASSWORD_VERIFIER="$(printf '%s\n' "$APP_MIGRATOR_PW" | python3 ops/db/bootstrap/scram.py)"` and the same for `APP_API_`, `APP_POLLER_`, `APP_BACKUP_`, `APP_CI_CHECK_PASSWORD_VERIFIER`.
   - The `postgres` password is never on the command line or in the URL: let `psql` prompt for it, or use a temporary `PGPASSFILE` (mode 0600, deleted afterwards). Never use the dashboard SQL editor (its history stores statements).
   - `psql -X -v ON_ERROR_STOP=1 -f ops/db/bootstrap/roles.sql "postgresql://postgres.<ref>@<pooler-host>:5432/postgres?sslmode=verify-full&sslrootcert=backend/certs/supabase-ca.crt"` (direct: `postgresql://postgres@db.<ref>.supabase.co:5432/postgres?...`).
   - Then, **only after the proposal-03 scratch-project checklist (runbook 15.1 step 0) has passed**, run `ops/db/bootstrap/auth_access.sql` the same way as `postgres`. Record the outcome in 04 section 6.5. Unset the exported variables and remove any PGPASSFILE afterwards.
10. Database → Settings → SSL Configuration: **Enforce SSL on incoming connections = on** (restarts the database for a few seconds; [SSL enforcement](https://supabase.com/docs/guides/platform/ssl-enforcement)).
11. Connection strings: Fly roles: `postgresql://<role>:<pw>@db.<ref>.supabase.co:5432/postgres?sslmode=verify-full&sslrootcert=/etc/ssl/app-certs/supabase-ca.crt`. CI migrator and exposure check: `postgresql://app_migrator.<ref>:<pw>@<pooler-host>:5432/postgres?sslmode=verify-full` and the same for `app_ci_check.<ref>` (CI appends `&sslrootcert=<path>`). Backup: `app_backup` direct URL.
12. After step 11 and after any Supabase settings change run `ops/db/checks/supabase_smoke.sh` and `ops/db/checks/exposure.sh <app_ci_check url>`; both must exit 0.
13. Free-plan facts to re-verify at setup ([billing](https://supabase.com/docs/guides/platform/billing-on-supabase)): 500 MB database (read-only above it), inactivity pause after about 1 week, no managed backups (hence section 9), 5 GB egress, built-in SMTP limits. **Upgrade to Pro (about $25/month) when** the size alert fires at 400 MB, a pause warning arrives, or the poller finds the project paused (C2). Size alerting is done by the backup job (9.3), not by Supabase.

## 8. Netlify design

### 8.1 Site settings (Rodney, UI)

- Create the site and link the repo in runbook step 2 (the Site URL is needed for Supabase). **Base directory `web`**; **production branch `production`** (not `main`); auto-publishing stays **on** (no lock, no build hook, no Netlify token anywhere in GitHub). `main` pushes build nothing because `main` is neither the production branch nor a branch-deploy branch. Only `deploy.yml` moves `production`, by fast-forwarding it to the release commit after the API deploy succeeds (11.4; proposals/04, accepted; conventions "Web headers and proxy"). Create the branch and its GitHub ruleset in runbook steps 2 and 13. Rollback: revert on `main` and let the workflow push again, or "Publish deploy" on an older deploy in the Netlify UI. Build command and publish directory come from `web/netlify.toml`; do not override in the UI.
- Deploy contexts: **Deploy Previews: on for pull requests** (decided; previews never publish to the main site, so the gate is unaffected; PRs target `main`, so choose the option that covers every pull request against the repository, not only PRs against the `production` branch; confirm the exact label in the UI); **Branch deploys: None** (S14 as revised; the production branch is excluded from "All" anyway). `[build] ignore` in `netlify.toml` skips deploy-preview builds for `dependabot/*` branches (exit 0 when `$CONTEXT = deploy-preview` and `$HEAD` starts with `dependabot/`, else exit 1). Previews use the same `PUBLIC_*` values as production and proxy `/api/*` to the production API, so treat a preview as running against real data: don't sign in on previews of changes you haven't read.
- **Post processing off:** Site configuration → Build & deploy → Post processing: disable "Snippet injection", asset optimization (bundling/minify), and pretty URLs rewriting if present. Reason: injected or rewritten scripts break the CSP hash match; smoke checks the deployed `200.html` (section 13, check 5).
- 2FA on the Netlify account (S16).
- Before the first Netlify build, the real Supabase project ref must be committed in `web/security-headers.json` (CSP `connect-src`) and `web/netlify.toml` regenerated with `node web/scripts/headers.mjs generate` (Stage B, runbook step 13); otherwise 01's check and the build fail on the `<project-ref>` placeholder.
- Environment variables, per 01-web: only `PUBLIC_SUPABASE_URL` and `PUBLIC_SUPABASE_ANON_KEY` (Builds scope) plus `NODE_VERSION` in the toml. Nothing secret starts with `PUBLIC_` (S14).
- Optional signed proxy (S3), **off for the first deploy**: conventions keep `signed = "NETLIFY_PROXY_SECRET"` commented out until enabled together with the API secret. To enable: create `NETLIFY_PROXY_SECRET` with scope **Runtime only** and "contains secret value" (same value as the Fly secret), uncomment the line, redeploy Netlify, set the secret on `integrated-awn-weather-api`. It is not available to builds, so it cannot reach the bundle. The toml names the variable only; no secret is in the repo.
- Custom domain optional; if used, update the Supabase Site URL and the admin app's `SITE_URL` (there is no callback allow-list any more).

### 8.2 `web/netlify.toml` (headers generated by `web/scripts/headers.mjs generate` from `web/security-headers.json`)

01-web is the source of truth for the whole file. 06 owns only these fills; if the file differs elsewhere, report it to 01:

```toml
[[redirects]]
  from = "/api/*"
  to = "https://<fly-app>.fly.dev/api/:splat"     # <fly-app> = `app` in ops/fly/api.toml (integrated-awn-weather-api)
  status = 200
  force = true
  # signed = "NETLIFY_PROXY_SECRET"               # commented out for the first deploy (8.1)
```
`<project-ref>` in the CSP `connect-src` must be a literal (netlify.toml cannot interpolate env vars) and equal to the origin of `PUBLIC_SUPABASE_URL` (01's parity check enforces this in the production build). Netlify does not apply `[[headers]]` to proxied content, so API responses carry their own `no-store`/`nosniff` (conventions). Netlify's proxy timeout is about 26 s (verify); no API endpoint may run longer. Signed proxy: [Netlify rewrites and proxies](https://docs.netlify.com/manage/routing/redirects/rewrites-proxies/) (`x-nf-sign` JWS, HS256, claims `deploy_context`, `exp`, `iss=netlify`, `netlify_id`, `site_url`). Headers: [Netlify headers](https://docs.netlify.com/manage/routing/headers/). Previews: [deploy previews](https://docs.netlify.com/deploy/deploy-types/deploy-previews/).

## 9. Backups (C5, S13)

### 9.1 What is dumped

1. `app` schema (schema + data + grants, `--no-owner`), custom format. Includes `app._sqlx_migrations` (`app_backup` has an explicit `SELECT` on it, conventions).
2. **Auth users (decided Oct 7): `app_auth.backup_users()`.** `app_backup` has no grants on `auth.*` (proposal 03); it executes the small hardened `SECURITY DEFINER` function `app_auth.backup_users()` (04 6.5: owned by `postgres`, `search_path = ''`, `REVOKE ... FROM PUBLIC`, EXECUTE for `app_backup` only) that returns only `id`, `email`, `created_at`. `backup.sh` writes that as `auth-users.csv`, age-encrypts it and uploads it next to the app dump as `auth-users-<ts>-<rand>.csv.age`. `BACKUP_INCLUDE_AUTH` stays as a switch and **defaults to on (1)**; if the capture fails while it is on, the whole backup fails loudly (a backup without logins is not a good backup). It logs only counts. On the first deploy compare the printed count with the dashboard user count. Restore path: `admin restore-auth-users` (04 10.2, runbook 15.6 step 5).

Dumps contain encrypted AWN keys, and the login list contains emails, so everything is age-encrypted before leaving the machine.

### 9.2 `ops/backup/Dockerfile` (build context: repo root)

```dockerfile
FROM postgres:17-alpine
# pg_dump major must be >= server major; bump with Supabase's Postgres version (dependabot docker ecosystem)
RUN apk add --no-cache bash age aws-cli curl ca-certificates \
 && adduser -D -u 10001 backup
COPY ops/backup/backup.sh /usr/local/bin/backup.sh
COPY ops/db/checks/size.sql /opt/checks/size.sql
# same guard as the backend image: `.keep` keeps the build working before the CA is committed
COPY backend/certs/ /etc/ssl/app-certs/
RUN chmod 0555 /usr/local/bin/backup.sh
USER backup
ENTRYPOINT ["/usr/local/bin/backup.sh"]
```
Pin the base image by digest once chosen (Dependabot keeps it current).

### 9.3 `ops/backup/backup.sh`

```bash
#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

: "${BACKUP_DATABASE_URL:?}" "${BACKUP_S3_BUCKET:?}" "${BACKUP_AGE_RECIPIENT:?}"
PREFIX="${BACKUP_S3_PREFIX:-weather}"
INCLUDE_AUTH="${BACKUP_INCLUDE_AUTH:-1}"      # default ON (owner decision, Oct 7); 0 only for a database without app_auth.backup_users()
CONDITIONAL="${BACKUP_S3_CONDITIONAL:-1}"      # 0 only for the local MinIO test
SIZE_SQL="${BACKUP_SIZE_SQL:-/opt/checks/size.sql}"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT

ping() { # ping <url-var-name> <suffix> [body]
  local url="${!1:-}"; [ -n "$url" ] || return 0
  curl -fsS -m 10 --retry 3 -o /dev/null --data-raw "${3:-}" "${url}${2:-}" || true
}
fail() { echo "backup failed: $1" >&2; ping HEALTHCHECK_URL_BACKUP /fail "$1"; exit 1; }
trap 'fail "unexpected error at line $LINENO"' ERR
ping HEALTHCHECK_URL_BACKUP /start

rflags=(); IFS=',' read -ra recips <<<"$BACKUP_AGE_RECIPIENT"
for r in "${recips[@]}"; do rflags+=(-r "$r"); done

server_major=$(( $(psql "$BACKUP_DATABASE_URL" -Atc 'show server_version_num') / 10000 ))
client_major=$(pg_dump --version | sed -E 's/.* ([0-9]+)[.].*/\1/')
[ "$client_major" -ge "$server_major" ] || fail "pg_dump $client_major older than server $server_major"

TS="$(date -u +%Y%m%dT%H%M%SZ)"; YM="$(date -u +%Y/%m)"; RAND="$(head -c 4 /dev/urandom | od -An -tx1 | tr -d ' \n')"
files=(app.dump)       # each entry is "<name>.<ext>"; uploaded as "<name>-<ts>-<rand>.<ext>.age"
pg_dump --dbname="$BACKUP_DATABASE_URL" --format=custom --compress=6 --no-owner \
  --schema=app --file="$WORK/app.dump"
if [ "$INCLUDE_AUTH" = 1 ]; then
  # Logins: exactly id, email, created_at from the hardened definer function app_auth.backup_users() (04 6.5), executable by app_backup only.
  # No password hashes, tokens or metadata. The CSV never leaves $WORK unencrypted (umask 077, removed after age).
  psql -X -v ON_ERROR_STOP=1 "$BACKUP_DATABASE_URL" --csv \
    -c "select id, email, created_at from app_auth.backup_users()" > "$WORK/auth-users.csv" \
    || fail "auth user capture failed (app_auth.backup_users)"
  auth_rows=$(( $(wc -l < "$WORK/auth-users.csv") - 1 ))
  app_rows=$(psql -X -Atc "select count(*) from app.users" "$BACKUP_DATABASE_URL")
  # first-deploy sanity (compare with the dashboard user count too): some logins must exist when app users exist
  [ "$auth_rows" -ge 1 ] || [ "$app_rows" -eq 0 ] || fail "auth capture returned no users but app.users has $app_rows"
  echo "auth users captured: $auth_rows (app.users rows: $app_rows)"   # counts only, never emails
  files+=(auth-users.csv)
fi

for f in "${files[@]}"; do
  name="${f%.*}"; ext="${f##*.}"
  if [ "$ext" = dump ]; then pg_restore --list "$WORK/$f" >/dev/null || fail "$f failed integrity listing"; fi
  age "${rflags[@]}" -o "$WORK/$f.age" "$WORK/$f"
  rm -f "$WORK/$f"
  [ "$(stat -c %s "$WORK/$f.age")" -gt 100 ] || fail "$f.age suspiciously small"
  # random suffix + conditional write: an existing object is never overwritten
  cond=(); [ "$CONDITIONAL" = 1 ] && cond=(--if-none-match '*')
  aws s3api put-object --bucket "$BACKUP_S3_BUCKET" --key "${PREFIX}/${YM}/${name}-${TS}-${RAND}.${ext}.age" \
    --body "$WORK/$f.age" --server-side-encryption AES256 --checksum-algorithm SHA256 \
    "${cond[@]}" >/dev/null || fail "upload of $f failed"
done

# size alert via 04's size.sql (raises above 400 MB); output is table sizes only
if out="$(psql -X -v ON_ERROR_STOP=1 "$BACKUP_DATABASE_URL" -f "$SIZE_SQL" 2>&1)"; then
  ping HEALTHCHECK_URL_DBSIZE "" "$(printf '%s' "$out" | head -c 1000)"
else
  ping HEALTHCHECK_URL_DBSIZE /fail "$(printf '%s' "$out" | head -c 1000)"
fi
ping HEALTHCHECK_URL_BACKUP "" "ok ${TS}"
echo "backup ok ${TS}"
```

Rules: never `set -x`; never echo URLs; no age private key in the image; `AWS_ENDPOINT_URL` may be set for the local test only. The upload-only IAM user cannot read back; integrity is checked before upload (`pg_restore --list`, S3 SHA-256 checksum on put) and by the restore drill. `DB_SIZE_ALERT_MB` and `BACKUP_OUTPUT_DIR` from conventions are not used by this design (threshold is in `size.sql`; no local compose backup).

### 9.4 Size alert

The `DBSIZE` healthchecks check expects a daily success ping; `size.sql` raising (database above 400 MB) sends `/fail`. This replaces a Supabase alert (the free plan has none).

### 9.5 AWS setup (Rodney, one-time; account MFA on, no root keys)

**Decided by Rodney, Oct 7, 2026: versioning plus 90-day expiry, no S3 Object Lock.** Versioning plus no-overwrite writes plus an uploader that cannot delete cover the realistic threats (a leaked uploader key, a bad script run, accidental overwrite). Object Lock would add only protection from an AWS-account compromise (and Governance mode is bypassable by an admin), at the cost of lock-versus-lifecycle coupling, mandatory checksums and harder cleanup. Revisit only if account-compromise resilience becomes a goal.

1. Create bucket `famweather-backups-<random>` in `us-west-2` (or nearest): Block Public Access all on, ACLs disabled (bucket owner enforced), default encryption SSE-S3 (AES256), **versioning on**.
2. Lifecycle on prefix `weather/`: expire current versions after **90 days** (decided); expire noncurrent versions after 7 days (a deleted/expired object stays recoverable for a week); remove expired delete markers; abort incomplete multipart uploads after 7 days ([lifecycle](https://docs.aws.amazon.com/AmazonS3/latest/userguide/object-lifecycle-mgmt.html)).
3. Bucket policy: deny non-TLS and deny any `PutObject` that does not use a conditional write (so a stolen uploader key cannot overwrite an old backup). Condition key per [AWS conditional writes](https://docs.aws.amazon.com/AmazonS3/latest/userguide/conditional-writes.html) (verify the key name when applying):
```json
{
  "Version": "2012-10-17",
  "Statement": [
    { "Sid": "DenyInsecureTransport", "Effect": "Deny", "Principal": "*", "Action": "s3:*",
      "Resource": ["arn:aws:s3:::famweather-backups-REPLACE", "arn:aws:s3:::famweather-backups-REPLACE/*"],
      "Condition": { "Bool": { "aws:SecureTransport": "false" } } },
    { "Sid": "RequireIfNoneMatch", "Effect": "Deny", "Principal": "*", "Action": "s3:PutObject",
      "Resource": "arn:aws:s3:::famweather-backups-REPLACE/weather/*",
      "Condition": { "Null": { "s3:if-none-match": "true" } } }
  ]
}
```
4. IAM user `integrated-awn-weather-backup-uploader`: no console access, one access key, inline policy. **No SSE condition** (the bucket default encryption applies, and a condition would break any request that does not send the header; the script sends `--server-side-encryption AES256` anyway). No multipart actions are needed: the script uses a single `put-object` (dumps are far below 5 GB).
```json
{
  "Version": "2012-10-17",
  "Statement": [
    { "Sid": "UploadOnly", "Effect": "Allow", "Action": "s3:PutObject",
      "Resource": "arn:aws:s3:::famweather-backups-REPLACE/weather/*",
      "Condition": { "Bool": { "aws:SecureTransport": "true" } } }
  ]
}
```
   No Get, List or Delete: a stolen key can add objects only (new keys; existing ones cannot be overwritten, versioning keeps history). [S3 actions](https://docs.aws.amazon.com/service-authorization/latest/reference/list_amazons3.html).
5. Separate **restore identity** for Rodney (SSO or IAM user with MFA) with `s3:GetObject`, `s3:GetObjectVersion`, `s3:ListBucket`, `s3:ListBucketVersions` on the bucket. Never placed on Fly or GitHub.

### 9.6 Age key

`age-keygen -o weather-backup.key` on an offline machine. The public recipient (`age1...`) goes to Fly secret `BACKUP_AGE_RECIPIENT` (comma-separated if a second recipient is ever added). **Decided (Oct 7): one age key.** Its private key lives in Rodney's password manager (separate entry) plus a printed copy in a safe; a second escrow recipient is optional and not required. **The private key never touches Fly, GitHub, Netlify or AWS.** [age](https://github.com/FiloSottile/age).

### 9.7 Backup test (`ops/backup/test.sh`; CI image job; no Compose backup service)

Implement exactly:
1. Throwaway network with `postgres:17` started as `--name db --network-alias db` and `minio/minio` (no Compose file needed; `docker run`). Every database URL in the test uses host `db` literally (the local-only refusal rules accept only `db`/`localhost`/`127.0.0.1`/`::1`).
2. Database setup in the same order as production: `ops/db/bootstrap/roles.sql` (test passwords via env) then `ops/db/tests/auth_stub.sql` then `ops/db/bootstrap/auth_access.sql`, then `admin migrate` (backend image, `APP_ENV=local`, `DATABASE_URL` as `app_migrator@db`), then insert a few rows, including two `auth.users` rows from the auth stub (one with a password hash and metadata, one soft-deleted).
3. `age-keygen`; run the backup image with `BACKUP_DATABASE_URL` as `app_backup@db` (`sslmode=disable`), `AWS_ENDPOINT_URL=http://minio:9000`, dummy `AWS_*`, `BACKUP_S3_CONDITIONAL=0` (MinIO may not support conditional writes), `HEALTHCHECK_URL_*` pointing at a tiny logging listener.
4. Assert: an `app-*.dump.age` object exists; decrypts with the private key; `pg_restore --list` shows `app._sqlx_migrations`; an `auth-users-*.csv.age` object exists (default `BACKUP_INCLUDE_AUTH=1`), decrypts to a header `id,email,created_at` plus exactly one row (the soft-deleted user is absent) and no password hash or metadata; with `BACKUP_INCLUDE_AUTH=0` no such object is written; with `app_auth.backup_users()` made to fail the run exits non-zero and sends `/fail`; the listener saw `/start` then a success ping and a DBSIZE success; with a wrong password the container exits non-zero and the listener saw `/fail`; container logs contain no DB password or AWS secret.
5. Run `shellcheck ops/backup/backup.sh ops/fly/run-oneoff.sh ops/ci/*.sh`.
Conditional-write behaviour and the bucket policy are checked against real S3 once at setup (section 13, check 15).

## 10. Monitoring and alerting (P4)

healthchecks.io (free plan; verify check limit): one project `family-weather`, notifications to Rodney's email plus a second channel. **At first go-live only `AWN_INGEST` and `BACKUP`/`DBSIZE` exist**; add the others (and their `HEALTHCHECK_URL_*` secrets) when those jobs ship (conventions build order). Checks (cadence per 03 section 3.5):

| Check (env suffix) | Period | Grace | Notes |
| --- | --- | --- | --- |
| `AWN_INGEST` | 5 min | 15 min | `/start`, success, `/fail` |
| `DAILY_ROLLUP` | 1 h | 2 h | create when the job ships |
| `REFERENCE_SYNC` | 24 h | 6 h | create when the job ships |
| `BACKUP` | 1 day | 6 h | scheduled machine drifts; wide grace |
| `DBSIZE` | 1 day | 6 h | `/fail` from `size.sql` above 400 MB |

`awn_backfill` is manual and has no check. **API uptime:** none, because an external probe every 5 minutes defeats `auto_stop_machines` (auto-stop is decided, Oct 7). The deploy smoke test and user reports cover the API; if an uptime probe is ever wanted, it also means turning auto-stop off (`min_machines_running = 1`, about $2 more a month). Fly logs: `fly logs` keeps a short rolling buffer; no log shipping by default (design: short fixed retention; Open question 11). Fly health checks mark the API unhealthy and restart it; Supabase pause/limit emails go to the project owner (monitor that mailbox). Quarterly alert drill: stop the poller (`fly machine stop`), confirm the email, restart.

## 11. CI/CD (S14)

### 11.1 Repository settings (Rodney)

- **Repository: public, GitHub Free (decided Oct 7).** A private repo on GitHub Free has no deployment environments, so the `production` environment's required-reviewer approval and environment-scoped secrets would not work. A public repo keeps every protection: environments, required reviewers, rulesets and branch protection are all enforced, and Actions minutes are unlimited on standard runners (path filters and caches stay for speed only, 11.2). The weekly exposure check (11.5) stays weekly. Nothing secret lives in the repo by design: every secret is in Fly, Supabase, the GitHub `production`/`ci-readonly` environments or Rodney's password manager.
- **Public repo hygiene (required because the repo is public).** (1) No real station MAC addresses, exact home coordinates, personal email addresses, API/application keys, tokens or passwords in code, fixtures, seed data, docs, test output or commit history; use the documented fake values (the fixture MACs `AA:BB:CC:00:00:01` and similar, coordinates rounded to two decimals at a city centre, `example.test` emails; the recording tool in 03 re-maps a real MAC with `--as`, and the Open-Meteo recorder in 02 7.9 records at the seed city-centre coordinates, never at a family member's station; 03's fixture lint scans `awn/`, `open-meteo/` and `acis/` coordinates). Anything committed stays in history even if later removed: if a real value is ever committed, rotate it (R-section) and treat the history as burned. (2) Settings > Code security: enable secret scanning and push protection (both free for public repos), Dependabot alerts and Dependabot security updates, and private vulnerability reporting. (3) Settings > Actions > General: "Require approval for all outside collaborators" for workflows from fork pull requests, and keep the default token read-only. (4) PR-triggered workflows use the `pull_request` event only, never `pull_request_target`, and receive no secrets: secrets exist only in the `production` and `ci-readonly` environments, which are restricted to `main` (and `ci-readonly` is read-only by construction). (5) Do not paste production logs, `fly` output or screenshots containing hostnames, project refs or keys into issues, PRs or docs. The Supabase project ref and the anon key are public by design (they ship in the web bundle) and are not secrets.
- Branch protection on `main` (enforced; the repo is public): PR required; the **single required status check is `ci-ok`** (the aggregate job of `.github/workflows/ci.yml`, 11.2a). Never list the individual jobs (`rust-lint`, `tests`, `supply-chain`, `image`, `web-tests`, `web-audit`) as required: when a path filter skips them they never report and the pull request would be blocked forever. `ci-ok` always runs, fails if any job it needs failed or was cancelled, and passes when jobs were skipped by the filters.
- Actions: default token read-only; only pinned actions. Environment `production`: deployment branch `main`, required reviewer Rodney; ruleset on the Git branch `production` (block deletion and force pushes only; no "restrict updates" rule, because `GITHUB_TOKEN` cannot be a bypass actor; see 11.4 notes; enforced, the repo is public); secrets stored **only here**: `FLY_API_TOKEN_API`, `FLY_API_TOKEN_POLLER`, `FLY_API_TOKEN_BACKUP` (all three must exist before the first deploy, because it validates all three config files; `fly tokens create deploy -a <app>`, [deploy tokens](https://fly.io/docs/security/tokens/)), `MIGRATOR_DATABASE_URL` (session pooler, `app_migrator`, ends `?sslmode=verify-full`), `EXPOSURE_DATABASE_URL` (session pooler, **`app_ci_check`**, no table grants). No Netlify credential is stored in GitHub (proposals/04). Repo variables `SITE_URL` (set after Netlify exists), `DEPLOY_API`, `DEPLOY_WEB`. Environment `ci-readonly` (no required reviewers, no branch restriction beyond `main`) holds one secret only: its own copy of `EXPOSURE_DATABASE_URL` (`app_ci_check`, no table grants); it exists solely for 11.5.
- All third-party actions pinned to full SHAs with version comments ([GitHub guidance](https://docs.github.com/en/actions/security-for-github-actions/security-guides/security-hardening-for-github-actions#using-third-party-actions)); SHAs below were resolved from tags on 2026-10-02 via `git ls-remote`; Dependabot updates them; add `zizmor` or `pinact` to fail on unpinned `uses:`.

### 11.2 `.github/workflows/backend.yml` (reusable; called by `ci.yml` on pull requests and by `deploy.yml` on `main`)

CI runs the make targets (`check-local-only test-db test-backend` here, `test-web` in `web.yml`; together they are `make test`, each in its own throwaway Compose project; conventions "Testing harness") and nothing that duplicates them on the host (no separate exposure, sqlx or bundle steps: 04/02/01 put those inside the make targets; see "Changes other guides must make"). No `push:` trigger (deploy.yml covers `main`) and no `concurrency:` here (called workflows must not cancel the deploy).

**Path filters (for speed; minutes are unlimited on a public repo).** `backend.yml` and `web.yml` have no `pull_request` trigger of their own and no `paths:`; they are reusable (`workflow_call`) workflows. On pull requests `ci.yml` (11.2a) decides with a `changes` job which of them to call (backend or ops files for `backend.yml`; `web/` or the Caddy headers file for `web.yml`), so a docs-only or web-only PR runs no backend jobs. On `main`, `deploy.yml` has its own `changes` job (11.4). Because a workflow that never triggers reports nothing, the required check lives in `ci.yml`, which triggers on every pull request.

**Caches.** `Swatinem/rust-cache` caches the host-side lint job. The Docker-based `tests` job restores the Compose test project's cargo registry and target directory from `actions/cache` (keys: `hashFiles('backend/Cargo.lock')`, restore-keys on the prefix), by setting `CARGO_REGISTRY_SRC` and `CARGO_TARGET_SRC` to relative paths under `.ci-cache/` (05's test overlay turns those named volumes into bind mounts when the variables are set). The web job caches npm through `actions/setup-node` (`cache: npm`). Cache hits cut a typical backend run from roughly 20 to 25 minutes (cold) to roughly 8 to 12.

**Minutes are not a constraint.** The repository is public, so standard Linux runner minutes are free and unlimited; there is no monthly budget to track. For reference a backend-touching run takes about 20 to 25 minutes cold and 8 to 12 warm, a web-touching run about 8, and a deploy run on `main` about 30. Path filters and caches stay purely to keep feedback fast, and docs-only pushes and PRs run nothing. Every job still sets `timeout-minutes` so a hung build cannot sit for the 6-hour default.

```yaml
name: backend
on:
  workflow_call:          # called by ci.yml (pull requests, path-decided there) and deploy.yml (main)
permissions:
  contents: read
jobs:
  rust-lint:
    runs-on: ubuntu-24.04
    timeout-minutes: 15
    steps:
      - uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683 # v4.2.2
        with: { persist-credentials: false }
      - run: rustup toolchain install stable --profile minimal -c rustfmt,clippy
      - uses: Swatinem/rust-cache@9d47c6ad4b02e050fd481d890b2ea34778fd09d6 # v2.7.8
        with: { workspaces: backend }
      - run: cargo fmt --manifest-path backend/Cargo.toml --all -- --check
      - run: cargo clippy --manifest-path backend/Cargo.toml --workspace --all-targets --locked -- -D warnings
      - name: feature isolation (S5; the canonical check from conventions, resolver 2/3)
        working-directory: backend
        run: |
          if [ -d crates/api ]; then
            if cargo tree -p api -e normal,build,features | grep -Eq 'decrypt|test-support|testdb'; then echo "api enables a test or decrypt feature"; exit 1; fi
          fi
          if [ -d crates/poller ]; then
            if cargo tree -p poller -e normal,build,features | grep -Eq 'test-support|testdb'; then echo "poller enables a test feature"; exit 1; fi
          fi
  tests:
    runs-on: ubuntu-24.04
    timeout-minutes: 45
    steps:
      - uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683 # v4.2.2
        with: { persist-credentials: false }
      - name: cache the cargo registry, target and tools used by the Compose test project (05 binds them when these variables are set)
        uses: actions/cache@0057852bfaa89a56745cba8c7296529d2fc39830 # v4.3.0
        with:
          path: .ci-cache
          key: cargo-compose-${{ hashFiles('backend/Cargo.lock') }}
          restore-keys: cargo-compose-
      - run: mkdir -p .ci-cache/registry .ci-cache/target
      - run: make init
      - name: backend tests (the web tests live in web.yml and run only when web/ changes)
        env:
          CARGO_REGISTRY_SRC: ./.ci-cache/registry
          CARGO_TARGET_SRC: ./.ci-cache/target
        run: make check-local-only test-db test-backend
      - name: production files contain no demo values
        run: |
          files="ops/fly/poller.toml ops/backup/fly.toml"
          for f in ops/fly/api.toml web/netlify.toml; do [ -f "$f" ] && files="$files $f"; done
          ops/ci/check-demo-values.sh $files
  supply-chain:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps:
      - uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683 # v4.2.2
        with: { persist-credentials: false }
      # advisories use the RustSec DB (same data as cargo-audit)
      - uses: EmbarkStudios/cargo-deny-action@34899fc7ba81ca6268d5947a7a16b4649013fea1 # v2.0.11
        with: { manifest-path: backend/Cargo.toml, command: check advisories bans licenses sources }
  image:
    runs-on: ubuntu-24.04
    timeout-minutes: 20
    steps:
      - uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683 # v4.2.2
        with: { persist-credentials: false }
      - run: docker build -t famweather-backend:ci backend
      - name: image sanity (distroless: no shell)
        run: |
          # 02's contract: started with an empty environment, a binary exits non-zero (and prints no values)
          for b in api poller admin; do
            [ -d "backend/crates/$b" ] || { echo "skip $b (crate not present yet)"; continue; }
            if docker run --rm famweather-backend:ci /usr/local/bin/$b >/dev/null 2>&1; then echo "$b started with an empty environment"; exit 1; fi
          done
          docker create --name chk famweather-backend:ci /usr/local/bin/api   # the image has no ENTRYPOINT/CMD; create needs a command
          if docker export chk | tar -t | grep -Eqi '(^|/)\.env($|\.)|supabase_service|private[_-]?key'; then
            echo "secret-like file found in the image"; exit 1
          fi
          docker cp chk:/etc/ssl/app-certs/. "$RUNNER_TEMP/certs-in-image"
          if [ -f backend/certs/supabase-ca.crt ]; then
            cmp backend/certs/supabase-ca.crt "$RUNNER_TEMP/certs-in-image/supabase-ca.crt"
          else
            echo "::warning::backend/certs/supabase-ca.crt not committed yet (runbook step 1b); CA content check skipped"
          fi
          docker rm chk
          docker image inspect famweather-backend:ci --format '{{.Config.User}}' | grep -Eq 'nonroot|65532'
      - name: backup image builds (backend/certs/.keep keeps this working before the CA exists)
        run: docker build -f ops/backup/Dockerfile -t famweather-backup:ci .
      - name: shellcheck and actionlint
        run: |
          sudo apt-get install -y shellcheck
          shellcheck ops/backup/backup.sh ops/backup/test.sh ops/fly/run-oneoff.sh ops/ci/*.sh
          bash <(curl -sSfL https://raw.githubusercontent.com/rhysd/actionlint/main/scripts/download-actionlint.bash) && ./actionlint
      - run: ops/backup/test.sh
```
(`flyctl config validate` needs a token, so it runs in the deploy job, 11.4. Pin the actionlint download script by version when implementing, or use a pinned action.) `backend/deny.toml` (06-owned): `[advisories] yanked = "deny"`; `[licenses] allow = [MIT, Apache-2.0, BSD-3-Clause, ISC, Unicode-3.0, ...]` (tune on first run); `[bans] multiple-versions = "warn"`, deny `openssl`/`native-tls` under the rustls policy; `[sources] unknown-registry = "deny"`, `unknown-git = "deny"`.

### 11.2a `.github/workflows/ci.yml` (pull requests; owns the one required check)

```yaml
name: ci
on:
  pull_request:                     # deliberately no `paths:` so this workflow, and therefore `ci-ok`, always reports
permissions:
  contents: read
concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: true
jobs:
  changes:
    runs-on: ubuntu-24.04
    timeout-minutes: 5
    outputs:
      backend: ${{ steps.f.outputs.backend }}
      web: ${{ steps.f.outputs.web }}
    steps:
      - uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683 # v4.2.2
        with: { persist-credentials: false, fetch-depth: 0 }
      - id: f
        env: { BASE_SHA: "${{ github.event.pull_request.base.sha }}" }
        run: |
          changed="$(git diff --name-only "$BASE_SHA" "$GITHUB_SHA")"
          hit() { printf '%s\n' "$changed" | grep -Eq "$1" && echo true || echo false; }
          echo "backend=$(hit '^(backend/|ops/|Makefile$|docker-compose|\.env\.example$|\.github/)')" >> "$GITHUB_OUTPUT"
          echo "web=$(hit '^(web/|ops/caddy/|\.github/)')" >> "$GITHUB_OUTPUT"
  backend:
    needs: changes
    if: ${{ needs.changes.outputs.backend == 'true' }}
    uses: ./.github/workflows/backend.yml
  web:
    needs: changes
    if: ${{ needs.changes.outputs.web == 'true' }}
    uses: ./.github/workflows/web.yml
  ci-ok:                            # THE required check on main; runs always, even when the jobs above were skipped
    if: ${{ always() }}
    needs: [changes, backend, web]
    runs-on: ubuntu-24.04
    timeout-minutes: 5
    steps:
      - name: fail if any needed job failed or was cancelled (skipped is fine)
        env: { RESULTS: "${{ join(needs.*.result, ' ') }}" }
        run: |
          echo "results: $RESULTS"
          case " $RESULTS " in *" failure "*|*" cancelled "*) exit 1 ;; esac
```
`changes` must succeed (a skipped `changes` is impossible; a failed one fails `ci-ok`). Jobs skipped by the filters report `skipped`, which passes. Add `ci.yml` to the `actionlint` and path lists that name workflow files.

### 11.3 `.github/workflows/web.yml`

Lint, type-check, unit tests and the production-build guard scripts run inside `make test-web`, which now has its own job here (`web-tests`) so it runs only when `web/` changes. `web-audit` adds the host-side dependency audit and the header drift check (Node built-ins only), so it must `npm ci` first where it needs `node_modules`. Both skip until `web/package.json` exists. npm downloads are cached by `actions/setup-node` in `web-audit`; `web-tests` restores the same `.ci-cache` as the backend job when `test-web` needs the backend image layers (05 decides what `test-web` builds).

```yaml
name: web
on:
  workflow_call:          # called by ci.yml (pull requests, path-decided there) and deploy.yml (main)
permissions:
  contents: read
jobs:
  web-tests:
    if: ${{ hashFiles('web/package.json') != '' }}
    runs-on: ubuntu-24.04
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683 # v4.2.2
        with: { persist-credentials: false }
      - run: make init
      - run: make test-web
  web-audit:
    if: ${{ hashFiles('web/package.json') != '' }}      # skipped (counts as passed) until 01 ships
    runs-on: ubuntu-24.04
    timeout-minutes: 15
    defaults: { run: { working-directory: web } }
    steps:
      - uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683 # v4.2.2
        with: { persist-credentials: false }
      - uses: actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020 # v4.4.0
        with: { node-version-file: web/.nvmrc, cache: npm, cache-dependency-path: web/package-lock.json }
      - run: npm ci --ignore-scripts
      - run: npm audit --omit=dev --audit-level=high
      - run: npm audit signatures
      - run: node scripts/headers.mjs check      # security-headers.json vs netlify.toml and the Caddy include
```
`web-tests` and `web-audit` are the web checks (see the 11.1 note on path-filtered required checks). 06 has no separate header or CSP check and no baseline CSP; `headers.mjs check` is 01's.

### 11.4 `.github/workflows/deploy.yml`

Tokens and URLs are passed only through step `env:` (never expanded into `run:` text). The admin binary is built in a **separate job without `environment:`** (so no production secrets exist there), uploaded as an artifact, and the deploy job verifies its SHA-256 (taken from the build job's output, not from the artifact) before the migrate step runs that binary.

```yaml
name: deploy
on:
  push:
    branches: [main]
    paths:                           # docs-only pushes deploy nothing and cost no minutes
      - 'backend/**'
      - 'ops/**'
      - 'web/**'
      - 'Makefile'
      - 'docker-compose*.yml'
      - '.env.example'
      - '.github/**'
permissions:
  contents: read
concurrency:
  group: deploy-production
  cancel-in-progress: false
jobs:
  changes:                          # which parts of the repo this push touched (workflow_call ignores path filters)
    runs-on: ubuntu-24.04
    timeout-minutes: 5
    outputs:
      backend: ${{ steps.f.outputs.backend }}
      web: ${{ steps.f.outputs.web }}
    steps:
      - uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683 # v4.2.2
        with: { persist-credentials: false, fetch-depth: 0 }
      - id: f
        env: { BEFORE: "${{ github.event.before }}" }
        run: |
          # a new branch or an unknown base counts as "everything changed"
          if ! git cat-file -e "${BEFORE}^{commit}" 2>/dev/null; then
            echo "backend=true" >> "$GITHUB_OUTPUT"; echo "web=true" >> "$GITHUB_OUTPUT"; exit 0
          fi
          changed="$(git diff --name-only "$BEFORE" "$GITHUB_SHA")"
          hit() { printf '%s\n' "$changed" | grep -Eq "$1" && echo true || echo false; }
          echo "backend=$(hit '^(backend/|ops/|Makefile$|docker-compose|\.env\.example$|\.github/)')" >> "$GITHUB_OUTPUT"
          echo "web=$(hit '^(web/|ops/caddy/|\.github/)')" >> "$GITHUB_OUTPUT"
  backend:
    needs: changes
    if: ${{ needs.changes.outputs.backend == 'true' }}
    uses: ./.github/workflows/backend.yml
  web:
    needs: changes
    if: ${{ needs.changes.outputs.web == 'true' }}
    uses: ./.github/workflows/web.yml
  build-admin:                      # no `environment:`, so no production secrets exist in this job
    needs: [changes, backend, web]
    if: ${{ !cancelled() && !contains(needs.*.result, 'failure') && needs.changes.outputs.backend == 'true' }}
    runs-on: ubuntu-24.04
    timeout-minutes: 30
    outputs:
      sha256: ${{ steps.sum.outputs.sha256 }}
    steps:
      - uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683 # v4.2.2
        with: { persist-credentials: false }
      - run: rustup toolchain install stable --profile minimal
      - uses: Swatinem/rust-cache@9d47c6ad4b02e050fd481d890b2ea34778fd09d6 # v2.7.8
        with: { workspaces: backend }
      - run: cargo build --release --manifest-path backend/Cargo.toml --locked -p admin
      - id: sum
        run: echo "sha256=$(sha256sum backend/target/release/admin | awk '{print $1}')" >> "$GITHUB_OUTPUT"
      - uses: actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02 # v4.6.2
        with: { name: admin, path: backend/target/release/admin, retention-days: 1, if-no-files-found: error }
  deploy:                           # always runs after the parts that ran, so the approval gate also guards web-only pushes
    needs: [changes, backend, web, build-admin]
    if: ${{ !cancelled() && !contains(needs.*.result, 'failure') }}
    runs-on: ubuntu-24.04
    environment: production
    timeout-minutes: 45
    steps:
      - uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683 # v4.2.2
        with: { persist-credentials: false, fetch-depth: 2 }
      - uses: superfly/flyctl-actions/setup-flyctl@fc53c09e1bc3be6f54706524e3b82c4f462f77be # 1.5
      - uses: actions/download-artifact@d3f86a106a0bac45b974a628896c90dbdf5c8093 # v4.3.0
        if: ${{ needs.changes.outputs.backend == 'true' }}
        with: { name: admin, path: admin-bin }
      - name: verify the admin binary is the one the secret-free job built
        if: ${{ needs.changes.outputs.backend == 'true' }}
        env: { ADMIN_SHA256: "${{ needs.build-admin.outputs.sha256 }}" }
        run: |
          echo "${ADMIN_SHA256}  admin-bin/admin" | sha256sum -c -
          chmod +x admin-bin/admin
      - name: derive app names from the configs
        run: |
          echo "API_APP=$(sed -n 's/^app = "\(.*\)"/\1/p' ops/fly/api.toml)" >> "$GITHUB_ENV"
          echo "POLLER_APP=$(sed -n 's/^app = "\(.*\)"/\1/p' ops/fly/poller.toml)" >> "$GITHUB_ENV"
          echo "BACKUP_APP=$(sed -n 's/^app = "\(.*\)"/\1/p' ops/backup/fly.toml)" >> "$GITHUB_ENV"
      - name: validate fly configs (needs the app tokens; api only once it ships)
        env:
          TOKEN_API: ${{ secrets.FLY_API_TOKEN_API }}
          TOKEN_POLLER: ${{ secrets.FLY_API_TOKEN_POLLER }}
          TOKEN_BACKUP: ${{ secrets.FLY_API_TOKEN_BACKUP }}
          DEPLOY_API: ${{ vars.DEPLOY_API }}
        run: |
          [ "$DEPLOY_API" != true ] || FLY_API_TOKEN="$TOKEN_API" flyctl config validate --config ops/fly/api.toml --strict
          FLY_API_TOKEN="$TOKEN_POLLER" flyctl config validate --config ops/fly/poller.toml --strict
          FLY_API_TOKEN="$TOKEN_BACKUP" flyctl config validate --config ops/backup/fly.toml --strict
      - name: migrate production (runs the prebuilt admin binary as app_migrator over the session pooler)
        if: ${{ needs.changes.outputs.backend == 'true' }}
        env:
          APP_ENV: production
          MIGRATOR_DATABASE_URL: ${{ secrets.MIGRATOR_DATABASE_URL }}   # ends with ?sslmode=verify-full
        run: |
          test -f backend/certs/supabase-ca.crt || { echo "backend/certs/supabase-ca.crt missing (runbook step 1b)"; exit 1; }
          DATABASE_URL="${MIGRATOR_DATABASE_URL}&sslrootcert=${GITHUB_WORKSPACE}/backend/certs/supabase-ca.crt" admin-bin/admin migrate
      - name: deploy poller first (exactly one machine; backend changes only)
        if: ${{ needs.changes.outputs.backend == 'true' }}
        env: { FLY_API_TOKEN: "${{ secrets.FLY_API_TOKEN_POLLER }}" }
        run: flyctl deploy backend --config ../ops/fly/poller.toml --dockerfile Dockerfile --remote-only --ha=false --image-label "sha-${GITHUB_SHA::12}"
      - name: deploy api (skipped until 02 ships: repo variable DEPLOY_API=true)
        if: ${{ vars.DEPLOY_API == 'true' && needs.changes.outputs.backend == 'true' }}
        env: { FLY_API_TOKEN: "${{ secrets.FLY_API_TOKEN_API }}" }
        run: flyctl deploy backend --config ../ops/fly/api.toml --dockerfile Dockerfile --remote-only --ha=false --image-label "sha-${GITHUB_SHA::12}"
      - name: assert machine counts
        env:
          TOKEN_API: ${{ secrets.FLY_API_TOKEN_API }}
          TOKEN_POLLER: ${{ secrets.FLY_API_TOKEN_POLLER }}
          DEPLOY_API: ${{ vars.DEPLOY_API }}
        run: |
          [ "$(FLY_API_TOKEN="$TOKEN_POLLER" flyctl machines list -a "$POLLER_APP" --json | jq length)" = 1 ]
          [ "$DEPLOY_API" != true ] || [ "$(FLY_API_TOKEN="$TOKEN_API" flyctl machines list -a "$API_APP" --json | jq length)" = 1 ]
      - name: update backup image (only when ops/backup, size.sql or certs changed, and only if the machine exists)
        env: { FLY_API_TOKEN: "${{ secrets.FLY_API_TOKEN_BACKUP }}" }
        run: |
          if git diff --quiet HEAD~1 HEAD -- ops/backup ops/db/checks/size.sql backend/certs; then echo unchanged; exit 0; fi
          id=$(flyctl machines list -a "$BACKUP_APP" --json | jq -r '.[0].id // empty')
          if [ -z "$id" ]; then echo "no backup machine yet (first deploy): skipping"; exit 0; fi
          flyctl deploy . --config ops/backup/fly.toml --dockerfile ops/backup/Dockerfile --build-only --push --image-label "sha-${GITHUB_SHA::12}"
          flyctl machine update "$id" --image "registry.fly.io/${BACKUP_APP}:sha-${GITHUB_SHA::12}" -a "$BACKUP_APP" --yes
      - name: production exposure check (04, S6) as app_ci_check
        env: { EXPOSURE_DATABASE_URL: "${{ secrets.EXPOSURE_DATABASE_URL }}" }
        run: ops/db/checks/exposure.sh "${EXPOSURE_DATABASE_URL}&sslrootcert=${GITHUB_WORKSPACE}/backend/certs/supabase-ca.crt"
      - name: smoke (API and Fly checks; the Netlify checks run in publish-web once the site is published, so SITE_URL is not passed here)
        env:
          DEPLOY_API: ${{ vars.DEPLOY_API }}
        run: |
          for i in $(seq 1 30); do ops/ci/smoke.sh && exit 0; echo "retry $i"; sleep 20; done; exit 1
  publish-web:                      # no `environment:` (no production secrets); gated because it needs `deploy`, which waited for approval
    if: ${{ vars.DEPLOY_WEB == 'true' && needs.changes.outputs.web == 'true' && !cancelled() && needs.deploy.result == 'success' }}   # skipped until 01 ships, and when web/ did not change
    needs: [changes, deploy]
    runs-on: ubuntu-24.04
    permissions:
      contents: write               # the only job with write access; used only by the push step
    timeout-minutes: 20
    steps:
      - uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683 # v4.2.2
        with: { persist-credentials: false, fetch-depth: 0 }     # full history so the fast-forward check against `production` works
      - name: require SITE_URL (fail fast rather than skipping the wait)
        env: { SITE_URL: "${{ vars.SITE_URL }}" }
        run: '[ -n "$SITE_URL" ] || { echo "DEPLOY_WEB is true but repository variable SITE_URL is empty"; exit 1; }'
      - name: publish web (fast-forwards the `production` branch; Netlify builds and publishes it; proposals/04)
        env: { PUSH_TOKEN: "${{ github.token }}" }
        run: git push "https://x-access-token:${PUSH_TOKEN}@github.com/${GITHUB_REPOSITORY}.git" "${GITHUB_SHA}:refs/heads/production"   # no --force: a non-fast-forward is rejected
      - name: wait for Netlify to publish this commit (/version.txt equals GITHUB_SHA; about 10 minutes at most)
        env: { SITE_URL: "${{ vars.SITE_URL }}" }
        run: |
          for i in $(seq 1 60); do
            [ "$(curl -fsS "$SITE_URL/version.txt" 2>/dev/null | tr -d '[:space:]')" = "$GITHUB_SHA" ] && exit 0
            echo "waiting for Netlify ($i)"; sleep 10
          done
          echo "Netlify did not publish ${GITHUB_SHA} in time (old site stays live)"; exit 1
      - name: smoke (Netlify checks)
        env:
          SITE_URL: ${{ vars.SITE_URL }}
          DEPLOY_API: ${{ vars.DEPLOY_API }}
        run: |
          for i in $(seq 1 30); do ops/ci/smoke.sh && exit 0; echo "retry $i"; sleep 20; done; exit 1
```
Notes: the first production deploy is **poller-only** (conventions build order): `DEPLOY_API` and `DEPLOY_WEB` repo variables are unset until 02 and 01 ship, which skips the API deploy, the machine-count check for the API and the web publish and wait steps; set each to `true` when its component ships. Web deploys are gated by the `production` branch (proposals/04, accepted): Netlify builds only that branch, only the `publish-web` job moves it (fast-forward, never force), after the `deploy` job (migration, poller, API, exposure check, smoke) succeeds, and no Netlify credential exists in GitHub. `publish-web` has `contents: write` (the only job that does), no `environment:`, and `persist-credentials: false` (the token is passed to `git push` explicitly through step `env:`); it needs `deploy`, which waited for the `production` GitHub environment's approval. If `production` is not an ancestor of the release commit (for example re-running an old workflow run), the push is rejected without `--force`: this fails safe after the poller and API have already deployed, and the fix is to re-run from the latest `main`. **Branch protection is honest but limited:** `GITHUB_TOKEN` (github-actions[bot]) cannot be added as a ruleset bypass actor, so the GitHub ruleset on the `production` branch only blocks deletion and non-fast-forward (force) pushes; it does **not** use a "restrict updates" rule, because that would block the workflow's own push. Anyone with repo write access (only Rodney today) could still push `production` by hand; the real controls are the environment approval and GitHub MFA, plus the workflow being the only thing meant to move the branch. Only if stricter control is wanted later: a repo deploy key stored in the `production` GitHub environment, added as a bypass actor with "restrict updates" on (this adds a credential). The repo is public (decided Oct 7), so the ruleset is enforced on GitHub Free (a private Free repo would not enforce it and would have no deployment environments at all); approval and MFA remain the primary controls. Path filtering (decided Oct 7): the `changes` job runs the backend part only when backend or ops files changed and the web part only when web files changed; the `deploy` job always runs after them (so the approval still guards web-only pushes) but its migrate, poller and API steps run only for backend changes, and `publish-web` runs only for web changes. Netlify publishes asynchronously, so the wait step polls `/version.txt` and fails the run on timeout (a failed Netlify build otherwise leaves the old site up silently). **Pre-build verification (required, Rodney; runbook step 0b):** in a throwaway Netlify site and throwaway repo, confirm that (1) a fast-forward push to the production branch from Actions with `GITHUB_TOKEN` (full-history checkout, `contents: write`) succeeds and Netlify builds and publishes it, with branch deploys None and `main` ignored; (2) with the real ruleset applied (block deletion and force pushes, no "restrict updates"), the Actions push still succeeds and a manual force push fails. **Result (Oct 7, 2026): passed.** A bot-pushed fast-forward produced a `context: production` deploy within seconds, `main` pushes built nothing, and the ruleset rejected force push and delete while allowing the Actions push. Only if that verification fails: use proposal 04 option (e) or (b) (a one-site Netlify user's token, or CI builds and `netlify deploy --prod`), which would reintroduce a Netlify credential in the `production` GitHub environment. In the `deploy` job, `fetch-depth: 2` plus `HEAD~1` (backup-image change detection) assumes squash or merge commits to `main`. Rollback always passes `--config` (4.7).

### 11.5 `.github/workflows/exposure-weekly.yml` (public repo, weekly)

The repository is public (decided Oct 7) and minutes are free, so the check stays weekly. One run is about one minute. GitHub disables scheduled workflows in a public repo after 60 days without repository activity; if that notice arrives (an email from GitHub), re-enable the workflow in the Actions tab (or push any commit), and the per-deploy run of `exposure.sh` plus the manual runbook run cover the gap. The environment `ci-readonly` has no reviewers (a scheduled job cannot wait for approval) and holds only the `app_ci_check` URL, which has no table grants, so a compromise of this workflow reveals no data.

```yaml
name: exposure-weekly
on:
  schedule:
    - cron: "17 6 * * 1"
  workflow_dispatch:
permissions:
  contents: read
jobs:
  exposure:
    runs-on: ubuntu-24.04
    timeout-minutes: 5
    environment: ci-readonly
    steps:
      - uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683 # v4.2.2
        with: { persist-credentials: false }
      - name: production exposure check (04, S6) as app_ci_check
        env: { EXPOSURE_DATABASE_URL: "${{ secrets.EXPOSURE_DATABASE_URL }}" }
        run: |
          test -f backend/certs/supabase-ca.crt || { echo "CA file missing (runbook step 1b)"; exit 1; }
          ops/db/checks/exposure.sh "${EXPOSURE_DATABASE_URL}&sslrootcert=${GITHUB_WORKSPACE}/backend/certs/supabase-ca.crt"
```
(The workflow's only secret is the `ci-readonly` environment's `app_ci_check` URL, which has no table grants; it is never available to fork pull requests because the workflow runs only on `schedule` and `workflow_dispatch`.)

### 11.6 `.github/dependabot.yml`

Monthly and grouped (one PR per ecosystem per month) to limit CI minutes and review load; security advisories still open immediate PRs.

```yaml
version: 2
updates:
  - { package-ecosystem: cargo, directory: /backend, schedule: { interval: monthly }, groups: { rust: { patterns: ["*"] } } }
  - { package-ecosystem: npm, directory: /web, schedule: { interval: monthly }, groups: { npm: { patterns: ["*"] } } }
  - { package-ecosystem: github-actions, directory: /, schedule: { interval: monthly }, groups: { actions: { patterns: ["*"] } } }
  - { package-ecosystem: docker, directory: /backend, schedule: { interval: monthly } }
  - { package-ecosystem: docker, directory: /ops/backup, schedule: { interval: monthly } }
```
Renovate is an acceptable alternative (choose one). Review security updates individually; no auto-merge.

## 12. Secrets inventory and rotation

All dashboards MFA (S16). "Readable by" assumes Rodney is the only human admin.

| Secret | Lives in | Readable by | Rotation |
| --- | --- | --- | --- |
| `CREDENTIALS_PRIVATE_KEY` (HPKE) | Fly secret `integrated-awn-weather-poller`; offline copy in password manager | poller machine, Rodney | R1 |
| `CREDENTIALS_PUBLIC_KEY`, `CREDENTIALS_KEY_VERSION` | Fly secrets api, admin (version also poller) | those apps (public value) | with R1 |
| `AWN_APPLICATION_KEY` | Fly secrets api, poller; password manager | those machines | R2 |
| `app_api` / `app_poller` / `app_migrator` / `app_backup` / `app_ci_check` passwords | Fly secrets inside each `DATABASE_URL` (api / poller / admin / backup); `app_migrator` and `app_ci_check` URLs also in GitHub env `production`; password manager | the respective app or workflow | R3 |
| `postgres` password | password manager only | Rodney | change in the dashboard; unused by apps |
| Supabase JWT signing keys | Supabase | Supabase | R4 |
| `sb_publishable_` key (`PUBLIC_SUPABASE_ANON_KEY`) | Netlify env (public by design) | anyone | R4 |
| `sb_secret_` key (`SUPABASE_SERVICE_ROLE_KEY`) | Fly secret `integrated-awn-weather-admin` only | admin one-off machines | R4 |
| `NETLIFY_PROXY_SECRET` (same value both places) | Fly secret api + Netlify runtime env (optional) | api, Netlify | R5 |
| (no Netlify credential) | web is published by a fast-forward push to the `production` branch using the `publish-web` job's short-lived `GITHUB_TOKEN` (`contents: write`) | the `publish-web` job (after `deploy` was approved) | R9 |
| Fly deploy tokens (api/poller/backup) | GitHub env `production` | workflows on `main` after approval | R6 |
| `MIGRATOR_DATABASE_URL`, `EXPOSURE_DATABASE_URL` | GitHub env `production` | workflows on `main` after approval | R3 |
| AWS uploader key | Fly secret `integrated-awn-weather-backup` | backup machine | R7 |
| AWS restore identity | Rodney (MFA) | Rodney | n/a |
| age private key | offline (password manager + physical copy) | Rodney | R8 |
| `BACKUP_AGE_RECIPIENT` | Fly secret backup | public value | with R8 |
| healthchecks ping URLs | Fly secrets poller/backup | holder can fake pings | regenerate in healthchecks, `fly secrets set` |

Rotation:
- **R1 HPKE (S8):** 1) `admin keygen` offline; 2) on the poller set `CREDENTIALS_PRIVATE_KEY_PREVIOUS` = old, `CREDENTIALS_KEY_VERSION_PREVIOUS` = n, `CREDENTIALS_PRIVATE_KEY` = new, `CREDENTIALS_KEY_VERSION` = n+1; 3) set `CREDENTIALS_PUBLIC_KEY` = new and `CREDENTIALS_KEY_VERSION` = n+1 on api and admin; `admin keygen --check` the new pair; deploy; 4) `ops/fly/run-oneoff.sh batch integrated-awn-weather-poller integrated-awn-weather-poller poller -- /usr/local/bin/poller run-job credential_reencrypt`; verify no `key_version` < n+1 (Rodney, Supabase SQL editor); 5) **unset the `*_PREVIOUS` secrets on Fly only; the old private key stays offline in the password manager** until the oldest backup that used version n has aged out (retention), then destroy it.
- **R2 AWN application key:** create a new key in AWN; `fly secrets set` on api and poller (staged), deploy both; watch `awn_ingest`; delete the old key.
- **R3 DB role password:** from a trusted shell as `postgres` (direct or session pooler as in 7 step 9, password via the `psql` prompt or a temporary `PGPASSFILE`, never on the command line): `psql ...` then `\password app_x` (psql hashes client-side; nothing lands in dashboard SQL history; **do not use the SQL editor**). Update the Fly secret or GitHub environment secret immediately, deploy or restart that app, verify the healthcheck. A brief reconnect window is acceptable (poller catches up). Applies to all five roles including `app_ci_check`. Update the offline record.
- **R4 Supabase keys:** signing key: create the standby key, rotate (new tokens carry the new `kid`), keep the old key "previously used" for at least the access-token lifetime plus refresh window, then revoke; the API refetches JWKS on an unknown `kid` (02). Secret key (`sb_secret_`): regenerate, `fly secrets set -a integrated-awn-weather-admin`. Publishable key rotation needs a Netlify env change and rebuild.
- **R5 Proxy secret:** set the new value on Netlify (runtime env) and on Fly `integrated-awn-weather-api`; expect a few minutes of 403 unless 02 accepts two values; trigger a redeploy of the `production` branch in the Netlify UI (Deploys, Trigger deploy).
- **R9 Web publish right (no Netlify credential, proposals/04):** the only way to publish the site from CI is pushing the `production` branch. Leak impact: anyone with repo write (or a compromised `deploy` job) can publish a chosen commit; the controls are the `production` GitHub environment approval, GitHub MFA and a ruleset that only blocks deletion and force pushes (it cannot restrict who updates the branch without blocking the workflow; it is enforced because the repo is public). Nothing to rotate. Only if the pre-build Netlify test fails and option (e) or (b) is used does a Netlify token appear (scope it to a user who sees only this site, add it to the `production` environment, and rotate like a deploy token).
- **R6 Fly deploy token:** `fly tokens create deploy -a <app>`, update the GitHub environment secret, `fly tokens revoke <id>` the old one.
- **R7 AWS key:** create a second access key, `fly secrets set` on backup, run a manual backup, deactivate then delete the old key.
- **R8 age key:** new keypair; add the new recipient (keep the old one for one overlap cycle); update `BACKUP_AGE_RECIPIENT`; run a backup and a decrypt test; drop the old recipient. Old backups need the old private key until they expire.

## 13. Testing and post-deploy verification

Automated: sections 9.7 and 11 (CI runs 05's make targets, 04's exposure check, image and script checks). Production checks: `ops/ci/smoke.sh` (unauthenticated, read-only) plus the manual table.

`ops/ci/smoke.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
api_app="$(sed -n 's/^app = "\(.*\)"/\1/p' ops/fly/api.toml)"
FLY_API_URL="${FLY_API_URL:-https://${api_app}.fly.dev}"
fail=0; ok() { echo "ok   $1"; }; bad() { echo "FAIL $1"; fail=1; }
code() { curl -s -o /dev/null -w '%{http_code}' --max-time 20 "$@"; }

# Fly-direct checks (production only, and only once the API is deployed: DEPLOY_API=true)
if [ "${SMOKE_LOCAL:-0}" != 1 ] && [ "${DEPLOY_API:-false}" = true ]; then
  [ "$(code "$FLY_API_URL/api/health")" = 200 ] && ok "fly direct /api/health" || bad "fly direct /api/health"
  d="$(code "$FLY_API_URL/api/v1/me")"; case "$d" in 401|403) ok "fly direct requires auth ($d)";; *) bad "fly direct /api/v1/me=$d";; esac
fi

# Netlify checks: skipped until SITE_URL is set (before Stage B)
if [ -z "${SITE_URL:-}" ]; then echo "skip Netlify checks (SITE_URL unset)"; exit $fail; fi
[ "$(code "$SITE_URL/api/health")" = 200 ] && ok "site /api/health via proxy" || bad "site /api/health"
[ "$(code "$SITE_URL/api/v1/me")" = 401 ] && ok "proxied /api/v1/me without token is 401" || bad "proxied /api/v1/me not 401"
h="$(curl -sI --max-time 20 "$SITE_URL/" | tr -d '\r')"
names="content-security-policy x-frame-options x-content-type-options referrer-policy"
[ "${SMOKE_LOCAL:-0}" = 1 ] || names="strict-transport-security $names"
for n in $names; do echo "$h" | grep -qi "^$n:" && ok "header $n" || bad "missing header $n"; done
echo "$h" | grep -i '^content-security-policy:' | grep -Eqi "unsafe-inline|unsafe-eval" && bad "CSP header has unsafe-inline/eval"
[ "$(code "$SITE_URL/some/client/route")" = 200 ] && ok "SPA fallback" || bad "SPA fallback"
node ops/ci/check-deployed-csp.mjs || fail=1
exit $fail
```

`ops/ci/check-deployed-csp.mjs` (the deployed `200.html` must keep its inline-script hashes; catches Netlify snippet injection or post-processing):

```js
import { createHash } from 'node:crypto';
const site = process.env.SITE_URL;
if (!site) { console.log('skip: SITE_URL unset'); process.exit(0); }
const html = await (await fetch(new URL('/200.html', site))).text();
const meta = html.match(/<meta[^>]+http-equiv="content-security-policy"[^>]*content="([^"]+)"/i);
if (!meta) { console.error('FAIL no CSP meta in deployed 200.html'); process.exit(1); }
const allowed = new Set([...meta[1].matchAll(/'sha256-([^']+)'/g)].map((m) => m[1]));
const bodies = [...html.matchAll(/<script(?![^>]*\bsrc=)[^>]*>([\s\S]*?)<\/script>/g)].map((m) => m[1]);
let bad = 0;
for (const b of bodies) {
  const h = createHash('sha256').update(b).digest('base64');
  if (!allowed.has(h)) { console.error('FAIL inline script not covered by CSP meta (injection or post-processing?)'); bad = 1; }
}
console.log(bad ? 'FAIL deployed CSP hashes' : `ok   deployed CSP hashes (${bodies.length} inline script(s))`);
process.exit(bad);
```

`ops/ci/check-demo-values.sh`: the production secret inventory (section 12) must never reuse a local demo value. `backend/demo-fingerprints.txt` (05 owns it; inside the backend build context) holds lines `<sha256-hex>  <VARIABLE_NAME>`. The script reads each given file, parses `KEY=VALUE` lines (also `KEY = "VALUE"` toml lines), hashes every value (and, for URL values, the password component) and fails if a hash equals column 1 of a fingerprint line. CI runs it on the committed production-facing files; Rodney runs it on every secrets file before `fly secrets import` (runbook step 5). It only ever prints variable names, never values.

```bash
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
fp=backend/demo-fingerprints.txt
[ -f "$fp" ] || { echo "missing $fp" >&2; exit 2; }
[ $# -ge 1 ] || { echo "usage: $0 <file>..." >&2; exit 2; }
sha() { if command -v sha256sum >/dev/null; then sha256sum | awk '{print $1}'; else shasum -a 256 | awk '{print $1}'; fi; }
hashes="$(awk '!/^#/ && NF { print $1 }' "$fp")"
bad=0
for f in "$@"; do
  while IFS= read -r line; do
    case "$line" in ''|'#'*|'['*) continue;; *=*) ;; *) continue;; esac
    key="$(printf '%s' "${line%%=*}" | tr -d ' ')"
    val="${line#*=}"; val="$(printf '%s' "$val" | sed -E 's/^[[:space:]]*"?//; s/"?[[:space:]]*$//')"
    [ -n "$val" ] || continue
    cands=("$val")
    if [[ "$val" =~ ^[a-z+]+://[^:/@]+:([^@]+)@ ]]; then cands+=("${BASH_REMATCH[1]}"); fi   # password inside a URL
    for c in "${cands[@]}"; do
      h="$(printf '%s' "$c" | sha)"
      if grep -qxF -- "$h" <<<"$hashes"; then echo "demo value found: $key in $f" >&2; bad=1; fi
    done
  done < "$f"
done
exit $bad
```

Manual checks (Rodney; they touch Supabase Auth or production machines, so agents must not run them):

| # | Check | Command / action | Expected |
| --- | --- | --- | --- |
| 1 | Direct fly.dev requires auth | `ops/ci/smoke.sh` and `curl -i https://integrated-awn-weather-api.fly.dev/api/v1/stations` | 401/403, never data |
| 2 | Signed proxy (**only after enabling it, 8.1**) | `curl -i -H 'x-nf-sign: junk' https://integrated-awn-weather-api.fly.dev/api/v1/me`; **positive:** `curl -i https://<site>/api/v1/me` | direct: 403; through Netlify: 401 (proves the proxy signs correctly; a 403 means signing is misconfigured) |
| 3 | Health exempt | `curl -i https://integrated-awn-weather-api.fly.dev/api/health` | 200, no data |
| 4 | Headers on HTML and assets | smoke.sh | present on `/`; `/_app/immutable/*` immutable; API responses `no-store` |
| 5 | Deployed CSP hash | `node ops/ci/check-deployed-csp.mjs` (in smoke) | inline scripts covered by the meta; Netlify snippet injection and post-processing are off |
| 6 | Sign-up disabled | `curl -s -X POST "$SUPABASE_URL/auth/v1/signup" -H "apikey: $PUBLISHABLE" -H 'Content-Type: application/json' -d '{"email":"probe+delete-me@example.invalid","password":"<random 24 chars>"}'`; and with body `{}` | first: `signup_disabled`; second: `anonymous_provider_disabled` or `signup_disabled`. If any returns a user, delete that user in the dashboard immediately and fix settings |
| 7 | Data API does not expose `app` | `curl -s -o /dev/null -w '%{http_code}' "$SUPABASE_URL/rest/v1/credentials" -H "apikey: $PUBLISHABLE" -H 'Accept-Profile: app'` (and without the header) | not 200; no rows |
| 8 | Publishable key is not a login | `curl -i -H "Authorization: Bearer $PUBLISHABLE" https://<site>/api/v1/me` | 401 |
| 9 | Machine counts | `fly machines list -a integrated-awn-weather-poller` and `-a integrated-awn-weather-api` | exactly 1 each |
| 10 | Key isolation | `fly secrets list -a integrated-awn-weather-api` | no `CREDENTIALS_PRIVATE_KEY`, no `SUPABASE_SERVICE_ROLE_KEY`, no poller DB URL |
| 11 | Poller lock prevents duplicates | `ops/fly/run-oneoff.sh batch integrated-awn-weather-poller integrated-awn-weather-poller poller -- /usr/local/bin/poller` (the script mirrors `POLLER_ENABLED=true`; batch mode blocks while the scheduler runs, so run it in a second terminal and stop it with Ctrl-C, whose trap destroys the machine), then `fly logs -a integrated-awn-weather-poller`, then **destroy that machine**: `fly machines list -a integrated-awn-weather-poller`, `fly machine destroy <oneoff-id> -a integrated-awn-weather-poller --force` | the second instance logs lock-held and writes nothing; **no job has two `running` rows**: in the Supabase SQL editor `select job, count(*) from app.job_runs where status='running' group by 1 having count(*)>1` returns 0 rows |
| 12 | DB TLS verify-full | first connectivity test from the backup image | connects with `sslmode=verify-full` |
| 13 | Heartbeats | healthchecks.io | all green within one period |
| 14 | Invite and login end to end | `ops/fly/run-oneoff.sh shell integrated-awn-weather-admin integrated-awn-weather-poller poller 15`, then `fly ssh console -a integrated-awn-weather-admin --machine <id> --pty -C "/usr/local/bin/admin invite --email <test address> --display-name test"` (shell mode only, so the link is not in `fly logs`); open the printed set-password link **on the deployed site**, choose a password and sign in; sign out and sign in again with it; run `admin reset-link --email <test address>` and set a new password from that link without the old one (this proves the two password-change settings of section 7 step 5); **clean up:** `admin deactivate-user` then `admin delete-user` for it | no email is sent or expected (none is configured); the link opens `https://<site>/set-password`; both password changes work; the test user is removed |
| 15 | Backup restores, S3 policy | section 15.6 drill; attempt to re-upload an existing key and a `put-object` without `--if-none-match` using the uploader key | restore succeeds; re-uploading an existing key with `--if-none-match '*'` returns **412** (PreconditionFailed); a `put-object` without the header returns **403** (bucket policy) |
| 16 | Weekly hygiene | `supabase_smoke.sh` needs the API keys and stays a manual weekly calendar item. `exposure.sh`: the repo is public (decided Oct 7) and minutes are free, so a scheduled workflow `.github/workflows/exposure-weekly.yml` (`on: schedule` weekly plus `workflow_dispatch`, `environment: ci-readonly`, an environment **without required reviewers** that holds only `EXPOSURE_DATABASE_URL`, i.e. the `app_ci_check` URL with no table grants) runs it; GitHub disables scheduled workflows in public repos after 60 days without repository activity, so if the weekly run stops, re-enable it in the Actions tab (11.5) | exit 0 |
| 17 | Exposure and Supabase smoke | `ops/db/checks/exposure.sh` as `app_ci_check`; `ops/db/checks/supabase_smoke.sh` | exit 0 |

Security tests named in the review (S1, S7, S10, S11) belong to 02/03/04; CI runs them via `make test`.

## 14. Cost summary (C1)

Assumptions: poller 256 MB 24x7; API 512 MB auto-stopping about 80% of the time; backup machine about 2 minutes per day; S3 holding about 90 daily dumps under 100 MB; no dedicated IPv4; no Netlify credit overrun. Verify against [Fly pricing](https://fly.io/docs/about/pricing/) and the Supabase and AWS pages.

| Item | Monthly |
| --- | --- |
| Netlify free (C6) | $0 |
| Fly poller shared-cpu-1x 256 MB 24x7 | about $2 |
| Fly API 512 MB with auto-stop (decided Oct 7; about $3.2 always on, so auto-stop saves about $1 to $2) | about $1 to $2 |
| Fly backup machine plus rootfs | under $0.50 |
| Supabase free (Pro about $25 trigger: C2) | $0 |
| AWS S3 (about 9 GB plus requests) | under $0.50 |
| healthchecks.io free | $0 |
| Email provider / SMTP | none now (decided Oct 7). Growth path only: SES would be about pennies plus a domain |
| GitHub Actions (public repo: unlimited minutes on standard runners) | $0 |
| Optional custom domain | about $1 |
| **Expected total** | **about $4 to $6** |

## 15. Runbooks (Rodney executes; agents may script but not run against production)

### 15.1 First deploy end to end

Stage A (phase 2: the poller goes live early because history cannot be rebuilt):
0. **Pre-build verifications (required, before relying on the decisions; Rodney). Step-by-step instructions: `docs/Pre-build Checks.md`.** (a) Proposal 03: run its scratch-project checklist on a throwaway hosted Supabase project and record the results in 04 section 6.5; if checklist step 3 fails (a definer function cannot read `auth.sessions`), apply the S2 downgrade described in 04 section 6.5 Task 0. (b) Proposal 04 (step 0b): on a throwaway Netlify site and throwaway repo, confirm that a fast-forward push to the production branch from Actions with `GITHUB_TOKEN` succeeds and Netlify builds and publishes it, with branch deploys None and `main` ignored; then apply the real ruleset (block deletion and force pushes, no "restrict updates") and confirm the Actions push still succeeds and a manual force push fails. If it fails, see 11.4 notes. Delete the scratch resources afterwards. (Proposal 05, the email + password login, has **no** pre-build check: its open items are tests in the build against the local GoTrue, and the two password-change switches plus minimum length are checked in section 7 step 5 while the real project is set up.)
1. Accounts and MFA: GitHub, Fly (org), Netlify, Supabase, AWS (backups only), healthchecks.io, password manager entries. No email provider.
1a. Tools on Rodney's machine: `flyctl` (provides `fly`; `run-oneoff.sh` calls `fly`), `jq`, `age`, `psql` 15 or newer (`roles.sql` uses `\getenv`), `python3`, `openssl`, Docker (for `make keygen`), Node per `web/.nvmrc` (Stage B), AWS CLI v2.
1b. Supabase CA certificate (7, step 1b) is downloaded and committed as `backend/certs/supabase-ca.crt` (needs the Supabase project: create it now with step 7.1, then return here). AWS setup (9.5) and the age key (9.6).
2. **Netlify:** create the site and link the repo; base directory `web`; **production branch `production`** (if the UI only offers existing branches, create `production` from `main` first; its first build fails or is empty until 01 ships, which is expected), auto-publishing left on, no build hook, no Netlify token; deploy previews on for pull requests, branch deploys None; post-processing and snippet injection off. Note the site URL. Do not deploy yet.
3. Supabase (section 7): Auth settings with Site URL, an empty redirect allow-list, and the password-settings check of step 5 (reauthentication off, require-current-password off, minimum length 12; fall back to proposal 05 option b1 if a switch cannot be turned off); no SMTP; signing keys; API keys (`sb_publishable_`, `sb_secret_`); Data API off; bootstrap `roles.sql` over the session pooler, then `auth_access.sql`; (The Netlify env vars `PUBLIC_SUPABASE_URL` and `PUBLIC_SUPABASE_ANON_KEY`, and the project ref in the web headers and `ops/fly/api.toml`, are done in Stage B, step 13.)
4. healthchecks.io: create only the `AWN_INGEST`, `BACKUP` and `DBSIZE` checks now (section 10); copy those ping URLs.
5. `fly apps create` for the four apps; **set secrets for the poller, backup and admin apps** (3.3; each from a 0600 secrets file that passed `check-demo-values.sh`, imported with `fly secrets import --stage < file`, then deleted, 4.6). The API's secrets are set at Stage B. Generate the HPKE pair with `make keygen` (runs `admin keygen` in a throwaway container, no database). Secrets files live outside the repo and are checked with an absolute path (4.6). Create **all three** Fly deploy tokens now (poller, api, backup): every deploy validates all three config files. **Before each `fly secrets import`, run `ops/ci/check-demo-values.sh <secrets-file>` on the file you are about to import** (it hashes each value and fails on a match in `backend/demo-fingerprints.txt`: local demo JWT secret, demo keys, compose passwords); delete the file afterwards. The apps also refuse demo values in production, so this is the earlier, friendlier failure.
6. GitHub: environment `production`, tokens (11.1), `ci-readonly`, repo variable `SITE_URL`; create `ops/fly/site-url.txt` (the site origin) and commit it with the CA certificate; open the pull request, wait for `ci-ok` green, then merge the workflows with the `main` ruleset.
7. Push to `main`. The workflow migrates the database and **deploys the poller** (poller-only first deploy: `DEPLOY_API`/`DEPLOY_WEB` are unset, so the API and web publish steps are skipped), then runs the exposure check and smoke.
8. Connectivity test: run the backup image once on demand; confirm `verify-full` TLS, IPv6 reachability, RTT. Create the daily backup machine (section 6; the app already exists from step 5), run it manually, confirm the objects in S3 and the healthcheck.
9. `admin invite --email <you> --display-name <you> --admin --follow-all` for Rodney, in Stage A, so his station has an owner (the website is not live yet, so ignore the printed link; issue a fresh `admin reset-link --email <you>` at step 13 when the site is up) (`shell` mode via `fly ssh console -C`, never `batch`, so the link stays out of `fly logs`; section 5), then the family.
10. `admin add-reference-station` for each baseline station.
11. Stations: `admin add-station` via the interactive `shell` mode (dry-run first, section 5) or, if that fails, through the web connect flow after Stage B.
12. Run `awn_backfill` once per station via `run-oneoff.sh batch integrated-awn-weather-poller integrated-awn-weather-poller poller -- /usr/local/bin/poller run-job awn_backfill --station <uuid>` (history ages out of Ambient, so do this at go-live), then watch `awn_ingest` go green.

Stage B (API and web):
13. When 02 and 01 have shipped: **commit the real Supabase project ref into `web/security-headers.json`, run `node web/scripts/headers.mjs generate`, and fill `<project-ref>` in `SUPABASE_URL` and `SUPABASE_JWKS_URL` in `ops/fly/api.toml`** so `web/netlify.toml` has the right `connect-src` before the first Netlify build; set the Netlify variables `PUBLIC_SUPABASE_URL` and `PUBLIC_SUPABASE_ANON_KEY` (here, not in Stage A: the site cannot build before the ref is filled in), set the API secrets, create the remaining healthchecks, set repo variables `DEPLOY_API=true`, `DEPLOY_WEB=true` and `SITE_URL`, create the GitHub ruleset on the Git branch `production` (block deletion and non-fast-forward pushes only, no "restrict updates" rule; GitHub MFA on; it is enforced because the repo is public); re-run the deploy workflow (after the API deploy the `publish-web` job fast-forwards `production`, Netlify builds and publishes, and the workflow waits for `/version.txt` to equal the commit SHA); verify checks 1 to 10 of section 13.
14. **JWKS check:** log in through the site and confirm `/api/v1/me` returns 200 with the new asymmetric key. Only then, in this order: disable the legacy `anon`/`service_role` API keys, move the legacy HS256 secret to "Previously used", wait at least 1 h 15 min, revoke it, re-run smoke and checks 6 to 8.
15. Enable the signed proxy when desired (8.1) and run check 2. Perform the first restore drill (15.6). Record results.

Done when: all section 13 checks pass, all healthchecks stay green for 24 h, a restore drill succeeded.

### 15.2 Credential marked invalid ("needs reconnecting")
1. Confirm from `job_runs`/healthchecks that only that owner's credential fails (an application-key failure affects every station: R2).
2. The owner reconnects via the connect flow; if they cannot, Rodney runs `admin add-station` (shell mode) with the new key.
3. Run `awn_backfill` for the gap. Many simultaneous failures suggest an AWN outage or a changed application key; check AWN status first.

### 15.3 Poller stalled
1. Healthchecks email; `fly status -a integrated-awn-weather-poller`, `fly logs -a integrated-awn-weather-poller`.
2. Stopped or crash-looping: `fly machine restart <id>`; if the image is bad, redeploy the previous one with `--config` (4.7).
3. Running but silent: suspect a stuck advisory lock or a stray one-off: `fly machines list -a integrated-awn-weather-poller`, destroy strays, restart the poller (locks release on disconnect).
4. DB unreachable: check Supabase paused or limits (15.4), IPv6 or `verify-full` errors in logs.
5. Confirm catch-up in `job_runs`; run `awn_backfill` for gaps longer than the catch-up horizon.

### 15.4 Supabase paused or near limit
- Paused: restore from the dashboard; verify the poller reconnects; backfill gaps; if a pause warning arrived, upgrade to Pro (C2).
- Near 500 MB (`DBSIZE` fail at 400 MB): find growth (`size.sql` output in the healthchecks body); observations are the main growth (60 to 100 MB a year; no forecast data is stored): upgrade to Pro. Never delete observations.

### 15.5 Key leak response
Rotate first, investigate second.
- A user's AWN key: owner regenerates, reconnects (15.2). Application key: R2.
- `CREDENTIALS_PRIVATE_KEY`: R1 now; every stored AWN key is exposed, so owners regenerate and reconnect; treat backups made under the old key as compromised.
- `sb_secret_` key: R4; review auth users for unknown accounts; deactivate them.
- A DB role password: R3; check Supabase logs for unknown IPs.
- Fly deploy token or GitHub environment leak: revoke (R6), rotate the migrator and exposure URLs (R3), review Fly releases and machines and the GitHub audit log.
- AWS uploader key: R7 (it can only add objects); inspect the bucket. age private key: R8; all stored backups are readable.
- Proxy secret: R5. Publishable key: not a secret; ensure checks 6 to 8 pass.

### 15.6 Restore (one procedure for the drill and for disaster)
Target: the **drill** restores into a local Compose database **on Rodney's laptop**. This is an explicit, accepted exception to S12 ("avoid copying family data onto laptops"), decided Oct 7, 2026: the drill data (encrypted AWN keys, the restored logins and emails, all family data) must be deleted when the drill ends (step 9). A **disaster** restores into a new Supabase project (repeat section 7 first).
1. Rodney assumes the restore identity (MFA). List and download the latest object(s): `aws s3 ls s3://<bucket>/weather/ --recursive | tail`, `aws s3 cp s3://<bucket>/weather/YYYY/MM/app-<ts>-<rand>.dump.age .` and the login list from the same backup run, `auth-users-<ts>-<rand>.csv.age`.
2. Decrypt with the offline key: `age -d -i weather-backup.key -o app.dump app-<ts>-<rand>.dump.age` and `age -d -i weather-backup.key -o auth-users.csv auth-users-<ts>-<rand>.csv.age`. Delete decrypted files after the drill (step 9).
3. Find the dump's migration version: `pg_restore --data-only --table=_sqlx_migrations -f - app.dump` (highest `version`). Check out the repo commit whose `backend/migrations/` ends at that version (`git log` on that migration file).
4. Prepare the target in production order. Local Compose: `docker compose up -d db auth`, then the `auth-access` one-shot (`docker compose run --rm auth-access`, which runs `ops/db/bootstrap/auth_access.sql` as `postgres` after GoTrue created `auth`; 05's role init alone is not enough), then `docker compose run --rm migrate` pinned to the checked-out commit from step 3. New Supabase project: `roles.sql`, then `auth_access.sql` as `postgres` (7, step 9), then `admin migrate` as `app_migrator`. The schema now matches the dump.
5. Restore data only, as `app_migrator` (so ownership stays correct), skipping the migrations table: `pg_restore -l app.dump | grep -v _sqlx_migrations > list.txt` then `pg_restore --data-only --single-transaction --use-list list.txt --dbname "$TARGET_URL_AS_APP_MIGRATOR" app.dump`. If circular FKs block the restore, treat that as a schema bug to fix in 04 (deferrable constraints), not something to bypass by disabling triggers. **Logins (decided Oct 7):** after the `app` data is in, run `admin restore-auth-users --dry-run --file auth-users.csv`, then without `--dry-run` (04 10.2; against the local `admin` service for a drill, or a `integrated-awn-weather-admin` one-off for a disaster, with `SUPABASE_URL` and the service key of the target). It recreates each missing Supabase login with its original id and email through the Admin API, so `app.users` ids line up; nothing is restored into `auth.*` directly. Passwords and sessions are not part of the backup; users sign in with the configured login method.
6. Verify: row counts for `observations` and `daily_rollups` against production (SQL editor); `max(ts_utc)` of observations within 36 h of the dump; `ops/db/checks/exposure.sh` as `app_ci_check`; `admin invite --email drill@example.invalid --display-name drill --follow-all` against the target, then redeem the printed set-password link, sign in as that user and load a chart for a followed station (restored logins have no password, so also confirm `admin reset-link` works for one restored user); also check that `auth-users.csv` has as many rows as the production dashboard shows users and that `restore-auth-users` reported every `app.users` row restored; if `api` or `web` are not yet deployable, rely on the SQL checks above. Record date, backup timestamp, duration and issues in `docs/ops-log.md` (create on first drill).
7. Disaster only: point Fly secrets, GitHub secrets and Netlify at the new project, redeploy, backfill gaps from AWN.
8. Cadence: quarterly (calendar reminder). A failed drill is a P1 issue.
9. **Cleanup after every laptop drill (the condition of the S12 exception):** `docker compose down -v` (removes the restored database and the restored GoTrue users), delete `app.dump`, `auth-users.csv`, the `.age` files and `list.txt` (`rm -P` or a secure-delete tool where available; the laptop disk is encrypted with FileVault), and confirm with `docker volume ls` and `ls` that nothing remains. Record "data deleted" in `docs/ops-log.md`.

## 16. Implementation tasks (ordered)

1. [ ] Create `ops/fly/api.toml`, `ops/fly/poller.toml`, `ops/backup/fly.toml`, root `.dockerignore` (4.2, 4.3, 6). *Status: files written and structure checked (`api.toml` `min_machines_running = 0`, `poller.toml` no http_service/services/auto_stop, backup toml no processes/restart; the `.dockerignore` context holds exactly the three backup inputs). Not verifiable without a Fly token: `flyctl config validate` and `flyctl deploy --build-only` both stop at "no access token available" (flyctl v0.5.1), so config validation is covered by the first deploy job (`config validate --strict`, then the deploy itself); the `--dockerfile` path resolution is verified (relative to `backend/`, see 4.7). Tick this box after that.* **Done when** `flyctl config validate --config <file>` passes where it needs no token (otherwise in the deploy job), api.toml has `min_machines_running = 0`, poller.toml has no `[http_service]`, `[[services]]` or `auto_stop_machines`, backup toml has no `[processes]`/`[[restart]]`, and `flyctl deploy backend --config ../ops/fly/api.toml --dockerfile Dockerfile --build-only` resolves the Dockerfile path.
2. [ ] Create `ops/fly/run-oneoff.sh` (section 5). *Status: written; shellcheck clean; no-args and 3-args print usage and exit 2; a non-`/usr/local/bin` command and `invite`/`reset-link` in batch mode exit 2. Remaining, Rodney only (needs a Fly account): the dry run below.* **Done when** `shellcheck` is clean, running without enough arguments prints usage and exits 2, and a non-`/usr/local/bin` command in `batch` mode is rejected, and a **dry run** against the poller image (`ops/fly/run-oneoff.sh batch integrated-awn-weather-poller integrated-awn-weather-poller poller -- /usr/local/bin/admin --version`, which exits 0 and touches no database) proves `fly machine wait` returns only after the command has finished, the logs are printed, the exit code is propagated, and the machine is destroyed.
3. [x] Create `ops/backup/{Dockerfile,backup.sh,test.sh}`. **Done when** `ops/backup/test.sh` passes locally (9.7). *Done; the S3 stand-in is `adobe/s3mock`, because MinIO's public images are no longer published (the conditional-write check stays a real-S3 item, section 13 check 15).*
4. [x] Create `ops/ci/smoke.sh`, `ops/ci/check-deployed-csp.mjs` (section 13) and `ops/ci/check-demo-values.sh` (below). **Done when** smoke passes against the Compose stack with `SMOKE_LOCAL=1 SITE_URL=http://localhost:8000` and the CSP script fails on a copy of `200.html` with an extra inline script.
5. [x] Confirm `web/netlify.toml` is the output of `node web/scripts/headers.mjs generate` (`headers.mjs check` exits 0). **Done when** its redirects equal 8.2 apart from the `<project-ref>` fill and the commented `signed` line.
6. [~] *Status: files written, `actionlint` 1.7.7 passes; the green PR run can only be seen on GitHub, so this stays open until then. Repo pins Rust 1.95 and Node 26 (`web/.nvmrc`); the `hashFiles` job conditions in web.yml were dropped (not valid at job level, and `web/package.json` exists).* Create `.github/workflows/{backend,ci,web,deploy,exposure-weekly}.yml` and `.github/dependabot.yml` (section 11). **Done when** `actionlint` passes and a PR run of `backend` and `web` is green in CI using the make targets.
7. [x] Add `backend/deny.toml`. **Done when** `cargo deny check` passes locally. *Done: passes (cargo-deny 0.20.2). Two advisories with no usable fix are ignored by id with reasons (RUSTSEC-2023-0071 rsa and RUSTSEC-2026-0387 crypto-bigint, both reached only through api dev-dependencies); three were fixed by patch bumps in `backend/Cargo.lock`. Added Zlib and CDLA-Permissive-2.0 to the licence allow-list; workspace crates are `licenses.private`.*
8. [ ] Hand Rodney the checklists (sections 7, 8.1, 9.5, 11.1) as written; no agent action. **Done when** Rodney confirms runbook 15.1 steps 1 to 6 (including the `ci-readonly` environment).
9. [ ] After the first production deploy, Rodney runs section 13 and records results. **Done when** all applicable checks pass (checks that need the API or web apply once those ship).

## 17. Security checklist (verify before merging any change here)

- [ ] No secret value, credentialed URL or AWN key appears in any committed file, workflow log or script argument.
- [ ] `CREDENTIALS_PRIVATE_KEY` appears only in the poller app configuration; `SUPABASE_SERVICE_ROLE_KEY` only in admin.
- [ ] Poller has no `[http_service]`, no `auto_stop_machines`, `restart always`; deploys use `--ha=false`; CI asserts one machine per app; `auto_stop_machines` appears only in `api.toml`.
- [ ] Every `uses:` is pinned to a 40-character SHA; workflows set `permissions: contents: read` and `persist-credentials: false` (exception: the `publish-web` job has `contents: write`, still with `persist-credentials: false`); production secrets only in the protected environment; deploy tokens are per-app; called workflows have no `push:` trigger or `concurrency:`.
- [ ] Netlify production branch is `production` (only `deploy.yml`'s `publish-web` job is meant to move it; the ruleset blocks deletion and force pushes only; anyone with repo write could still push, so approval and MFA are the controls), `main` builds nothing, no Netlify token or build hook exists in GitHub, `contents: write` is on the `publish-web` job only (no `environment:`, needs `deploy`); Netlify branch deploys, snippet injection and post-processing are off; deploy previews build for pull requests and skip `dependabot/*`; Supabase's redirect allow-list is empty (previews sign in with a password like production); only `PUBLIC_*` (plus the optional runtime-only proxy secret) in Netlify; the bundle scan and parity check pass.
- [ ] Backup uploader IAM allows `s3:PutObject` only; bucket policy denies non-TLS and unconditional writes; versioning on; the age private key is in no online system; the root `.dockerignore` keeps `.env` out of the backup build context; the login list is captured only through `app_auth.backup_users()` (id, email, created_at), is age-encrypted, and the script logs counts only.
- [ ] CI: every job has `timeout-minutes`; path filters on PR and a `changes` job on `main`; caches in place; the public-repo hygiene settings in 11.1 are on (secret scanning, push protection, Dependabot alerts, fork-PR approval, no `pull_request_target`); the restore drill cleanup (15.6 step 9) is done after each laptop drill.
- [ ] Supabase: sign-up and anonymous off, Data API off, SSL enforced, empty redirect allow-list, the two password-change switches off and minimum length set (step 5), no SMTP configured, legacy HS256 revoked only after the JWKS check.
- [ ] Production never uses the `postgres` login in apps, `JWT_HS256_SECRET`, `AWN_MODE=replay`, or non-`verify-full` TLS.
- [ ] Exposure check runs as `app_ci_check`, not as a role with data access.
- [ ] Every production secrets file passed `ops/ci/check-demo-values.sh` before import; no value in the inventory matches `backend/demo-fingerprints.txt`.

## 18. Open questions and assumptions

1. (Closed) Architecture: four Fly apps, accepted (proposals/02).
2. (Closed, Nov 1) Region: Supabase specific region `us-west-1`, Fly `sjc` (section 4.1). Backups stay in AWS `us-west-2`, a different region from the database on purpose. Still measure RTT after deploy.
3. (Closed, Oct 7) S3 retention: versioning plus 90 days current, 7-day noncurrent grace, no Object Lock (9.5). Escrow (decided Oct 7): one age key, private key in Rodney's password manager plus a printed copy in a safe; a second escrow recipient is optional.
4. (Closed, Oct 7) Fly API auto-stop is allowed; no external uptime probe (4.2, 10).
5. (Closed, Oct 7) **No SMTP provider for now** (proposals/05, option b2): login is email + password with hand-delivered set-password links. Growth path (documented, not built): custom SMTP (Amazon SES acceptable) plus emailed links; see section 7 step 5 and 04 section 8.1a. Open until the project is created: whether the two password-change switches can be turned off on the free plan (section 7 step 5; fallback proposal 05 option b1).
6. (Closed, Oct 7) Auth user capture: `app_auth.backup_users()` feeds an encrypted `auth-users` CSV in every backup (`BACKUP_INCLUDE_AUTH` defaults to 1); `admin restore-auth-users` restores through the Admin API (no direct `auth.*` restore). Still to verify at the first drill: GoTrue accepts an explicit `id` on admin create-user (04 10.2).
7. (Closed) `integrated-awn-weather-admin` holds no `AWN_APPLICATION_KEY`, matching the updated conventions; `admin add-station` takes the user's key from the prompt and does not need it.
8. (Closed, Oct 7) Restore drill target: Rodney's laptop is allowed for now, as an explicit accepted exception to S12, with the data deleted after each drill (15.6 steps 9). Revisit if more people ever hold production data.
9. Optional guard against re-enabled branch deploys: a failing `[context.branch-deploy]` build command in `netlify.toml` (01 owns the file).
10. Unverified Fly behaviours: `fly ssh console --pty -C` on a distroless image with `admin wait` (dry run, section 5); optional build-once `--image` deploys across apps with app-scoped tokens (proposal 02 fact 6; per-app builds are the default); `fly machine update --image` keeping `--schedule`; `--dockerfile` path resolution (4.7).
11. Verify at deploy time: direct-host TLS hostname matches with `verify-full` (and the pooler's); Netlify proxy timeout; healthchecks.io free check limit; current Supabase free-plan limits; whether S3 conditional writes (`--if-none-match`, policy key `s3:if-none-match`) behave as written with the installed AWS CLI.
12. How `headers.mjs check` treats the `<project-ref>` placeholder before the real Supabase project exists, and outside `CONTEXT=production` (01).
13. Poller has no HTTP port; the dead-man's-switch is the only liveness signal. On SIGTERM the poller waits up to a **25 s grace** for in-flight units (03); `kill_timeout = "30s"` on the poller (and API) leaves 5 s of margin before Fly sends SIGKILL.
14. `fetch-depth: 2` change detection for the backup image misses multi-commit pushes; accepted.
15. (Closed, "not needed") Web publishing uses the `production` branch (proposals/04, accepted); no build hook or lock. Still to verify before the first web deploy: the throwaway-Netlify-site test in runbook 15.1 step 0 (bot-pushed fast-forward builds and publishes).
16. (Closed, decided Oct 7) The repo is **public** on GitHub Free. Reason: a private Free repo has no deployment environments, so the `production` reviewer gate and environment secrets would not work. Public keeps environments, the enforced `production` ruleset and unlimited Actions minutes. Nothing secret lives in the repo; the hygiene rules are in 11.1.

## 19. Changes other guides must make (for this guide to hold)

- **02:** Dockerfile `COPY certs/ /etc/ssl/app-certs/` (conventions). (Superseded: there is no `cargo sqlx prepare --check` step; queries are runtime `sqlx::query`, 02 4.12.)
- **04:** `roles.sql` takes the five `*_PASSWORD_VERIFIER` values; the roles section should mention the session-pooler alternative for an IPv4-only operator; `exposure.sh` runs as `app_ci_check` and inside `make test-db`; `size.sql` must run as `app_backup` and exit non-zero above 400 MB; `app_backup` has `SELECT` on `app._sqlx_migrations`; the admin client sends `sb_secret_` only in the `apikey` header; `admin invite` and `admin reset-link` create the login / link without sending any email and print the set-password link once (check 14 expects no email at all); settle `admin invite --admin`; optionally `admin migrate --target <version>`; `app_auth.backup_users()` and `admin restore-auth-users` exist (decided Oct 7; 04 6.5 and 10.2).
- **05:** the test overlay's cargo volumes take `CARGO_REGISTRY_SRC` / `CARGO_TARGET_SRC` (done in 05; the tools volume and `SQLX_OFFLINE` were removed) so CI can cache them; `backend/demo-fingerprints.txt` with `<sha256>  <NAME>` lines covering every committed demo value (HS256 secret, anon and service keys, HPKE pair, five role passwords); no Compose `backup` service or `make backup` target.
- **01:** `headers.mjs check` must tolerate the `<project-ref>` placeholder before the Supabase project exists (or fail only when `CONTEXT=production`); `make test-web` runs the production-build guard scripts so 06 does not.
- **00-conventions:** `DB_SIZE_ALERT_MB`, `BACKUP_OUTPUT_DIR` and `HEALTHCHECK_URL_AWN_BACKFILL` are unused by this design; drop `NETLIFY_BUILD_HOOK_URL` (no Netlify credential in GitHub) and add `BACKUP_INCLUDE_AUTH` / `BACKUP_S3_CONDITIONAL` (backup-local) if it lists backup variables.
