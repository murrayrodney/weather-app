# One readable line per JSON log event: time, level, job, status, message, then the non-empty fields and job detail.
# Lines that are not JSON (the api's local text logs, Compose notices) pass through unchanged.
# Used by `make logs-pretty`; run with `jq -Rr -f ops/local/pretty-logs.jq`.
def kv: to_entries
  | map(select(.value != 0 and .value != [] and .value != {} and .value != false and .value != null and .value != ""))
  | map("\(.key)=\(.value | tostring)")
  | join(" ");

. as $line
| (try fromjson catch null) as $e
| if ($e | type) != "object" or ($e.fields | type) != "object" then $line
  else
    ($e.fields.detail // "{}" | try fromjson catch {}) as $detail
    | [ ($e.timestamp // "")[11:19],
        ($e.level // ""),
        ($e.fields.job // "-"),
        ($e.fields.status // ""),
        ($e.fields.message // ""),
        ($e.fields | del(.message, .job, .status, .detail, .run_id) | kv),
        ($detail | kv) ]
    | map(select(. != ""))
    | join("  ")
  end
