# Proposal 05: How family members log in

**Status:** Accepted by Rodney, Oct 7, 2026 (b2: set-password links sent by hand), with no separate scratch project (Rodney, Oct 7): the unverified items are settled by tests written into the build against the local GoTrue in Docker Compose (same software as hosted; 04 `admin invite` / `admin reset-link` compose tests, 01 set-password e2e) and by two dashboard settings checked during the real Supabase project setup (06 section 7, step 5). If a hosted setting cannot be turned off, fall back to (b1). **Growth path (Rodney):** b2 is for a few people; beyond that, add custom SMTP (Amazon SES is acceptable then) and turn on emailed links. Keep the design so that this is a settings change plus small code paths, not a rewrite. Research date: 2026-10-07. Doc pages were read through a fetch tool that summarises them, and GoTrue behaviour comes from reading its source on GitHub; anything I could not see is marked unverified.

## Plain-language summary

- The problem: sign-in by emailed link needs a real email service. Supabase's free built-in sender only mails people on your Supabase team (2 per hour), so the guides picked Amazon SES. You do not want more AWS.
- Every non-AWS email service I checked also needs you to own a domain name and add DNS records. Email is the expensive part, not the login.
- You do not need email. **Recommendation: email + password where each person sets their own password.** When you invite someone, the command prints a one-time "set your password" link. You text it to them (or hand it over); they open it, choose a password, done. You never learn their password.
- Forgot a password? You run one command, get a new link, text it. Signed-in people can change their password any time in the app.
- Cost: no AWS, no domain, no email provider, less code than today. Trade-off: "forgot password" means asking you, and the link expires (about an hour by default), so you send it when they are ready.
- Fallback: Resend or Brevo (free) plus a domain of about $10-15 a year (price unverified) to keep emailed links. Optional later add-on: "Sign in with Google".

## Problem

`01-web` 4.2 signs in with `signInWithOtp` (magic link, PKCE). `04-database` 10.3 says `admin invite` sends no email and the person requests their own link, so every login needs working SMTP. 06 section 5 step 5 and the earlier drafts propose SES for that. Rodney asked whether staying inside Supabase, or a simpler method than magic links, would do, and whether users can set their own password.

## Verified facts

From docs and source read on 2026-10-07.

1. **Supabase built-in email.** [Auth SMTP](https://supabase.com/docs/guides/auth/auth-smtp): only addresses on the project's organization team are accepted (others fail with "Email address not authorized"); 2 messages per hour, can change without notice, best effort, "not for production". [Rate limits](https://supabase.com/docs/guides/auth/rate-limits) agrees. With custom SMTP any address works and a new setup starts at 30 per hour. There is no way to use the built-in sender for non-team family.
2. **Adding family to the Supabase org is a bad idea.** [Access control](https://supabase.com/docs/guides/platform/access-control): Developer can read the service and anon API keys and manage project data; Administrator can change auth settings and the database; the Read-only role (which cannot read keys) is Team/Enterprise only, and free plans cannot restrict a member to one project. Each family member would get the service key or more. Reject.
3. **Providers** (Supabase's recommended list on the Auth SMTP page: Resend, SES, Postmark, SendGrid, ZeptoMail, Brevo).
   - [Resend pricing](https://resend.com/pricing): free is 100 emails/day, 3,000/month, 3 domains. [Resend domains](https://resend.com/docs/dashboard/domains/introduction): "add and verify at least one domain" to send. (A test sender `onboarding@resend.dev` exists to my recollection but only to your own address; unverified.)
   - [Brevo](https://www.brevo.com/free-smtp-server): free SMTP relay, 300/day, no expiry (Brevo's page); the free tier adds Brevo branding to transactional mail (fetch summary of [pricing](https://www.brevo.com/pricing/)); DKIM on a domain you own is expected, otherwise mail goes out from a Brevo domain (third-party guides; unverified). A `gmail.com` sender cannot be authenticated since you do not control that DNS (my inference).
   - [Postmark](https://postmarkapp.com/pricing): free Developer plan is 100 emails/month, no overage, no expiry; domain requirements not stated on the page (unverified). Tight for 10 people.
   - Not researched: SendGrid, ZeptoMail, Mailgun.
   - Conclusion: no durable free option sends from a shared domain without caveats, so Rodney needs a domain for any email route. Gmail SMTP with an app password is described by Supabase's [Google SMTP note](https://supabase.com/docs/guides/troubleshooting/using-google-smtp-with-supabase-custom-smtp-ZZzU4Y) for Workspace accounts only and called fragile; not recommended.
4. **Admin-created users.** Admin API `createUser` takes `password` and `email_confirm: true` ([reference examples](https://supabase.com/docs/reference/javascript/auth-admin-createuser); fields not described individually). The admin update handler ([admin.go](https://github.com/supabase/auth/blob/master/internal/api/admin.go)) accepts `password` (runs the strength check) and `ban_duration`. Creating a user without a password is allowed by GoTrue as far as I know (unverified; step 1 of the check).
5. **`generate_link` sends nothing and returns the token.** [mail.go](https://github.com/supabase/auth/blob/master/internal/api/mail.go): `POST /auth/v1/admin/generate_link` with `type` (`signup`, `invite`, `magiclink`, `recovery`, `email_change_*`), `email`, optional `redirect_to`. It builds the link and returns JSON (`action_link`, `email_otp`, `hashed_token`, `verification_type`, `redirect_to`); the handler does not send mail. `recovery` needs an existing user; `invite` fails for an already-confirmed user. The [reference](https://supabase.com/docs/reference/javascript/auth-admin-generatelink) describes it as for sending "via a custom email provider". Not shown: whether SMTP must be configured for the endpoint to work (unverified; the scratch check settles this), the default token lifetime (the Auth settings call it "Email OTP expiration"; I recall 1 hour default and 24 hours maximum, unverified), and whether a used token is invalidated (unverified).
6. **A token can be redeemed directly in the app, with no Supabase redirect.** [verify.go](https://github.com/supabase/auth/blob/master/internal/api/verify.go): `POST /auth/v1/verify` accepts `type=recovery` with `token_hash` and returns a session; expired or invalid tokens give 403 `otp_expired`. supabase-js `verifyOtp({ token_hash, type: 'recovery' })` is documented for email links ([email templates](https://supabase.com/docs/guides/auth/auth-email-templates) shows `token_hash`/`type` handed to `verifyOtp`), so the app can build its own link `https://<site>/set-password#token_hash=...` from `hashed_token` and never use Supabase's `action_link`. That avoids the redirect allow-list, the PKCE exchange and mail-scanner prefetch, and the token sits in the URL fragment, so Netlify never receives it. The session created this way is marked as OTP, not "recovery" (verify.go).
7. **Users changing their own password.** [user.go](https://github.com/supabase/auth/blob/master/internal/api/user.go): `updateUser({ password })` works for a signed-in user. Two settings can block it. If "Secure password change"-style reauthentication is on, the call needs an emailed nonce when the session is more than 24 hours old (error `reauthentication_needed`). That would need SMTP, so the setting must be off. If "require current password" is on and the user already has a password, `current_password` is required; this only applies when a password exists, and is not waived for sessions from our reset link (verify.go marks them OTP, not recovery). The family's reset flow needs it off, or the user cannot set a new password without knowing the old one. Where these two switches live in the dashboard, and whether free-plan projects can change them, I did not confirm (unverified). Users can also write their own `user_metadata` through `updateUser`, but not `app_metadata` (403, admin only).
8. **Password policy and brute force.** [Password security](https://supabase.com/docs/guides/auth/password-security): minimum length and required character classes are settable; "Leaked password protection is available on the Pro Plan and above", so not on free. [Rate limits](https://supabase.com/docs/guides/auth/rate-limits): sign-in endpoints are limited per IP (30 requests per 5 minutes); I found no per-account lockout. Free-plan ability to change length or the limits: unverified. So protection rests on password length, enforced by the Supabase setting and also in the app form (a client check alone is bypassable).
9. **Sign-up toggle.** [General configuration](https://supabase.com/docs/guides/auth/general-configuration): with "Allow new users to sign up" off, only existing users can sign in. Password sign-in for admin-created users is unaffected (standard behaviour; confirm in the check).
10. **Google sign-in with sign-ups off.** GoTrue [external.go](https://github.com/supabase/auth/blob/master/internal/api/external.go): with sign-ups disabled a new Google identity gets "Signups not allowed for this instance"; an existing user with the same email is linked without checking that flag. [Identity linking](https://supabase.com/docs/guides/auth/auth-identity-linking): linking by email happens only for verified emails. So an invited, confirmed user can sign in with Google and a stranger cannot. [Google audience help](https://support.google.com/cloud/answer/15549945): Testing mode has 100 test users and 7-day expiry, but neither applies if the app asks only for `openid`/email/profile; the page gives no exact verification rule for basic scopes. [Supabase Google guide](https://supabase.com/docs/guides/auth/social-login/auth-google): needs a Google Cloud project, consent screen and Web client ID; the consent screen shows `<project>.supabase.co` unless you use a custom domain (paid, unverified). Live behaviour is unverified.
11. **Phone/SMS.** [Phone login](https://supabase.com/docs/guides/auth/phone-login): MessageBird, Twilio, Vonage, TextLocal; no free provider named. Dropped.
12. **Redirect allow-list.** [Redirect URLs](https://supabase.com/docs/guides/auth/redirect-urls) applies to OAuth, magic-link and reset flows (it lists Netlify preview wildcards). `signInWithPassword` and `verifyOtp` with `token_hash` are direct calls with no redirect.

**Unverified, settled by the build-time tests and the project setup (see "Verification" below):** `generate_link` works with no SMTP configured; token lifetime and single use; creating a user with no password; where the "require recent login / current password" switches live and that both can be off on free; minimum-length setting on free; banned users cannot sign in; Google linking.

## Options compared

| | (a) Magic link + custom SMTP | (b1) Temp password from admin | (b2) Admin link, user sets own password | (c) Google sign-in | (d) Org members / (e) SMS |
|---|---|---|---|---|---|
| AWS | No | No | No | No | No |
| Domain needed | Yes | No | No | No | No / No |
| Extra accounts for Rodney | Mail provider + registrar | None | None | Google Cloud project | None / SMS provider |
| Rodney learns the password | No | Yes | **No** | n/a | n/a |
| Forced "set your own" on first login | n/a | Soft only (see below) | **Built in** (no password exists until set) | n/a | n/a |
| Forgot password | Emailed link | Admin command | Admin command, new link | Google's own | n/a |
| Link/secret expires | Yes | No, until changed | Yes (about 1 hour, unverified) | n/a | n/a |
| Needs "reauth by email" off | n/a | Yes | Yes | n/a | n/a |
| Code change vs 01-web | None | Small | Small (set-password page) | Moderate | n/a |
| Security vs today | Baseline | Weaker (shared secret, may linger) | Comparable | Comparable or better | Rejected |

**(a) Magic link + Resend or Brevo.** Keeps the guides as written except SES. Costs a domain, DNS records, a provider account, and email deliverability on every login. Good fallback.

**(b1) Temp password.** Invite generates a random password and creates the user with `email_confirm: true`; Rodney hands it over; the SPA forces a change on first login. How the app knows: a flag the invite command sets in `app.users` (`must_change_password`, written by `app_migrator`) cannot be cleared by the user's own token or by the API (no service key, no write path from the user), so the clear step would need a new API endpoint that trusts the user's claim "I changed it" (the API cannot see the Supabase password). A flag in `user_metadata` can be set and cleared by the user in the same `updateUser({ password, data: { password_set: true } })` call, but also bypassed by them, and `app_metadata` (admin only) can only be cleared by an admin. So enforcement is soft: the SPA nags, the API cannot enforce. The temp password remains valid until changed, Rodney knows it, and it travels in clear text. Simplest to build, weakest.

**(b2) Admin link, user sets own password (recommended).**
- `admin invite`: create the user with `email_confirm: true` and no password, insert the app rows as today, call `generate_link` with `type: recovery` (the user exists by then; `invite` type would also work but `recovery` is the same flow reused for resets), and print `https://<site>/set-password#token_hash=<hashed_token>&type=recovery`. The command never prints or stores the token elsewhere; it is not in the audit row.
- `/set-password` page: on load read the fragment, call `verifyOtp({ token_hash, type: 'recovery' })`, remove the fragment from the URL, show "Choose a password" (min length enforced in the form and by the Supabase setting), call `updateUser({ password })`, then continue to the dashboard. Expired or used link shows "That link has expired. Ask Rodney for a new one."
- First login is forced by construction: until the link is used there is no password, so there is no other way in. No flag is needed.
- Forgot password: `admin reset-link --email` calls `generate_link` (recovery) and then `revoke_sessions`; Rodney texts the new link. Changing password while signed in: `updateUser({ password })` (add `current_password` only if the Supabase setting is left on, which breaks the reset flow; see fact 7).
- Rodney should hand the link over only when the person is ready, and say it expires. Anyone who gets the link first can set the password, so use a private channel (Signal, iMessage to the right number, in person). This is the same exposure as (b1) but time limited, and the secret is not a password.
- Weak points: link lifetime is short (regenerate if missed); single use unverified, so treat the link as live until it expires; no email recovery for a lost phone number change; users still choose their own passwords, with no leaked-password check on free (length rule is the control).

**(c) Google.** Nicest for people with Google accounts, but needs a Google Cloud project and consent screen, a plan for relatives without one, and rests on source-reading, not a documented guarantee. Best as an optional add-on to (b2).

**(d), (e)** rejected (facts 2, 11).

## Recommendation

**Adopt (b2): email + password, where `admin invite` and `admin reset-link` print a one-time set-password link that Rodney sends out of band.** It needs no email provider, no domain and no AWS; Rodney never sees anyone's password; first login is forced to choose one. Sign-ups stay off, and `is_active` plus the per-request `session_active` check remain the real gate, so deactivation behaves as before.

If the scratch check shows `generate_link` needs SMTP, or the switches in fact 7 cannot be turned off on free, fall back to (b1) (temp password, soft first-login nudge), which has no such dependency. If Rodney prefers emailed links or a relative cannot manage passwords, fall back to (a) with Resend or Brevo and a domain; `app.users` and the invite code do not change between them. Google sign-in (c) can be added beside any of these.

## What changes in the guides if accepted

- **01-web:** 4.2 login becomes `signInWithPassword({ email, password })` with one generic error for wrong email or wrong password; remove `requestMagicLink` and the `weather.next` second-tab handoff and "same browser" text (`?next=` now stays in the same tab); the `/auth/callback` route and tests are replaced by `/set-password` (token in fragment, `verifyOtp`, form, `updateUser`); add "Change password" on the account page and "Ask Rodney to reset it" on the login page (no forgot-password email link); redirect allow-list text, including the Netlify preview wildcard, goes. CSP `connect-src` is unchanged (same Supabase host). e2e: drop `helpers/mailpit.ts`; `helpers/invite.ts` reads the printed link from the CLI instead.
- **02-api:** no change to JWT and `session_active`. The gotrue round-trip already needs the password grant.
- **04-database:** 10.3 `invite` creates the user without a password, then calls `generate_link` and prints the link once; add `admin reset-link --email` (recovery link, then `app_auth.revoke_sessions`); audit `user.invited` and new `user.reset_link_issued` carry no token. `deactivate-user` unchanged (ban plus `revoke_sessions`). Backup open question: back up `app.users` only and re-invite after an auth-side loss (password hashes need no backup).
- **00-conventions:** CLI list gets `reset-link`; drop `GOTRUE_MAILER_URLPATHS_*` and the redirect allow-list text; the `mail` (Mailpit) service row is removed. Local GoTrue needs password grant on and the same two switches off.
- **05-local-dev:** remove the mail catcher and Mailpit smoke steps; seed users get demo passwords (local only, fingerprinted per the demo-value rule).
- **06-deploy-ops:** section 5 step 5 (custom SMTP) and the SES instructions are deleted; add "Auth settings: sign-ups off, email provider on, minimum password length 12 (if settable), reauthentication-by-email and require-current-password off, Email OTP expiration as wanted"; Site URL set; no redirect URLs needed.

## Verification (decided Oct 7: no scratch project, nothing for Rodney to run before the build)

The checklist that used to be a scratch-project check is split in two, and neither is a pre-build step.

**A. Tests in the build, against the local GoTrue in Docker Compose (same software as hosted).** Written as explicit tests: 04 section 11 (`admin invite` / `admin reset-link` compose tests) and 01 section 7 (set-password e2e).
1. Sign-ups off. Create a user with `email_confirm: true` and no password: succeeds.
2. `generate_link` (`recovery`) with no SMTP configured: returns `hashed_token`, sends nothing, no SMTP error. Record the default link lifetime from the GoTrue setting actually in force.
3. `verifyOtp({token_hash, type:'recovery'})` returns a session. Repeat with the same token: record whether it is rejected (single use). Past expiry: `otp_expired`.
4. `updateUser({password})`, then `signInWithPassword` works; wrong password and unknown email give the same message; a too-short password is rejected.
5. A user who already has a password can set a new one through a fresh reset link without giving the old one (with the two switches off in the local config).
6. `ban_duration` via the admin update: sign-in and refresh are refused; `app_auth.revoke_sessions` ends existing sessions.

**B. Hosted settings, checked while setting up the real project (06 section 7, step 5).** The reauthentication ("secure password change") switch, the "require current password" switch, and the minimum password length: confirm each is visible and can be set on the free plan. If either switch cannot be turned off, fall back to option (b1) (see Recommendation). If the minimum length cannot be raised, keep the length rule in the app form and the API-side check and note it in the security notes.

**Optional, for (c) Google sign-in:** not part of the build; a later add-on.
