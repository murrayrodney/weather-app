#!/bin/bash
# The web app pins Node's major (web/.nvmrc, engines ">=26 <27", engine-strict). Host steps (make web-dev, make test-e2e)
# must run on that major. Prints a directory to prepend to PATH, or nothing when the current `node` already matches.
# Order: current node -> nvm -> fnm -> volta. Nothing is ever downloaded; when none matches it exits non-zero with
# install instructions.
set -euo pipefail
cd "$(dirname "$0")/../.."
want=$(tr -d '[:space:]' < web/.nvmrc)
major() { "$1" -p 'process.versions.node.split(".")[0]' 2>/dev/null || true; }

if command -v node >/dev/null && [ "$(major node)" = "$want" ]; then exit 0; fi

for dir in "${NVM_DIR:-$HOME/.nvm}"/versions/node/v"$want".*/bin; do
  [ -x "$dir/node" ] && [ "$(major "$dir/node")" = "$want" ] && { echo "$dir"; exit 0; }
done
if command -v fnm >/dev/null; then
  dir=$(fnm exec --using="$want" dirname "$(fnm exec --using="$want" which node 2>/dev/null)" 2>/dev/null || true)
  [ -n "$dir" ] && [ "$(major "$dir/node")" = "$want" ] && { echo "$dir"; exit 0; }
fi
if [ -x "$HOME/.volta/bin/node" ] && command -v volta >/dev/null; then
  dir=$(dirname "$(volta run --node "$want" which node 2>/dev/null)" 2>/dev/null || true)
  [ -n "$dir" ] && [ "$(major "$dir/node")" = "$want" ] && { echo "$dir"; exit 0; }
fi

cat >&2 <<EOF
web needs Node $want (web/.nvmrc, engines ">=26 <27"); this host has $(node -v 2>/dev/null || echo "no node").
Install it with one of:
  nvm install $want      (then: nvm use $want, or just re-run make: this script finds it under \$NVM_DIR)
  fnm install $want
  volta install node@$want
Nothing is downloaded automatically. (The e2e suite cannot run in a container: it must reach the stack as
localhost:8000, which the pinned auth issuer and build-time URLs require, and its helpers call the host's docker.)
EOF
exit 1
