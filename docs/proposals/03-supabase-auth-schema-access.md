# Proposal 03: How the app reaches Supabase's `auth` schema

**Status:** Accepted by Rodney, Oct 5, 2026 (the scratch Supabase project check remains a required pre-build verification). Research date: 2026-10-03.

## Problem

The guides give `app_api` and `app_migrator` direct grants on Supabase-owned tables (`auth.sessions`, `auth.users`), applied by `auth_grants.sql` as `supabase_auth_admin`. On hosted Supabase the operator cannot log in as `supabase_auth_admin`, and it is unproven that `postgres` can grant on tables it does not own. If the grants fail, the per-request session check (S2), session deletion on deactivation (S11) and the FK to `auth.users` all break, and local compose (where we own everything) would hide the problem.

## Verified facts

Confirmed from docs I read this session:

1. **The April 2025 restriction is narrower than the reviewer says.** Per [discussion 34270](https://github.com/orgs/supabase/discussions/34270): since 21 Apr 2025 you cannot create/drop tables or functions, create indexes, or write to migration tables in `auth`, `storage`, `realtime`, nor revoke API-role privileges there. Still allowed: "foreign keys referencing tables in" those schemas, and RLS policies and triggers on specific tables such as `auth.users`. The discussion says nothing about GRANT. So FKs to `auth.users` are explicitly permitted; creating our function **inside** `auth` is not (ours would live elsewhere).
2. **`session_id` is a JWT claim.** [JWT fields](https://supabase.com/docs/guides/auth/jwt-fields) lists it as always present and not removable.
3. **Access tokens outlive revoked sessions.** [Signing out](https://supabase.com/docs/guides/auth/signout): global sign-out destroys refresh tokens, but "Access Tokens of revoked sessions remain valid until their expiry". [Discussion 13941](https://github.com/orgs/supabase/discussions/13941): Supabase does not check the session on access-token use; deleting rows by hand does not log the client out. This is exactly why our own per-request check is needed (the Rust API is the only party that checks).
4. **Admin sign-out needs the user's JWT, not a user id.** [`auth.admin.signOut(jwt, scope)`](https://supabase.com/docs/reference/javascript/auth-admin-signout) takes "a valid, logged-in JWT". Reading the GoTrue source ([logout.go](https://github.com/supabase/auth/blob/master/internal/api/logout.go)): scopes are global (default), local, others; it removes sessions (and revokes refresh tokens). [api.go](https://github.com/supabase/auth/blob/master/internal/api/api.go) shows admin routes for users, factors, passkeys, generate_link, SSO, OAuth clients and **no** admin route to delete a user's sessions by id. So an operator who only has a user id cannot revoke sessions via the Admin API.
5. **Foreign-key and trigger guidance.** [Managing user data](https://supabase.com/docs/guides/auth/managing-user-data): reference only `auth.users`'s primary key (others "may change at any time"), `ON DELETE CASCADE` recommended, trigger functions as `SECURITY DEFINER` with `search_path = ''`. Staff confirm triggers on `auth.users` work from the `postgres` role ([discussion 38887](https://github.com/orgs/supabase/discussions/38887)).
6. **`postgres` is not the owner and not a superuser.** Same discussion: `must be owner of table users` (42501) and role memberships such as `supabase_auth_admin` "are reserved, only superusers can grant them". Staff guidance on direct access ([26973](https://github.com/orgs/supabase/discussions/26973)) points to security definer functions rather than grants to other roles.

**Unverified (the scratch-project checklist below settles these):**

- Whether `postgres` can `GRANT SELECT/REFERENCES/DELETE ... ON auth.*` to a custom role. Nothing I found says yes or no; it depends on whether `postgres` holds the privileges WITH GRANT OPTION. Inference: probably not, since it is not the owner.
- Whether a `postgres`-owned `SECURITY DEFINER` function can read and delete `auth.sessions`. Inference: likely yes (reading `auth.users` this way is documented, and `postgres` normally has table privileges on `auth.*`), but I found no doc naming `auth.sessions`.
- Whether `postgres` bypasses RLS on `auth.sessions` (it has BYPASSRLS on hosted, by my recollection, not confirmed).
- That `ban_duration` stops refresh and that deleting a user cascades to `auth.sessions` (both match how GoTrue behaves as I understand it; not confirmed against source this session).

**Local compose.** Plain Postgres container: `postgres` is a real superuser and `supabase_auth_admin` owns the GoTrue tables, so every option works locally. Local therefore proves nothing about hosted.

## Options compared

| | (a) Direct grants | (b) Definer functions in `app_auth` | (c) Drop session check | (d) No FK to `auth.users` |
|---|---|---|---|---|
| Works on hosted today | Unverified; likely fails | Likely (checklist item 2-4) | Yes | Yes |
| S2 per-request session check | Yes | Yes | No (replaced by short expiry) | n/a |
| S11 immediate removal | Yes | Yes | `is_active` only; sessions linger | Yes |
| Least privilege (S12) | Column grants | Boolean only; no table access | Best | Best |
| Local parity | Needs `supabase_auth_admin` path | Same SQL, run as `postgres` both places | Same | Same |
| Complexity | Low | Low-medium | Lowest | Lowest |

**(a) Direct grants.** What the guides have now. Needs a login we cannot use on hosted, and forks the bootstrap into a local path (`supabase_auth_admin`) and a hosted path (`postgres`, may fail). Keep only as a fallback if the checklist shows `postgres` can grant.

**(b) Definer functions.** A schema `app_auth`, created by `postgres` (allowed: it is not `auth`), holding `session_active(session uuid, uid uuid) returns boolean` and `revoke_sessions(uid uuid) returns int`. Both `SECURITY DEFINER`, `SET search_path = ''`, every object schema-qualified, `REVOKE ALL ... FROM PUBLIC`, `EXECUTE` granted only to `app_api` (check) and `app_migrator` (revoke); `USAGE` on the schema likewise. `app_api` never touches `auth.sessions`, so S12 improves: no column grant to audit, and the function returns only true/false. Owner is `postgres`, whose own privileges on `auth.sessions` are the only thing that must hold (unverified, see above). Same file runs on local and hosted. Risk: GoTrue column renames; the guide already has a startup self-test and the checklist covers it.

**(c) Drop the check.** Rely on `app.users.is_active` (checked per request, so removed users lose access immediately regardless) plus the one-hour access-token expiry. What is lost: a user who signs out everywhere, or whose session is revoked in Supabase, still works for up to an hour. For a family app that may be acceptable, but it reverses decision S2. Good as a degraded mode, not as the plan.

**(d) FK vs none.** An FK needs the *creating* role to hold REFERENCES on `auth.users`, and `app_migrator` cannot be given it unless `postgres` can grant (unverified). Workaround: have `postgres` add the constraint in the bootstrap step, which requires it to own or be a member of the role owning `app.users`: more moving parts. Without the FK, integrity is app-level: `admin invite` and `admin delete-user` are the only writers; an `admin doctor` check (via a third definer function, or the Admin API list-users) finds orphans. Dashboard deletion of a user then no longer fails loudly; but `is_active` plus the session check mean a deleted Supabase user cannot reach the API anyway (their sessions are gone and `sub` stops resolving at refresh).

## Recommendation

**(b) plus (d), with the FK as an optional upgrade.**

- Session check and revocation go through `app_auth` definer functions. This is the pattern Supabase staff point to, avoids needing grants from a role we cannot use, and gives identical SQL on hosted and local.
- Drop the FK by default. Keep `ON DELETE RESTRICT` as a bonus only if checklist step 5 succeeds cheaply; do not design the bootstrap around it.
- Deactivation (`admin deactivate-user`) does three things: set `is_active=false` (immediate, the real S11 guarantee); call `app_auth.revoke_sessions(uid)` (stops refresh, so the user is out within the token lifetime even if the flag were skipped); and set `ban_duration` via the Admin API as belt and braces (best effort, warn on failure).
- If checklist 3 fails (definer function cannot read `auth.sessions`), fall back to (c) and record it as a downgrade of S2 in the review findings.
- Reduce exposure regardless of option: set the JWT expiry to the shortest the owner will tolerate (the dashboard default is one hour).
- `app_backup` currently reads `auth.users`/`auth.identities` directly: it has the same problem. Either give it a definer function that returns `(id, email)` only, or back up `app.users` alone and rebuild logins by re-inviting (simpler; owner decision).

## What changes in the guides if accepted

- **04-database**: replace 6.5 `auth_grants.sql` with `ops/db/bootstrap/auth_schema.sql` run as `postgres` everywhere (locally by the `migrate` one-shot using the superuser, hosted by the operator); create `app_auth` and the two functions; remove the `supabase_auth_admin` path and the "hosted verification task" fallback text; drop `REFERENCES auth.users` from `app.users` (section 5, line ~198 and the RESTRICT paragraph); update the role table (section 4), the privilege tests (row 14 becomes: `app_api` can `EXECUTE app_auth.session_active`, cannot `SELECT` any `auth.*`), and `auth_stub.sql` (the stub needs the functions too).
- **02-api**: section 4.4 query becomes `SELECT app_auth.session_active($1,$2)` (or a one-row join with `app.users` and the boolean); startup self-test calls the function instead of `SELECT ... FROM auth.sessions`; update the table at line ~77 and the S2 row.
- **05-local-dev**: remove `AUTH_GRANTS_DATABASE_URL`/`AUTH_GRANTS_FILE` and the `supabase_auth_admin` connection from `migrate`; replace with `AUTH_SCHEMA_DATABASE_URL` as the Postgres superuser (or run the file in the one-shot with `psql`); resolves open question 3. `SUPABASE_AUTH_ADMIN_PASSWORD` stays for GoTrue itself.
- **06-deploy-ops**: runbook step "run `auth_schema.sql` as `postgres` over the direct connection, once, after GoTrue has migrated"; rerun after Supabase major auth upgrades; add the checklist below as the first-deploy gate; `deactivate-user` runbook includes the Admin API ban call.
- **00-conventions**: role table (lines ~109-119) and the "auth schema owned by GoTrue" sentence; `bootstrap/` filenames.

## Verify on a scratch hosted project (about 10 minutes)

Create a free project, open the SQL editor (runs as `postgres`) or connect with the direct `postgres` URL, then:

1. `\dp auth.sessions` and `\dp auth.users` (or `select relacl from pg_class where oid='auth.sessions'::regclass;`). Note whether `postgres=...*` (grant option) appears. Also `select rolsuper, rolbypassrls from pg_roles where rolname='postgres';`.
2. `create role t_api nologin; grant select (id,user_id,not_after) on auth.sessions to t_api;` Expect success or 42501. (Settles option (a).)
3. `create schema app_auth; create function app_auth.session_active(s uuid, u uuid) returns boolean language sql security definer set search_path='' as $$ select exists(select 1 from auth.sessions where id=s and user_id=u and (not_after is null or not_after>now())) $$; revoke all on function app_auth.session_active(uuid,uuid) from public; grant execute on function app_auth.session_active(uuid,uuid) to t_api;` then `set role t_api; select app_auth.session_active(gen_random_uuid(), gen_random_uuid());` Expect `false`, no error; `select * from auth.sessions` as `t_api` must fail.
4. Sign in a test user (magic link or dashboard-created password), decode the access token and confirm `session_id`; call the function with it: `true`. Then `create function app_auth.revoke_sessions(u uuid) returns int language sql security definer set search_path='' as $$ with d as (delete from auth.sessions where user_id=u returning 1) select count(*)::int from d $$;` run it, re-check: `false`; confirm refresh with the old refresh token now fails.
5. Optional FK: `create table public.t(id uuid primary key references auth.users(id));` as `postgres` (docs say allowed); then try `grant references (id) on auth.users to t_api;`.
6. Admin API: `PUT /auth/v1/admin/users/<id>` with `{"ban_duration":"24h"}` using the secret key; try refreshing the session afterwards (expect refusal), and `DELETE /auth/v1/admin/users/<id>` then confirm `auth.sessions` rows are gone.
7. Clean up: drop the scratch project. Record the results in 04 section 6.5.
