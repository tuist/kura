#!/usr/bin/env bats

setup_file() {
  export COMPOSE_PROJECT_NAME="cache-handoff"
  export CACHE_US_PORT=4301
  export CACHE_EU_PORT=4302
  export CACHE_AP_PORT=4303
  export TEMPO_PORT=3302
  export OTLP_PORT=4419
  export CACHE_US_URL="http://localhost:${CACHE_US_PORT}"
  export CACHE_EU_URL="http://localhost:${CACHE_EU_PORT}"
  export CACHE_AP_URL="http://localhost:${CACHE_AP_PORT}"
  dc down -v --remove-orphans >/dev/null 2>&1 || true
  dc build cache-us cache-eu cache-ap
  dc up -d cache-us

  wait_for_http "${CACHE_US_URL}/up"
  wait_for_contains "${CACHE_US_URL}/up" '"ring_members":1'
}

teardown_file() {
  dc logs --no-color >"${BATS_FILE_TMPDIR}/compose.log" 2>&1 || true
  dc down -v --remove-orphans >/dev/null 2>&1 || true
}

dc() {
  docker compose "$@"
}

wait_for_http() {
  local url="$1"

  for _ in $(seq 1 90); do
    if curl -fsS "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done

  echo "Timed out waiting for $url" >&2
  return 1
}

wait_for_contains() {
  local url="$1"
  local needle="$2"

  for _ in $(seq 1 45); do
    local body
    body="$(curl -fsS "$url" 2>/dev/null || true)"
    if [[ "$body" == *"$needle"* ]]; then
      printf '%s' "$body"
      return 0
    fi
    sleep 2
  done

  return 1
}

status_only() {
  curl -sS -o /dev/null -w "%{http_code}" "$@"
}

@test "outbox replication moves singleton data to joined nodes" {
  run status_only -X PUT \
    "${CACHE_US_URL}/api/cache/keyvalue?account_handle=acme&project_handle=handoff" \
    -H "content-type: application/json" \
    -d '{"cas_id":"handoff-1","entries":[{"value":"from-singleton"},{"value":"ready-for-join"}]}'
  [ "$status" -eq 0 ]
  [ "$output" = "204" ]

  run wait_for_contains \
    "${CACHE_US_URL}/api/cache/keyvalue/handoff-1?account_handle=acme&project_handle=handoff" \
    '"from-singleton"'
  [ "$status" -eq 0 ]

  dc up -d cache-eu cache-ap

  wait_for_http "${CACHE_EU_URL}/up"
  wait_for_http "${CACHE_AP_URL}/up"
  wait_for_contains "${CACHE_US_URL}/up" '"ring_members":3'
  wait_for_contains "${CACHE_EU_URL}/up" '"ring_members":3'
  wait_for_contains "${CACHE_AP_URL}/up" '"ring_members":3'

  run wait_for_contains \
    "${CACHE_EU_URL}/api/cache/keyvalue/handoff-1?account_handle=acme&project_handle=handoff" \
    '"from-singleton"'
  [ "$status" -eq 0 ]
  [[ "$output" == *'"ready-for-join"'* ]]

  run wait_for_contains \
    "${CACHE_AP_URL}/api/cache/keyvalue/handoff-1?account_handle=acme&project_handle=handoff" \
    '"from-singleton"'
  [ "$status" -eq 0 ]
  [[ "$output" == *'"ready-for-join"'* ]]
}
