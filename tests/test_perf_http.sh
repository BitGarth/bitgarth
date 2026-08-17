#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
SCRIPT="$ROOT_DIR/scripts/perf/http"
REMOVED_PREFIX='BITGARTH''_PERF_'

fail() {
  echo "$1" >&2
  exit 1
}

zsh -n "$SCRIPT"

if grep -Fq "$REMOVED_PREFIX" "$SCRIPT"; then
  fail "perf wrapper still references the removed environment namespace"
fi
grep -q '^SERVER_PORT=8081$' "$SCRIPT" || fail "default server port is not 8081"
grep -Fq 'SERVER_BIN="${ROOT_DIR}/target/dx/bitgarth-app/release/web/server"' "$SCRIPT" \
  || fail "perf wrapper does not launch the release web server"
if grep -Fq 'if [[ ! -x "${SERVER_BIN}" ]]' "$SCRIPT"; then
  fail "real-server mode can reuse a stale release server"
fi
grep -Fq './scripts/dx-web-build' "$SCRIPT" \
  || fail "real-server mode does not build the dev-config release server"
grep -Fq 'BITGARTH_SYNC_CONTROL=1 "${SERVER_BIN}"' "$SCRIPT" \
  || fail "real-server child does not suppress automatic sync"
if grep -q '^export BITGARTH_SYNC_CONTROL' "$SCRIPT"; then
  fail "sync control must be scoped to the child server process"
fi
grep -Fq -- '--features server,dev-config' "$SCRIPT" \
  || fail "perf runner is not built with dev-config"

for value in 0 65536 nope -1; do
  set +e
  output=$("$SCRIPT" --server-port "$value" 2>&1)
  status=$?
  set -e
  [ "$status" -eq 2 ] || fail "--server-port $value exited $status instead of 2"
  case "$output" in
    *"--server-port must be an integer from 1 through 65535"*) ;;
    *) fail "--server-port $value did not report the validation error" ;;
  esac
done

set +e
output=$("$SCRIPT" --server-port 2>&1)
status=$?
set -e
[ "$status" -eq 2 ] || fail "missing --server-port value exited $status instead of 2"
case "$output" in
  *"missing value for --server-port"*) ;;
  *) fail "missing --server-port value did not report the validation error" ;;
esac
