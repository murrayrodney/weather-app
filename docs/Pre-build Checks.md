# Pre-build Checks

Two checks to run before building. Each takes 15–30 minutes, and both use throwaway accounts or projects that you delete afterwards. Runbook step 0 in `guides/06-deploy-ops.md` (section 15.1) points here.

You don't need to interpret the results. Fill in the "Result" lines and hand them back; I'll update the guides.

---

## Check 1: Supabase lets our locked-down function read login sessions (proposal 03)

**Why:** the API asks the database "is this login session still valid?" on every request. On hosted Supabase, our database logins may be blocked from Supabase's login tables. The plan is a small function that has permission to answer yes or no. Local Docker can't test this, because there the admin login can do anything.

### Setup
1. In the Supabase dashboard, create a new **free** project named `scratch-auth-test`. Don't use the real project.
2. Open **SQL Editor** in that project.

### Step A: what can the `postgres` login do? (paste, run, copy the output)
```sql
select rolsuper, rolbypassrls from pg_roles where rolname = 'postgres';
select relname, relacl from pg_class where oid in ('auth.sessions'::regclass, 'auth.users'::regclass);
```
Result: ______

### Step B: can we grant direct table access? (expected to fail; that's fine)
```sql
create role t_api nologin;
grant select (id, user_id, not_after) on auth.sessions to t_api;
```
Result (success, or the error text): ______

### Step C: the function itself (the important one)
Run this as one block:
```sql
create schema app_auth;
create function app_auth.session_active(s uuid, u uuid) returns boolean
  language sql security definer set search_path = '' as $$
  select exists (select 1 from auth.sessions
                 where id = s and user_id = u and (not_after is null or not_after > now()))
$$;
revoke all on function app_auth.session_active(uuid, uuid) from public;
grant usage on schema app_auth to t_api;
grant execute on function app_auth.session_active(uuid, uuid) to t_api;
grant t_api to postgres;  -- lets the editor act as t_api; ignore an error here
```
Then run each of these on its own:
```sql
set role t_api; select app_auth.session_active(gen_random_uuid(), gen_random_uuid()); reset role;
```
**Expected:** `false`, no error.
```sql
set role t_api; select * from auth.sessions; reset role;
```
**Expected:** a permission error. That's good: the role can ask the function, but can't read the table.

Result: ______

### Step D: a real login, then sign-out
1. Go to **Authentication → Users → Add user → Create new user**. Use any email and password and tick **Auto confirm**.
2. Sign that user in from a terminal. Take the URL and **anon key** from **Project Settings → API**, and put the throwaway email and password in place of the placeholders:
   ```bash
   curl -s -X POST "https://<ref>.supabase.co/auth/v1/token?grant_type=password" -H "apikey: <anon key>" -H "Content-Type: application/json" -d '{"email":"<email>","password":"<password>"}'
   ```
   Save the `refresh_token` from the response.
3. Back in the SQL Editor, find the session and check it:
   ```sql
   select id, user_id from auth.sessions;
   select app_auth.session_active('<id>', '<user_id>');
   ```
   **Expected:** `true`.
4. Create the "sign out everywhere" function and run it:
   ```sql
   create function app_auth.revoke_sessions(u uuid) returns int
     language sql security definer set search_path = '' as $$
     with d as (delete from auth.sessions where user_id = u returning 1) select count(*)::int from d
   $$;
   select app_auth.revoke_sessions('<user_id>');
   select app_auth.session_active('<id>', '<user_id>');
   ```
   **Expected:** `1`, then `false`.
5. Confirm the old refresh token no longer works:
   ```bash
   curl -s -X POST "https://<ref>.supabase.co/auth/v1/token?grant_type=refresh_token" -H "apikey: <anon key>" -H "Content-Type: application/json" -d '{"refresh_token":"<refresh_token>"}'
   ```
   **Expected:** an error.

Result: ______

### Step E (optional): ban and delete
Use the **secret** (service role) key from **Project Settings → API**. Sign in again first (step D2) to get a fresh session.
```bash
curl -s -X PUT "https://<ref>.supabase.co/auth/v1/admin/users/<user_id>" -H "apikey: <secret key>" -H "Authorization: Bearer <secret key>" -H "Content-Type: application/json" -d '{"ban_duration":"24h"}'
```
Then try the refresh from step D5 with the new refresh token. **Expected:** refused.

Then delete the user:
```bash
curl -s -X DELETE "https://<ref>.supabase.co/auth/v1/admin/users/<user_id>" -H "apikey: <secret key>" -H "Authorization: Bearer <secret key>"
```
Run `select count(*) from auth.sessions;` in the SQL Editor. **Expected:** `0`.

Result: ______

### Cleanup
Delete the `scratch-auth-test` project: **Project Settings → General → Delete project**.

**If step C fails** (the function errors instead of returning `false`), stop and tell me. The plan falls back to a weaker session check, and that needs your OK.

---

## Check 2: GitHub Actions can publish the website to Netlify (proposal 04)

**Why:** the plan is that Netlify publishes only a `production` branch, and only the deploy workflow moves that branch forward. This needs no Netlify token in GitHub. We haven't confirmed that Netlify publishes when it's the workflow, not a person, that pushes.

### Setup
1. On GitHub, create a new **public** repo named `netlify-gate-test` with a README. It must be public: branch rules don't apply to private repos on GitHub's free plan.
2. Clone it and add two files:

   `index.html`
   ```html
   <h1>version 1</h1>
   ```

   `.github/workflows/publish.yml`
   ```yaml
   name: publish
   on: workflow_dispatch
   permissions:
     contents: write
   jobs:
     publish:
       runs-on: ubuntu-latest
       steps:
         - uses: actions/checkout@v4
           with:
             fetch-depth: 0
         - run: git push origin "$GITHUB_SHA:refs/heads/production"
   ```
3. Commit and push to `main`, then create the branch:
   ```bash
   git push origin main:production
   ```
4. In Netlify, go to **Add new site → Import from Git** and pick the repo. Leave the build command empty and set the publish directory to `.`.
5. In **Site configuration → Build & deploy → Branches and deploy contexts**:
   - Production branch: `production`.
   - Branch deploys: **None**.
   - Deploy previews: any.
6. Open the site URL. It should show "version 1".

### Test 1: pushing to main publishes nothing
Change `index.html` to `version 2`, then commit and push to `main`.
**Expected:** Netlify's **Deploys** tab shows no new production deploy, and the site still shows "version 1".

### Test 2: the workflow publishes
In GitHub, go to **Actions → publish → Run workflow** on `main`.
**Expected:** the workflow passes, Netlify starts a production deploy, and within a minute or two the site shows "version 2".

Result: ______

### Test 3: with the branch rule on, the workflow still works and force pushes fail
1. In GitHub, go to **Settings → Rules → Rulesets → New branch ruleset**:
   - Name: `production`.
   - Enforcement: **Active**.
   - Target: include by pattern `production`.
   - Tick **Restrict deletions** and **Block force pushes**. Leave everything else unticked.
2. Change `index.html` to `version 3`, push to `main`, and run the workflow again.
   **Expected:** it passes and the site shows "version 3".
3. From your laptop, try rewinding the branch:
   ```bash
   git push --force origin HEAD~1:production
   ```
   **Expected:** rejected by the ruleset.

Result: ______

### Cleanup
Delete the Netlify site (**Site configuration → Danger zone → Delete**) and the GitHub repo (**Settings → Danger zone → Delete**).

**If test 2 fails** (the workflow passes but Netlify doesn't publish), stop and tell me. The fallback puts a Netlify token in GitHub, and that needs your OK.
