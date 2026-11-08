# Production runbook (Rodney only)

Step-by-step instructions to put the Family Weather app into production. You are the only person who runs these steps. Coding agents never run production commands.

This is the friendly version of [`docs/guides/06-deploy-ops.md`](guides/06-deploy-ops.md), the full spec. Where this page and the spec disagree, this page follows the files in the repo (the list is at the end).

**Rules for yourself**

- Real secrets live in 1Password (vault `Development`) and reach commands through `op read`. Never in this repo, never in chat, never in a ticket. The repo is public.
- Commands are copy-and-run: nothing to edit. They use the variables from "Every terminal session" below.
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

- [ ] 1Password, with the CLI (`op`) signed in.
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
| `op` (1Password CLI) | `brew install 1password-cli`, then turn on "Integrate with 1Password CLI" in the 1Password app | `op vault list` |
| `flyctl` (also gives `fly`) | `brew install flyctl` | `fly version` |
| `gh` | `brew install gh` | `gh auth status` |
| `age` | `brew install age` | `age --version` and `age-keygen --version` |
| `psql` 15 or newer (`roles.sql` uses `\getenv`; Homebrew's `postgresql@14` is too old) | `brew install libpq`, then `printf '%s\n' 'export PATH=/opt/homebrew/opt/libpq/bin:$PATH' >> ~/.zshrc` and open a new terminal | `psql --version` |
| `jq` (`run-oneoff.sh` needs it; it also needs the `fly` command name) | `brew install jq` | `jq --version` |
| `python3` (makes password verifiers) | already on macOS | `python3 --version` |
| `openssl` | already on macOS | `openssl version` |
| Docker Desktop (`make keygen`, Stage A) | docker.com | `docker --version` |
| Node (version in `web/.nvmrc`, Stage B only) | `nvm install` in `web/` | `node --version` |
| AWS CLI v2 (restore drill and the S3 check) | `brew install awscli` | `aws --version` |

- [ ] All checks print a version.
- [ ] `fly auth login` opens the browser; `fly auth whoami` shows you.

### Every terminal session

Run this first in every new terminal. It sets the project values (not secret, committed in `ops/prod.env`: `PROJECT_REF`, `POOLER_HOST`, `SITE_ORIGIN`, `SUPABASE_URL`, `OP_VAULT`, `POSTGRES_POOLER_URL`) and unlocks 1Password:

```sh
. ops/prod.env && op vault list >/dev/null && echo "ready: $PROJECT_REF"
```

Some values are not in 1Password or must never be committed. Set each one **in the session** when a step first needs it; each step says which. Secrets use `read -rs` (hidden, and kept out of your shell history); press Enter after pasting:

```sh
read -rs AWN_APPLICATION_KEY && export AWN_APPLICATION_KEY        # secret: Ambient Application Key (A12, B6)
export MY_EMAIL='...' MY_NAME='...'                               # you (A19, B13)
export STATION_MAC='...' STATION_NAME='...' STATION_LAT='...' STATION_LON='...'
export STATION_TZ='America/Denver' STATION_STD_OFFSET='-420'      # your time zone; standard-time offset in minutes (A20)
```

### Where every value lives

`op://` paths are in vault `Development` (written `$OP_VAULT` in commands). "session" means a variable you set as above.

| Name | Source | Where it goes |
| --- | --- | --- |
| `postgres` database password | `op://…/AWN-Weather-Postgres/password` | bootstrap and emergencies only (A7) |
| `app_migrator` password | `op://…/App Migrator/password` | admin `DATABASE_URL`, `MIGRATOR_DATABASE_URL` |
| `app_api` password | `op://…/App API/password` | api `DATABASE_URL` |
| `app_poller` password | `op://…/App Poller/password` | poller `DATABASE_URL` |
| `app_backup` password | `op://…/App Backup PW/password` | `BACKUP_DATABASE_URL` |
| `app_ci_check` password | `op://…/App CI Check/password` | `EXPOSURE_DATABASE_URL` |
| Project ref, pooler host, site origin | `ops/prod.env` (public by design) | URLs below; the ref is also committed in `ops/fly/api.toml` and `web/security-headers.json` at B7 |
| `PUBLIC_SUPABASE_ANON_KEY` (`sb_publishable_...`) | `op://…/Supabase Publishable Key/credential` | Netlify (public by design) |
| `SUPABASE_SERVICE_ROLE_KEY` (`sb_secret_...`) | `op://…/Supabase Secret Key/password` | Fly `integrated-awn-weather-admin` only |
| `AWN_APPLICATION_KEY` | session `$AWN_APPLICATION_KEY` | Fly poller and api |
| `CREDENTIALS_PUBLIC_KEY` | `op://…/Admin Public/credential` (A11) | Fly admin and api |
| `CREDENTIALS_PRIVATE_KEY` | `op://…/Admin Privat/credential` (A11) | Fly poller only |
| `CREDENTIALS_KEY_VERSION` | the value `1` | Fly poller, admin, api |
| `HEALTHCHECK_URL_AWN_INGEST` | `op://…/AWN Ingest Health Check/url` | Fly poller |
| `HEALTHCHECK_URL_BACKUP`, `HEALTHCHECK_URL_DBSIZE` | session (B3) | Fly backup |
| `FLY_API_TOKEN_POLLER`, `_API`, `_BACKUP` | `op://…/FLY_API_TOKEN_POLLER/password` (and `_API`, `_BACKUP`) | GitHub environment `production` |
| `MIGRATOR_DATABASE_URL`, `EXPOSURE_DATABASE_URL` | built from the passwords above (A15) | GitHub environments |
| Backup uploader key pair | session `UPLOADER_AWS_ACCESS_KEY_ID`, `UPLOADER_AWS_SECRET_ACCESS_KEY` (B1) | Fly backup |
| `BACKUP_S3_BUCKET` | session (B1) | Fly backup, restore drill |
| `BACKUP_AGE_RECIPIENT` (`age1...`, public) | session (B2) | Fly backup |
| age private key | 1Password document `Backup age key` (B2) plus a printed copy | never online |
| AWS restore identity (user with MFA) | AWS CLI profile, session `RESTORE_PROFILE` | you only; never Fly or GitHub |
| `SITE_URL` | `$SITE_ORIGIN` | GitHub repo variable; `ops/fly/site-url.txt` |
| `DEPLOY_API`, `DEPLOY_WEB` | the value `true` | GitHub repo variables (Stage B) |

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
7. Put the site address (`https://<site-name>.netlify.app`, no trailing slash) in `ops/prod.env` as `SITE_ORIGIN`.

- **Check:** the site exists. Its first build **failing or being empty is expected** (the project ref is not filled in yet).

## A2. Supabase project

- [ ] **Where:** Supabase dashboard.
1. New project, free plan. Region: do **not** take the general "Americas" choice (it places the project in Virginia). Choose the **specific region West US (North California)**, which is `us-west-1` and sits next to Fly's `sjc` (San Jose) region used in the Fly files. Fly has no Oregon region, so `us-west-2` would be farther away. ([Supabase: regions](https://supabase.com/docs/guides/platform/regions), [Fly: regions](https://docs.fly.io/reference/regions))
2. Choose a long database password and save it as the `password` of the 1Password item `AWN-Weather-Postgres`. Do not use it anywhere else.
3. Put the project ref (the part of the project URL before `.supabase.co`) in `ops/prod.env` as `PROJECT_REF`.
4. Turn on MFA for your Supabase account.

- **Check:** the project shows as healthy, and Project Settings, General shows the region as West US (North California).

## A3. Supabase sign-in settings

- [ ] **Where:** Supabase, Authentication.
1. Sign In / Providers: **turn off "Allow new users to sign up"**. **Turn off anonymous sign-ins**. Email provider **on** (this is what allows password login). "Confirm email" on. Phone and all social providers off.
2. Email OTP expiration: **3600** seconds (this is how long a set-password link lives).
3. Minimum password length: **12**.
4. Email provider settings: turn **off** "Require reauthentication when changing password" (older name: "Secure password change") and **off** "Require current password when changing password". Write down what you found. ([Supabase: password security](https://supabase.com/docs/guides/auth/password-security))
5. SMTP: leave custom SMTP **unconfigured**. Nothing in this app sends email.
6. URL Configuration: Site URL = your site origin (`echo $SITE_ORIGIN`). Leave **Redirect URLs empty**.

- **Check:** read the page back; each setting matches.
- **If it fails:** if either switch in item 4 cannot be turned off, or the minimum length cannot be raised, **stop** and tell the coding agent. There is a documented fallback design (proposal 05 option b1).

## A4. Supabase keys, Data API and SSL

- [ ] **Where:** Supabase dashboard (the page for each item is named).
1. Project Settings, **JWT Signing Keys**: **nothing to change.** New projects already sign with an ES256 (elliptic curve, P-256) key; there is no "Migrate" step to do. Just look: the current key should be the elliptic-curve one. Leave any legacy secret alone until B14. (Checked Nov 1, 2026 against this project's public key list, `$SUPABASE_URL/auth/v1/.well-known/jwks.json`, which shows one `ES256` key. [Supabase: JWT signing keys](https://supabase.com/docs/guides/auth/signing-keys) describes the migration only for older projects.)
2. Project Settings, **API Keys**: the new-style keys already exist. Save `sb_publishable_...` as the `credential` of 1Password item `Supabase Publishable Key`, and `sb_secret_...` as the `password` of `Supabase Secret Key`. Do not touch the legacy tab yet.
3. **Integrations, Data API**: turn **off** "Enable Data API". (If it cannot be turned off, expose no schemas except an empty `public`, and confirm `app` and `app_auth` are not exposed.) ([Supabase: hardening the Data API](https://supabase.com/docs/guides/database/hardening-data-api))
4. Database, Settings, **SSL Configuration**: turn **on** "Enforce SSL on incoming connections". The database restarts for a few seconds. ([Supabase: SSL enforcement](https://supabase.com/docs/guides/platform/ssl-enforcement))
5. **Connect** button (top of the project page), Session pooler: copy the host (`aws-<n>-us-west-1.pooler.supabase.com`) into `ops/prod.env` as `POOLER_HOST`. It cannot be guessed from the region, so copy it.

- **Check:** each toggle reads back as set.

## A5. Download the database CA certificate

- [ ] **Where:** Supabase, Database settings, SSL configuration, then terminal.
1. Download the CA certificate.
2. `mv ~/Downloads/prod-ca-2021.crt backend/certs/supabase-ca.crt` (use the downloaded file's name if it differs).

- **Check:** `head -1 backend/certs/supabase-ca.crt` prints `-----BEGIN CERTIFICATE-----`. You commit it at A16.

## A6. Make the five database passwords

- [ ] **Where:** 1Password, then a terminal at the repo root. This shell stays open through A8.
1. In the 1Password vault `Development`, create five Login items, each with a `password` field made by `openssl rand -hex 24` (letters and digits only, so they work in connection URLs): `App Migrator`, `App API`, `App Poller`, `App Backup PW`, `App CI Check`.
2. Make the verifiers (a scrambled form of each password that the SQL needs). Each password goes straight from 1Password into `scram.py`, so it is never shown or kept in a shell variable:
   ```sh
   export APP_MIGRATOR_PASSWORD_VERIFIER="$(op read "op://$OP_VAULT/App Migrator/password" | python3 ops/db/bootstrap/scram.py)"
   export APP_API_PASSWORD_VERIFIER="$(op read "op://$OP_VAULT/App API/password" | python3 ops/db/bootstrap/scram.py)"
   export APP_POLLER_PASSWORD_VERIFIER="$(op read "op://$OP_VAULT/App Poller/password" | python3 ops/db/bootstrap/scram.py)"
   export APP_BACKUP_PASSWORD_VERIFIER="$(op read "op://$OP_VAULT/App Backup PW/password" | python3 ops/db/bootstrap/scram.py)"
   export APP_CI_CHECK_PASSWORD_VERIFIER="$(op read "op://$OP_VAULT/App CI Check/password" | python3 ops/db/bootstrap/scram.py)"
   ```

- **Check:** `echo "$APP_API_PASSWORD_VERIFIER" | cut -c1-14` prints `SCRAM-SHA-256$`.

## A7. Create the database roles and the login-check functions

- [ ] **Where:** the same terminal. Never use the Supabase SQL editor for this (it stores history).
1. Run both bootstrap scripts as `postgres` over the session pooler (the password comes from 1Password for these two commands only):
   ```sh
   PGPASSWORD="$(op read -n "op://$OP_VAULT/AWN-Weather-Postgres/password")" psql -X -v ON_ERROR_STOP=1 -f ops/db/bootstrap/roles.sql "$POSTGRES_POOLER_URL"
   PGPASSWORD="$(op read -n "op://$OP_VAULT/AWN-Weather-Postgres/password")" psql -X -v ON_ERROR_STOP=1 -f ops/db/bootstrap/auth_access.sql "$POSTGRES_POOLER_URL"
   ```
   `roles.sql` creates the five roles; `auth_access.sql` creates the `app_auth` functions the app uses to check sessions.
2. Clear the shell: `unset APP_MIGRATOR_PASSWORD_VERIFIER APP_API_PASSWORD_VERIFIER APP_POLLER_PASSWORD_VERIFIER APP_BACKUP_PASSWORD_VERIFIER APP_CI_CHECK_PASSWORD_VERIFIER`

- **Check:** both commands end without an error. If `verify-full` rejects the host name, that is the first real test of Open question 11 in the spec: tell the coding agent. Do not fall back to `require`.
- **If it fails:** the direct host `db.$PROJECT_REF.supabase.co` is IPv6 only, which is why this step uses the pooler. `roles.sql` is safe to repeat, and each run resets the five passwords to what 1Password holds.

## A8. Check the Supabase settings from outside

- [ ] **Where:** terminal at the repo root.
1. `PUBLIC_SUPABASE_ANON_KEY="$(op read -n "op://$OP_VAULT/Supabase Publishable Key/credential")" sh ops/db/checks/supabase_smoke.sh; echo "exit=$?"`

- **Check:** every line says PASS and `exit=0`. The table checks return 503 because the Data API is off. If it fails, a setting in A3 or A4 is wrong. Fix it and run again.

## A9. healthchecks.io

- [ ] **Where:** healthchecks.io.
1. New project `family-weather`. Add your email plus a second notification channel.
2. Add a check named `AWN_INGEST`: period **5 minutes**, grace **15 minutes**.
3. Save its ping URL in 1Password as the `url` of a Secure Note named `AWN Ingest Health Check`.

- **Check:** the check exists and shows "new". The BACKUP and DBSIZE checks come in Stage B.

## A10. Fly apps

- [ ] **Where:** terminal at the repo root, logged in (`fly auth whoami`).
1. `for a in poller api admin backup; do fly apps create integrated-awn-weather-$a; done`
2. Validate the three config files (this is item V1 in "First-run verification"): `for c in ops/fly/poller.toml ops/fly/api.toml ops/backup/fly.toml; do fly config validate --config $c --strict; done`

- **Check:** `fly apps list` shows all four. Each validate prints that the configuration is valid.
- **If it fails:** if a name is taken, pick another and ask the coding agent to rename it everywhere (`grep -rn integrated-awn-weather-`). See spec section 3.1.

## A11. Make the credential key pair (`admin keygen`)

This pair encrypts station API keys. The private half goes to the poller only.

- [ ] **Where:** terminal at the repo root. No network is needed after the build.
1. `make keygen` (first run builds the backend image, which takes several minutes). It starts only a throwaway admin container, with no database and nothing saved.
2. It prints `CREDENTIALS_PUBLIC_KEY=...` and `CREDENTIALS_PRIVATE_KEY=...`. Save them in 1Password as the `credential` of API Credential items `Admin Public` and `Admin Privat`. The key version is `1`.
3. Clear the screen: `clear`.

- **Check:** both items exist (`op read -n "op://$OP_VAULT/Admin Public/credential" >/dev/null && echo ok`). You will test that they match at A12.

## A12. Poller secrets (private file, checked, deleted)

- [ ] **Where:** terminal. The file is written **outside** the repo, in your home folder, readable only by you. Needs `AWN_APPLICATION_KEY` set in the session.
1. Write the file from 1Password:
   ```sh
   : "${AWN_APPLICATION_KEY:?set it first: read -rs AWN_APPLICATION_KEY && export AWN_APPLICATION_KEY}"
   umask 077; cat > ~/fw-poller.env <<EOF
   DATABASE_URL=postgresql://app_poller:$(op read -n "op://$OP_VAULT/App Poller/password")@db.$PROJECT_REF.supabase.co:5432/postgres?sslmode=verify-full&sslrootcert=/etc/ssl/app-certs/supabase-ca.crt
   AWN_APPLICATION_KEY=$AWN_APPLICATION_KEY
   CREDENTIALS_PRIVATE_KEY=$(op read -n "op://$OP_VAULT/Admin Privat/credential")
   CREDENTIALS_KEY_VERSION=1
   HEALTHCHECK_URL_AWN_INGEST=$(op read -n "op://$OP_VAULT/AWN Ingest Health Check/url")
   EOF
   ```
2. Check it: `grep -c '=$' ~/fw-poller.env` prints `0` (no empty values), and `ops/ci/check-demo-values.sh ~/fw-poller.env; echo "exit=$?"` prints only `exit=0`. A "demo value found" message means a local demo value got into 1Password.
3. `fly secrets import -a integrated-awn-weather-poller --stage < ~/fw-poller.env && rm -f ~/fw-poller.env`
4. `fly secrets list -a integrated-awn-weather-poller`

- **Check:** the list shows the five names (digests only, no values) and the file is gone (`ls ~/fw-poller.env` says no such file).

## A13. Admin secrets (same method)

- [ ] **Where:** terminal.
1. Write the file:
   ```sh
   umask 077; cat > ~/fw-admin.env <<EOF
   DATABASE_URL=postgresql://app_migrator:$(op read -n "op://$OP_VAULT/App Migrator/password")@db.$PROJECT_REF.supabase.co:5432/postgres?sslmode=verify-full&sslrootcert=/etc/ssl/app-certs/supabase-ca.crt
   SUPABASE_URL=$SUPABASE_URL
   SUPABASE_SERVICE_ROLE_KEY=$(op read -n "op://$OP_VAULT/Supabase Secret Key/password")
   CREDENTIALS_PUBLIC_KEY=$(op read -n "op://$OP_VAULT/Admin Public/credential")
   CREDENTIALS_KEY_VERSION=1
   EOF
   ```
2. Check and import as in A12: `grep -c '=$' ~/fw-admin.env` prints `0`; `ops/ci/check-demo-values.sh ~/fw-admin.env; echo "exit=$?"` prints `exit=0`; then `fly secrets import -a integrated-awn-weather-admin --stage < ~/fw-admin.env && rm -f ~/fw-admin.env`.

- **Check:** `fly secrets list -a integrated-awn-weather-admin` shows the five names. The admin app deliberately has no `AWN_APPLICATION_KEY`.

## A14. Fly deploy tokens

- [ ] **Where:** terminal. You need all three now, because every deploy validates all three config files.
1. For each of `poller`, `api`, `backup`: `fly tokens create deploy -a integrated-awn-weather-poller` (and so on), and save the output as the `password` of 1Password Login items `FLY_API_TOKEN_POLLER`, `FLY_API_TOKEN_API`, `FLY_API_TOKEN_BACKUP`.

- **Check:** `for n in POLLER API BACKUP; do a=$(echo $n | tr A-Z a-z); FLY_API_TOKEN="$(op read -n "op://$OP_VAULT/FLY_API_TOKEN_$n/password")" fly status -a integrated-awn-weather-$a >/dev/null && echo "$a ok"; done` prints three `ok` lines.

## A15. GitHub settings and environments

- [ ] **Where:** terminal (`gh auth status` shows you). `R=murrayrodney/weather-app` in each block.
1. Make the repo public, then turn on the security features (on a free private repo most of them are not offered):
   ```sh
   R=murrayrodney/weather-app
   gh repo edit $R --visibility public --accept-visibility-change-consequences
   gh api -X PATCH repos/$R --input - <<'EOF'
   {"security_and_analysis":{"secret_scanning":{"status":"enabled"},"secret_scanning_push_protection":{"status":"enabled"}}}
   EOF
   gh api -X PUT repos/$R/vulnerability-alerts
   gh api -X PUT repos/$R/automated-security-fixes
   gh api -X PUT repos/$R/private-vulnerability-reporting
   gh api -X PUT repos/$R/actions/permissions/fork-pr-contributor-approval -f approval_policy=all_external_contributors
   gh api -X PUT repos/$R/actions/permissions/workflow -f default_workflow_permissions=read
   ```
2. Environments: `production` (you as required reviewer, self-review allowed, `main` only) and `ci-readonly` (`main` only, no reviewers). GitHub may already have created either one when a run started; these commands update it in place:
   ```sh
   R=murrayrodney/weather-app; me=$(gh api user --jq .id)
   gh api -X PUT repos/$R/environments/production --input - <<EOF
   {"reviewers":[{"type":"User","id":$me}],"prevent_self_review":false,"deployment_branch_policy":{"protected_branches":false,"custom_branch_policies":true}}
   EOF
   gh api -X PUT repos/$R/environments/ci-readonly --input - <<'EOF'
   {"deployment_branch_policy":{"protected_branches":false,"custom_branch_policies":true}}
   EOF
   for e in production ci-readonly; do gh api -X POST repos/$R/environments/$e/deployment-branch-policies -f name=main -f type=branch; done
   ```
3. Secrets, straight from 1Password (the database URLs must end exactly with `?sslmode=verify-full`; the workflow appends the certificate path):
   ```sh
   R=murrayrodney/weather-app
   for n in FLY_API_TOKEN_POLLER FLY_API_TOKEN_API FLY_API_TOKEN_BACKUP; do
     op read -n "op://$OP_VAULT/$n/password" | gh secret set $n -R $R --env production
   done
   mig() { printf 'postgresql://app_migrator.%s:%s@%s:5432/postgres?sslmode=verify-full' "$PROJECT_REF" "$(op read -n "op://$OP_VAULT/App Migrator/password")" "$POOLER_HOST"; }
   exp() { printf 'postgresql://app_ci_check.%s:%s@%s:5432/postgres?sslmode=verify-full' "$PROJECT_REF" "$(op read -n "op://$OP_VAULT/App CI Check/password")" "$POOLER_HOST"; }
   mig | gh secret set MIGRATOR_DATABASE_URL -R $R --env production
   exp | gh secret set EXPOSURE_DATABASE_URL -R $R --env production
   exp | gh secret set EXPOSURE_DATABASE_URL -R $R --env ci-readonly
   gh variable set SITE_URL -R $R --body "$SITE_ORIGIN"
   ```
   Leave `DEPLOY_API` and `DEPLOY_WEB` unset for now.

- **Check:** `gh secret list -R murrayrodney/weather-app --env production` shows five secrets, `--env ci-readonly` shows one, and `gh variable list -R murrayrodney/weather-app` shows `SITE_URL`.

## A16. Commit the certificate and site address, open the pull request

The deploy files themselves are already on `main`; this pull request adds only your two project-specific files.

- [ ] **Where:** terminal at the repo root.
1. Start a branch from the latest `main`: `git checkout main && git pull && git checkout -b go-live-files`.
2. Create the site address file (admin one-offs read it): `printf '%s\n' "$SITE_ORIGIN" > ops/fly/site-url.txt`.
3. Run the two Fly checks from "First-run verification" **V1 and V2** now. If V2 shows the `--dockerfile` path is wrong, tell the coding agent before continuing.
4. `git status`. It should show exactly two new files: `backend/certs/supabase-ca.crt` and `ops/fly/site-url.txt`, and **no** `.env` or secrets files. Then `git add backend/certs/supabase-ca.crt ops/fly/site-url.txt && git commit -m "Add the database CA certificate and site address"` and `git push -u origin go-live-files`.
5. `gh pr create --base main --fill`

- **Check:** the PR's `ci-ok` check turns **green** (this is the first ever run of the workflows: item V5). It can take 20 to 25 minutes.
- **If it fails:** read the failing job's log. Do not paste logs with hostnames into public places. Tell the coding agent which job failed.

## A17. Protect `main`, then merge

- [ ] **Where:** GitHub, Settings, Rules, New branch ruleset.
1. Target `main`. Require a pull request. Require status check **`ci-ok`** (and nothing else: the other job names never report when skipped). Block force pushes and deletion.
2. If you turned the deploy workflow off earlier, turn it back on: `gh workflow enable deploy.yml`.
3. Merge the PR with **Squash and merge** (the workflow compares to the previous commit): `gh pr merge --squash`.

- **Check:** the merge starts the `deploy` workflow on `main`.

## A18. Approve the first deploy

- [ ] **Where:** GitHub, Actions, the `deploy` run.
1. Wait for the `deploy` job to show "Waiting for review". Click Review deployments, tick `production`, Approve and deploy.
2. The job runs: validate configs, migrate the database, deploy the poller, check machine counts, run the exposure check and the smoke test. The API and web steps are skipped.
3. `fly status -a integrated-awn-weather-poller` and `fly logs -a integrated-awn-weather-poller` (do not paste the logs anywhere).

- **Check:** the deploy run is green. The poller shows one machine, started. `fly machines list -a integrated-awn-weather-poller` lists exactly one.
- **If it fails:** the log names the step. If the migrate step cannot connect, re-run A15 step 3. If the exposure step fails, the same. Fix and re-run the failed workflow.
- Now do **V3** (the one-off dry run), which proves the tool used for the next steps.

## A19. Create your account

You are the admin. The website is not live yet, so the link this prints cannot be opened yet. The account is created now so your station has an owner; you get a fresh link at B13.

- [ ] **Where:** two terminals at the repo root, both after "Every terminal session". Needs `MY_EMAIL` and `MY_NAME` in terminal 2.
`run-oneoff.sh` only accepts two app combinations, `integrated-awn-weather-poller integrated-awn-weather-poller poller` and `integrated-awn-weather-admin integrated-awn-weather-poller poller`; every command in this runbook uses one of them. In batch mode it refuses any argument named `invite` or `reset-link`.

1. Terminal 1: `ops/fly/run-oneoff.sh shell integrated-awn-weather-admin integrated-awn-weather-poller poller 15`. It starts a machine, then waits. Leave it open.
2. Terminal 2, pick up that machine and define a shortcut for running admin on it:
   ```sh
   export MACHINE_ID=$(fly machines list -a integrated-awn-weather-admin --json | jq -r '.[] | select(.name | startswith("oneoff-")) | .id')
   admin() { fly ssh console -a integrated-awn-weather-admin --machine "$MACHINE_ID" --pty -C "/usr/local/bin/admin $*"; }
   echo "machine: $MACHINE_ID"
   ```
3. `admin invite --email "$MY_EMAIL" --display-name "\"$MY_NAME\"" --admin --follow-all`
4. `admin list-users`
5. Keep the session open for A20 (15 minutes in total), then press Enter in terminal 1.

- **Check:** `invite` prints an id and a set-password link (ignore the link for now); `list-users` shows you. After pressing Enter in terminal 1, `fly machines list -a integrated-awn-weather-admin` shows no machine.
- **If it fails:** if `fly ssh console -C` does not work on this image, stop and tell the coding agent. There is no batch-mode fallback for `invite`: `run-oneoff.sh` refuses it on purpose so the link never reaches Fly's logs.

## A20. Connect your station

- [ ] **Where:** terminal 2, inside the same 15-minute session (run A19 steps 1 and 2 again if it has ended). Needs the `STATION_*` variables.
1. `admin add-station --owner "\"$MY_NAME\"" --mac "$STATION_MAC" --name "\"$STATION_NAME\"" --lat "$STATION_LAT" --lon "$STATION_LON" --timezone "$STATION_TZ" --std-offset-minutes "$STATION_STD_OFFSET"`
   - `STATION_STD_OFFSET` is your **standard-time** offset from UTC in minutes (ignore daylight saving; negative west of Greenwich, for example `-420` for Mountain time).
2. It asks for your station's **API Key** from your Ambient Weather account. Type or paste it at the hidden prompt.
3. It prints the new station id (a UUID): `export STATION_UUID=<the printed id>` for A21.

- **Check:** a line like `<uuid> created`. Within a few minutes `fly logs -a integrated-awn-weather-poller` shows ingest activity.
- **If it fails:** the key is not checked against Ambient in production, so a typo in the MAC or key only shows up as ingest errors in the logs. Run the command again with the right values; it reports "already exists (unchanged)" for a repeat, so ask the coding agent about `--replace-key` before fixing a wrong key.

## A21. Pull the history (`awn_backfill`)

Your station has history back to June 2023. This copies it into the database. It is safe to repeat.

- [ ] **Where:** terminal at the repo root. Do not run it while a deploy is running. Needs `STATION_UUID` from A20.
1. `ops/fly/run-oneoff.sh batch integrated-awn-weather-poller integrated-awn-weather-poller poller -- /usr/local/bin/poller run-job awn_backfill --station "$STATION_UUID"`
2. Leave the terminal open. The script waits up to an hour and prints the logs and an exit code at the end.

- **Check:** `exit code: 0`, and `fly machines list -a integrated-awn-weather-poller` shows only the one poller machine afterwards (the script destroys its own machine).
- **If it fails:** run it again. Backfill takes the same rate limits as the live poller, so it is slow, not broken. If the exit code is non-zero twice, see 06 section 15.2 and tell the coding agent.

## A22. Watch ingest go green

- [ ] **Where:** healthchecks.io.
1. Open the `AWN_INGEST` check.

- **Check:** it turns green within about 15 minutes of A20 and stays green. If it does not, see "When something goes wrong".

**Stage A is done.** The poller is recording. You can stop here for a day.

---

# Stage B: the website and API go live

## B1. AWS S3 bucket and users (backups)

- [ ] **Where:** AWS console as your IAM admin user, plus a terminal for the policy text.
1. Pick the bucket name and keep it for the session: `export BACKUP_S3_BUCKET=famweather-backups-$(openssl rand -hex 4); echo $BACKUP_S3_BUCKET`. Note it in 1Password too (it is not secret, but you need it for every restore).
2. S3, create that bucket in `us-west-2`: Block Public Access all on, ACLs disabled, default encryption SSE-S3, **versioning on**.
3. Management, lifecycle rule on prefix `weather/`: expire current versions after **90 days**; permanently delete noncurrent versions after **7 days**; remove expired delete markers; abort incomplete multipart uploads after 7 days.
4. Permissions, bucket policy. Print it with your bucket name filled in and paste it into the console:
   ```sh
   cat <<EOF
   {
     "Version": "2012-10-17",
     "Statement": [
       { "Sid": "DenyInsecureTransport", "Effect": "Deny", "Principal": "*", "Action": "s3:*",
         "Resource": ["arn:aws:s3:::$BACKUP_S3_BUCKET", "arn:aws:s3:::$BACKUP_S3_BUCKET/*"],
         "Condition": { "Bool": { "aws:SecureTransport": "false" } } },
       { "Sid": "RequireIfNoneMatch", "Effect": "Deny", "Principal": "*", "Action": "s3:PutObject",
         "Resource": "arn:aws:s3:::$BACKUP_S3_BUCKET/weather/*",
         "Condition": { "Null": { "s3:if-none-match": "true" } } }
     ]
   }
   EOF
   ```
5. IAM, new user `integrated-awn-weather-backup-uploader`: no console access, one access key, inline policy:
   ```sh
   cat <<EOF
   {
     "Version": "2012-10-17",
     "Statement": [
       { "Sid": "UploadOnly", "Effect": "Allow", "Action": "s3:PutObject",
         "Resource": "arn:aws:s3:::$BACKUP_S3_BUCKET/weather/*",
         "Condition": { "Bool": { "aws:SecureTransport": "true" } } }
     ]
   }
   EOF
   ```
   Then keep its key pair for the session (prefixed so the AWS CLI never picks it up by accident): `read -rs UPLOADER_AWS_ACCESS_KEY_ID && read -rs UPLOADER_AWS_SECRET_ACCESS_KEY && export UPLOADER_AWS_ACCESS_KEY_ID UPLOADER_AWS_SECRET_ACCESS_KEY`.
6. A separate restore identity for you (SSO, or an IAM user with MFA) allowed `s3:GetObject`, `s3:GetObjectVersion`, `s3:ListBucket` and `s3:ListBucketVersions` on the bucket, set up as an AWS CLI profile. `export RESTORE_PROFILE=<that profile name>`. Never put it on Fly or GitHub.

- **Check:** the bucket shows versioning enabled and the policy saved without errors. Item V6 later tests the policy for real.

## B2. The age key (backup encryption)

- [ ] **Where:** terminal on your own computer.
1. `umask 077; age-keygen -o weather-backup.key` prints `Public key: age1...`. Keep it: `export BACKUP_AGE_RECIPIENT=$(age-keygen -y weather-backup.key)`.
2. Store the private key file in 1Password and delete it: `op document create weather-backup.key --title "Backup age key" --vault "$OP_VAULT" && rm -f weather-backup.key`. Print a copy for a safe from the 1Password app.

- **Check:** `op document get "Backup age key" --vault "$OP_VAULT" | grep -c AGE-SECRET-KEY-` prints `1`. **The private key never goes on Fly, GitHub, Netlify or AWS.** Lose it and every backup is unreadable.

## B3. healthchecks for backup

- [ ] **Where:** healthchecks.io, project `family-weather`, then the terminal.
1. Add a check `BACKUP`: period **1 day**, grace **6 hours**. Add a check `DBSIZE`: period **1 day**, grace **6 hours**.
2. Keep their ping URLs for the session: `read -rs HEALTHCHECK_URL_BACKUP && read -rs HEALTHCHECK_URL_DBSIZE && export HEALTHCHECK_URL_BACKUP HEALTHCHECK_URL_DBSIZE`.

- **Check:** two more checks exist.

## B4. Backup secrets

- [ ] **Where:** terminal (same private-file method as A12). Needs the B1 to B3 session variables.
1. Write the file:
   ```sh
   : "${BACKUP_S3_BUCKET:?B1}" "${BACKUP_AGE_RECIPIENT:?B2}" "${UPLOADER_AWS_SECRET_ACCESS_KEY:?B1}" "${HEALTHCHECK_URL_DBSIZE:?B3}"
   umask 077; cat > ~/fw-backup.env <<EOF
   BACKUP_DATABASE_URL=postgresql://app_backup:$(op read -n "op://$OP_VAULT/App Backup PW/password")@db.$PROJECT_REF.supabase.co:5432/postgres?sslmode=verify-full&sslrootcert=/etc/ssl/app-certs/supabase-ca.crt
   BACKUP_S3_BUCKET=$BACKUP_S3_BUCKET
   BACKUP_AGE_RECIPIENT=$BACKUP_AGE_RECIPIENT
   AWS_ACCESS_KEY_ID=$UPLOADER_AWS_ACCESS_KEY_ID
   AWS_SECRET_ACCESS_KEY=$UPLOADER_AWS_SECRET_ACCESS_KEY
   HEALTHCHECK_URL_BACKUP=$HEALTHCHECK_URL_BACKUP
   HEALTHCHECK_URL_DBSIZE=$HEALTHCHECK_URL_DBSIZE
   EOF
   ```
2. Check and import as in A12: `grep -c '=$' ~/fw-backup.env` prints `0`; `ops/ci/check-demo-values.sh ~/fw-backup.env; echo "exit=$?"` prints `exit=0`; then `fly secrets import -a integrated-awn-weather-backup --stage < ~/fw-backup.env && rm -f ~/fw-backup.env`.

- **Check:** `fly secrets list -a integrated-awn-weather-backup` shows seven names.

## B5. Build the backup image and create the daily machine

- [ ] **Where:** terminal at the repo root, on `main` updated to the merged commit (`git checkout main && git pull`).
1. `sha=$(git rev-parse --short=12 HEAD)`
2. `fly deploy . --config ops/backup/fly.toml --dockerfile ops/backup/Dockerfile --build-only --push --image-label sha-$sha`
3. ```sh
   fly machine run registry.fly.io/integrated-awn-weather-backup:sha-$sha -a integrated-awn-weather-backup \
     --name backup-daily --region sjc --schedule daily --restart no --vm-size shared-cpu-1x --vm-memory 256 \
     -e AWS_DEFAULT_REGION=us-west-2 -e BACKUP_S3_PREFIX=weather
   ```
   The schedule counts from creation, so do this at a quiet hour (about 09:00 UTC).
4. `fly logs -a integrated-awn-weather-backup`. If nothing has run yet, start it once: `fly machine start $(fly machines list -a integrated-awn-weather-backup --json | jq -r '.[] | select(.name == "backup-daily") | .id') -a integrated-awn-weather-backup`.

- **Check:** the log shows `auth users captured: N (app.users rows: M)` (compare N with the Supabase dashboard user count: it should be 1, you) and `backup ok <timestamp>`. Then `aws s3 ls "s3://$BACKUP_S3_BUCKET/weather/" --recursive --profile "$RESTORE_PROFILE"` shows an `app-...dump.age` and an `auth-users-...csv.age`. The `BACKUP` and `DBSIZE` checks turn green.
- **If it fails:** the log names the failing step. A TLS error means the CA file is missing from the image; a `403` on upload means the bucket policy or uploader policy is wrong (see V6).

## B6. API secrets

- [ ] **Where:** terminal (same private-file method). Needs `AWN_APPLICATION_KEY` in the session.
1. Write the file:
   ```sh
   : "${AWN_APPLICATION_KEY:?set it first: read -rs AWN_APPLICATION_KEY && export AWN_APPLICATION_KEY}"
   umask 077; cat > ~/fw-api.env <<EOF
   DATABASE_URL=postgresql://app_api:$(op read -n "op://$OP_VAULT/App API/password")@db.$PROJECT_REF.supabase.co:5432/postgres?sslmode=verify-full&sslrootcert=/etc/ssl/app-certs/supabase-ca.crt
   AWN_APPLICATION_KEY=$AWN_APPLICATION_KEY
   CREDENTIALS_PUBLIC_KEY=$(op read -n "op://$OP_VAULT/Admin Public/credential")
   CREDENTIALS_KEY_VERSION=1
   EOF
   ```
2. Check and import as in A12: `grep -c '=$' ~/fw-api.env` prints `0`; `ops/ci/check-demo-values.sh ~/fw-api.env; echo "exit=$?"` prints `exit=0`; then `fly secrets import -a integrated-awn-weather-api --stage < ~/fw-api.env && rm -f ~/fw-api.env`.
3. `fly secrets list -a integrated-awn-weather-api`

- **Check:** four names. The list must **not** contain `CREDENTIALS_PRIVATE_KEY`, `SUPABASE_SERVICE_ROLE_KEY` or a poller database URL.

## B7. Fill in the Supabase project ref in three files

- [ ] **Where:** terminal at the repo root, on a new branch (`git checkout main && git pull && git checkout -b go-live`).
1. `sed -i '' "s/<project-ref>/$PROJECT_REF/g" ops/fly/api.toml web/security-headers.json` (the ref is public by design).
2. Regenerate and verify the Netlify file (needs the Node in `web/.nvmrc`): `node web/scripts/headers.mjs generate && node web/scripts/headers.mjs check`.
3. `git diff` and look for anything that is not the ref. Then `git add -A && git commit -m "Set the Supabase project ref" && git push -u origin go-live && gh pr create --base main --fill`. **Do not merge yet.**

- **Check:** `grep -rn "<project-ref>" ops/fly/api.toml web/security-headers.json web/netlify.toml` prints nothing; `ci-ok` on the PR is green.

## B8. Netlify variables

- [ ] **Where:** Netlify, Site configuration, Environment variables (scope: Builds). Copy each value to the clipboard from the terminal, then paste it in.
1. `PUBLIC_SUPABASE_URL`: `printf %s "$SUPABASE_URL" | pbcopy`
2. `PUBLIC_SUPABASE_ANON_KEY`: `op read -n "op://$OP_VAULT/Supabase Publishable Key/credential" | pbcopy`
3. Add nothing else. Never add a secret that starts with `PUBLIC_`.

- **Check:** exactly these two variables are listed.

## B9. Ruleset for the `production` branch

- [ ] **Where:** GitHub, Settings, Rules, New branch ruleset.
1. Target branch `production`. Turn on **block deletions** and **block force pushes** only. Do **not** add "restrict updates" (it would block the deploy workflow's own push).

- **Check:** the ruleset is Active.

## B10. Turn on the API and web deploys

- [ ] **Where:** terminal.
1. `gh variable set DEPLOY_API -R murrayrodney/weather-app --body true && gh variable set DEPLOY_WEB -R murrayrodney/weather-app --body true`

- **Check:** `gh variable list -R murrayrodney/weather-app` shows `SITE_URL`, `DEPLOY_API`, `DEPLOY_WEB`.

## B11. Merge and deploy everything

- [ ] **Where:** GitHub.
1. Squash and merge the `go-live` PR (`gh pr merge go-live --squash`).
2. Actions, `deploy` run, approve as in A18.
3. The run now also validates the API config, deploys the API, then the `publish-web` job fast-forwards the `production` branch, Netlify builds, and the job waits until `$SITE_ORIGIN/version.txt` equals the commit and runs the web smoke checks.

- **Check:** the whole run is green, including `publish-web`. `fly machines list -a integrated-awn-weather-api` shows exactly one machine. `open "$SITE_ORIGIN"`: the login page appears.
- **If it fails:** a `publish-web` rejection means `production` is not an ancestor of the release commit (for example when re-running an old run): re-run from the latest `main`. If Netlify's build fails, open its deploy log and tell the coding agent; the old site stays up.

## B12. Post-deploy checks

These are the manual checks from spec section 13.

- [ ] **Where:** terminal. First: `PK="$(op read -n "op://$OP_VAULT/Supabase Publishable Key/credential")"` (the publishable key is public by design).
1. Fly direct needs auth: `SITE_URL="$SITE_ORIGIN" DEPLOY_API=true ops/ci/smoke.sh` all lines `ok`, and `curl -i https://integrated-awn-weather-api.fly.dev/api/v1/stations` returns 401 or 403, never data (checks 1, 3, 4, 5).
2. Sign-up is off: `curl -s -X POST "$SUPABASE_URL/auth/v1/signup" -H "apikey: $PK" -H 'Content-Type: application/json' -d "{\"email\":\"probe+delete-me@example.invalid\",\"password\":\"$(openssl rand -hex 12)\"}"` returns `signup_disabled`. Repeat with `-d '{}'`: `anonymous_provider_disabled` or `signup_disabled`. If any call returns a user, delete that user in the dashboard now and fix A3 (check 6).
3. Data API is closed: `curl -s -o /dev/null -w '%{http_code}\n' "$SUPABASE_URL/rest/v1/credentials" -H "apikey: $PK" -H 'Accept-Profile: app'` is not 200 (check 7).
4. The publishable key is not a login: `curl -i -H "Authorization: Bearer $PK" "$SITE_ORIGIN/api/v1/me"` returns 401 (check 8).
5. Machine counts: `fly machines list -a integrated-awn-weather-poller` and `-a integrated-awn-weather-api` show one machine each (check 9).
6. Key isolation: `fly secrets list -a integrated-awn-weather-api` has no `CREDENTIALS_PRIVATE_KEY` and no `SUPABASE_SERVICE_ROLE_KEY` (check 10).
7. healthchecks.io: `AWN_INGEST`, `BACKUP` and `DBSIZE` are green (check 13).

- **Check:** every line above matches. If one fails, stop and tell the coding agent before inviting anyone.

## B13. Your first login, and the signing-key check

- [ ] **Where:** two terminals as in A19, then a browser.
1. Get a fresh link (the one from A19 has expired): A19 steps 1 and 2, then `admin reset-link --email "$MY_EMAIL"`. Press Enter in the first terminal when done.
2. Open the printed link in your browser (it opens `$SITE_ORIGIN/set-password`). Choose a password of 12 or more characters. Sign in. Sign out and sign in again with it.
3. Test the password-change settings from A3: run `reset-link` again, open the link, and set a new password **without** entering the old one. Then, signed in, change your password from the account page.

- **Check:** you reach the dashboard and see your station. In the browser's developer tools (Network), the `/api/v1/me` request returns **200**. Your own station's data is shown.
- **If it fails:** if setting a new password asks for the current password, the "Require current password" switch in A3 is still on. Turn it off and repeat.

## B14. Disable the legacy Supabase keys

Only after B13 shows `/api/v1/me` returning 200.

- [ ] **Where:** Supabase, Project Settings. ([Supabase: API keys](https://supabase.com/docs/guides/api/api-keys))
1. API Keys, legacy tab: **disable** the legacy `anon` and `service_role` keys.
2. JWT Signing Keys: your project still has a legacy shared secret (its legacy `anon` key is signed with HS256). If the page lists it (as "Legacy JWT secret", a shared-secret or HS256 key), move it to **"Previously used"**. If the page does not show it at all, skip steps 2 and 3 and tell the coding agent what the page does show. Then **wait at least 1 hour 15 minutes** (sign-in tokens live 1 hour; revoking sooner signs everyone out).
3. JWT Signing Keys: **revoke** the legacy HS256 secret. (Key changes are throttled; if a button is greyed out, wait about 5 minutes.)
4. Re-run A8 and the checks in B12 items 2 to 4. Reload the site and sign in again.

- **Check:** login and your dashboard still work, and all the B12 checks still match.

## B15. Airport baselines

Baselines are the long-record airport stations your data is compared with.

- [ ] **Where:** the web app, signed in.
1. Open your station, then its baseline. The picker suggests the nearest airport stations from ACIS and fills in their details; pick the right one and confirm. The airports for this project's stations: Helena `USW00024144`, Laramie `USW00024022`, Loveland (Northern Colorado Regional, FNL) `USW00094035` (guide 04 section 5).

- **Check:** the station page shows the baseline.

## B16. Invite the family

- [ ] **Where:** two terminals as in A19. Do this when the person is ready, because the link expires in about an hour.
1. Terminal 2: `export THEIR_EMAIL='...' THEIR_NAME='...'`
2. A19 steps 1 and 2, then `admin invite --email "$THEIR_EMAIL" --display-name "\"$THEIR_NAME\"" --follow-all`
3. Send the printed link privately (Signal, or a message to the right number, or in person). Never put it in a ticket or a file.
4. Their station: A20 again with their station's variables and `--owner "\"$THEIR_NAME\""`, or let them connect it in the web app.
5. Forgotten password later: the same shell, but `admin reset-link --email "$THEIR_EMAIL"`.

- **Check:** they open the link, choose a password and see the dashboard. `admin list-users` shows them.

**Go-live is done when:** every check in B12 matches, the healthchecks stay green for 24 hours, and you have done one restore drill (below).

---

# After go-live

## Restore drill (every 3 months; a calendar reminder)

A drill proves the backups can be restored. It copies family data onto your laptop, so you delete it at the end. This is spec section 15.6.

- [ ] **Where:** terminal on your laptop, after "Every terminal session", with `BACKUP_S3_BUCKET` and `RESTORE_PROFILE` set.
1. Download the newest pair (both files of one run share the same `<ts>-<rand>` suffix):
   ```sh
   key=$(aws s3 ls "s3://$BACKUP_S3_BUCKET/weather/" --recursive --profile "$RESTORE_PROFILE" | awk '$4 ~ /\/app-.*\.dump\.age$/ {print $4}' | sort | tail -1)
   suffix=${key##*/app-}; suffix=${suffix%.dump.age}; dir=${key%/*}
   aws s3 cp "s3://$BACKUP_S3_BUCKET/$key" app.dump.age --profile "$RESTORE_PROFILE"
   aws s3 cp "s3://$BACKUP_S3_BUCKET/$dir/auth-users-$suffix.csv.age" auth-users.csv.age --profile "$RESTORE_PROFILE"
   ```
2. Decrypt with the key from 1Password: `umask 077; op document get "Backup age key" --vault "$OP_VAULT" --out-file weather-backup.key`, then `age -d -i weather-backup.key -o app.dump app.dump.age && age -d -i weather-backup.key -o auth-users.csv auth-users.csv.age`.
3. Find the schema version: `pg_restore --data-only --table=_sqlx_migrations -f - app.dump` (highest `version`). Check out the commit whose `backend/migrations/` ends at that version.
4. Start a local database in production order: `docker compose up -d db auth`, `docker compose run --rm auth-access`, `docker compose run --rm migrate`.
5. Restore the data (skipping the migrations table): `pg_restore -l app.dump | grep -v _sqlx_migrations > list.txt`, then `pg_restore --data-only --single-transaction --use-list list.txt --dbname "<local app_migrator url>" app.dump`.
6. Restore the logins: `admin restore-auth-users --dry-run --file auth-users.csv`, then without `--dry-run` (spec 15.6 step 5 has the details).
7. Verify: row counts for `observations` and `daily_rollups`; the newest observation is within 36 hours of the backup; `list-users` matches the dashboard count. Write the date, the backup timestamp, how long it took and any problems in `docs/ops-log.md` (create it on the first drill).
8. **Delete everything:** `docker compose down -v`, then `rm -f app.dump auth-users.csv app.dump.age auth-users.csv.age list.txt weather-backup.key` (the key stays in 1Password).

- **Check:** `docker volume ls` and `ls` show no leftovers. A failed drill is a priority bug: tell the coding agent.

## When something goes wrong

| Symptom | Go to |
| --- | --- |
| A station shows "needs reconnecting", or one owner's data stops | 06 section 15.2 |
| `AWN_INGEST` check goes red or the poller is silent | 06 section 15.3 |
| Supabase paused, or `DBSIZE` fails (database near 400 MB) | 06 section 15.4 |
| A key, password or token may have leaked | 06 section 15.5 (rotate first, investigate second; recipes R1 to R9 in section 12) |
| Backups fail or you must restore for real | `fly logs -a integrated-awn-weather-backup`, then the drill above |

Other upkeep: GitHub emails you if the weekly exposure check stops after 60 quiet days (re-enable it in the Actions tab); Dependabot opens monthly update pull requests (review each one, no auto-merge).

---

# First-run verification items

These can only be checked with your real accounts. Do each once, and tell the coding agent the result (pass or the exact error, without secrets).

- [ ] **V1. Fly config files.** (A10, A16) `for c in ops/fly/poller.toml ops/fly/api.toml ops/backup/fly.toml; do fly config validate --config $c --strict; done`. Expect "valid" each time.
- [ ] **V2. Dockerfile path.** (A16) From the repo root: `fly deploy backend --config ../ops/fly/api.toml --dockerfile Dockerfile --build-only`. Expect a finished build. `fly deploy backend` changes into `backend/` first, so `--config` and `--dockerfile` are relative to `backend/` (verified Nov 8, 2026, flyctl 0.5.5).
- [ ] **V3. One-off dry run.** (after A18) `time ops/fly/run-oneoff.sh batch integrated-awn-weather-poller integrated-awn-weather-poller poller -- /usr/local/bin/admin --version; echo "exit=$?"`. The command only prints its version and touches no database. Expect: the logs show `admin 0.1.0`, `exit code: 0`, `exit=0`, and `fly machines list -a integrated-awn-weather-poller` shows no `oneoff-*` machine afterwards. If the exit code is wrong or a machine is left, destroy it (`fly machine destroy <id from the list> -a integrated-awn-weather-poller --force`) and tell the coding agent (the `fly machine wait` and `jq` parts are unverified).
- [ ] **V4. Site address file.** (A16, before A19) `cat ops/fly/site-url.txt` prints exactly one line, equal to `$SITE_ORIGIN` (https, no path). `run-oneoff.sh` refuses to start admin one-offs without it (it prints a message saying to create the file).
- [ ] **V5. First pull request run.** (A16) The `ci-ok` check is green on the first PR. Also open the run and confirm the jobs that ran (and the ones skipped) look right.
- [ ] **V6. S3 conditional writes and the bucket policy.** (after B5) Uses the uploader key pair from the session, in a subshell so it never leaks into later commands:
  ```sh
  (
    export AWS_ACCESS_KEY_ID="$UPLOADER_AWS_ACCESS_KEY_ID" AWS_SECRET_ACCESS_KEY="$UPLOADER_AWS_SECRET_ACCESS_KEY" AWS_DEFAULT_REGION=us-west-2
    echo probe > probe.txt
    aws s3api put-object --bucket "$BACKUP_S3_BUCKET" --key weather/probe/probe.txt --body probe.txt --if-none-match '*'
    aws s3api put-object --bucket "$BACKUP_S3_BUCKET" --key weather/probe/probe.txt --body probe.txt --if-none-match '*'
    aws s3api put-object --bucket "$BACKUP_S3_BUCKET" --key weather/probe/probe2.txt --body probe.txt
    rm -f probe.txt
  )
  ```
  Expect: the first succeeds, the second returns **412 PreconditionFailed**, the third returns **403 AccessDenied**. (The probe object expires with the 90-day rule.) If the first call is denied or the daily backup gets a 403, the policy condition name or the AWS CLI version is the suspect: tell the coding agent.

---

# Notes for the coding agent

Resolved: the earlier mismatches between this runbook, the built files and guide 06 were fixed in both places. Still open:

1. `make keygen` and the dry-run command (`admin --version`) were checked locally only. V1 to V6 above still need real accounts.
2. `ops/prod.env` holds the non-secret production values; keep it in step with A1, A2 and A4.
