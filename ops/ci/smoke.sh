#!/usr/bin/env bash
# Post-deploy smoke test (06 section 13). Unauthenticated and read-only: only GET and HEAD requests.
#   production:  SITE_URL=https://<site> DEPLOY_API=true ops/ci/smoke.sh
#   local stack: SMOKE_LOCAL=1 SITE_URL=http://localhost:8000 ops/ci/smoke.sh
set -euo pipefail
cd "$(dirname "$0")/../.."
api_app="$(sed -n 's/^app = "\(.*\)"/\1/p' ops/fly/api.toml)"
FLY_API_URL="${FLY_API_URL:-https://${api_app}.fly.dev}"
fail=0; ok() { echo "ok   $1"; }; bad() { echo "FAIL $1"; fail=1; }
code() { curl -s -o /dev/null -w '%{http_code}' --max-time 20 "$@"; }

# Fly-direct checks (production only, and only once the API is deployed: DEPLOY_API=true)
if [ "${SMOKE_LOCAL:-0}" != 1 ] && [ "${DEPLOY_API:-false}" = true ]; then
  if [ "$(code "$FLY_API_URL/api/health")" = 200 ]; then ok "fly direct /api/health"; else bad "fly direct /api/health"; fi
  d="$(code "$FLY_API_URL/api/v1/me")"; case "$d" in 401|403) ok "fly direct requires auth ($d)";; *) bad "fly direct /api/v1/me=$d";; esac
fi

# Netlify checks: skipped until SITE_URL is set (before Stage B)
if [ -z "${SITE_URL:-}" ]; then echo "skip Netlify checks (SITE_URL unset)"; exit $fail; fi
if [ "$(code "$SITE_URL/api/health")" = 200 ]; then ok "site /api/health via proxy"; else bad "site /api/health"; fi
if [ "$(code "$SITE_URL/api/v1/me")" = 401 ]; then ok "proxied /api/v1/me without token is 401"; else bad "proxied /api/v1/me not 401"; fi
h="$(curl -sI --max-time 20 "$SITE_URL/" | tr -d '\r')"
names="content-security-policy x-frame-options x-content-type-options referrer-policy"
[ "${SMOKE_LOCAL:-0}" = 1 ] || names="strict-transport-security $names"
for n in $names; do if echo "$h" | grep -qi "^$n:"; then ok "header $n"; else bad "missing header $n"; fi; done
echo "$h" | grep -i '^content-security-policy:' | grep -Eqi "unsafe-inline|unsafe-eval" && bad "CSP header has unsafe-inline/eval"
if [ "$(code "$SITE_URL/some/client/route")" = 200 ]; then ok "SPA fallback"; else bad "SPA fallback"; fi
node ops/ci/check-deployed-csp.mjs || fail=1
exit $fail
