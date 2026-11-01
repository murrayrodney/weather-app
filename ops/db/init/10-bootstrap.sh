#!/bin/sh
set -eu
# roles.sql reads APP_*_PASSWORD_VERIFIER from the environment with \getenv; nothing on argv.
psql -X -v ON_ERROR_STOP=1 -U "$POSTGRES_USER" -d "$POSTGRES_DB" -f /ops/db/bootstrap/roles.sql
