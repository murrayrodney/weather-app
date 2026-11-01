#!/usr/bin/env bash
# Fails when a file holds a local demo value (06 section 13). Reads KEY=VALUE lines (also `KEY = "VALUE"` toml lines),
# hashes every value (and the password inside a URL value) and compares against column 1 of backend/demo-fingerprints.txt.
# Prints variable names and file names only, never values.
#   ops/ci/check-demo-values.sh <file>...
set -euo pipefail
cd "$(dirname "$0")/../.."
fp=backend/demo-fingerprints.txt
[ -f "$fp" ] || { echo "missing $fp" >&2; exit 2; }
[ $# -ge 1 ] || { echo "usage: $0 <file>..." >&2; exit 2; }
sha() { if command -v sha256sum >/dev/null; then sha256sum | awk '{print $1}'; else shasum -a 256 | awk '{print $1}'; fi; }
hashes="$(awk '!/^#/ && NF { print $1 }' "$fp")"
bad=0
for f in "$@"; do
  while IFS= read -r line; do
    case "$line" in ''|'#'*|'['*) continue;; *=*) ;; *) continue;; esac
    key="$(printf '%s' "${line%%=*}" | sed -E 's/^[[:space:]]*export[[:space:]]+//' | tr -d ' ')"
    val="${line#*=}"
    # drop a trailing " # comment", then surrounding whitespace and one layer of single or double quotes
    val="$(printf '%s' "$val" | sed -E "s/[[:space:]]+#.*\$//; s/^[[:space:]]*[\"']?//; s/[\"']?[[:space:]]*\$//")"
    [ -n "$val" ] || continue
    cands=("$val")
    if [[ "$val" =~ ^[a-z+]+://[^:/@]+:([^@]+)@ ]]; then cands+=("${BASH_REMATCH[1]}"); fi   # password inside a URL
    for c in "${cands[@]}"; do
      h="$(printf '%s' "$c" | sha)"
      if grep -qxF -- "$h" <<<"$hashes"; then echo "demo value found: $key in $f" >&2; bad=1; fi
    done
  done < "$f"
done
exit $bad
