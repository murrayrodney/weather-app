#!/bin/bash
# Data for 01's e2e tests and for looking at the app locally (05 section 4.9). Runs the poller jobs once through
# `poller run-job` (an explicit operator action: it works while POLLER_ENABLED=false), in this order:
#   reference_sync -> awn_backfill -> awn_ingest -> daily_rollup
# (backfill before ingest: a station with no data gets only one page from ingest, 03 section 7.1), then asserts through
# psql that every seeded station has observations and rollups and every reference station has >= 12 years of record.
# Idempotent. Prerequisite: `make seed`. AWN_MODE comes from .env (replay by default). The gates run in replay too, at a
# 20 ms spacing (poller GateConfig::for_awn_mode); live mode keeps AWN's 1 request per second limit.
set -euo pipefail
cd "$(dirname "$0")/../.."
C=${COMPOSE:-docker compose}

job() {
  echo "e2e-data: poller run-job $*"
  $C run --rm --no-deps -T poller poller run-job "$@"
}

q() { $C exec -T -u postgres db psql -X -At -d postgres -c "$1"; }

job reference_sync
job awn_backfill
job awn_ingest
job daily_rollup

fail=0
for mac in AA:BB:CC:00:00:01 AA:BB:CC:00:00:02 AA:BB:CC:00:00:03; do
  obs=$(q "select count(*) from app.observations o join app.stations s on s.id = o.station_id where s.mac = '$mac'")
  roll=$(q "select count(*) from app.daily_rollups r join app.stations s on s.id = r.station_id where s.mac = '$mac'")
  if [ "${obs:-0}" -le 0 ] || [ "${roll:-0}" -le 0 ]; then
    echo "e2e-data: FAIL station $mac: observations=$obs rollups=$roll" >&2
    fail=1
  else
    echo "e2e-data: ok   station $mac: observations=$obs rollups=$roll"
  fi
done
for id in USW00099001 USW00099002 USW00099003; do
  years=$(q "select count(distinct extract(year from date)) from app.reference_daily where reference_station_id = '$id'")
  if [ "${years:-0}" -lt 12 ]; then
    echo "e2e-data: FAIL reference station $id: $years distinct years (need 12)" >&2
    fail=1
  else
    echo "e2e-data: ok   reference station $id: $years distinct years"
  fi
done
exit "$fail"
