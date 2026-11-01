#!/bin/sh
# Usage: check-secret-logs.sh "<compose command prefix>"   (05 section 5.6)
# Greps the logs of db, auth, migrate and api for demo plaintext passwords, the JWT secret, the seeded poller key and the
# sentinel prefix `canary-secret-`. Prints only the matched NAME, never the value. Exit 1 on any hit.
set -eu
cd "$(dirname "$0")/.."/..
prefix=${1:?usage: check-secret-logs.sh "<compose command prefix>"}
# shellcheck disable=SC2086
logs=$($prefix logs --no-color db auth migrate api 2>/dev/null || true)
rc=0
hit() { echo "secret-log check: $1 found in container logs" >&2; rc=1; }

names=$(awk '{print $2}' backend/demo-fingerprints.txt | grep -E '(PASSWORD|JWT_HS256_SECRET)$' || true)
for name in $names; do
  value=$(grep -E "^${name}=" .env.example | head -1 | cut -d= -f2-)
  [ -n "$value" ] || continue
  printf '%s' "$logs" | grep -qF -- "$value" && hit "$name"
done
printf '%s' "$logs" | grep -qF -- 'fixture-poller-key' && hit "fixture-poller-key"
printf '%s' "$logs" | grep -qF -- 'canary-secret-' && hit "canary-secret-"
# Set-password tokens minted by the admin compose tests (one per line) must not appear in any container log.
tokens=${CANARY_TOKENS_FILE:-.local/canary-tokens}
if [ -f "$tokens" ]; then
  n=0
  while IFS= read -r t; do
    [ -n "$t" ] || continue
    n=$((n + 1))
    printf '%s' "$logs" | grep -qF -- "$t" && hit "set-password token"
  done < "$tokens"
  echo "secret-log check: $n set-password token(s) checked against the container logs"
  rm -f "$tokens"
fi
exit "$rc"
