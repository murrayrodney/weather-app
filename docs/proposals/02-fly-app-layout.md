# Proposal 02: How many Fly apps, and where the secrets live

**Status:** Accepted by Rodney, Oct 5, 2026. Date: 2026-10-02.
Touches decisions: S5, S12, P1, P3, C5, D6, G2. Related: proposal 01 (credential encryption), deploy guide `06-deploy-ops.md` section 4.

## Problem

P3 decided one Fly app with two process groups (`api`, `poller`) from one image. S5 and proposal 01 need only the poller to hold `CREDENTIALS_PRIVATE_KEY` (and the `app_poller` database login), while the API holds only `app_api`. The deploy guide author found that Fly secrets are shared by every machine in an app, so process groups cannot keep them apart, and proposed four Fly apps. This proposal checks that claim and weighs it against cheaper alternatives.

## Verified facts about Fly secrets

Checked on 2026-10-02 against the current docs and community threads.

1. **Secrets are app-wide.** Fly's docs say secrets are "available as environment variables at runtime on every Machine belonging to that Fly App" ([Secrets and Fly Apps](https://docs.fly.io/apps/secrets)). The page describes no per-machine or per-process-group scoping.
2. **`fly.toml` has no per-process env or secrets.** `[env]` applies to the whole app. The [configuration reference](https://docs.fly.io/reference/configuration) does allow `[[files]]` entries with `secret_name` and a `processes` filter, so a file can be written only on the poller's machine. But the file's content comes from an app secret, which is still injected as an environment variable on every machine. So this gives no isolation. (The docs say nothing that contradicts this; it follows from fact 1.)
3. **Community confirmation.** A user asked for different secrets per process group and was told Fly does not support it; the advice was to use separate apps, because "apps" are the natural boundary ([thread 18005](https://community.fly.io/t/different-secrets-for-each-process-group/18005)).
4. **A low-level workaround exists but is not `fly deploy`-friendly.** The Machines API machine config has a per-process `secrets` list and an `ignore_app_secrets` flag ("If true, only use the secrets provided at the process level", [Machines API](https://docs.fly.io/machines/api/machines-resource/); discussion in [thread 21206](https://community.fly.io/t/hide-secrets-from-some-machines/21206)). I could not find anything saying `fly deploy` or `fly.toml` sets or preserves these fields. The thread itself warns the default (`false`) must be set again "when updating apps". Treat it as likely to be lost on a redeploy. **Not verified; do not rely on it.**
5. **Apps cost nothing by themselves.** Fly bills machines, volumes, dedicated IPs and bandwidth. A shared-cpu-1x 256 MB machine is about $2.19/month in the cheapest regions, volumes $0.15/GB/month, dedicated IPv4 $2/month, shared IPv4 and IPv6 free ([pricing](https://docs.fly.io/about/pricing)). An app with zero machines costs $0.
6. **One image can serve several apps in the same organization.** `fly deploy --build-only --push` builds and pushes once, then `fly deploy --image registry.fly.io/<app>:<tag>` deploys it to another app ([Fly registry guide](https://fly.io/docs/blueprints/using-the-fly-docker-registry/)). Secrets must be set on each app separately. **Not verified:** whether a single-app deploy token (the guide's plan, section 8) can pull from a sibling app's registry path. The [tokens page](https://docs.fly.io/security/tokens/) does not say, and search results disagree on whether an org-level token is needed.

## What is actually at stake

Per proposal 01, an AWN key is read-only access to one person's weather station: low value. In a single app, a compromised API process (remote code execution, or a leaked environment dump) could read from its own environment:

- `CREDENTIALS_PRIVATE_KEY`, and the `app_poller` database login (which can read the ciphertext). With both, the attacker decrypts every stored AWN key. That empties S5 and proposal 01's main property.
- Not the Supabase service-role key, if that is kept out of the app (it lives in the admin app in every option below).

Honest size of the loss: all ~5 users' weather-station read keys, plus a poller-level database login that can write ingest tables (corrupting weather data). It does not reach passwords, payments or the Supabase auth admin. For a private family app this is a modest harm, but S5 was an explicit decision and the fix is cheap, so the question is only how much operational friction the fix costs.

## Options compared

| | (a) Four apps | (b) One app, accept app-wide secrets | (c) One app, Machines API per-process secrets | (d) Two apps | (e) External secret store | (f) Key file on poller-only volume |
|---|---|---|---|---|---|---|
| API compromise reads private key? | No | **Yes** | No (if it holds) | No | **Yes if API shares the store credential; no if only poller does, but that credential is still app-wide** | No |
| Poller compromise reaches service-role key / AWS creds? | No | **Yes** | Depends on setup | **Yes** (admin and backup share its secrets) | n/a | Depends on layout |
| Works with `fly deploy` unchanged | Yes | Yes | **Unverified, likely not** | Yes | Yes | Yes |
| Solo-dev CI/CD | 3 deploy tokens, 3 `fly.toml` files, one workflow | 1 token, 1 file | Custom scripts against the Machines API | 2 tokens, 2 files | One more vendor, IAM setup, code path | 1 token, 1 file, one manual upload |
| Extra Fly cost | $0 | $0 | $0 | $0 | $0 to ~$1 | ~$0.15/month (1 GB volume) |
| Compose parity | Direct: one service per app | Direct, but compose would be *less* strict than prod | Not reproducible in compose | Direct | Needs a local stand-in | Mount a dev key file |

### (a) Four apps (the deploy guide's proposal)

`integrated-awn-weather-api`, `integrated-awn-weather-poller`, `integrated-awn-weather-admin` (zero machines, used only for one-off runs holding the service-role key and migrator login), `integrated-awn-weather-backup` (scheduled). Each app holds exactly the secrets for its job, which is the only isolation Fly supports natively and the one the community recommends (fact 3). Cost is the same as one app because only machines are billed (fact 5): about $2 to $4/month for two always-on small machines plus a few cents for the daily backup run. Overhead is real but small and one-time: three `fly.toml` files, three deploy tokens in the GitHub `production` environment, and secrets set per app. The image must reach three apps. Simplest is building per app with `--remote-only` (cached layers, a few minutes); building once and using `--image` is nicer but depends on unverified fact 6.

### (b) One app, accept app-wide secrets (guide "Variant B")

Simplest operations, one token, one file. But S5 becomes a paper decision: any API environment leak yields the private key, the poller database login and the AWS backup keys. The database login names also collide (one `DATABASE_URL` namespace), forcing code changes. The saving over (a) is roughly one afternoon of setup and a few lines of CI. Not recommended since the security work in proposal 01 would buy nothing.

### (c) One app, per-process secrets through the Machines API

Technically gives the isolation we want inside one app (fact 4). But `fly.toml` cannot express it, `fly deploy` may overwrite it, and a mistake fails open (secrets reappear on the API machine). It also cannot be mirrored in compose. A silent security regression after a routine deploy is the worst failure mode for a solo developer who rarely touches this. Rejected unless Fly documents `fly.toml` support for it.

### (d) Two apps (api; poller plus backup plus admin one-offs)

Fixes the stated S5 problem (API cannot see the private key) with one fewer app. The cost: the service-role key, migrator login and AWS credentials share an environment with the long-running poller, which fetches data from the internet and is the more exposed process after the API. The guide put admin in its own app specifically for the service-role key. Since an empty app costs nothing, merging saves only a `fly.toml` and a token while widening the poller's blast radius. Acceptable fallback if three or four apps feels like too much.

### (e) Fetch the private key from an external secret store at poller start

For example AWS Secrets Manager or SSM Parameter Store. The poller would need an AWS credential, and that credential would sit in Fly secrets, app-wide in a single-app layout, so the API could use it to fetch the key. It only helps if the poller is already in a separate app, at which point the store adds nothing. It also adds a vendor, an IAM policy, a network dependency at poller start, and a second code path for compose. Rejected.

### (f) Private key file on a volume attached only to the poller machine

Fly volumes attach to one machine, and `fly.toml` can mount a volume for one process group only. Putting the key file (uploaded once through `fly ssh sftp`) on the poller's volume gives real per-machine isolation without separate apps. Costs: about $0.15/month, a manual upload that must be repeated if the volume is lost, volume snapshots to consider, and it does not solve the database logins or service-role key (still app-wide env). Not unique-to-Fly code either (a "read key from file path" mode), though compose parity is easy. It protects one secret only. This is a possible add-on for hardening later, not a replacement for (a).

## Recommendation

Adopt **(a), four apps**, with the deploy guide's layout unchanged. Reasons:

- It is the only option that gives isolation using documented, deploy-stable Fly features (facts 1 to 4).
- It costs the same as one app in Fly charges (fact 5). The extra effort is configuration, written once by implementing agents, plus three tokens.
- It matches local compose naturally: one service per app, each with its own env file; `admin` under a compose profile so it only runs on demand.
- It lets every S5, S12 and C5 property be tested by `fly secrets list -a <app>` (the guide's check 9 and 14).

Two simplifications to keep it light: (1) default to building the image per app with `--remote-only` until someone verifies that a single-app deploy token can use `--image` from a sibling app (fact 6); (2) if the project owner prefers fewer moving parts, fall back to (d), not (b).

## What changes in the docs if accepted

- Design Doc: replace "two process groups in one app" with four Fly apps from one codebase (and one image per deployable); note the reason (Fly secrets are app-wide).
- Design Review Findings: amend P3 and S5 decisions to name the four apps; note G2 still resolved.
- `06-deploy-ops.md`: remove the "needs Rodney" flag and the Variant B section (4.5) or mark it rejected; keep section 4.1 table; add the fact 6 caveat to the CI section; update open question 1.
- `00-conventions.md`: record the app names and which secrets each holds, plus "process group names kept as `api` and `poller`".
- Proposal 01: note the poller-leak threat means only the poller app holds the private key; API-leak exposes public key only (already assumed).

If instead (b) or (d) is chosen: delete the checks that assume separate apps (guide 13 checks 9 and 14) and add the exposure to the Design Doc risk list.

## Open risks

- **Cross-app image pull with app-scoped tokens is unverified.** If it fails, the fallback is per-app builds (slower CI, same image from the same commit). Test once with `fly deploy --image` before relying on it.
- **Per-app deploy tokens multiply what a GitHub compromise can do.** Each token can deploy and read secrets of its own app (`fly ssh`, `fly secrets` are not exposed to deploy tokens in the guide's plan, but this was not re-verified). The production environment's required reviewer remains the control.
- **Consistency drift:** the same AWN application key and shared values (`CREDENTIALS_PUBLIC_KEY`, `AWN_APPLICATION_KEY`) must be set on several apps and rotated together; the guide's rotation runbooks (R1, R2) already list this.
- **One-off machines inherit their app's secrets** (poller backfill uses poller secrets, by design). A mistaken `run-oneoff.sh` app argument runs a command with the wrong secrets; the helper script should validate its arguments.
- **Fly may change secrets scoping.** Re-check the docs before merging, and the layout can collapse later: process group names are kept so a future single app remains possible.
