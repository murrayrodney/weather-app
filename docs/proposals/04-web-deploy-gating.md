# Proposal 04: How the web (Netlify) deploy is gated behind the API deploy

**Status:** Accepted by Rodney, Oct 5, 2026 (the throwaway Netlify site test remains a required pre-build verification). Research date: 2026-10-04. **Amended at acceptance:** the `production` ruleset only blocks deletion and force pushes (no "restrict updates"), because `GITHUB_TOKEN` cannot be a bypass actor; see 06-deploy-ops 11.4 notes.

## Problem

The guides say Netlify auto-publishing is off and `deploy.yml` (protected `production` environment) publishes the web build with a build hook after the poller and API deploy. But a locked site still builds without publishing, so a hook probably builds a deploy that nobody publishes. The CLI route (`netlify deploy --prod`) needs a Netlify token, and Netlify tokens cannot be limited to one site. We need a way to publish the web only after the API, with a credential that fits a one-person project.

## Verified facts

Read from Netlify docs this session (links below).

1. **Locking only stops publishing.** "Lock to stop auto publishing" pins the site to the current deploy; Netlify "still builds new deploys" and they wait for manual publishing. [Manage deploys](https://docs.netlify.com/deploy/manage-deploys/manage-deploys-overview/). Stopping builds is a different setting that also blocks git pushes, build hooks and the builds API. [Stop or activate builds](https://docs.netlify.com/build/configure-builds/stop-or-activate-builds/).
2. **Publishing a locked deploy is a manual "Publish deploy" click** on that deploy's page ([Manage deploys](https://docs.netlify.com/deploy/manage-deploys/manage-deploys-overview/), [blog](https://www.netlify.com/blog/2021/12/04/controlling-auto-publishing-for-coordinated-releases/)). Neither says a build hook publishes while locked. I found no statement that it does. That gap is Open question 15.
3. **Lock is site-wide, not per branch.** A forum thread reports that locking also stops branch deploys from publishing; Netlify has no per-branch auto-publish control. [Forum](https://answers.netlify.com/t/turn-off-autopublishing-only-for-production-branch/87149).
4. **API publish exists:** `POST /api/v1/sites/{site_id}/deploys/{deploy_id}/restore` "restores an old deploy and makes it the live version". [API guide](https://docs.netlify.com/api-and-cli-guides/api-guides/get-started-with-api/). The docs describe it for old deploys (rollback); that it works on a never-published, locked deploy is not documented.
5. **Tokens are account-wide.** Personal access tokens cover the account (needs an expiry; password reset invalidates them). [API guide](https://docs.netlify.com/api-and-cli-guides/api-guides/get-started-with-api/). A community forum thread says per-site tokens were not supported and suggests a separate user who has access to only that site. [Forum](https://answers.netlify.com/t/per-site-access-tokens/52144). Not re-checked for newer features; this is the latest I found.
6. **Production branch is configurable, and pushing to it auto-creates and publishes a production deploy** (unless locked). [Production deploys](https://docs.netlify.com/deploy/deploy-types/production-deploy/). Branch deploys can be set to None, and the production branch is excluded from "All". [Branch deploys](https://docs.netlify.com/deploy/deploy-types/branch-deploys/).
7. **Build hooks are exempt from Netlify's "Deploy Request Policy"** (which only builds private-repo changes from recognized authors) and are treated as trusted. [Deploy overview](https://docs.netlify.com/deploy/deploy-overview/).
8. **CLI manual deploys** (`netlify deploy --prod`) run no Netlify build; you upload a directory. [Create deploys](https://docs.netlify.com/deploy/create-deploys/).

**Inference, not verified:** a push made by Actions with `GITHUB_TOKEN` still fires Netlify's GitHub webhook (GitHub only suppresses *workflow* triggers for that token); Netlify's Deploy Request Policy might treat a bot-authored commit as "unrecognized" on a private repo, though a fast-forward adds no new commits, so the commit author is Rodney. `netlify.toml` headers and redirects still apply to CLI deploys.

## Options compared

| | Gates web behind API? | Credential in GitHub | Blast radius if leaked | Solo setup | Verify |
|---|---|---|---|---|---|
| (a) git build, locked, CI publishes via API | Yes, if restore works on unpublished deploys (unproven) | Netlify token | Whole Netlify account | Medium; find deploy by SHA, poll until ready | `/version.txt` |
| (b) CI builds, `netlify deploy --prod --dir` | Yes | Netlify token + site ID; `PUBLIC_*` vars also in GitHub | Whole Netlify account | Medium; Netlify's own build disabled | `/version.txt` |
| (c) ungated auto-builds from `main` | No | None | None | Lowest | `/version.txt` |
| (d) `production` branch fast-forwarded by CI | Yes (only CI moves that branch) | Git push right only (no Netlify token) | Can rewrite `production` branch; cannot touch Netlify settings | Low | `/version.txt` |
| (e) one-site Netlify user + (a) or (b) | Yes | Token of a user who sees only this site | One site | Medium-high; extra account | `/version.txt` |

**(a)** Keeps Netlify building (so `PUBLIC_*` vars stay in Netlify) and CI publishes the deploy whose commit equals `GITHUB_SHA`. It is attractive but rests on two unproven points (restore works on a locked, never-published deploy; lock does not break the build hook or API), and it holds an account-wide token. Needs a test on the real site.

**(b)** Fully deterministic: CI builds, tests and uploads exactly that output. Costs: Netlify's build is turned off, Supabase public values live in GitHub too, and the account-wide token sits in GitHub (protected environment limits who can use it, not what it can do).

**(c)** Simplest and honest about the real risk: a static web that ships a few minutes before the API. With a backward-compatible API (additive changes only) and a feature flag for breaking UI changes, ordering stops mattering. It drops the gate entirely, so poller, API and web can briefly disagree, and a bad web deploy is not behind the approval click.

**(d)** Netlify is set to build only a `production` branch (production branch = `production`, branch deploys None, deploy previews on for pull requests — previews never publish to the main site, so the gate is unaffected). `main` pushes build nothing. After poller and API succeed, `deploy.yml` runs `git push origin $GITHUB_SHA:refs/heads/production` (fast-forward only, no force). Netlify builds that commit and publishes it automatically; no lock, no hook, no token, so Facts 1 to 3 do not matter. Gate strength is "only the approved workflow pushes there". Enforce with a GitHub ruleset on `production`: restrict updates and block force pushes, with the Actions bot (and Rodney as admin) as the only bypass. Weakness: Rodney, or anyone with repo write, could push there by hand, so protect the ruleset with MFA. Web publishing is asynchronous: the workflow must wait for Netlify's build.

**(e)** Reduces (a)/(b)'s blast radius by creating a second Netlify user who is a member of only this site's team and using that user's token. It needs a team/seat arrangement that may not fit the free plan (not verified), and a second login to secure. It is the fallback if (d) turns out not to work.

## Recommendation

**Adopt (d).** It is the only option that is both really gated and needs no Netlify credential. The CI secret becomes a git push right on one branch, which the existing protected environment already guards, and the Netlify account holds nothing CI can reach. It removes the unverified build-hook and lock behavior from the plan, drops `NETLIFY_BUILD_HOOK_URL`, and keeps Netlify building (so Netlify env vars and `netlify.toml` stay as designed). Rollback of web is a revert plus the same push, or "Publish deploy" in the Netlify UI, which stays available.

Implementation sketch for `deploy.yml`: after the API deploy, a step with `contents: write` (job-level, only that job) pushes `production`; the smoke step polls `/version.txt` until it equals `GITHUB_SHA` (timeout about 10 minutes), then runs the Netlify checks. `/version.txt` is written during the web build from `GITHUB_SHA`/Netlify's `COMMIT_REF` env var (01 owns the build step).

Do first, in a throwaway Netlify site: confirm that a fast-forward push by `GITHUB_TOKEN` builds and publishes, with branch deploys None and `main` ignored.

## What changes in the guides if accepted

- **00-conventions** ("Web headers and proxy"): replace "auto-publishing is off; build hook or CLI" with "Netlify's production branch is `production`; only `deploy.yml` fast-forwards it after the API deploy; `main` builds nothing; no Netlify token exists in GitHub". Add `/version.txt` (contains the commit SHA) to the conventions.
- **01-web:** the web build writes `version.txt` from the commit SHA; add it to the CSP/cache checks as a no-cache file.
- **06-deploy-ops:** section 8.1 (production branch `production`, branch deploys None, PR deploy previews on, no auto-publish lock; remove build hook creation); section 11.4 (replace the hook step with the push step and the poll for the SHA; remove `NETLIFY_BUILD_HOOK_URL` from environment secrets; grant `contents: write` only to the push job); section 12 (secrets table and rotation: no Netlify credential in GitHub); runbook steps 2 and 13 (create the `production` branch, GitHub ruleset on it); close Open question 15 as "not needed". Add the ruleset to the first-deploy checklist.
- Local Docker Compose: no change.

## Open risks

- **Untested:** Netlify's behavior on a bot-pushed `production` fast-forward (Deploy Request Policy, webhook delivery). The throwaway-site test settles it; if it fails, fall back to (e) or (b).
- Anyone with repo write (Rodney only, today) can push `production` by hand; the ruleset and GitHub MFA are the controls.
- Web goes live a few minutes after the workflow ends; the smoke step must wait, and a failed Netlify build leaves the old site published with no alert unless the smoke step times out and fails the run.
- Netlify billing credits count builds; one build per release is small, but a deploy failure loop would count too.
- The GitHub repo link on Netlify (the GitHub app) keeps read access to the repo; keep that scoped to this repo.
