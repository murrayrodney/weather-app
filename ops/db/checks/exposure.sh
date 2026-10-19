#!/bin/sh
# Usage: exposure.sh <connection-url>   (connect as app_ci_check). Exit code is psql's.
set -eu
[ "$#" -eq 1 ] || { echo "usage: exposure.sh <connection-url>" >&2; exit 2; }
here=$(dirname "$0")
PGOPTIONS='-c default_transaction_read_only=on' exec psql -X -v ON_ERROR_STOP=1 -f "$here/exposure.sql" "$1"
