#!/bin/sh
# Waits for the stack started by `docker compose up -d` (05 section 4.8 fallback).
# Why not `up -d --wait`: Compose 5.x reports an exited one-shot (auth-access, migrate) as a failure even when it exited 0.
# Long-running services must be healthy; one-shots must have exited 0. Gives up after 180 s.
set -eu
cd "$(dirname "$0")/../.."
C=${COMPOSE:-docker compose}
LONG_RUNNING="db auth poller api caddy"
ONE_SHOTS="auth-access migrate"
deadline=$(( $(date +%s) + 180 ))

while :; do
  pending=""
  for s in $LONG_RUNNING; do
    id=$($C ps -a -q "$s")
    status=$([ -n "$id" ] && docker inspect -f '{{.State.Status}} {{if .State.Health}}{{.State.Health.Status}}{{end}}' "$id" || echo "missing")
    case "$status" in
      "running healthy") ;;
      exited*|dead*|missing*) echo "wait-up: $s is $status" >&2; $C logs --tail=20 "$s" >&2 || true; exit 1 ;;
      *) pending="$pending $s" ;;
    esac
  done
  for s in $ONE_SHOTS; do
    id=$($C ps -a -q "$s")
    status=$([ -n "$id" ] && docker inspect -f '{{.State.Status}} {{.State.ExitCode}}' "$id" || echo "missing")
    case "$status" in
      "exited 0") ;;
      exited*) echo "wait-up: $s failed ($status)" >&2; $C logs --tail=20 "$s" >&2 || true; exit 1 ;;
      *) pending="$pending $s" ;;
    esac
  done
  [ -z "$pending" ] && break
  if [ "$(date +%s)" -ge "$deadline" ]; then echo "wait-up: timed out waiting for:$pending" >&2; exit 1; fi
  sleep 2
done
echo "stack is up: $LONG_RUNNING healthy; $ONE_SHOTS completed"
