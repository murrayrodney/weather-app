#!/bin/sh
# Exposure check tests (04 section 11.4). Usage: DB=<scratch database> OPS_DIR=<ops/db> sh exposure_test.sh
# Runs the check as app_ci_check (read-only session) against the scratch database; violating changes are made as the
# superuser in a separate connection and reverted. Connects over the unix socket (trust), not TCP.
set -eu
: "${DB:?DB (scratch database name) is required}"
: "${OPS_DIR:=/work/ops/db}"
SOCK=$(psql -X -U postgres -d postgres -Atc 'SHOW unix_socket_directories' | cut -d, -f1)
URL="postgresql://app_ci_check@/${DB}?host=${SOCK}"
su() { psql -X -q -o /dev/null -v ON_ERROR_STOP=1 -U postgres -d "$DB" -c "$1"; }
check() { "$OPS_DIR/checks/exposure.sh" "$URL" 2>&1; }

fails=0
fail() { echo "exposure_test FAIL: $1" >&2; fails=$((fails + 1)); }

expect_pass() {
  if ! out=$(check); then fail "$1: expected the check to pass, got: $out"; fi
}
# expect_violation <label> <pattern> [<pattern>...]: the check must fail and mention every pattern
expect_violation() {
  label=$1; shift
  if out=$(check); then fail "$label: expected the check to fail"; return; fi
  for pat in "$@"; do
    printf '%s' "$out" | grep -q -- "$pat" || fail "$label: output lacks $pat: $out"
  done
}
# case <label> <bad sql> <revert sql> <pattern>...
case_() {
  label=$1; bad=$2; revert=$3; shift 3
  su "$bad"
  expect_violation "$label" "$@"
  su "$revert"
  expect_pass "$label (reverted)"
}

# 1 baseline
expect_pass "baseline"

# 2 a table in public
su "CREATE TABLE public.leak (id int)"
expect_violation "public table" OBJECT_IN_PUBLIC EXPOSED_TABLE_WITHOUT_RLS
su "ALTER TABLE public.leak ENABLE ROW LEVEL SECURITY"
if out=$(check); then fail "public table with RLS: expected OBJECT_IN_PUBLIC"; else
  printf '%s' "$out" | grep -q OBJECT_IN_PUBLIC || fail "public table with RLS lacks OBJECT_IN_PUBLIC"
  printf '%s' "$out" | grep -q EXPOSED_TABLE_WITHOUT_RLS && fail "public table with RLS still reports EXPOSED_TABLE_WITHOUT_RLS"
fi
su "DROP TABLE public.leak"
expect_pass "public table dropped"

# 3 exposed schema list
case_ "pgrst.db_schemas" "ALTER ROLE authenticator SET pgrst.db_schemas = 'public,app'" \
  "ALTER ROLE authenticator RESET pgrst.db_schemas" SCHEMA_EXPOSED

# 4 watched roles
case_ "anon table grant" "GRANT SELECT ON app.users TO anon" "REVOKE SELECT ON app.users FROM anon" WATCHED_ROLE_PRIVILEGE
case_ "authenticated column grant" "GRANT SELECT (id) ON app.users TO authenticated" "REVOKE SELECT (id) ON app.users FROM authenticated" WATCHED_ROLE_PRIVILEGE
case_ "PUBLIC execute app.can_view" "GRANT EXECUTE ON FUNCTION app.can_view(uuid,uuid) TO PUBLIC" "REVOKE EXECUTE ON FUNCTION app.can_view(uuid,uuid) FROM PUBLIC" WATCHED_ROLE_PRIVILEGE
case_ "anon usage on app_auth" "GRANT USAGE ON SCHEMA app_auth TO anon" "REVOKE USAGE ON SCHEMA app_auth FROM anon" WATCHED_ROLE_PRIVILEGE
case_ "PUBLIC execute session_active" "GRANT EXECUTE ON FUNCTION app_auth.session_active(uuid,uuid) TO PUBLIC" "REVOKE EXECUTE ON FUNCTION app_auth.session_active(uuid,uuid) FROM PUBLIC" WATCHED_ROLE_PRIVILEGE
case_ "service_role sequence grant" "GRANT USAGE ON SEQUENCE app.audit_log_id_seq TO service_role" "REVOKE USAGE ON SEQUENCE app.audit_log_id_seq FROM service_role" WATCHED_ROLE_PRIVILEGE

# 5 the API's column privileges
case_ "api select credentials" "GRANT SELECT ON app.credentials TO app_api" "REVOKE SELECT ON app.credentials FROM app_api" API_COLUMN_PRIVILEGE
case_ "api update key_version" "GRANT UPDATE (key_version) ON app.credentials TO app_api" "REVOKE UPDATE (key_version) ON app.credentials FROM app_api" API_COLUMN_PRIVILEGE
case_ "api update stations.owner_id" "GRANT UPDATE (owner_id) ON app.stations TO app_api" "REVOKE UPDATE (owner_id) ON app.stations FROM app_api" API_COLUMN_PRIVILEGE

# 6 ownership
case_ "table owner" "ALTER TABLE app.users OWNER TO postgres" "ALTER TABLE app.users OWNER TO app_migrator" RELATION_NOT_OWNED_BY_MIGRATOR
case_ "function owner" "ALTER FUNCTION app.can_view(uuid,uuid) OWNER TO postgres" "ALTER FUNCTION app.can_view(uuid,uuid) OWNER TO app_migrator" FOREIGN_OWNED_FUNCTION

# 7 SECURITY DEFINER hygiene
case_ "definer in app" "CREATE FUNCTION app.evil() RETURNS int LANGUAGE sql SECURITY DEFINER AS 'SELECT 1'" "DROP FUNCTION app.evil()" SECURITY_DEFINER_OUTSIDE_APP_AUTH
case_ "extra app_auth function" "CREATE FUNCTION app_auth.extra() RETURNS int LANGUAGE sql AS 'SELECT 1'" "DROP FUNCTION app_auth.extra()" UNEXPECTED_APP_AUTH_FUNCTION
case_ "session_active search_path" "ALTER FUNCTION app_auth.session_active(uuid,uuid) RESET search_path" "ALTER FUNCTION app_auth.session_active(uuid,uuid) SET search_path = ''" APP_AUTH_FUNCTION_NOT_HARDENED
case_ "backup_users search_path" "ALTER FUNCTION app_auth.backup_users() RESET search_path" "ALTER FUNCTION app_auth.backup_users() SET search_path = ''" APP_AUTH_FUNCTION_NOT_HARDENED
case_ "revoke_sessions to api" "GRANT EXECUTE ON FUNCTION app_auth.revoke_sessions(uuid) TO app_api" "REVOKE EXECUTE ON FUNCTION app_auth.revoke_sessions(uuid) FROM app_api" APP_AUTH_EXECUTE_GRANT
case_ "backup_users to api" "GRANT EXECUTE ON FUNCTION app_auth.backup_users() TO app_api" "REVOKE EXECUTE ON FUNCTION app_auth.backup_users() FROM app_api" APP_AUTH_EXECUTE_GRANT
case_ "backup_users to poller" "GRANT EXECUTE ON FUNCTION app_auth.backup_users() TO app_poller" "REVOKE EXECUTE ON FUNCTION app_auth.backup_users() FROM app_poller" APP_AUTH_EXECUTE_GRANT
case_ "backup_users to PUBLIC" "GRANT EXECUTE ON FUNCTION app_auth.backup_users() TO PUBLIC" "REVOKE EXECUTE ON FUNCTION app_auth.backup_users() FROM PUBLIC" WATCHED_ROLE_PRIVILEGE

# 8 GoTrue tables
case_ "api on auth.sessions" "GRANT SELECT ON auth.sessions TO app_api" "REVOKE SELECT ON auth.sessions FROM app_api" APP_ROLE_PRIVILEGE_ON_AUTH

# 9 the check performs no writes
if grep -qiE 'create (temp|temporary|table)|insert into|delete from|update ' "$OPS_DIR/checks/exposure.sql"; then
  fail "exposure.sql contains a write statement"
fi
if out=$(PGOPTIONS='-c default_transaction_read_only=on' psql -X -v ON_ERROR_STOP=1 -U app_ci_check -d "$DB" -c 'CREATE TEMP TABLE zz_probe (i int)' 2>&1); then
  fail "the check's read-only session accepted a write"
else
  printf '%s' "$out" | grep -qi 'read-only' || fail "unexpected error for a write in the read-only session: $out"
fi

expect_pass "final state"
if [ "$fails" -ne 0 ]; then echo "exposure_test: $fails failure(s)" >&2; exit 1; fi
echo "exposure_test: OK"
