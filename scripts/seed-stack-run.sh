#!/usr/bin/env bash
set -euo pipefail

COLLECTOR_BASE_URL="${REPLAYKIT_COLLECTOR_URL:-http://127.0.0.1:4100}"
RUN_TITLE="${REPLAYKIT_SEED_TITLE:-smoke test run}"
STARTED_AT="${REPLAYKIT_SEED_STARTED_AT:-1000}"

log() {
  printf '%s\n' "$*" >&2
}

begin_run_payload=$(cat <<JSON
{
  "title": "${RUN_TITLE}",
  "entrypoint": "smoke.main",
  "adapter_name": "smoke",
  "adapter_version": "0.1.0",
  "started_at": ${STARTED_AT}
}
JSON
)

log "--- Seeding run via collector at ${COLLECTOR_BASE_URL} ---"
run_response=$(curl -sf -X POST "${COLLECTOR_BASE_URL}/v1/runs" \
  -H "Content-Type: application/json" \
  -d "${begin_run_payload}")
run_id=$(printf '%s' "${run_response}" | sed -n 's/.*"run_id":"\([^"]*\)".*/\1/p')
if [ -z "${run_id}" ]; then
  log "Failed to parse run_id from collector response: ${run_response}"
  exit 1
fi
log "Created run: ${run_id}"

span_response=$(curl -sf -X POST "${COLLECTOR_BASE_URL}/v1/runs/${run_id}/spans" \
  -H "Content-Type: application/json" \
  -d "{
    \"kind\": \"ToolCall\",
    \"name\": \"smoke-tool\",
    \"started_at\": $((STARTED_AT + 1))
  }")
span_id=$(printf '%s' "${span_response}" | sed -n 's/.*"span_id":"\([^"]*\)".*/\1/p')
if [ -z "${span_id}" ]; then
  log "Failed to parse span_id from collector response: ${span_response}"
  exit 1
fi
log "Created span: ${span_id}"

curl -sf -X POST "${COLLECTOR_BASE_URL}/v1/runs/${run_id}/artifacts" \
  -H "Content-Type: application/json" \
  -d "{
    \"artifact_type\": \"ToolOutput\",
    \"mime\": \"text/plain\",
    \"created_at\": $((STARTED_AT + 2)),
    \"span_id\": \"${span_id}\",
    \"content_base64\": \"c21va2UgdGVzdCBhcnRpZmFjdA==\"
  }" > /dev/null
log "Added artifact."

curl -sf -X POST "${COLLECTOR_BASE_URL}/v1/runs/${run_id}/spans/${span_id}/end" \
  -H "Content-Type: application/json" \
  -d "{
    \"ended_at\": $((STARTED_AT + 3)),
    \"status\": \"Completed\"
  }" > /dev/null
log "Ended span."

curl -sf -X POST "${COLLECTOR_BASE_URL}/v1/runs/${run_id}/finish" \
  -H "Content-Type: application/json" \
  -d "{
    \"ended_at\": $((STARTED_AT + 4)),
    \"status\": \"Completed\"
  }" > /dev/null
log "Finished run."

printf '%s\n' "${run_id}"
