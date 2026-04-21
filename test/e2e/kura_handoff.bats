#!/usr/bin/env bats

setup_file() {
  export COMPOSE_PROJECT_NAME="kura-handoff"
  export KURA_US_PORT=4301
  export KURA_EU_PORT=4302
  export KURA_AP_PORT=4303
  export TEMPO_PORT=3302
  export OTLP_PORT=4419
  export KURA_US_URL="http://localhost:${KURA_US_PORT}"
  export KURA_EU_URL="http://localhost:${KURA_EU_PORT}"
  export KURA_AP_URL="http://localhost:${KURA_AP_PORT}"
  dc down -v --remove-orphans >/dev/null 2>&1 || true
  dc build kura-us kura-eu kura-ap
  dc up -d kura-us

  wait_for_http "${KURA_US_URL}/up"
  wait_for_contains "${KURA_US_URL}/up" '"ring_members":1'
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
    "${KURA_US_URL}/api/cache/keyvalue?tenant_id=acme&namespace_id=handoff" \
    -H "content-type: application/json" \
    -d '{"cas_id":"handoff-1","entries":[{"value":"from-singleton"},{"value":"ready-for-join"}]}'
  [ "$status" -eq 0 ]
  [ "$output" = "204" ]

  run wait_for_contains \
    "${KURA_US_URL}/api/cache/keyvalue/handoff-1?tenant_id=acme&namespace_id=handoff" \
    '"from-singleton"'
  [ "$status" -eq 0 ]

  dc up -d kura-eu kura-ap

  wait_for_http "${KURA_EU_URL}/up"
  wait_for_http "${KURA_AP_URL}/up"
  wait_for_contains "${KURA_US_URL}/up" '"ring_members":3'
  wait_for_contains "${KURA_EU_URL}/up" '"ring_members":3'
  wait_for_contains "${KURA_AP_URL}/up" '"ring_members":3'

  run wait_for_contains \
    "${KURA_EU_URL}/api/cache/keyvalue/handoff-1?tenant_id=acme&namespace_id=handoff" \
    '"from-singleton"'
  [ "$status" -eq 0 ]
  [[ "$output" == *'"ready-for-join"'* ]]

  run wait_for_contains \
    "${KURA_AP_URL}/api/cache/keyvalue/handoff-1?tenant_id=acme&namespace_id=handoff" \
    '"from-singleton"'
  [ "$status" -eq 0 ]
  [[ "$output" == *'"ready-for-join"'* ]]
}
