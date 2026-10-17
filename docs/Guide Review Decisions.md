# Guide Review: Decisions for Rodney

The six component guides in `docs/guides/` went through four rounds of adversarial review (two Opus reviewers per guide per round). By round 4, five of the twelve reviews said "only minor issues"; the rest were small, localized fixes, applied in a final pass. No reviewer proposed an architecture change beyond the four proposals below.

The four proposals are accepted and written into the guides. The product and operations questions below are still waiting on you; mark each item and I'll update the guides.

## 1. Proposals (architecture changes researched online)

- [x] **P-01 Credential encryption** — [proposals/01-credential-encryption.md](proposals/01-credential-encryption.md). Use public-key ("lock with public, unlock with private") encryption so the API can save a new AWN key but only the poller can read it. Recommended: adopt. — Accepted by Rodney (Oct 5, 2026)
- [x] **P-02 Fly app layout** — [proposals/02-fly-app-layout.md](proposals/02-fly-app-layout.md). Fly gives every machine in an app the same secrets, so keeping the decryption key away from the API needs separate apps. Recommended: four small apps (api, poller, admin, backup) from one image, at no extra cost. Fallback: two apps. — Accepted by Rodney (Oct 5, 2026): four apps
- [x] **P-03 Supabase login tables** — [proposals/03-supabase-auth-schema-access.md](proposals/03-supabase-auth-schema-access.md). Since April 2025 Supabase may not let our database logins read its login tables directly. Recommended: two small locked-down database functions for "is this session still valid?" and "sign this user out everywhere", and no direct link between our user table and Supabase's. **Needs a 10-minute check on a scratch Supabase project first** (checklist in the proposal). — Accepted by Rodney (Oct 5, 2026); the scratch check is a required pre-build verification (below)
- [x] **P-04 Web deploy gating** — [proposals/04-web-deploy-gating.md](proposals/04-web-deploy-gating.md). Locking Netlify's auto-publish doesn't work with build hooks. Recommended: Netlify publishes only a `production` branch, which the approved deploy workflow moves forward after the API deploy, so no Netlify token is needed in GitHub. Needs a test on a throwaway Netlify site; fallbacks listed. — Accepted by Rodney (Oct 5, 2026); the throwaway-site test is a required pre-build verification (below)

### Pre-build verifications (Rodney runs these before relying on P-03, P-04 and P-05) — step-by-step: [Pre-build Checks.md](Pre-build%20Checks.md)

- [x] **P-03 scratch Supabase project check** (about 10 minutes; checklist at the end of the proposal). If a definer function cannot read `auth.sessions`, the session check falls back to proposal option (c) and S2 is recorded as downgraded. Record results in 04 section 6.5. — **Passed Oct 7, 2026**; `app_auth` confirmed, results in 04 section 6.5.
- [x] **P-04 throwaway Netlify site test:** a fast-forward push to the production branch by `GITHUB_TOKEN` builds and publishes, with branch deploys None and `main` ignored. If it fails, use the proposal's fallback (a one-site Netlify user's token, or CI builds and `netlify deploy --prod`). — **Passed Oct 7, 2026**; results in `Pre-build Checks.md`.
- [x] **P-05 login check: no separate scratch project.** The set-password link, link lifetime and single use, and creating a user without a password are tested during the build against the local Supabase auth server (same software as hosted), as part of the admin and web tests. The two password-change settings are checked in the dashboard when the real Supabase project is set up (deploy runbook). (Rodney, Oct 7, 2026)

Both are step 0 of the first-deploy runbook (06 section 15.1).

## 2. Decisions I made during the loop — all accepted by Rodney (Oct 7, 2026)

- **Tests never touch your working stack.** Every `make test-*` runs in its own throwaway Docker Compose stack; CI runs the same commands.
- **Build order:** shared Rust basics → database/admin → poller (goes live first, poller-only deploy) → API → web.
- **Removed devices:** if a family member removes a device from their AWN account, reconnecting leaves that station on the old key, tells the user, and you disable it.
- **Rate limiting with AWN** uses a small "next allowed time" table, so a cancelled request can never leave a stuck lock.
- **Daily rollups** use a small "days to recompute" table, so late data is never missed.
- **Strict browser security policy** has one exception: SvelteKit's built-in screen-reader announcer.
- **Chart:** today's partial reading is drawn lighter; a station that started mid-season begins its line on its first day.
- **Trace rain** in the airport records counts as 0, not missing.
- **One AWN key rejected and no other key to compare:** the poller alerts you instead of marking the key bad (it can't tell your app key apart from the user's).

- [x] **Netlify previews (decided by Rodney):** deploy previews stay on for pull requests; branch deploys off (the production branch aside); Dependabot pull requests skipped; Supabase's redirect allow-list stays empty (password sign-in is a direct call, so previews sign in with a password exactly like production and need no entry).
- [x] **`production` branch protection (decided by Rodney, Oct 5, 2026):** the GitHub ruleset only blocks deleting the branch and force pushes, with no "restrict updates" rule; the deploy approval and GitHub two-factor login are the real gate. No deploy key will be added.

## 3. Open product questions

- [x] **Follow discovery:** users search for a person by name and send a follow request; the owner approves as before. No "people you may know" for now. (Rodney, Oct 7, 2026)
- [x] **New stations:** the app looks up the nearest airport weather station from the station's position and suggests it; the user confirms (or picks another nearby one). (Rodney, Oct 7, 2026)
- [x] **Off-season dashboard (Nov–Mar):** show last season. (Rodney, Oct 7, 2026)
- [x] **GDD:** simple average at 50 °F by default, configurable (method and base). Start date not answered; the guides default to April 1, also configurable — reverse if you prefer Jan 1. (Rodney, Oct 7, 2026)
- [x] **Loveland baseline:** Northern Colorado Regional (FNL). Station id and record length to be confirmed when the guides are updated. (Rodney, Oct 7, 2026)
- [x] **Open-Meteo history (F2):** no backfill and no archiving of percentiles. Percentile bands are used only in the live forecast charts (hourly/daily, like a normal forecast); forecast-change history covers the forecast itself, not its percentiles. Proposal 06 accepted: option A, store nothing; the change view is day-by-day from Open-Meteo's Previous Runs API. (Rodney, Oct 7, 2026)

## 4. Open operations questions

- [x] **Login method:** proposal 05 (b2) accepted: email + password, each person sets their own via a one-time link you send by hand (invite and reset). No email provider for now; leave room to add SMTP (AWS SES is fine then) when there are more than a few people. Verified by build-time tests and a dashboard check at project setup. (Rodney, Oct 7, 2026)
- [x] **Restore drill vs. S12:** allowed on your laptop for now. (Rodney, Oct 7, 2026)
- [x] **Backing up user logins:** a small definer function returning `(id, email, created_at)`, included in the backup. (Rodney, Oct 7, 2026)
- [x] **S3 retention:** versioning + 90-day expiry, no Object Lock. (Rodney, Oct 7, 2026)
- [x] **Repo visibility:** **public** (Rodney, Oct 7, 2026). A private repo on GitHub Free has no deployment environments, so it would lose the deploy approval step and environment-locked secrets. Public keeps every protection and has unlimited CI minutes. Nothing secret lives in the repo by design. Was: private or public? (Affects the weekly security check schedule and CI minutes. Also: GitHub rulesets, such as the one protecting the `production` branch, are not enforced on private repos on the free plan.)
- [x] **API auto-stop on Fly:** allowed (~1 s first-load delay). (Rodney, Oct 7, 2026) Was: allow the API machine to sleep when idle (cheaper, ~1 s first-load delay) or keep it always on (~$2/month more)?
