#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

: "${BACKUP_DATABASE_URL:?}" "${BACKUP_S3_BUCKET:?}" "${BACKUP_AGE_RECIPIENT:?}"
PREFIX="${BACKUP_S3_PREFIX:-weather}"
INCLUDE_AUTH="${BACKUP_INCLUDE_AUTH:-1}"      # default ON (owner decision, Oct 7); 0 only for a database without app_auth.backup_users()
CONDITIONAL="${BACKUP_S3_CONDITIONAL:-1}"      # 0 only for the local MinIO test
SIZE_SQL="${BACKUP_SIZE_SQL:-/opt/checks/size.sql}"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT

ping() { # ping <url-var-name> <suffix> [body]
  local url="${!1:-}"; [ -n "$url" ] || return 0
  curl -fsS -m 10 --retry 3 -o /dev/null --data-raw "${3:-}" "${url}${2:-}" || true
}
fail() { echo "backup failed: $1" >&2; ping HEALTHCHECK_URL_BACKUP /fail "$1"; exit 1; }
trap 'fail "unexpected error at line $LINENO"' ERR
ping HEALTHCHECK_URL_BACKUP /start

rflags=(); IFS=',' read -ra recips <<<"$BACKUP_AGE_RECIPIENT"
for r in "${recips[@]}"; do rflags+=(-r "$r"); done

server_major=$(( $(psql "$BACKUP_DATABASE_URL" -Atc 'show server_version_num') / 10000 ))
client_major=$(pg_dump --version | sed -E 's/.* ([0-9]+)[.].*/\1/')
[ "$client_major" -ge "$server_major" ] || fail "pg_dump $client_major older than server $server_major"

TS="$(date -u +%Y%m%dT%H%M%SZ)"; YM="$(date -u +%Y/%m)"; RAND="$(head -c 4 /dev/urandom | od -An -tx1 | tr -d ' \n')"
files=(app.dump)       # each entry is "<name>.<ext>"; uploaded as "<name>-<ts>-<rand>.<ext>.age"
pg_dump --dbname="$BACKUP_DATABASE_URL" --format=custom --compress=6 --no-owner \
  --schema=app --file="$WORK/app.dump"
if [ "$INCLUDE_AUTH" = 1 ]; then
  # Logins: exactly id, email, created_at from the hardened definer function app_auth.backup_users() (04 6.5), executable by app_backup only.
  # No password hashes, tokens or metadata. The CSV never leaves $WORK unencrypted (umask 077, removed after age).
  psql -X -v ON_ERROR_STOP=1 "$BACKUP_DATABASE_URL" --csv \
    -c "select id, email, created_at from app_auth.backup_users()" > "$WORK/auth-users.csv" \
    || fail "auth user capture failed (app_auth.backup_users)"
  auth_rows=$(( $(wc -l < "$WORK/auth-users.csv") - 1 ))
  app_rows=$(psql -X -Atc "select count(*) from app.users" "$BACKUP_DATABASE_URL")
  # first-deploy sanity (compare with the dashboard user count too): some logins must exist when app users exist
  [ "$auth_rows" -ge 1 ] || [ "$app_rows" -eq 0 ] || fail "auth capture returned no users but app.users has $app_rows"
  echo "auth users captured: $auth_rows (app.users rows: $app_rows)"   # counts only, never emails
  files+=(auth-users.csv)
fi

for f in "${files[@]}"; do
  name="${f%.*}"; ext="${f##*.}"
  if [ "$ext" = dump ]; then pg_restore --list "$WORK/$f" >/dev/null || fail "$f failed integrity listing"; fi
  age "${rflags[@]}" -o "$WORK/$f.age" "$WORK/$f"
  rm -f "$WORK/$f"
  [ "$(stat -c %s "$WORK/$f.age")" -gt 100 ] || fail "$f.age suspiciously small"
  # random suffix + conditional write: an existing object is never overwritten
  cond=(); [ "$CONDITIONAL" = 1 ] && cond=(--if-none-match '*')
  aws s3api put-object --bucket "$BACKUP_S3_BUCKET" --key "${PREFIX}/${YM}/${name}-${TS}-${RAND}.${ext}.age" \
    --body "$WORK/$f.age" --server-side-encryption AES256 --checksum-algorithm SHA256 \
    "${cond[@]}" >/dev/null || fail "upload of $f failed"
done

# size alert via 04's size.sql (raises above 400 MB); output is table sizes only
if out="$(psql -X -v ON_ERROR_STOP=1 "$BACKUP_DATABASE_URL" -f "$SIZE_SQL" 2>&1)"; then
  ping HEALTHCHECK_URL_DBSIZE "" "$(printf '%s' "$out" | head -c 1000)"
else
  ping HEALTHCHECK_URL_DBSIZE /fail "$(printf '%s' "$out" | head -c 1000)"
fi
ping HEALTHCHECK_URL_BACKUP "" "ok ${TS}"
echo "backup ok ${TS}"
