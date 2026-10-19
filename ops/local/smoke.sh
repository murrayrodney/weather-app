#!/bin/bash
# Smoke test for the local stack (05 section 5.8). Local only: refuses any BASE that is not localhost.
#   ./ops/local/smoke.sh [--clean] [--replay] [--keep]
# Output: "PASS <name>" or "FAIL <name>: <reason>"; exits non-zero on the first failure.
set -euo pipefail
cd "$(dirname "$0")/../.."

CLEAN=0 REPLAY=0 KEEP=0
for a in "$@"; do
  case "$a" in
    --clean) CLEAN=1 ;;
    --replay) REPLAY=1 ;;
    --keep) KEEP=1 ;;
    *) echo "usage: smoke.sh [--clean] [--replay] [--keep]" >&2; exit 2 ;;
  esac
done

BASE=${BASE:-http://localhost:8000}
SMOKE_AUTHED_ROUTE=${SMOKE_AUTHED_ROUTE:-/api/v1/me}
case "$BASE" in http://localhost:*|http://127.0.0.1:*) ;; *) echo "smoke: BASE must be a local address" >&2; exit 2 ;; esac
C="docker compose"
EPOCH=$(date +%s)
STARTED=$(date -u +%Y-%m-%dT%H:%M:%SZ)
EMAIL="smoke-${EPOCH}@example.test"
NAME="Smoke-${EPOCH}"
TMPS=()
cleanup_tmp() { for f in "${TMPS[@]:-}"; do [ -n "$f" ] && rm -rf "$f"; done; }
trap cleanup_tmp EXIT

pass() { echo "PASS $1"; }
fail() { echo "FAIL $1: $2" >&2; exit 1; }
env_get() { grep -E "^$1=" "$2" | head -1 | cut -d= -f2- | sed -e "s/^'//" -e "s/'$//"; }
# run a command with a time limit (macOS has no `timeout`)
run_limited() { local secs=$1; shift; perl -e 'alarm shift; exec @ARGV' "$secs" "$@"; }
http_code() { curl -s -o /dev/null -w '%{http_code}' "$@"; }
listening() { (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null; }

# ---- 1 preflight
for tool in jq curl openssl shasum perl; do command -v "$tool" >/dev/null || fail preflight "$tool is missing"; done
ver=$($C version --short 2>/dev/null | sed 's/^v//')
[ -n "$ver" ] || fail preflight "docker compose is missing"
[ "$(printf '%s\n2.24\n' "$ver" | sort -V | head -1)" = "2.24" ] || fail preflight "compose $ver is older than 2.24"
if [ "$CLEAN" = 1 ]; then
  echo "smoke --clean: this will DELETE the local stack's volumes (make reset), after saving .env to .local/ and resetting it to .env.example"
  sleep 5
  mkdir -p .local
  [ -f .env ] && cp .env ".local/env.$(date +%Y%m%d%H%M%S).bak"
  cp .env.example .env
  make reset >/dev/null 2>&1
fi
if [ -z "$($C ps -q caddy 2>/dev/null)" ]; then
  for p in 8000 5432; do listening "$p" && fail preflight "port $p is in use by something else"; done
fi
pass preflight

# ---- 2 up
[ -f web/build/200.html ] || make web-build >/dev/null
make up >/dev/null
for s in auth-access migrate; do
  id=$($C ps -a -q "$s")
  [ "$(docker inspect -f '{{.State.ExitCode}}' "$id")" = 0 ] || fail up "$s did not exit 0"
done
pass up

# ---- 3 fingerprints
while read -r hash name; do
  [ -n "${name:-}" ] || continue
  val=$(env_get "$name" .env.example)
  [ -n "$val" ] || fail fingerprints "$name is missing from .env.example"
  got=$(printf '%s' "$val" | shasum -a 256 | cut -d' ' -f1)
  [ "$got" = "$hash" ] || fail fingerprints "$name no longer matches backend/demo-fingerprints.txt"
done < backend/demo-fingerprints.txt
pass fingerprints

# ---- 4 health
[ "$(http_code "$BASE/healthz")" = 200 ] || fail health "/healthz"
body=$(curl -s "$BASE/api/health"); [ "$(http_code "$BASE/api/health")" = 200 ] || fail health "/api/health"
[ "$(echo "$body" | jq 'del(.status) | length' 2>/dev/null || echo 0)" = 0 ] || fail health "/api/health returns data fields"
[ "$(curl -s "$BASE/auth/v1/health" | jq -r .name)" = GoTrue ] || fail health "/auth/v1/health is not GoTrue"
ct=$(curl -s -o /dev/null -w '%{content_type}' "$BASE/"); case "$ct" in text/html*) ;; *) fail health "/ is $ct" ;; esac
curl -s "$BASE/" | cmp -s - web/build/200.html || fail health "/ differs from web/build/200.html"
curl -s "$BASE/some/deep/route" | cmp -s - web/build/200.html || fail health "deep route is not the SPA fallback"
nope=$(curl -s -o /dev/null -w '%{http_code} %{content_type}' "$BASE/api/nope"); case "$nope" in "401 application/json"*|"404 application/json"*) ;; *) fail health "/api/nope is $nope" ;; esac
asset=$(cd web/build && find _app/immutable -name '*.js' | head -1)
[ "$(http_code "$BASE/$asset")" = 200 ] || fail health "asset $asset"
[ "$(http_code "$BASE/_app/immutable/missing.js")" = 404 ] || fail health "missing chunk is not 404"
pass health

# ---- 5 headers
hdrs=$(curl -s -D - -o /dev/null "$BASE/" | tr -d '\r')
csp=$(jq -r '.csp | to_entries | map(([.key] + .value) | join(" ")) | join("; ")' web/security-headers.json)
hv() { echo "$hdrs" | grep -i "^$1:" | head -1 | cut -d: -f2- | sed 's/^ //'; }
[ "$(hv Content-Security-Policy)" = "$csp" ] || fail headers "CSP differs from security-headers.json"
while IFS=$'\t' read -r k v; do
  [ "$(hv "$k")" = "$v" ] || fail headers "$k differs"
done < <(jq -r '.headers_all | to_entries[] | [.key, .value] | @tsv' web/security-headers.json)
echo "$hdrs" | grep -qi '^strict-transport-security:' && fail headers "HSTS present locally"
echo "$hdrs" | grep -qi '^server:' && fail headers "Server header present"
[ "$(hv Cache-Control)" = "$(jq -r .expected_html_cache_control.caddy web/security-headers.json)" ] || fail headers "HTML Cache-Control"
ac=$(curl -s -D - -o /dev/null "$BASE/$asset" | tr -d '\r' | grep -i '^cache-control:' | cut -d: -f2- | sed 's/^ //')
[ "$ac" = "$(jq -r '.cache_control["/_app/immutable/*"]' web/security-headers.json)" ] || fail headers "asset Cache-Control"
case "$csp" in *unsafe-eval*|*default-src*) fail headers "CSP has unsafe-eval or default-src" ;; esac
[ "$(echo "$csp" | grep -o "unsafe-[a-z-]*" | sort -u | tr '\n' ' ')" = "unsafe-hashes " ] || fail headers "unexpected unsafe-* token"
pass headers

# ---- 6 signup_disabled
sc=$(http_code -X POST "$BASE/auth/v1/signup" -H 'content-type: application/json' -d "{\"email\":\"x-${EPOCH}@example.test\",\"password\":\"x12345678\"}")
case "$sc" in 4*) ;; *) fail signup_disabled "signup returned $sc" ;; esac
sc=$(http_code -X POST "$BASE/auth/v1/otp" -H 'content-type: application/json' -d "{\"email\":\"stranger-${EPOCH}@example.test\",\"create_user\":true}")
case "$sc" in 4*) ;; *) fail signup_disabled "otp create_user returned $sc" ;; esac
users=$($C --profile tools run --rm -T admin admin list-users 2>/dev/null || true)
echo "$users" | grep -E "x-${EPOCH}@|stranger-${EPOCH}@" && fail signup_disabled "a user was created"
pass signup_disabled

# ---- 7 invite_prints_link
out=$($C --profile tools run --rm -T admin admin invite --email "$EMAIL" --display-name "$NAME" 2>/dev/null)
LINK=$(echo "$out" | grep -E "^http://localhost:8000/set-password#token_hash=" || true)
[ "$(echo "$LINK" | grep -c .)" = 1 ] || fail invite_prints_link "expected exactly one set-password link"
users=$($C --profile tools run --rm -T admin admin list-users 2>/dev/null || true)
echo "$users" | awk -F'\t' -v n="$NAME" -v e="$EMAIL" '$2==n && $3==e && $4=="active" && $6=="auth_ok" {f=1} END {exit !f}' || fail invite_prints_link "user row not found or not active/auth_ok"
ANON=$(env_get PUBLIC_SUPABASE_ANON_KEY .env)
pw_grant() { curl -s -o /tmp/smoke-grant.$$ -w '%{http_code}' -X POST "$BASE/auth/v1/token?grant_type=password" -H "apikey: $ANON" -H 'content-type: application/json' -d "{\"email\":\"$1\",\"password\":\"$2\"}"; }
TMPS+=("/tmp/smoke-grant.$$")
case "$(pw_grant "$EMAIL" "whatever-password-1")" in 4*) ;; *) fail invite_prints_link "password grant worked before the link was used" ;; esac
pass invite_prints_link

# ---- 8 set_password_login
frag_token() { echo "$1" | sed -n 's/.*token_hash=\([^&]*\).*/\1/p'; }
PW1=$(openssl rand -base64 24 | tr -dc 'A-Za-z0-9' | head -c 20)
TH=$(frag_token "$LINK")
v=$(curl -s -X POST "$BASE/auth/v1/verify" -H "apikey: $ANON" -H 'content-type: application/json' -d "{\"type\":\"recovery\",\"token_hash\":\"$TH\"}")
AT=$(echo "$v" | jq -r '.access_token // empty'); [ -n "$AT" ] || fail set_password_login "verify returned no access_token"
[ "$(http_code -X PUT "$BASE/auth/v1/user" -H "apikey: $ANON" -H "Authorization: Bearer $AT" -H 'content-type: application/json' -d "{\"password\":\"$PW1\"}")" = 200 ] || fail set_password_login "PUT /user failed"
[ "$(pw_grant "$EMAIL" "$PW1")" = 200 ] || fail set_password_login "password grant failed with the new password"
RT=$(jq -r .refresh_token /tmp/smoke-grant.$$)
pw_grant "$EMAIL" "wrong-password-xyz-1" >/dev/null; wrong=$(jq -S -c '{error_code, msg}' /tmp/smoke-grant.$$)
pw_grant "nobody-${EPOCH}@example.test" "wrong-password-xyz-1" >/dev/null; unknown=$(jq -S -c '{error_code, msg}' /tmp/smoke-grant.$$)
[ "$wrong" = "$unknown" ] || fail set_password_login "wrong password and unknown email answer differently"
case "$wrong" in *invalid_credentials*) ;; *) fail set_password_login "expected invalid_credentials, got $wrong" ;; esac
second=$(http_code -X POST "$BASE/auth/v1/verify" -H "apikey: $ANON" -H 'content-type: application/json' -d "{\"type\":\"recovery\",\"token_hash\":\"$TH\"}")
echo "NOTE set_password_login: a second verify of the same token_hash returned HTTP $second ($([ "$second" = 200 ] && echo 'NOT single use' || echo 'single use'))"
out=$($C --profile tools run --rm -T admin admin reset-link --email "$EMAIL" 2>/dev/null)
LINK2=$(echo "$out" | grep -E "^http://localhost:8000/set-password#token_hash=" || true)
[ "$(echo "$LINK2" | grep -c .)" = 1 ] || fail set_password_login "reset-link did not print one link"
rc=$(http_code -X POST "$BASE/auth/v1/token?grant_type=refresh_token" -H "apikey: $ANON" -H 'content-type: application/json' -d "{\"refresh_token\":\"$RT\"}")
case "$rc" in 4*) ;; *) fail set_password_login "the old refresh token still works after reset-link (HTTP $rc)" ;; esac
PW2=$(openssl rand -base64 24 | tr -dc 'A-Za-z0-9' | head -c 20)
v=$(curl -s -X POST "$BASE/auth/v1/verify" -H "apikey: $ANON" -H 'content-type: application/json' -d "{\"type\":\"recovery\",\"token_hash\":\"$(frag_token "$LINK2")\"}")
AT2=$(echo "$v" | jq -r '.access_token // empty'); [ -n "$AT2" ] || fail set_password_login "reset link did not redeem"
[ "$(http_code -X PUT "$BASE/auth/v1/user" -H "apikey: $ANON" -H "Authorization: Bearer $AT2" -H 'content-type: application/json' -d "{\"password\":\"$PW2\"}")" = 200 ] || fail set_password_login "password change without the old password failed"
pass set_password_login

# ---- 9 authed_api
[ "$(pw_grant "$EMAIL" "$PW2")" = 200 ] || fail authed_api "could not sign in with the final password"
TOKEN=$(jq -r .access_token /tmp/smoke-grant.$$)
[ "$(http_code "$BASE$SMOKE_AUTHED_ROUTE" -H "Authorization: Bearer $TOKEN")" = 200 ] || fail authed_api "valid token is not 200"
[ "$(http_code "$BASE$SMOKE_AUTHED_ROUTE")" = 401 ] || fail authed_api "no header is not 401"
[ "$(http_code "$BASE$SMOKE_AUTHED_ROUTE" -H "Authorization: Bearer $ANON")" = 401 ] || fail authed_api "anon key is not 401"
tampered="${TOKEN%?}$([ "${TOKEN: -1}" = A ] && echo B || echo A)"
[ "$(http_code "$BASE$SMOKE_AUTHED_ROUTE" -H "Authorization: Bearer $tampered")" = 401 ] || fail authed_api "tampered token is not 401"
b64u() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }
payload=$(echo "$TOKEN" | cut -d. -f2)
none="$(printf '{"alg":"none","typ":"JWT"}' | b64u).${payload}."
[ "$(http_code "$BASE$SMOKE_AUTHED_ROUTE" -H "Authorization: Bearer $none")" = 401 ] || fail authed_api "alg:none token is not 401"
pass authed_api

# ---- 10 poller_idle
pid=$($C ps -q poller)
penv=$(docker inspect --format '{{json .Config.Env}}' "$pid")
if echo "$penv" | grep -q 'POLLER_ENABLED=false'; then
  [ "$(docker inspect -f '{{.State.Status}} {{.State.Health.Status}}' "$pid")" = "running healthy" ] || fail poller_idle "poller is not running and healthy"
  echo "$penv" | grep -q 'AWN_MODE=replay' || fail poller_idle "AWN_MODE is not replay"
  $C logs poller 2>&1 | grep -q 'poller disabled: POLLER_ENABLED=false' || fail poller_idle "missing the disabled log line"
  sleep 20
  n=$($C exec -T -u postgres db psql -X -At -d postgres -c "select count(*) from app.job_runs where started_at > '$STARTED'")
  [ "$n" = 0 ] || fail poller_idle "$n job_runs started while the poller is disabled"
  if [ "$CLEAN" = 1 ] && [ "$REPLAY" = 0 ]; then
    t=$($C exec -T -u postgres db psql -X -At -d postgres -c "select (select count(*) from app.job_runs) + (select count(*) from app.observations)")
    [ "$t" = 0 ] || fail poller_idle "job_runs/observations are not empty after --clean"
  fi
  pass poller_idle
else
  echo "SKIP poller_idle: the poller was enabled by make poller-replay or make live ($(echo "$penv" | grep -o 'POLLER_ENABLED=[a-z]*'))"
fi

# ---- 11 env_isolation
cenv() { docker inspect --format '{{json .Config.Env}}' "$($C ps -a -q "$1")"; }
e=$(cenv api); echo "$e" | grep -qE 'CREDENTIALS_PRIVATE_KEY|SUPABASE_SERVICE_ROLE_KEY' && fail env_isolation "api holds a private or service key"
e=$(cenv poller); echo "$e" | grep -q 'SUPABASE_SERVICE_ROLE_KEY' && fail env_isolation "poller holds the service key"
e=$(cenv migrate); extra=$(echo "$e" | jq -r '.[]' | grep -E '^(APP_|DATABASE_URL|RUST_LOG|CREDENTIALS|JWT|SUPABASE|AWN|POLLER)' | grep -vE '^(APP_ENV|DATABASE_URL|RUST_LOG)=' || true)
[ -z "$extra" ] || fail env_isolation "migrate has extra app variables"
e=$(cenv db); echo "$e" | jq -r '.[]' | grep -E '^APP_[A-Z_]+_PASSWORD=' && fail env_isolation "db holds an app role plaintext password"
for s in api poller migrate admin; do
  id=$($C ps -a -q "$s"); [ -n "$id" ] || continue
  u=$(docker inspect --format '{{json .Config.Env}}' "$id" | jq -r '.[]' | grep '^DATABASE_URL=' | head -1)
  [ -z "$u" ] || echo "$u" | grep -q '@db:5432/' || fail env_isolation "$s DATABASE_URL host is not db"
done
pass env_isolation

# ---- 12 refusal_rules
IMG=weather-backend:local
a=$(run_limited 10 $C run --rm --no-deps -T -e DATABASE_URL='postgres://app_api:x@prod.example.com:5432/postgres' api api 2>&1) && fail refusal_rules "(a) api accepted a non-local DATABASE_URL"
echo "$a" | grep -qi 'DATABASE_URL' || fail refusal_rules "(a) the message does not name DATABASE_URL"
b=$(run_limited 10 $C run --rm --no-deps -T -e AWN_MODE=live -e AWN_APPLICATION_KEY=REPLACE_WITH_DEV_APPLICATION_KEY poller poller 2>&1) && fail refusal_rules "(b) poller accepted live mode with the placeholder key"
echo "$b" | grep -qi 'AWN_APPLICATION_KEY' || fail refusal_rules "(b) the message does not name AWN_APPLICATION_KEY"
ENVF=$(mktemp); EMPTY=$(mktemp); TMPS+=("$ENVF" "$EMPTY")
RANDPW=$(openssl rand -hex 12)
cat > "$ENVF" <<EOF
APP_ENV=production
DATABASE_URL=postgres://app_api:${RANDPW}@db.example.invalid:5432/postgres?sslmode=verify-full&sslrootcert=/etc/ssl/app-certs/supabase-ca.crt
SUPABASE_URL=https://example.invalid
SUPABASE_JWKS_URL=https://example.invalid/auth/v1/.well-known/jwks.json
AWN_MODE=live
AWN_APPLICATION_KEY=prod-looking-key-0123456789abcdef
CREDENTIALS_KEY_VERSION=1
CREDENTIALS_PUBLIC_KEY=$(env_get CREDENTIALS_PUBLIC_KEY .env.example)
EOF
c=$(run_limited 10 docker run --rm --env-file "$ENVF" -v "$EMPTY:/etc/ssl/app-certs/supabase-ca.crt:ro" "$IMG" api 2>&1) && fail refusal_rules "(c) production start accepted a demo value"
echo "$c" | grep -q 'refusing CREDENTIALS_PUBLIC_KEY: demo value' || fail refusal_rules "(c) expected 'refusing CREDENTIALS_PUBLIC_KEY: demo value'"
pass refusal_rules

# ---- 13 secret_hygiene
logs=$($C logs api poller auth 2>&1 || true)
for name in JWT_HS256_SECRET AWN_APPLICATION_KEY; do
  val=$(env_get "$name" .env.example)
  [ -z "$val" ] || ! printf '%s' "$logs" | grep -qF -- "$val" || fail secret_hygiene "$name appears in the logs"
done
printf '%s' "$logs" | grep -qF 'fixture-poller-key' && fail secret_hygiene "fixture-poller-key appears in the logs"
pass secret_hygiene

# ---- 14 replay
if [ "$REPLAY" = 1 ]; then
  make seed >/dev/null && make e2e-data >/dev/null || fail replay "make seed / make e2e-data failed"
  pass replay
fi

# ---- 15 teardown
if [ "$CLEAN" = 1 ] && [ "$KEEP" = 0 ]; then
  make reset >/dev/null 2>&1
  pass teardown
fi
echo "smoke: all checks passed"
