#!/usr/bin/env bash
set -euo pipefail

# ReplayKit smoke test
# Proves: collector writes -> API reads -> data persists across restart

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
SMOKE_DATA_ROOT=$(mktemp -d)
read -r COLLECTOR_PORT API_PORT < <(python3 - <<'PORTS'
import socket
with socket.socket() as collector, socket.socket() as api:
    collector.bind(("127.0.0.1", 0)); api.bind(("127.0.0.1", 0))
    print(collector.getsockname()[1], api.getsockname()[1])
PORTS
)
COLLECTOR_PID=""
API_PID=""

cleanup() {
  [ -n "$COLLECTOR_PID" ] && kill "$COLLECTOR_PID" 2>/dev/null || true
  [ -n "$API_PID" ] && kill "$API_PID" 2>/dev/null || true
  wait "$COLLECTOR_PID" 2>/dev/null || true
  wait "$API_PID" 2>/dev/null || true
  rm -rf "$SMOKE_DATA_ROOT"
}
trap cleanup EXIT

echo "=== ReplayKit Smoke Test ==="
echo "Data root: $SMOKE_DATA_ROOT"

# ── Build ────────────────────────────────────────────────────────────
echo ""
echo "--- Building workspace ---"
cargo build --locked --bin replaykit --bin replaykit-collector --quiet
TARGET_DIR=$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')

# ── Start collector ──────────────────────────────────────────────────
echo "--- Starting collector on port $COLLECTOR_PORT ---"
REPLAYKIT_DATA_ROOT="$SMOKE_DATA_ROOT" \
REPLAYKIT_PORT="$COLLECTOR_PORT" \
  "$TARGET_DIR/debug/replaykit-collector" >"$SMOKE_DATA_ROOT/collector.log" 2>&1 &
COLLECTOR_PID=$!

wait_for_port() {
  local port=$1 timeout=15
  for i in $(seq 1 "$timeout"); do
    if python3 - "$port" <<'READY'
import socket, sys
try:
    with socket.create_connection(("127.0.0.1", int(sys.argv[1])), timeout=0.2):
        pass
except OSError:
    sys.exit(1)
READY
    then
      return 0
    fi
    sleep 1
  done
  echo "FAIL: port $port not ready after ${timeout}s"
  cat "$SMOKE_DATA_ROOT/collector.log" "$SMOKE_DATA_ROOT/api.log" 2>/dev/null || true
  return 1
}

wait_for_port "$COLLECTOR_PORT"
echo "Collector ready."

# ── Seed data via collector HTTP API ─────────────────────────────────
echo ""
echo "--- Seeding data via collector ---"
RUN_ID=$(REPLAYKIT_COLLECTOR_URL="http://127.0.0.1:${COLLECTOR_PORT}" \
  bash scripts/seed-stack-run.sh)

# ── Start API server ─────────────────────────────────────────────────
echo ""
echo "--- Starting API on port $API_PORT ---"
"$TARGET_DIR/debug/replaykit" \
  --storage sqlite --data-root "$SMOKE_DATA_ROOT" \
  serve --port "$API_PORT" >"$SMOKE_DATA_ROOT/api.log" 2>&1 &
API_PID=$!
wait_for_port "$API_PORT"
echo "API ready."

# ── Verify API reads collector-written data ──────────────────────────
echo ""
echo "--- Verifying API reads ---"

RUNS=$(curl -sf "http://127.0.0.1:${API_PORT}/api/v1/runs")
if ! echo "$RUNS" | grep -q "$RUN_ID"; then
  echo "FAIL: API /runs does not contain seeded run $RUN_ID"
  echo "Response: $RUNS"
  exit 1
fi
echo "PASS: API lists seeded run"

RUN_DETAIL=$(curl -sf "http://127.0.0.1:${API_PORT}/api/v1/runs/${RUN_ID}")
if ! echo "$RUN_DETAIL" | grep -q "smoke test run"; then
  echo "FAIL: run detail missing expected title"
  exit 1
fi
echo "PASS: run detail matches"

TREE=$(curl -sf "http://127.0.0.1:${API_PORT}/api/v1/runs/${RUN_ID}/tree")
if ! echo "$TREE" | grep -q "smoke-tool"; then
  echo "FAIL: tree does not contain smoke-tool span"
  exit 1
fi
echo "PASS: tree contains expected span"

# ── Verify persistence across restart ────────────────────────────────
echo ""
echo "--- Verifying persistence across restart ---"

kill "$API_PID" 2>/dev/null; wait "$API_PID" 2>/dev/null || true
kill "$COLLECTOR_PID" 2>/dev/null; wait "$COLLECTOR_PID" 2>/dev/null || true
API_PID=""
COLLECTOR_PID=""

# Restart API only
"$TARGET_DIR/debug/replaykit" \
  --storage sqlite --data-root "$SMOKE_DATA_ROOT" \
  serve --port "$API_PORT" >"$SMOKE_DATA_ROOT/api.log" 2>&1 &
API_PID=$!
wait_for_port "$API_PORT"

RUNS_AFTER=$(curl -sf "http://127.0.0.1:${API_PORT}/api/v1/runs")
if ! echo "$RUNS_AFTER" | grep -q "$RUN_ID"; then
  echo "FAIL: run not found after restart"
  exit 1
fi
echo "PASS: data persists across restart"

# ── Verify storage on disk ───────────────────────────────────────────
echo ""
echo "--- Verifying storage files ---"

if [ ! -f "$SMOKE_DATA_ROOT/replaykit.db" ]; then
  echo "FAIL: replaykit.db not found"
  exit 1
fi
echo "PASS: replaykit.db exists"

BLOB_COUNT=$(find "$SMOKE_DATA_ROOT/blobs/sha256" -name "*.blob" 2>/dev/null | wc -l | tr -d ' ')
if [ "$BLOB_COUNT" -lt 1 ]; then
  echo "FAIL: no blobs in $SMOKE_DATA_ROOT/blobs/sha256/"
  exit 1
fi
echo "PASS: $BLOB_COUNT blob(s) on disk"

echo ""
echo "=== All smoke tests passed ==="
