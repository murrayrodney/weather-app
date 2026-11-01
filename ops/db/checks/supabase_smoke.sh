#!/bin/sh
# Black-box check of hosted Supabase auth / Data API settings (04 section 8.3). HOSTED PROJECTS ONLY: the local
# Caddy serves 200.html for /rest/v1, so this is meaningless locally and the local stack must not run it.
# Inputs: SUPABASE_URL, PUBLIC_SUPABASE_ANON_KEY. Every step must produce a non-2xx status, else exit 1.
# Never prints keys.
set -eu
: "${SUPABASE_URL:?SUPABASE_URL is required}"
: "${PUBLIC_SUPABASE_ANON_KEY:?PUBLIC_SUPABASE_ANON_KEY is required}"
base=${SUPABASE_URL%/}
fail=0

rand() { LC_ALL=C tr -dc 'a-z0-9' </dev/urandom | head -c "$1"; }
probe="probe-$(rand 12)@example.invalid"
pw="$(rand 24)"

# status <name> <curl args...>: prints PASS when the HTTP status is not 2xx.
expect_non2xx() {
  name=$1; shift
  code=$(curl -sS -o /dev/null -w '%{http_code}' --max-time 20 -H "apikey: $PUBLIC_SUPABASE_ANON_KEY" "$@" || echo 000)
  case "$code" in
    2??) echo "FAIL $name: HTTP $code"; fail=1 ;;
    *) echo "PASS $name: HTTP $code" ;;
  esac
}

expect_non2xx "data_api_root" "$base/rest/v1/"
for t in users credentials stations; do
  expect_non2xx "data_api_$t" "$base/rest/v1/$t"
done
expect_non2xx "signup_disabled" -X POST -H 'Content-Type: application/json' \
  -d "{\"email\":\"$probe\",\"password\":\"$pw\"}" "$base/auth/v1/signup"
expect_non2xx "anonymous_signin_disabled" -X POST -H 'Content-Type: application/json' -d '{}' "$base/auth/v1/signup"
expect_non2xx "password_grant_unknown_user" -X POST -H 'Content-Type: application/json' \
  -d "{\"email\":\"$probe\",\"password\":\"$pw\"}" "$base/auth/v1/token?grant_type=password"
# Any status is accepted for the OTP endpoint (it exists but must not create a user: create_user=false).
curl -sS -o /dev/null --max-time 20 -X POST -H "apikey: $PUBLIC_SUPABASE_ANON_KEY" -H 'Content-Type: application/json' \
  -d "{\"email\":\"$probe\",\"create_user\":false}" "$base/auth/v1/otp" || true

echo "REMINDER: run 'admin list-users' on a one-off machine; this check FAILS if its output contains any auth_only line."
exit "$fail"
