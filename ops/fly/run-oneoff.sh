#!/usr/bin/env bash
# usage: run-oneoff.sh batch <app> <image-src-app> <image-src-group> -- <abs-binary> [args...]
#        run-oneoff.sh shell <app> <image-src-app> <image-src-group> [minutes]
set -euo pipefail
usage() { sed -n '2,3p' "$0" >&2; exit 2; }
[ $# -ge 4 ] || usage
mode=$1; app=$2; src=$3; grp=$4; shift 4
# a wrong app would run a command with the wrong secrets: only these (app, image source app, group) triples are allowed
case "$app $src $grp" in
  "famweather-poller famweather-poller poller"|"famweather-admin famweather-poller poller") ;;
  *) echo "unsupported app/source/group: $app $src $grp (allowed: 'famweather-poller famweather-poller poller', 'famweather-admin famweather-poller poller')" >&2; exit 2 ;;
esac
case "$mode" in
  batch) [ "${1:-}" = "--" ] || usage; shift
         [[ "${1:-}" == /usr/local/bin/* ]] || { echo "command must be an absolute /usr/local/bin/... path (distroless)" >&2; exit 2; }
         case "$1" in *..*) echo "'..' is not allowed in the command path" >&2; exit 2 ;; esac
         # batch output is read back through `fly logs`, so a set-password link would land in Fly's log store: refuse
         for a in "$@"; do
           case "$a" in invite|reset-link) echo "admin ${a} prints a set-password link: run it in shell mode (fly ssh console -C), never batch" >&2; exit 2 ;; esac
         done ;;
  shell) minutes="${1:-15}" ;;
  *) usage ;;
esac
# image source = the single running machine of the source process group (not a stray one-off)
cfg="$(fly machines list -a "$src" --json | jq -c --arg g "$grp" \
  '[.[] | select(.config.metadata.fly_process_group == $g)] | if length == 1 then .[0] else error("expected exactly one machine in group " + $g) end')"
image="$(jq -r .config.image <<<"$cfg")"
region="$(jq -r .region <<<"$cfg")"
envargs=()
while IFS= read -r kv; do envargs+=(-e "$kv"); done < <(jq -r '.config.env // {} | to_entries[] | "\(.key)=\(.value)"' <<<"$cfg")  # mirrors POLLER_ENABLED, RUST_LOG, AWN_MODE, APP_ENV
name="oneoff-$(date -u +%Y%m%d-%H%M%S)"
common=(-a "$app" --name "$name" --region "$region" --vm-size shared-cpu-1x --vm-memory 256 --restart no ${envargs[@]+"${envargs[@]}"} -e APP_ENV=production)
if [ "$app" = famweather-admin ]; then
  # famweather-admin has no config file, so the non-secret SITE_URL (origin printed in set-password links) comes from a committed constant
  site_url_file="$(dirname "$0")/site-url.txt"
  [ -f "$site_url_file" ] || { echo "ops/fly/site-url.txt is missing: create it with one line, the production site origin (for example https://<your-site>.netlify.app). See docs/guides/06-deploy-ops.md section 3.3 (admin row)." >&2; exit 2; }
  site_url="$(tr -d '[:space:]' < "$site_url_file")"
  [[ "$site_url" =~ ^https://[^/]+$ ]] || { echo "ops/fly/site-url.txt must hold the production origin (https, no path)" >&2; exit 2; }
  common+=(-e "SITE_URL=$site_url")
fi
machine_id() { fly machines list -a "$app" --json | jq -r --arg n "$name" '.[] | select(.name == $n) | .id'; }
cleanup() { local id; id="$(machine_id 2>/dev/null || true)"; [ -z "$id" ] || fly machine destroy "$id" -a "$app" --force || true; }
trap cleanup EXIT                      # armed BEFORE the machine exists: a failed or interrupted run never leaks one

if [ "$mode" = batch ]; then
  fly machine run "$image" "${common[@]}" -- "$@"
  id="$(machine_id)"; [ -n "$id" ] || { echo "machine $name not found after run" >&2; exit 1; }
  fly machine wait "$id" -a "$app" --state stopped --wait-timeout 1h
  fly logs -a "$app" --machine "$id" --no-tail || true
  code="$(fly machine status "$id" -a "$app" --json | jq -r '[.events[]? | select(.type == "exit") | .request.exit_event.exit_code] | last // 1')"
  echo "exit code: $code"
  exit "$code"                         # the trap destroys the machine
else
  fly machine run "$image" "${common[@]}" -- /usr/local/bin/admin wait --minutes "$minutes"
  id="$(machine_id)"; [ -n "$id" ] || { echo "machine $name not found after run" >&2; exit 1; }
  echo "machine $id is up for $minutes min. In another terminal:"
  echo "  fly ssh console -a $app --machine $id --pty -C \"/usr/local/bin/admin <subcommand> ...\""
  read -r -p "Press Enter when finished (the machine is destroyed on exit) " _
fi
