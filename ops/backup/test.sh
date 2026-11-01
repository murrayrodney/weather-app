#!/usr/bin/env bash
# Backup image test (06 section 9.7). Local Docker only: a throwaway network with Postgres, an S3 mock (adobe/s3mock;
# the guide names MinIO, whose public images are no longer published) and a tiny logging HTTP listener (the healthchecks
# stand-in). Touches no Compose project, no host ports, no real AWS.
#   ops/backup/test.sh
# Env (optional): BACKUP_IMAGE (default: build ops/backup/Dockerfile), BACKEND_IMAGE (default: build backend/ for `admin migrate`).
set -euo pipefail
cd "$(dirname "$0")/../.."

SFX="$$"
NET="fwbk-test-$SFX"
DB="fwbk-db-$SFX"; S3C="fwbk-s3-$SFX"; HC="fwbk-hc-$SFX"
BUCKET="test-backups"
TMP="$(mktemp -d)"
PG_IMAGE="${PG_IMAGE:-postgres:17}"
S3_IMAGE="${S3_IMAGE:-adobe/s3mock@sha256:ab01a6946750f451ca215a47e91030695b260e4003b8a5a6201d25029b8fca92}"  # pinned by digest (no release tags are published for it)
NODE_IMAGE="${NODE_IMAGE:-node:24-bookworm-slim}"
fails=0

cleanup() {
  docker rm -f "$DB" "$S3C" "$HC" >/dev/null 2>&1 || true
  docker network rm "$NET" >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

pass() { echo "PASS $1"; }
bad() { echo "FAIL $1" >&2; fails=$((fails + 1)); }
check() { # check <name> <command...>
  local name=$1; shift
  if "$@"; then pass "$name"; else bad "$name"; fi
}

# ---- images
if [ -z "${BACKUP_IMAGE:-}" ]; then
  BACKUP_IMAGE="famweather-backup:test"
  docker build -q -f ops/backup/Dockerfile -t "$BACKUP_IMAGE" . >/dev/null
fi
if [ -z "${BACKEND_IMAGE:-}" ]; then
  BACKEND_IMAGE="famweather-backend:backup-test"
  docker build -q -t "$BACKEND_IMAGE" backend >/dev/null
fi

# ---- throwaway secrets (test values, generated per run)
rnd() { openssl rand -hex 12; }
MIGRATOR_PW="$(rnd)"; API_PW="$(rnd)"; POLLER_PW="$(rnd)"; BACKUP_PW="$(rnd)"; CI_PW="$(rnd)"
PG_SUPER_PW="$(rnd)"; S3_USER="test$(rnd)"; S3_PW="$(rnd)"
verifier() { printf '%s\n' "$1" | python3 ops/db/bootstrap/scram.py; }

docker network create "$NET" >/dev/null

# ---- 1 Postgres, MinIO, listener
docker run -d --name "$DB" --network "$NET" --network-alias db -e POSTGRES_PASSWORD="$PG_SUPER_PW" \
  -v "$PWD/ops:/work/ops:ro" "$PG_IMAGE" >/dev/null
docker run -d --name "$S3C" --network "$NET" --network-alias s3 \
  -e COM_ADOBE_TESTING_S3MOCK_STORE_INITIAL_BUCKETS="$BUCKET" \
  "$S3_IMAGE" >/dev/null
docker run -d --name "$HC" --network "$NET" --network-alias hc "$NODE_IMAGE" node -e \
  "require('http').createServer((q,s)=>{q.resume();console.log(q.method,q.url);s.end('ok')}).listen(8080)" >/dev/null

# Run a tool from the backup image on the test network (root, read-only view of $TMP, MinIO credentials).
tool() {
  docker run --rm -i --network "$NET" --user 0 -v "$TMP:/t:ro" \
    -e AWS_ACCESS_KEY_ID="$S3_USER" -e AWS_SECRET_ACCESS_KEY="$S3_PW" -e AWS_DEFAULT_REGION=us-east-1 \
    -e AWS_ENDPOINT_URL=http://s3:9090 --entrypoint "$@"
}
tool_no_stdin() { tool "$@" </dev/null; }
# Run SQL as the postgres superuser inside the db container (unix socket, trust).
psql_super() { docker exec -i "$DB" psql -X -q -v ON_ERROR_STOP=1 -U postgres "$@"; }

for _ in $(seq 1 60); do
  psql_super -h 127.0.0.1 -Atc 'select 1' >/dev/null 2>&1 && break
  sleep 1
done
psql_super -h 127.0.0.1 -Atc 'select 1' >/dev/null
for _ in $(seq 1 60); do
  tool_no_stdin aws "$BACKUP_IMAGE" s3 ls >/dev/null 2>&1 && break
  sleep 1
done
tool_no_stdin aws "$BACKUP_IMAGE" s3api head-bucket --bucket "$BUCKET" >/dev/null

# ---- 2 database setup, in production order
docker exec -i \
  -e APP_MIGRATOR_PASSWORD_VERIFIER="$(verifier "$MIGRATOR_PW")" -e APP_API_PASSWORD_VERIFIER="$(verifier "$API_PW")" \
  -e APP_POLLER_PASSWORD_VERIFIER="$(verifier "$POLLER_PW")" -e APP_BACKUP_PASSWORD_VERIFIER="$(verifier "$BACKUP_PW")" \
  -e APP_CI_CHECK_PASSWORD_VERIFIER="$(verifier "$CI_PW")" \
  "$DB" psql -X -q -v ON_ERROR_STOP=1 -U postgres -f /work/ops/db/bootstrap/roles.sql >/dev/null
psql_super -f /work/ops/db/tests/auth_stub.sql >/dev/null
psql_super -f /work/ops/db/bootstrap/auth_access.sql >/dev/null
docker run --rm --network "$NET" -e APP_ENV=local \
  -e DATABASE_URL="postgresql://app_migrator:${MIGRATOR_PW}@db:5432/postgres?sslmode=disable" \
  "$BACKEND_IMAGE" /usr/local/bin/admin migrate >/dev/null
# two logins: one live (with a password hash and metadata), one soft-deleted; one app user for the live login
psql_super >/dev/null <<'SQL'
INSERT INTO auth.users (id, email, encrypted_password, raw_user_meta_data, deleted_at) VALUES
  ('00000000-0000-4000-8000-000000000001', 'live@example.test', 'canary-password-hash', '{"note":"canary-metadata"}', NULL),
  ('00000000-0000-4000-8000-000000000002', 'gone@example.test', 'canary-password-hash', '{"note":"canary-metadata"}', now());
INSERT INTO app.users (id, display_name) VALUES ('00000000-0000-4000-8000-000000000001', 'Test Person');
SQL
pass "database setup (roles, auth stub, auth_access, admin migrate, rows)"

# ---- 3 age key and backup runs
docker run --rm --entrypoint age-keygen "$BACKUP_IMAGE" >"$TMP/age.key" 2>/dev/null
RECIPIENT="$(docker run --rm -i --entrypoint age-keygen "$BACKUP_IMAGE" -y <"$TMP/age.key")"
chmod 644 "$TMP/age.key"

DB_URL="postgresql://app_backup:${BACKUP_PW}@db:5432/postgres?sslmode=disable"
ALL_OUT="$TMP/all-output.txt"; : >"$ALL_OUT"
out=""; rc=0
run_backup() { # run_backup <scenario> <database url> [extra docker args...]; sets $out and $rc
  local s=$1 url=$2; shift 2
  rc=0
  out="$(docker run --rm --network "$NET" \
    -e BACKUP_DATABASE_URL="$url" -e BACKUP_S3_BUCKET="$BUCKET" -e BACKUP_AGE_RECIPIENT="$RECIPIENT" \
    -e AWS_ACCESS_KEY_ID="$S3_USER" -e AWS_SECRET_ACCESS_KEY="$S3_PW" -e AWS_DEFAULT_REGION=us-east-1 \
    -e AWS_ENDPOINT_URL=http://s3:9090 -e BACKUP_S3_CONDITIONAL=0 \
    -e HEALTHCHECK_URL_BACKUP="http://hc:8080/$s/backup" -e HEALTHCHECK_URL_DBSIZE="http://hc:8080/$s/dbsize" \
    "$@" "$BACKUP_IMAGE" 2>&1 </dev/null)" || rc=$?
  printf '%s\n' "$out" >>"$ALL_OUT"
}
keys() { tool_no_stdin aws "$BACKUP_IMAGE" s3api list-objects-v2 --bucket "$BUCKET" --query 'Contents[].Key' --output text | tr '\t' '\n'; }
count_keys() { keys | grep -c "$1" || true; }
hc_seq() { sleep 1; docker logs "$HC" 2>&1 | awk -v p="/$1/" 'index($2, p) == 1 { print $2 }'; }
fetch() { tool_no_stdin aws "$BACKUP_IMAGE" s3 cp "s3://$BUCKET/$1" -; }
decrypt() { tool age "$BACKUP_IMAGE" -d -i /t/age.key; }

# 3a default run: app dump and login list
run_backup s1 "$DB_URL"
check "default run exits 0" [ "$rc" = 0 ]
check "default run logs a count of captured logins" grep -q 'auth users captured: 1 (app.users rows: 1)' <<<"$out"
app_key="$(keys | grep '/app-.*\.dump\.age$' | head -1 || true)"
csv_key="$(keys | grep '/auth-users-.*\.csv\.age$' | head -1 || true)"
check "app-*.dump.age object exists" [ -n "$app_key" ]
check "auth-users-*.csv.age object exists" [ -n "$csv_key" ]
fetch "$app_key" | decrypt >"$TMP/app.dump" || bad "dump decrypts"
archive_lists_migrations() { docker run --rm -i --entrypoint pg_restore "$BACKUP_IMAGE" --list <"$TMP/app.dump" | grep '_sqlx_migrations' >/dev/null; }
csv_has_live_only() { grep 'live@example.test' "$TMP/auth.csv" >/dev/null && ! grep 'gone@example.test' "$TMP/auth.csv" >/dev/null; }
no_canary_anywhere() { ! grep canary "$TMP/auth.csv" >/dev/null && ! grep canary "$ALL_OUT" >/dev/null; }
check "dump decrypts to a pg_custom archive listing app._sqlx_migrations" archive_lists_migrations
fetch "$csv_key" | decrypt >"$TMP/auth.csv" || bad "login list decrypts"
check "login list has header id,email,created_at" [ "$(head -1 "$TMP/auth.csv")" = "id,email,created_at" ]
check "login list has exactly one row" [ "$(wc -l <"$TMP/auth.csv" | tr -d ' ')" = 2 ]
check "login list holds the live user and not the soft-deleted one" csv_has_live_only
check "login list and logs hold no password hash or metadata" no_canary_anywhere
check "listener saw /start, DBSIZE success, then BACKUP success" \
  [ "$(hc_seq s1 | paste -sd' ' -)" = "/s1/backup/start /s1/dbsize /s1/backup" ]

# 3b BACKUP_INCLUDE_AUTH=0: no new login list
before_csv="$(count_keys 'auth-users-')"; before_app="$(count_keys '/app-')"
run_backup s2 "$DB_URL" -e BACKUP_INCLUDE_AUTH=0
check "BACKUP_INCLUDE_AUTH=0 run exits 0" [ "$rc" = 0 ]
check "BACKUP_INCLUDE_AUTH=0 adds no login list" [ "$(count_keys 'auth-users-')" = "$before_csv" ]
check "BACKUP_INCLUDE_AUTH=0 adds an app dump" [ "$(count_keys '/app-')" = "$((before_app + 1))" ]

# 3c app_auth.backup_users() fails: exit non-zero, /fail, nothing uploaded
psql_super -c 'REVOKE EXECUTE ON FUNCTION app_auth.backup_users() FROM app_backup' >/dev/null
before_all="$(keys | wc -l | tr -d ' ')"
run_backup s3 "$DB_URL"
psql_super -c 'GRANT EXECUTE ON FUNCTION app_auth.backup_users() TO app_backup' >/dev/null
check "failing backup_users() exits non-zero" [ "$rc" != 0 ]
check "failing backup_users() sends /fail" [ "$(hc_seq s3 | tail -1)" = "/s3/backup/fail" ]
check "failing backup_users() uploads nothing" [ "$(keys | wc -l | tr -d ' ')" = "$before_all" ]

# 3d wrong password: exit non-zero, /fail
run_backup s4 "postgresql://app_backup:wrong-${BACKUP_PW}@db:5432/postgres?sslmode=disable"
check "wrong password exits non-zero" [ "$rc" != 0 ]
check "wrong password sends /fail" [ "$(hc_seq s4 | tail -1)" = "/s4/backup/fail" ]

# 3d2 size check fails (missing size.sql): the backup still succeeds, DBSIZE gets /fail
run_backup s5 "$DB_URL" -e BACKUP_SIZE_SQL=/nonexistent/size.sql
check "failing size check keeps the backup successful" [ "$rc" = 0 ]
check "failing size check sends DBSIZE /fail and BACKUP success" \
  [ "$(hc_seq s5 | paste -sd' ' -)" = "/s5/backup/start /s5/dbsize/fail /s5/backup" ]

# every uploaded object is age-encrypted
unencrypted_keys() { keys | grep -v '\.age$' || true; }
check "every uploaded object key ends in .age" [ -z "$(unencrypted_keys)" ]

# 3e logs hold no secrets
no_secrets_in_logs() { ! grep -F -e "$BACKUP_PW" -e "$S3_PW" -e "$PG_SUPER_PW" "$ALL_OUT" >/dev/null; }
check "container logs contain no DB password or AWS secret" no_secrets_in_logs

if [ "$fails" -ne 0 ]; then echo "backup test: $fails check(s) failed" >&2; exit 1; fi
echo "backup test: all checks passed"
