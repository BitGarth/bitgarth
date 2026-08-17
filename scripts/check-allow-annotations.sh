#!/usr/bin/env bash
set -euo pipefail

BASELINE_FILE="scripts/allow-annotations-baseline.txt"

if [ ! -f "$BASELINE_FILE" ]; then
  echo "error: missing baseline file '$BASELINE_FILE'" >&2
  exit 1
fi

tmp_current="$(mktemp)"
tmp_baseline="$(mktemp)"
trap 'rm -f "$tmp_current" "$tmp_baseline"' EXIT

{
  grep -R -n -E '^[[:space:]]*#!?\[(cfg_attr\(.*allow\(dead_code\)|.*allow\((dead_code|unused_imports)\))' --include='*.rs' src || true
} \
  | awk -F: '{
      path=$1;
      line=$0;
      sub(/^[^:]*:[0-9]+:/, "", line);
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", line);
      print path "|" line;
    }' \
  | LC_ALL=C sort > "$tmp_current"

LC_ALL=C sort "$BASELINE_FILE" > "$tmp_baseline"

disallowed="$(comm -23 "$tmp_current" "$tmp_baseline")"
if [ -n "$disallowed" ]; then
  echo "error: detected new or expanded allow-annotation usage not in baseline." >&2
  echo "Additions must be removed or explicitly approved by updating:" >&2
  echo "  $BASELINE_FILE" >&2
  echo >&2
  echo "$disallowed" >&2
  exit 1
fi

echo "allow-annotation guard passed"
