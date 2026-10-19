#!/bin/sh
# SQL-level database tests (04 section 11.1). POSIX sh; needs only sh, psql and the Postgres server binaries.
# Run as the `postgres` OS user inside the throwaway test project's db container (repo mounted read-only at /work):
#   docker compose -p famweather-test ... exec -T -u postgres -e MIGRATIONS_DIR=... -e OPS_DIR=... db sh /work/ops/db/tests/run.sh
# It may re-run roles.sql and change role passwords freely: the cluster is throwaway.
set -eu
: "${MIGRATIONS_DIR:=/work/backend/migrations}"
: "${OPS_DIR:=/work/ops/db}"
export OPS_DIR

PSQL="psql -X -q -v ON_ERROR_STOP=1"
SUPER="$PSQL -U postgres"
BINDIR=$(pg_config --bindir 2>/dev/null || ls -d /usr/lib/postgresql/*/bin | tail -1)
PRIV=/tmp/privcluster
PRIVPORT=55432
DB="dbtest_$(date +%s)_$$"
step() { printf '\n== %s\n' "$*"; }
die() { echo "FAIL: $*" >&2; exit 1; }

cleanup() {
  rc=$?
  "$BINDIR/pg_ctl" -D "$PRIV" -m immediate stop >/dev/null 2>&1 || true
  rm -rf "$PRIV"
  $SUPER -d postgres -c "DROP DATABASE IF EXISTS \"$DB\" WITH (FORCE)" >/dev/null 2>&1 || true
  exit "$rc"
}
trap cleanup EXIT INT TERM

for v in APP_MIGRATOR APP_API APP_POLLER APP_BACKUP APP_CI_CHECK; do
  eval "[ -n \"\${${v}_PASSWORD_VERIFIER:-}\" ]" || die "${v}_PASSWORD_VERIFIER is not set in the container environment"
done

# ------------------------------------------------------------------------------------------------------------------
step "1. bootstrap on a private cluster (no app_* roles exist yet)"
rm -rf "$PRIV"
"$BINDIR/initdb" -D "$PRIV" -U postgres --auth-local=trust --auth-host=scram-sha-256 >/dev/null
"$BINDIR/pg_ctl" -D "$PRIV" -o "-p $PRIVPORT -c listen_addresses='' -c unix_socket_directories=/tmp" -w -l "$PRIV/log" start >/dev/null
PRIVPSQL="$PSQL -h /tmp -p $PRIVPORT -d postgres"
role_count() { $PRIVPSQL -U postgres -Atc "SELECT count(*) FROM pg_roles WHERE rolname LIKE 'app\\_%'"; }
[ "$(role_count)" = 0 ] || die "private cluster already has app_ roles"

for var in APP_MIGRATOR_PASSWORD_VERIFIER APP_API_PASSWORD_VERIFIER APP_POLLER_PASSWORD_VERIFIER APP_BACKUP_PASSWORD_VERIFIER APP_CI_CHECK_PASSWORD_VERIFIER; do
  if env -u "$var" $PRIVPSQL -U postgres -f "$OPS_DIR/bootstrap/roles.sql" >/dev/null 2>&1; then die "roles.sql succeeded with $var unset"; fi
  [ "$(role_count)" = 0 ] || die "roles.sql created roles before aborting on unset $var"
done
if env APP_API_PASSWORD_VERIFIER='plaintext-password' $PRIVPSQL -U postgres -f "$OPS_DIR/bootstrap/roles.sql" >/dev/null 2>&1; then die "roles.sql accepted a plaintext password"; fi
[ "$(role_count)" = 0 ] || die "roles.sql created roles before rejecting a plaintext password"
# wrong key length (StoredKey one char short)
if env APP_API_PASSWORD_VERIFIER='SCRAM-SHA-256$4096:8j+XVk8A+MNPkE1F9ikN5w==$z6bHBDJ0y4QPtQ9jxvQ/l7ujPgoQm1MrTieEbeWqe8=:v7Nl6K7PD29975xasa4rpxOvw8C1NxNp0B+/J+lm0TM=' \
   $PRIVPSQL -U postgres -f "$OPS_DIR/bootstrap/roles.sql" >/dev/null 2>&1; then die "roles.sql accepted a malformed verifier"; fi
[ "$(role_count)" = 0 ] || die "roles.sql created roles before rejecting a malformed verifier"
echo "ok: roles.sql aborts on unset / plaintext / malformed verifiers before creating anything"

step "1b. hosted-shape bootstrap: roles.sql as a non-superuser CREATEROLE login"
# Hosted Supabase's postgres has CREATEROLE and CREATEDB (a non-superuser may only change attributes it holds itself,
# and roles.sql names NOCREATEDB), but is not a superuser.
$PRIVPSQL -U postgres -c "CREATE ROLE bootstrap_nonsuper LOGIN CREATEROLE CREATEDB NOSUPERUSER" -c "GRANT CREATE ON DATABASE postgres TO bootstrap_nonsuper"
$PRIVPSQL -U bootstrap_nonsuper -f "$OPS_DIR/bootstrap/roles.sql" >/dev/null || die "roles.sql failed as a non-superuser login"
$PRIVPSQL -U bootstrap_nonsuper -f "$OPS_DIR/bootstrap/roles.sql" >/dev/null || die "roles.sql is not idempotent as a non-superuser login"
[ "$($PRIVPSQL -U postgres -Atc "SELECT nspowner::regrole FROM pg_namespace WHERE nspname = 'app'")" = app_migrator ] || die "schema app is not owned by app_migrator"
[ "$($PRIVPSQL -U postgres -Atc "SELECT count(*) FROM pg_roles WHERE rolname LIKE 'app\\_%' AND (rolsuper OR rolreplication OR rolbypassrls)")" = 0 ] || die "an app role has a forbidden attribute"
$PRIVPSQL -U app_migrator -f "$MIGRATIONS_DIR/0001_schema_and_helpers.up.sql" >/dev/null || die "migration 0001 failed after the hosted-shape bootstrap"
echo "ok: roles.sql works as a non-superuser CREATEROLE login and is idempotent; 0001 applies"
"$BINDIR/pg_ctl" -D "$PRIV" -m immediate stop >/dev/null
rm -rf "$PRIV"

# ------------------------------------------------------------------------------------------------------------------
step "2. scratch database, roles.sql, auth stub, auth_access.sql"
$SUPER -d postgres -c "CREATE DATABASE \"$DB\""
$SUPER -d postgres -f "$OPS_DIR/bootstrap/roles.sql" >/dev/null     # idempotent re-run on the shared cluster
for pair in app_migrator:APP_MIGRATOR app_api:APP_API app_poller:APP_POLLER app_backup:APP_BACKUP app_ci_check:APP_CI_CHECK; do
  role=${pair%%:*}; var=${pair##*:}_PASSWORD_VERIFIER
  eval "vf=\${$var}"
  # -c does not interpolate psql variables; stdin does.
  [ "$(printf '%s\n' "SELECT count(*) FROM pg_authid WHERE rolname = :'r' AND rolpassword = :'v';" | $SUPER -d postgres -v r="$role" -v v="$vf" -At)" = 1 ] \
    || die "$role: rolpassword differs from $var"
done
echo "ok: stored role verifiers equal the environment verifiers"
$SUPER -d "$DB" -f "$OPS_DIR/tests/auth_stub.sql"
$SUPER -d "$DB" -f "$OPS_DIR/bootstrap/auth_access.sql" >/dev/null
$SUPER -d "$DB" -f "$OPS_DIR/bootstrap/auth_access.sql" >/dev/null   # idempotent

# ------------------------------------------------------------------------------------------------------------------
step "3. apply every migration as app_migrator"
MIG="$PSQL -U app_migrator -d $DB"
$MIG -c "CREATE TABLE app._sqlx_migrations (version bigint PRIMARY KEY)"
apply_ups() { for f in "$MIGRATIONS_DIR"/*.up.sql; do $MIG -f "$f" >/dev/null || die "up migration failed: $f"; done; }
apply_ups
# Recent pg_dump versions print a random \restrict / \unrestrict token; drop those two lines before comparing.
dump() { pg_dump -s -n app -d "$DB" -U postgres | grep -v -e '^\\restrict ' -e '^\\unrestrict '; }
dump > /tmp/schema_A.sql
[ "$($SUPER -d "$DB" -Atc "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = 'public'")" = 0 ] || die "objects in public"

step "4. down migrations in reverse, then up again: schema dumps must be identical"
for f in $(ls "$MIGRATIONS_DIR"/*.down.sql | sort -r); do
  PGOPTIONS='-c app.allow_destructive_down=on' $MIG -f "$f" >/dev/null || die "down migration failed: $f"
done
left=$($SUPER -d "$DB" -Atc "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = 'app' AND c.relname <> '_sqlx_migrations' AND c.relkind IN ('r','p','v','m','S','f')")
[ "$left" = 0 ] || die "$left relations remain in app after the down migrations"
fns=$($SUPER -d "$DB" -Atc "SELECT count(*) FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace WHERE n.nspname = 'app'")
[ "$fns" = 0 ] || die "$fns functions remain in app after the down migrations"
apply_ups
dump > /tmp/schema_B.sql
diff /tmp/schema_A.sql /tmp/schema_B.sql >/dev/null || { diff /tmp/schema_A.sql /tmp/schema_B.sql | head -40; die "schema differs after down + up"; }
rm -f /tmp/schema_A.sql /tmp/schema_B.sql
echo "ok: down migrations revert cleanly and up re-applies to an identical schema"

step "5. a down migration refuses to run without app.allow_destructive_down"
if out=$($MIG -f "$MIGRATIONS_DIR/0009_can_view.down.sql" 2>&1); then die "down migration ran without the setting"; fi
printf '%s' "$out" | grep -q 'destructive down migration refused' || die "unexpected error without the setting: $out"
echo "ok: refused"

# ------------------------------------------------------------------------------------------------------------------
step "6. SQL tests"
for t in grants_test can_view_test data_model_test; do
  echo "-- $t"
  $SUPER -o /dev/null -d "$DB" -f "$OPS_DIR/tests/$t.sql" || die "$t failed"
done
echo "-- mutation checks: grants_test must FAIL after each deliberately broken grant"
mutate() {
  bad=$1; good=$2; expect=$3
  $SUPER -d "$DB" -c "$bad"
  if out=$($SUPER -o /dev/null -d "$DB" -f "$OPS_DIR/tests/grants_test.sql" 2>&1); then
    $SUPER -d "$DB" -c "$good"; die "grants_test passed with a broken grant: $bad"
  fi
  $SUPER -d "$DB" -c "$good"
  printf '%s' "$out" | grep -q "$expect" || die "grants_test failed for the wrong reason after: $bad ($out)"
  echo "ok: caught: $bad"
}
mutate "GRANT SELECT (api_key_ciphertext) ON app.credentials TO app_api" "REVOKE SELECT (api_key_ciphertext) ON app.credentials FROM app_api" "grant matrix failures"
mutate "GRANT UPDATE (key_version) ON app.credentials TO app_api" "REVOKE UPDATE (key_version) ON app.credentials FROM app_api" "grant matrix failures"
mutate "GRANT SELECT ON auth.sessions TO app_poller" "REVOKE SELECT ON auth.sessions FROM app_poller" "auth.\* matrix failures"
$SUPER -o /dev/null -d "$DB" -f "$OPS_DIR/tests/grants_test.sql" || die "grants_test fails after the mutation checks restored the grants"
echo "-- exposure_test"
DB="$DB" OPS_DIR="$OPS_DIR" sh "$OPS_DIR/tests/exposure_test.sh" || die "exposure_test failed"

# ------------------------------------------------------------------------------------------------------------------
step "7. real logins over TCP (SCRAM), with a test password set on the throwaway cluster"
for role in app_migrator app_api app_poller app_backup app_ci_check; do
  $SUPER -d postgres -c "ALTER ROLE $role PASSWORD 'test-only-password'"
done
login() { PGPASSWORD=test-only-password psql -X -q -At -v ON_ERROR_STOP=1 -h 127.0.0.1 -U "$1" -d "$DB" -c "$2"; }
for role in app_migrator app_api app_poller app_backup app_ci_check; do
  [ "$(login "$role" 'SELECT current_user')" = "$role" ] || die "TCP login as $role failed"
done
if login app_api 'SET ROLE app_poller' >/dev/null 2>&1; then die "app_api could SET ROLE app_poller"; fi
if login app_ci_check 'SELECT 1 FROM app.users' >/dev/null 2>&1; then die "app_ci_check could read app.users"; fi
if PGPASSWORD=wrong-password psql -X -q -At -h 127.0.0.1 -U app_api -d "$DB" -c 'SELECT 1' >/dev/null 2>&1; then die "a wrong password was accepted"; fi
[ "$(login app_backup 'SHOW default_transaction_read_only')" = on ] || die "app_backup is not read-only by default"
# exposure check over TCP as app_ci_check, exactly as CI / the operator run it
PGPASSWORD=test-only-password "$OPS_DIR/checks/exposure.sh" "postgresql://app_ci_check@127.0.0.1:5432/$DB" >/dev/null || die "exposure.sh over TCP failed"
echo "ok: logins, role separation and exposure.sh over TCP"

# put the demo verifiers back so a later reader of this cluster sees what .env.example promises
$SUPER -d postgres -f "$OPS_DIR/bootstrap/roles.sql" >/dev/null
step "all database tests passed"
