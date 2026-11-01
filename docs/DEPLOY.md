# Production runbook (Rodney only)

Step-by-step instructions to put the Family Weather app into production. You are the only person who runs these steps. Coding agents never run production commands.

This is the friendly version of [`docs/guides/06-deploy-ops.md`](guides/06-deploy-ops.md), the full spec. Where this page and the spec disagree, this page follows the files in the repo (the list is at the end).

**Rules for yourself**

- Real values (passwords, keys, URLs with passwords, emails, MAC addresses, coordinates) go in your password manager only. Never in this repo, never in chat, never in a ticket. The repo is public.
- Angle brackets like `<project-ref>` are placeholders for values you look up.
- Every command runs in a terminal at the repo root unless a step says otherwise.
- `fly` and `flyctl` are the same program. This page uses `fly`, because `ops/fly/run-oneoff.sh` calls `fly`.

## What you will end up with

- **Poller** on Fly.io: reads your weather station every few minutes, 24 hours a day.
- **API** on Fly.io: answers the web app. It sleeps when idle.
- **Web app** on Netlify: published from a branch named `production`, which the deploy workflow moves for you.
- **Database and logins** on Supabase (free plan). Password login. No email is sent. You send each person a set-password link by hand.
- **Daily encrypted backups** to AWS S3 (backups only), made on Fly.io.
- **Monitoring** by healthchecks.io: it emails you if the poller or backup goes quiet.

**Time:** Stage A takes about 3 to 4 hours, Stage B about 3 hours. You can stop between stages. **Cost:** about $4 to $6 a month (spec section 14).

**Why Stage A first:** Ambient Weather only keeps limited history. Every day the poller is not running is data lost for good. Stage A gets the poller recording; Stage B adds the website.

## Before you start

### Accounts (turn on two-factor authentication on every one)

- [ ] Password manager (you need it to hold every real value below).
- [ ] GitHub (the repo is public so deployment environments work).
- [ ] Fly.io (needs a card). Create an organisation or use your personal one.
- [ ] Netlify.
- [ ] Supabase.
- [ ] AWS: turn on MFA on the root user, create no root access keys, and do the work as an IAM admin user.
- [ ] healthchecks.io.
- [ ] Ambient Weather account with an **Application Key** for production (create it on your account's API Keys page; make a new one, separate from your local dev key). Your station's own **API Key** is typed later at a hidden prompt.

### Tools

| Tool | Install | Check |
| --- | --- | --- |
| `flyctl` (also gives `fly`) | `brew install flyctl` | `fly version` |
| `gh` (optional) | `brew install gh` | `gh --version` |
| `age` | `brew install age` | `age --version` and `age-keygen --version` |
| `psql` 15 or newer (`roles.sql` uses `\getenv`) | `brew install libpq` and add it to your PATH | `psql --version` |
| `jq` (`run-oneoff.sh` needs it; it also needs the `fly` command name) | `brew install jq` | `jq --version` |
| `python3` (makes password verifiers) | already on macOS | `python3 --version` |
| `openssl` | already on macOS | `openssl version` |
| Docker Desktop (`make keygen`, Stage A) | docker.com | `docker --version` |
| Node (version in `web/.nvmrc`, Stage B only) | `nvm install` in `web/` | `node --version` |
| AWS CLI v2 (restore drill and the S3 check) | `brew install awscli` | `aws --version` |

- [ ] All checks print a version.
- [ ] `fly auth login` opens the browser; `fly auth whoami` shows you.

### Secrets worksheet

Names and homes only. Put the real values in your password manager as you create them. The last column says where each value is used. "pm" means password manager.

| # | Name | Where it comes from | Where it goes |
| --- | --- | --- | --- |
| 1 | `postgres` database password | You choose it when creating the Supabase project | pm only (used only for bootstrap and emergencies) |
| 2 | `app_migrator` password | `openssl rand -hex 24` (A6) | pm; inside rows 17, 20 |
| 3 | `app_api` password | same | pm; inside row 18 |
| 4 | `app_poller` password | same | pm; inside row 16 |
| 5 | `app_backup` password | same | pm; inside row 19 |
| 6 | `app_ci_check` password | same | pm; inside row 21 |
| 7 | Project ref (`<project-ref>`) | Supabase dashboard | URLs below; committed in `ops/fly/api.toml` and `web/security-headers.json` (not secret) |
| 8 | Pooler host (`<pooler-host>`) | Supabase Connect panel | rows 20, 21 and the bootstrap command |
| 9 | `SUPABASE_URL` / `PUBLIC_SUPABASE_URL` (`https://<project-ref>.supabase.co`) | Supabase | Fly `famweather-admin`; Netlify `PUBLIC_SUPABASE_URL` |
| 10 | `PUBLIC_SUPABASE_ANON_KEY` (`sb_publishable_...`) | Supabase API Keys | Netlify (public by design) |
| 11 | `SUPABASE_SERVICE_ROLE_KEY` (`sb_secret_...`) | Supabase API Keys | Fly `famweather-admin` only |
| 12 | `AWN_APPLICATION_KEY` | Ambient Weather account | Fly `famweather-poller` and `famweather-api` |
| 13 | `CREDENTIALS_PUBLIC_KEY` | `admin keygen` (A11) | Fly `famweather-admin` and `famweather-api` |
| 14 | `CREDENTIALS_PRIVATE_KEY` | `admin keygen` (A11) | Fly `famweather-poller` only; pm |
| 15 | `CREDENTIALS_KEY_VERSION` (value `1`) | you | Fly poller, admin, api |
| 16 | poller `DATABASE_URL` (`app_poller`, direct host) | built from rows 4, 7 | Fly `famweather-poller` |
| 17 | admin `DATABASE_URL` (`app_migrator`, direct host) | rows 2, 7 | Fly `famweather-admin` |
| 18 | api `DATABASE_URL` (`app_api`, direct host) | rows 3, 7 | Fly `famweather-api` |
| 19 | `BACKUP_DATABASE_URL` (`app_backup`, direct host) | rows 5, 7 | Fly `famweather-backup` |
| 20 | `MIGRATOR_DATABASE_URL` (`app_migrator`, pooler) | rows 2, 7, 8 | GitHub environment `production` |
| 21 | `EXPOSURE_DATABASE_URL` (`app_ci_check`, pooler) | rows 6, 7, 8 | GitHub environments `production` and `ci-readonly` |
| 22 | `HEALTHCHECK_URL_AWN_INGEST` | healthchecks.io | Fly `famweather-poller` |
| 23 | `HEALTHCHECK_URL_BACKUP` | healthchecks.io | Fly `famweather-backup` |
| 24 | `HEALTHCHECK_URL_DBSIZE` | healthchecks.io | Fly `famweather-backup` |
| 25 | `FLY_API_TOKEN_POLLER` | `fly tokens create deploy -a famweather-poller` | GitHub environment `production` |
| 26 | `FLY_API_TOKEN_API` | same for `famweather-api` | GitHub environment `production` |
| 27 | `FLY_API_TOKEN_BACKUP` | same for `famweather-backup` | GitHub environment `production` |
| 28 | `AWS_ACCESS_KEY_ID` (uploader) | AWS IAM (B1) | Fly `famweather-backup` |
| 29 | `AWS_SECRET_ACCESS_KEY` (uploader) | AWS IAM (B1) | Fly `famweather-backup` |
| 30 | `BACKUP_S3_BUCKET` | AWS (B1) | Fly `famweather-backup` |
| 31 | `BACKUP_AGE_RECIPIENT` (`age1...`, public) | `age-keygen` (B2) | Fly `famweather-backup` |
| 32 | age private key | `age-keygen` (B2) | pm plus a printed copy; never online |
| 33 | AWS restore identity (user with MFA) | AWS IAM (B1) | you only; never Fly or GitHub |
| 34 | `SITE_URL` (the site origin, no path) | Netlify (A1) | GitHub repo variable `SITE_URL`; the file `ops/fly/site-url.txt` |
| 35 | `DEPLOY_API`, `DEPLOY_WEB` (value `true`) | you | GitHub repo variables (set in Stage B) |

Not secret, but note them: your station's MAC, name, latitude, longitude, time zone and standard-time offset (used at A20), and your email and display name.

---

# Stage A: the poller goes live

## A1. Netlify site (only to get its URL)

- [ ] **Where:** terminal, then the Netlify dashboard.
1. Create the `production` branch from `main`: `git fetch origin && git push origin origin/main:refs/heads/production`.
2. Netlify: Add new site, Import an existing project, GitHub, pick this repo. Branch to deploy: `production`. Base directory: `web`. Leave the build command and publish directory alone (they come from `web/netlify.toml`). Deploy.
3. Rename the site (Site configuration, Change site name) to a name you like.
4. Site configuration, Build and deploy, Branches and deploy contexts: production branch `production`; **Branch deploys: None**; **Deploy Previews: every pull request against the repository**.
5. Same area, Post processing: turn **off** snippet injection, asset optimization (bundling and minify) and pretty URLs if shown. They break the strict content security policy.
6. Leave auto publishing on. Create no build hook and no Netlify token.
7. Write down the site address as `<site-origin>`, like `https://<site-name>.netlify.app` (no trailing slash, no path).

- **Check:** the site exists and shows `<site-origin>`. Its first build **failing or being empty is expected** (the project ref is not filled in yet).

## A2. Supabase project

- [ ] **Where:** Supabase dashboard.
1. New project, free plan. Region: West US (North California), which is `us-west-1` and matches the `sjc` region in the Fly files.
2. Choose a long database password. Save it in your password manager as row 1. Do not use it anywhere else.
3. Note the project ref (the part of the project URL before `.supabase.co`) as `<project-ref>`.
4. Turn on MFA for your Supabase account.

- **Check:** the project shows as healthy. You have rows 1 and 7 saved.

## A3. Supabase sign-in settings

- [ ] **Where:** Supabase, Authentication.
1. Sign In / Providers: **turn off "Allow new users to sign up"**. **Turn off anonymous sign-ins**. Email provider **on** (this is what allows password login). "Confirm email" on. Phone and all social providers off.
2. Email OTP expiration: **3600** seconds (this is how long a set-password link lives).
3. Minimum password length: **12**.
4. Find and turn **off** "Secure password change / reauthentication" and **off** "Require current password when changing it". Write down what you found.
5. SMTP: leave custom SMTP **unconfigured**. Nothing in this app sends email.
6. URL Configuration: Site URL = `<site-origin>`. Leave **Redirect URLs empty**.

- **Check:** read the page back; each setting matches.
- **If it fails:** if either switch in item 4 cannot be turned off, or the minimum length cannot be raised, **stop** and tell the coding agent. There is a documented fallback design (proposal 05 option b1).

## A4. Supabase keys, Data API and SSL

- [ ] **Where:** Supabase, Project Settings.
1. JWT Keys: migrate to asymmetric signing keys (ES256) and make the new key the current one. **Keep the legacy HS256 secret for now.**
2. API Keys (new-style): save `sb_publishable_...` (row 10) and `sb_secret_...` (row 11) in your password manager. Do not touch the legacy keys yet.
3. Data API: **disable** it. (If it cannot be disabled, expose no schemas except an empty `public`, and confirm `app` and `app_auth` are not exposed.)
4. Database settings: **Enforce SSL on incoming connections: on**.
5. Connect panel: copy the **session pooler** host (row 8) as `<pooler-host>`.

- **Check:** each toggle reads back as set.

## A5. Download the database CA certificate

- [ ] **Where:** Supabase, Database settings, SSL configuration, then terminal.
1. Download the CA certificate.
2. `mv ~/Downloads/<downloaded-file> backend/certs/supabase-ca.crt`

- **Check:** `head -1 backend/certs/supabase-ca.crt` prints `-----BEGIN CERTIFICATE-----`. You commit it at A16.

## A6. Make the five database passwords

- [ ] **Where:** terminal at the repo root. This shell stays open through A8.
1. Run, saving each printed password in your password manager under rows 2 to 6:
   ```sh
   APP_MIGRATOR_PW="$(openssl rand -hex 24)"; echo "app_migrator: $APP_MIGRATOR_PW"
   APP_API_PW="$(openssl rand -hex 24)";      echo "app_api:      $APP_API_PW"
   APP_POLLER_PW="$(openssl rand -hex 24)";   echo "app_poller:   $APP_POLLER_PW"
   APP_BACKUP_PW="$(openssl rand -hex 24)";   echo "app_backup:   $APP_BACKUP_PW"
   APP_CI_CHECK_PW="$(openssl rand -hex 24)"; echo "app_ci_check: $APP_CI_CHECK_PW"
   ```
2. Make the verifiers (a scrambled form of each password that the SQL needs):
   ```sh
   export APP_MIGRATOR_PASSWORD_VERIFIER="$(printf '%s\n' "$APP_MIGRATOR_PW" | python3 ops/db/bootstrap/scram.py)"
   export APP_API_PASSWORD_VERIFIER="$(printf '%s\n' "$APP_API_PW" | python3 ops/db/bootstrap/scram.py)"
   export APP_POLLER_PASSWORD_VERIFIER="$(printf '%s\n' "$APP_POLLER_PW" | python3 ops/db/bootstrap/scram.py)"
   export APP_BACKUP_PASSWORD_VERIFIER="$(printf '%s\n' "$APP_BACKUP_PW" | python3 ops/db/bootstrap/scram.py)"
   export APP_CI_CHECK_PASSWORD_VERIFIER="$(printf '%s\n' "$APP_CI_CHECK_PW" | python3 ops/db/bootstrap/scram.py)"
   ```

- **Check:** `echo "$APP_API_PASSWORD_VERIFIER" | cut -c1-14` prints `SCRAM-SHA-256$`.

## A7. Create the database roles and the login-check functions

- [ ] **Where:** the same terminal. Never use the Supabase SQL editor for this (it stores history).
1. Run `roles.sql` as `postgres` over the session pooler. `psql` asks for the `postgres` password (row 1); paste it at the prompt:
   ```sh
   psql -X -v ON_ERROR_STOP=1 -f ops/db/bootstrap/roles.sql \
     "postgresql://postgres.<project-ref>@<pooler-host>:5432/postgres?sslmode=verify-full&sslrootcert=backend/certs/supabase-ca.crt"
   ```
2. Then the same for `auth_access.sql` (creates the `app_auth` functions the app uses to check sessions):
   ```sh
   psql -X -v ON_ERROR_STOP=1 -f ops/db/bootstrap/auth_access.sql \
     "postgresql://postgres.<project-ref>@<pooler-host>:5432/postgres?sslmode=verify-full&sslrootcert=backend/certs/supabase-ca.crt"
   ```
3. Clear the shell: `unset APP_MIGRATOR_PASSWORD_VERIFIER APP_API_PASSWORD_VERIFIER APP_POLLER_PASSWORD_VERIFIER APP_BACKUP_PASSWORD_VERIFIER APP_CI_CHECK_PASSWORD_VERIFIER APP_MIGRATOR_PW APP_API_PW APP_POLLER_PW APP_BACKUP_PW APP_CI_CHECK_PW`

- **Check:** both commands end without an error. If `verify-full` rejects the host name, that is the first real test of Open question 11 in the spec: tell the coding agent. Do not fall back to `require`.
- **If it fails:** the direct host `db.<project-ref>.supabase.co` is IPv6 only, which is why this step uses the pooler. `roles.sql` is safe to repeat.

## A8. Check the Supabase settings from outside

- [ ] **Where:** terminal at the repo root.
1. `SUPABASE_URL="https://<project-ref>.supabase.co" PUBLIC_SUPABASE_ANON_KEY="<sb_publishable key>" sh ops/db/checks/supabase_smoke.sh`

- **Check:** exit code 0 (`echo $?`). If it fails, a setting in A3 or A4 is wrong. Fix it and run again.

## A9. healthchecks.io

- [ ] **Where:** healthchecks.io.
1. New project `family-weather`. Add your email plus a second notification channel.
2. Add a check named `AWN_INGEST`: period **5 minutes**, grace **15 minutes**.
3. Copy its ping URL (row 22) into your password manager.

- **Check:** the check exists and shows "new". The BACKUP and DBSIZE checks come in Stage B.

## A10. Fly apps

- [ ] **Where:** terminal at the repo root, logged in (`fly auth whoami`).
1. `fly apps create famweather-poller`, then the same for `famweather-api`, `famweather-admin` and `famweather-backup`.
2. Validate the three config files (this is item V1 in "First-run verification"): `fly config validate --config ops/fly/poller.toml --strict`, then `ops/fly/api.toml`, then `ops/backup/fly.toml`.

- **Check:** `fly apps list` shows all four. Each validate prints that the configuration is valid.
- **If it fails:** if a name is taken, replace `famweather-` everywhere with one `grep -rn famweather-` and ask the coding agent to do it. See spec section 3.1.

## A11. Make the credential key pair (`admin keygen`)

This pair encrypts station API keys. The private half goes to the poller only.

- [ ] **Where:** terminal at the repo root. No network is needed after the build.
1. `make keygen` (first run builds the backend image, which takes several minutes). It starts only a throwaway admin container, with no database and nothing saved.
2. It prints `CREDENTIALS_PUBLIC_KEY=...` and `CREDENTIALS_PRIVATE_KEY=...`. Save both in your password manager (rows 13 and 14). The key version is `1` (row 15).
3. Clear the screen: `clear`.

- **Check:** both lines were printed. You will test that they match at A12.

## A12. Poller secrets (private file, checked, deleted)

- [ ] **Where:** terminal. The file lives **outside** the repo, in your home folder. Use an absolute path: `check-demo-values.sh` changes directory to the repo root, so a relative path would not be found.
1. `umask 077; ${EDITOR:-nano} ~/fw-poller.env` and write these five lines (replace each `<...>` with the real value, no quotes):
   ```
   DATABASE_URL=postgresql://app_poller:<app_poller password>@db.<project-ref>.supabase.co:5432/postgres?sslmode=verify-full&sslrootcert=/etc/ssl/app-certs/supabase-ca.crt
   AWN_APPLICATION_KEY=<awn application key>
   CREDENTIALS_PRIVATE_KEY=<from A11>
   CREDENTIALS_KEY_VERSION=1
   HEALTHCHECK_URL_AWN_INGEST=<ping url from A9>
   ```
2. `ops/ci/check-demo-values.sh ~/fw-poller.env` must print nothing and exit 0 (`echo $?`). A "demo value found" message means you pasted a local demo value.
3. `fly secrets import -a famweather-poller --stage < ~/fw-poller.env`
4. `rm -f ~/fw-poller.env`
5. `fly secrets list -a famweather-poller`

- **Check:** the list shows the five names (digests only, no values) and the file is gone (`ls ~/fw-poller.env` says no such file).

## A13. Admin secrets (same method)

- [ ] **Where:** terminal.
1. `umask 077; ${EDITOR:-nano} ~/fw-admin.env`:
   ```
   DATABASE_URL=postgresql://app_migrator:<app_migrator password>@db.<project-ref>.supabase.co:5432/postgres?sslmode=verify-full&sslrootcert=/etc/ssl/app-certs/supabase-ca.crt
   SUPABASE_URL=https://<project-ref>.supabase.co
   SUPABASE_SERVICE_ROLE_KEY=<sb_secret key>
   CREDENTIALS_PUBLIC_KEY=<from A11>
   CREDENTIALS_KEY_VERSION=1
   ```
2. `ops/ci/check-demo-values.sh ~/fw-admin.env` (exit 0), then `fly secrets import -a famweather-admin --stage < ~/fw-admin.env`, then `rm -f ~/fw-admin.env`.

- **Check:** `fly secrets list -a famweather-admin` shows the five names. The admin app deliberately has no `AWN_APPLICATION_KEY`.

## A14. Fly deploy tokens

- [ ] **Where:** terminal. You need all three now, because every deploy validates all three config files.
1. `fly tokens create deploy -a famweather-poller`. Copy the output into your password manager as row 25. Repeat for `famweather-api` (row 26) and `famweather-backup` (row 27).

- **Check:** three tokens saved.

## A15. GitHub settings and environments

- [ ] **Where:** GitHub, repo Settings.
1. Code security: turn on **secret scanning**, **push protection**, **Dependabot alerts**, **Dependabot security updates** and **private vulnerability reporting**. Do this **before** making the repo public.
2. If the repo is still private: General, Danger Zone, Change visibility, Public.
3. Actions, General: "Require approval for all outside collaborators"; workflow permissions "Read repository contents".
4. Environments, New environment `production`: deployment branches = selected branches, `main` only; required reviewers = you (leave "prevent self-review" off). Add these environment secrets from your password manager: `FLY_API_TOKEN_POLLER`, `FLY_API_TOKEN_API`, `FLY_API_TOKEN_BACKUP`, `MIGRATOR_DATABASE_URL`, `EXPOSURE_DATABASE_URL`.
   - `MIGRATOR_DATABASE_URL` = `postgresql://app_migrator.<project-ref>:<app_migrator password>@<pooler-host>:5432/postgres?sslmode=verify-full`
   - `EXPOSURE_DATABASE_URL` = `postgresql://app_ci_check.<project-ref>:<app_ci_check password>@<pooler-host>:5432/postgres?sslmode=verify-full`
   - Both must end exactly with `?sslmode=verify-full`; the workflow appends the certificate path.
5. New environment `ci-readonly`: no reviewers, selected branch `main`. One secret: `EXPOSURE_DATABASE_URL` (the same value as above).
6. Settings, Secrets and variables, Actions, Variables: add `SITE_URL` = `<site-origin>`. Leave `DEPLOY_API` and `DEPLOY_WEB` unset for now.

- **Check:** the production environment lists five secrets, `ci-readonly` lists one, and the repo has the variable `SITE_URL`. (Optional: `gh variable list`.)

## A16. Commit the certificate and site address, open the pull request

- [ ] **Where:** terminal at the repo root, on branch `frameworking`.
1. Create the site address file (admin one-offs read it): `printf '%s\n' '<site-origin>' > ops/fly/site-url.txt`. It holds one line, `https://...`, no path.
2. Run the two Fly checks from "First-run verification" **V1 and V2** now. If V2 shows the `--dockerfile` path is wrong, tell the coding agent before continuing.
3. `git status`. The list should show the CA file, `site-url.txt` and the new workflow, `ops/`, `backend/deny.toml` and `.dockerignore` files. It must show **no** `.env` or secrets files. Then `git add -A && git commit -m "Add production deploy files, CA certificate and site address"` and `git push -u origin frameworking`.
4. Open a pull request into `main` (GitHub banner, or `gh pr create --base main`).

- **Check:** the PR's `ci-ok` check turns **green** (this is the first ever run of the workflows: item V5). It can take 20 to 25 minutes.
- **If it fails:** read the failing job's log. Do not paste logs with hostnames into public places. Tell the coding agent which job failed.

## A17. Protect `main`, then merge

- [ ] **Where:** GitHub, Settings, Rules, New branch ruleset.
1. Target `main`. Require a pull request. Require status check **`ci-ok`** (and nothing else: the other job names never report when skipped). Block force pushes and deletion.
2. Merge the PR with **Squash and merge** (the workflow compares to the previous commit).

- **Check:** the merge starts the `deploy` workflow on `main`.

## A18. Approve the first deploy

- [ ] **Where:** GitHub, Actions, the `deploy` run.
1. Wait for the `deploy` job to show "Waiting for review". Click Review deployments, tick `production`, Approve and deploy.
2. The job runs: validate configs, migrate the database, deploy the poller, check machine counts, run the exposure check and the smoke test. The API and web steps are skipped.
3. `fly status -a famweather-poller` and `fly logs -a famweather-poller` (do not paste the logs anywhere).

- **Check:** the deploy run is green. The poller shows one machine, started. `fly machines list -a famweather-poller` lists exactly one.
- **If it fails:** the log names the step. If the migrate step cannot connect, re-check row 20 (it must end `?sslmode=verify-full` and use the pooler). If the exposure step fails, re-check row 21. Fix and re-run the failed workflow.
- Now do **V3** (the one-off dry run), which proves the tool used for the next steps.

## A19. Create your account

You are the admin. The website is not live yet, so the link this prints cannot be opened yet. The account is created now so your station has an owner; you get a fresh link at B14.

- [ ] **Where:** two terminals at the repo root.
`run-oneoff.sh` only accepts two app combinations, `famweather-poller famweather-poller poller` and `famweather-admin famweather-poller poller`; every command in this runbook uses one of them. In batch mode it refuses any argument named `invite` or `reset-link`.

1. Terminal 1: `ops/fly/run-oneoff.sh shell famweather-admin famweather-poller poller 15`. It prints a machine id and an ssh command, then waits. Leave it open.
2. Terminal 2: `fly ssh console -a famweather-admin --machine <machine-id> --pty -C "/usr/local/bin/admin invite --email <your-email> --display-name <your-name> --admin --follow-all"`
3. Then in the same terminal: `fly ssh console -a famweather-admin --machine <machine-id> --pty -C "/usr/local/bin/admin list-users"`
4. Keep the session open for A20 (15 minutes in total), then press Enter in terminal 1.

- **Check:** `invite` prints an id and a set-password link (ignore the link for now); `list-users` shows you. Press Enter in terminal 1 at the end, then `fly machines list -a famweather-admin` shows no machine.
- **If it fails:** if `fly ssh console -C` does not work on this image, stop and tell the coding agent. There is no batch-mode fallback for `invite`: `run-oneoff.sh` refuses it on purpose so the link never reaches Fly's logs.

## A20. Connect your station

- [ ] **Where:** terminal 2, inside the same 15-minute session (run A19 again if it has ended).
1. `fly ssh console -a famweather-admin --machine <machine-id> --pty -C "/usr/local/bin/admin add-station --owner <your-name> --mac <station-mac> --name <station-name> --lat <lat> --lon <lon> --timezone <iana-timezone> --std-offset-minutes <minutes>"`
   - `<std-offset-minutes>` is your **standard-time** offset from UTC in minutes (ignore daylight saving; negative for places west of Greenwich).
2. It asks for your station's **API Key** from your Ambient Weather account. Type or paste it at the hidden prompt.
3. It prints the new station id (a UUID). Save it as `<station-uuid>` in your password manager.

- **Check:** a line like `<station-uuid> created`. Within a few minutes `fly logs -a famweather-poller` shows ingest activity.
- **If it fails:** the key is not checked against Ambient in production, so a typo in the MAC or key only shows up as ingest errors in the logs. Run the command again with the right values; it reports "already exists (unchanged)" for a repeat, so ask the coding agent about `--replace-key` before fixing a wrong key.

## A21. Pull the history (`awn_backfill`)

Your station has history back to June 2023. This copies it into the database. It is safe to repeat.

- [ ] **Where:** terminal at the repo root. Do not run it while a deploy is running.
1. `ops/fly/run-oneoff.sh batch famweather-poller famweather-poller poller -- /usr/local/bin/poller run-job awn_backfill --station <station-uuid>`
2. Leave the terminal open. The script waits up to an hour and prints the logs and an exit code at the end.

- **Check:** `exit code: 0`, and `fly machines list -a famweather-poller` shows only the one poller machine afterwards (the script destroys its own machine).
- **If it fails:** run it again. Backfill takes the same rate limits as the live poller, so it is slow, not broken. If the exit code is non-zero twice, see 06 section 15.2 and tell the coding agent.

## A22. Watch ingest go green

- [ ] **Where:** healthchecks.io.
1. Open the `AWN_INGEST` check.

- **Check:** it turns green within about 15 minutes of A20 and stays green. If it does not, see "When something goes wrong".

**Stage A is done.** The poller is recording. You can stop here for a day.

---

# Stage B: the website and API go live

## B1. AWS S3 bucket and users (backups)

- [ ] **Where:** AWS console, as your IAM admin user.
1. S3, create a bucket `famweather-backups-<random>` in `us-west-2`: Block Public Access all on, ACLs disabled, default encryption SSE-S3, **versioning on**. This name is `BACKUP_S3_BUCKET` (row 30).
2. Management, lifecycle rule on prefix `weather/`: expire current versions after **90 days**; permanently delete noncurrent versions after **7 days**; remove expired delete markers; abort incomplete multipart uploads after 7 days.
3. Permissions, bucket policy (replace `REPLACE` with the bucket name):
   ```json
   {
     "Version": "2012-10-17",
     "Statement": [
       { "Sid": "DenyInsecureTransport", "Effect": "Deny", "Principal": "*", "Action": "s3:*",
         "Resource": ["arn:aws:s3:::REPLACE", "arn:aws:s3:::REPLACE/*"],
         "Condition": { "Bool": { "aws:SecureTransport": "false" } } },
       { "Sid": "RequireIfNoneMatch", "Effect": "Deny", "Principal": "*", "Action": "s3:PutObject",
         "Resource": "arn:aws:s3:::REPLACE/weather/*",
         "Condition": { "Null": { "s3:if-none-match": "true" } } }
     ]
   }
   ```
4. IAM, new user `famweather-backup-uploader`: no console access, one access key, inline policy (rows 28 and 29):
   ```json
   {
     "Version": "2012-10-17",
     "Statement": [
       { "Sid": "UploadOnly", "Effect": "Allow", "Action": "s3:PutObject",
         "Resource": "arn:aws:s3:::REPLACE/weather/*",
         "Condition": { "Bool": { "aws:SecureTransport": "true" } } }
     ]
   }
   ```
5. A separate restore identity for you (SSO, or an IAM user with MFA) allowed `s3:GetObject`, `s3:GetObjectVersion`, `s3:ListBucket` and `s3:ListBucketVersions` on the bucket (row 33). Never put it on Fly or GitHub.

- **Check:** the bucket shows versioning enabled and the policy saved without errors. Item V6 later tests the policy for real.

## B2. The age key (backup encryption)

- [ ] **Where:** terminal on your own computer.
1. `age-keygen -o weather-backup.key`. It prints `Public key: age1...`: that is `BACKUP_AGE_RECIPIENT` (row 31).
2. Save the whole file `weather-backup.key` in your password manager (row 32) and print a copy for a safe.
3. Delete the file: `rm -f weather-backup.key`.

- **Check:** you can open the password-manager entry and see a line starting `AGE-SECRET-KEY-`. **The private key never goes on Fly, GitHub, Netlify or AWS.** Lose it and every backup is unreadable.

## B3. healthchecks for backup

- [ ] **Where:** healthchecks.io, project `family-weather`.
1. Add a check `BACKUP`: period **1 day**, grace **6 hours**. Add a check `DBSIZE`: period **1 day**, grace **6 hours**.
2. Save their ping URLs (rows 23 and 24).

- **Check:** two more checks exist.

## B4. Backup secrets

- [ ] **Where:** terminal (same private-file method as A12).
1. `umask 077; ${EDITOR:-nano} ~/fw-backup.env`:
   ```
   BACKUP_DATABASE_URL=postgresql://app_backup:<app_backup password>@db.<project-ref>.supabase.co:5432/postgres?sslmode=verify-full&sslrootcert=/etc/ssl/app-certs/supabase-ca.crt
   BACKUP_S3_BUCKET=<bucket>
   BACKUP_AGE_RECIPIENT=<age1... public key>
   AWS_ACCESS_KEY_ID=<uploader access key id>
   AWS_SECRET_ACCESS_KEY=<uploader secret>
   HEALTHCHECK_URL_BACKUP=<ping url>
   HEALTHCHECK_URL_DBSIZE=<ping url>
   ```
2. `ops/ci/check-demo-values.sh ~/fw-backup.env` (exit 0), `fly secrets import -a famweather-backup --stage < ~/fw-backup.env`, `rm -f ~/fw-backup.env`.

- **Check:** `fly secrets list -a famweather-backup` shows seven names.

## B5. Build the backup image and create the daily machine

- [ ] **Where:** terminal at the repo root, on `main` updated to the merged commit (`git checkout main && git pull`).
1. `sha=$(git rev-parse --short=12 HEAD)`
2. `fly deploy . --config ops/backup/fly.toml --dockerfile ops/backup/Dockerfile --build-only --push --image-label sha-$sha`
3. ```sh
   fly machine run registry.fly.io/famweather-backup:sha-$sha -a famweather-backup \
     --name backup-daily --region sjc --schedule daily --restart no --vm-size shared-cpu-1x --vm-memory 256 \
     -e AWS_DEFAULT_REGION=us-west-2 -e BACKUP_S3_PREFIX=weather
   ```
   The schedule counts from creation, so do this at a quiet hour (about 09:00 UTC).
4. `fly logs -a famweather-backup`. If nothing has run yet, start it once: `fly machines list -a famweather-backup`, then `fly machine start <machine-id> -a famweather-backup`.

- **Check:** the log shows `auth users captured: N (app.users rows: M)` (compare N with the Supabase dashboard user count: it should be 1, you) and `backup ok <timestamp>`. Then, with the restore identity: `aws s3 ls s3://<bucket>/weather/ --recursive` shows an `app-...dump.age` and an `auth-users-...csv.age`. The `BACKUP` and `DBSIZE` checks turn green.
- **If it fails:** the log names the failing step. A TLS error means the CA file is missing from the image; a `403` on upload means the bucket policy or uploader policy is wrong (see V6).

## B6. API secrets

- [ ] **Where:** terminal (same private-file method).
1. `umask 077; ${EDITOR:-nano} ~/fw-api.env`:
   ```
   DATABASE_URL=postgresql://app_api:<app_api password>@db.<project-ref>.supabase.co:5432/postgres?sslmode=verify-full&sslrootcert=/etc/ssl/app-certs/supabase-ca.crt
   AWN_APPLICATION_KEY=<awn application key>
   CREDENTIALS_PUBLIC_KEY=<from A11>
   CREDENTIALS_KEY_VERSION=1
   ```
2. `ops/ci/check-demo-values.sh ~/fw-api.env` (exit 0), `fly secrets import -a famweather-api --stage < ~/fw-api.env`, `rm -f ~/fw-api.env`.
3. `fly secrets list -a famweather-api`

- **Check:** four names. The list must **not** contain `CREDENTIALS_PRIVATE_KEY`, `SUPABASE_SERVICE_ROLE_KEY` or a poller database URL.

## B7. Fill in the Supabase project ref in three files

- [ ] **Where:** terminal at the repo root, on a new branch (`git checkout -b go-live`).
1. In `ops/fly/api.toml`, replace both `<project-ref>` (in `SUPABASE_URL` and `SUPABASE_JWKS_URL`) with the real ref. The ref is public by design.
2. In `web/security-headers.json`, replace `<project-ref>` in `connect-src`.
3. Regenerate and verify the Netlify file (needs the Node in `web/.nvmrc`): `node web/scripts/headers.mjs generate` then `node web/scripts/headers.mjs check`.
4. `git diff` and look for anything that is not the ref. Commit and push: `git add -A && git commit -m "Set the Supabase project ref" && git push -u origin go-live`.
5. Open a pull request into `main`. **Do not merge yet.**

- **Check:** `grep -rn "<project-ref>" ops/fly/api.toml web/security-headers.json web/netlify.toml` prints nothing; `ci-ok` on the PR is green.

## B8. Netlify variables

- [ ] **Where:** Netlify, Site configuration, Environment variables (scope: Builds).
1. `PUBLIC_SUPABASE_URL` = `https://<project-ref>.supabase.co`
2. `PUBLIC_SUPABASE_ANON_KEY` = the `sb_publishable_...` key.
3. Add nothing else. Never add a secret that starts with `PUBLIC_`.

- **Check:** exactly these two variables are listed.

## B9. Ruleset for the `production` branch

- [ ] **Where:** GitHub, Settings, Rules, New branch ruleset.
1. Target branch `production`. Turn on **block deletions** and **block force pushes** only. Do **not** add "restrict updates" (it would block the deploy workflow's own push).

- **Check:** the ruleset is Active.

## B10. Turn on the API and web deploys

- [ ] **Where:** GitHub, Settings, Secrets and variables, Actions, Variables.
1. Add `DEPLOY_API` = `true` and `DEPLOY_WEB` = `true`. (Optional: `gh variable set DEPLOY_API --body true`, same for `DEPLOY_WEB`.)

- **Check:** three variables are listed: `SITE_URL`, `DEPLOY_API`, `DEPLOY_WEB`.

## B11. Merge and deploy everything

- [ ] **Where:** GitHub.
1. Squash and merge the `go-live` PR.
2. Actions, `deploy` run, approve as in A18.
3. The run now also validates the API config, deploys the API, then the `publish-web` job fast-forwards the `production` branch, Netlify builds, and the job waits until `<site-origin>/version.txt` equals the commit and runs the web smoke checks.

- **Check:** the whole run is green, including `publish-web`. `fly machines list -a famweather-api` shows exactly one machine. Open `<site-origin>` in a browser: the login page appears.
- **If it fails:** a `publish-web` rejection means `production` is not an ancestor of the release commit (for example when re-running an old run): re-run from the latest `main`. If Netlify's build fails, open its deploy log and tell the coding agent; the old site stays up.

## B12. Post-deploy checks

These are the manual checks from spec section 13. Replace the placeholders.

- [ ] **Where:** terminal.
1. Fly direct needs auth: `SITE_URL=<site-origin> DEPLOY_API=true ops/ci/smoke.sh` all lines `ok`, and `curl -i https://famweather-api.fly.dev/api/v1/stations` returns 401 or 403, never data (checks 1, 3, 4, 5).
2. Sign-up is off: `curl -s -X POST "https://<project-ref>.supabase.co/auth/v1/signup" -H "apikey: <sb_publishable key>" -H 'Content-Type: application/json' -d '{"email":"probe+delete-me@example.invalid","password":"<random 24 chars>"}'` returns `signup_disabled`. Repeat with `-d '{}'`: `anonymous_provider_disabled` or `signup_disabled`. If any call returns a user, delete that user in the dashboard now and fix A3 (check 6).
3. Data API is closed: `curl -s -o /dev/null -w '%{http_code}' "https://<project-ref>.supabase.co/rest/v1/credentials" -H "apikey: <sb_publishable key>" -H 'Accept-Profile: app'` is not 200 (check 7).
4. The publishable key is not a login: `curl -i -H "Authorization: Bearer <sb_publishable key>" <site-origin>/api/v1/me` returns 401 (check 8).
5. Machine counts: `fly machines list -a famweather-poller` and `-a famweather-api` show one machine each (check 9).
6. Key isolation: `fly secrets list -a famweather-api` has no `CREDENTIALS_PRIVATE_KEY` and no `SUPABASE_SERVICE_ROLE_KEY` (check 10).
7. healthchecks.io: `AWN_INGEST`, `BACKUP` and `DBSIZE` are green (check 13).

- **Check:** every line above matches. If one fails, stop and tell the coding agent before inviting anyone.

## B13. Your first login, and the signing-key check

- [ ] **Where:** terminal, then a browser.
1. Get a fresh link (the one from A19 has expired): run `ops/fly/run-oneoff.sh shell famweather-admin famweather-poller poller 15`, then `fly ssh console -a famweather-admin --machine <machine-id> --pty -C "/usr/local/bin/admin reset-link --email <your-email>"`. Press Enter in the first terminal when done.
2. Open the printed link in your browser (it opens `<site-origin>/set-password`). Choose a password of 12 or more characters. Sign in. Sign out and sign in again with it.
3. Test the password-change settings from A3: run `reset-link` again, open the link, and set a new password **without** entering the old one. Then, signed in, change your password from the account page.

- **Check:** you reach the dashboard and see your station. In the browser's developer tools (Network), the `/api/v1/me` request returns **200**. Your own station's data is shown.
- **If it fails:** if setting a new password asks for the current password, the "Require current password" switch in A3 is still on. Turn it off and repeat.

## B14. Disable the legacy Supabase keys

Only after B13 shows `/api/v1/me` returning 200.

- [ ] **Where:** Supabase, Project Settings.
1. API Keys, legacy tab: **disable** the legacy `anon` and `service_role` keys.
2. JWT Keys: revoke the legacy HS256 secret.
3. Re-run the smoke script and the checks in B12 items 2 to 4. Reload the site and sign in again.

- **Check:** login and your dashboard still work, and all the B12 checks still match.

## B15. Airport baselines

Baselines are the long-record airport stations your data is compared with.

- [ ] **Where:** terminal at the repo root.
1. For each airport: `ops/fly/run-oneoff.sh batch famweather-admin famweather-poller poller -- /usr/local/bin/admin add-reference-station --id <ghcn-id> --name "<airport name>" --lat <lat> --lon <lon> --timezone <iana-timezone> --std-offset-minutes <minutes>`
   - GHCN-Daily ids are in `docs/guides/04-database.md` section 5 (the seed baselines).
2. In the web app, open your station and confirm the baseline.

- **Check:** each run prints `exit code: 0` and leaves no `oneoff-*` machine (`fly machines list -a famweather-admin`). The station page shows the baseline.

## B16. Invite the family

- [ ] **Where:** two terminals (same as A19 and B13). Do this when the person is ready, because the link expires in about an hour.
1. `ops/fly/run-oneoff.sh shell famweather-admin famweather-poller poller 15`
2. `fly ssh console -a famweather-admin --machine <machine-id> --pty -C "/usr/local/bin/admin invite --email <their-email> --display-name \"<their-name>\" --follow-all"`
3. Send the printed link privately (Signal, or a message to the right number, or in person). Never put it in a ticket or a file.
4. Their station: A20 again, with `--owner <their-name>`, or let them connect it in the web app.
5. Forgotten password later: the same shell, but `admin reset-link --email <their-email>`.

- **Check:** they open the link, choose a password and see the dashboard. `list-users` shows them.

**Go-live is done when:** every check in B12 matches, the healthchecks stay green for 24 hours, and you have done one restore drill (below).

---

# After go-live

## Restore drill (every 3 months; a calendar reminder)

A drill proves the backups can be restored. It copies family data onto your laptop, so you delete it at the end. This is spec section 15.6.

- [ ] **Where:** terminal on your laptop, using the AWS restore identity (profile `<restore-profile>`).
1. List and download the newest pair: `aws s3 ls s3://<bucket>/weather/ --recursive --profile <restore-profile> | tail`, then `aws s3 cp s3://<bucket>/weather/<yyyy>/<mm>/app-<ts>-<rand>.dump.age . --profile <restore-profile>` and the matching `auth-users-<ts>-<rand>.csv.age`.
2. Decrypt with the key from your password manager (save it as `weather-backup.key` just for now): `age -d -i weather-backup.key -o app.dump app-<ts>-<rand>.dump.age` and the same for `auth-users.csv`.
3. Find the schema version: `pg_restore --data-only --table=_sqlx_migrations -f - app.dump` (highest `version`). Check out the commit whose `backend/migrations/` ends at that version.
4. Start a local database in production order: `docker compose up -d db auth`, `docker compose run --rm auth-access`, `docker compose run --rm migrate`.
5. Restore the data (skipping the migrations table): `pg_restore -l app.dump | grep -v _sqlx_migrations > list.txt`, then `pg_restore --data-only --single-transaction --use-list list.txt --dbname "<local app_migrator url>" app.dump`.
6. Restore the logins: `admin restore-auth-users --dry-run --file auth-users.csv`, then without `--dry-run` (spec 15.6 step 5 has the details).
7. Verify: row counts for `observations` and `daily_rollups`; the newest observation is within 36 hours of the backup; `list-users` matches the dashboard count. Write the date, the backup timestamp, how long it took and any problems in `docs/ops-log.md` (create it on the first drill).
8. **Delete everything:** `docker compose down -v`, remove `app.dump`, `auth-users.csv`, the `.age` files, `list.txt` and `weather-backup.key` (the key stays in the password manager).

- **Check:** `docker volume ls` and `ls` show no leftovers. A failed drill is a priority bug: tell the coding agent.

## When something goes wrong

| Symptom | Go to |
| --- | --- |
| A station shows "needs reconnecting", or one owner's data stops | 06 section 15.2 |
| `AWN_INGEST` check goes red or the poller is silent | 06 section 15.3 |
| Supabase paused, or `DBSIZE` fails (database near 400 MB) | 06 section 15.4 |
| A key, password or token may have leaked | 06 section 15.5 (rotate first, investigate second; recipes R1 to R9 in section 12) |
| Backups fail or you must restore for real | `fly logs -a famweather-backup`, then the drill above |

Other upkeep: GitHub emails you if the weekly exposure check stops after 60 quiet days (re-enable it in the Actions tab); Dependabot opens monthly update pull requests (review each one, no auto-merge).

---

# First-run verification items

These can only be checked with your real accounts. Do each once, and tell the coding agent the result (pass or the exact error, without secrets).

- [ ] **V1. Fly config files.** (A10, A16) `fly config validate --config ops/fly/poller.toml --strict`, and the same for `ops/fly/api.toml` and `ops/backup/fly.toml`. Expect "valid" each time.
- [ ] **V2. Dockerfile path.** (A16) From the repo root: `fly deploy backend --config ops/fly/api.toml --dockerfile backend/Dockerfile --build-only`. Expect a finished build. If it cannot find the Dockerfile, retry with `--dockerfile Dockerfile` and tell the coding agent to change it in `.github/workflows/deploy.yml` (poller and api steps) and the spec section 4.7.
- [ ] **V3. One-off dry run.** (after A18) `time ops/fly/run-oneoff.sh batch famweather-poller famweather-poller poller -- /usr/local/bin/admin --version; echo "exit=$?"`. The command only prints its version and touches no database. Expect: the logs show `admin 0.1.0`, `exit code: 0`, `exit=0`, and `fly machines list -a famweather-poller` shows no `oneoff-*` machine afterwards. If the exit code is wrong or a machine is left, run `fly machine destroy <id> -a famweather-poller --force` and tell the coding agent (the `fly machine wait` and `jq` parts are unverified).
- [ ] **V4. Site address file.** (A16, before A19) `cat ops/fly/site-url.txt` prints exactly one line, your `<site-origin>` (https, no path). `run-oneoff.sh` refuses to start admin one-offs without it (it prints a message saying to create the file).
- [ ] **V5. First pull request run.** (A16) The `ci-ok` check is green on the first PR. Also open the run and confirm the jobs that ran (and the ones skipped) look right.
- [ ] **V6. S3 conditional writes and the bucket policy.** (after B5) Use the uploader key in a temporary shell, never saved to disk:
  ```sh
  read -rs AWS_ACCESS_KEY_ID; read -rs AWS_SECRET_ACCESS_KEY; export AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY AWS_DEFAULT_REGION=us-west-2
  echo probe > probe.txt
  aws s3api put-object --bucket <bucket> --key weather/probe/probe.txt --body probe.txt --if-none-match '*'
  aws s3api put-object --bucket <bucket> --key weather/probe/probe.txt --body probe.txt --if-none-match '*'
  aws s3api put-object --bucket <bucket> --key weather/probe/probe2.txt --body probe.txt
  unset AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY; rm -f probe.txt
  ```
  Expect: the first succeeds, the second returns **412 PreconditionFailed**, the third returns **403 AccessDenied**. (The probe object expires with the 90-day rule.) If the first call is denied or the daily backup gets a 403, the policy condition name or the AWS CLI version is the suspect: tell the coding agent.

---

# Notes for the coding agent

Resolved: the earlier mismatches between this runbook, the built files and guide 06 were fixed in both places. Still open:

1. `make keygen` and the dry-run command (`admin --version`) were checked locally only. V1 to V6 above still need real accounts.
