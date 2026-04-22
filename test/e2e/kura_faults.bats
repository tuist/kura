#!/usr/bin/env bats

setup_file() {
  export COMPOSE_PROJECT_NAME="kura-faults"
  export KURA_US_PORT=4501
  export KURA_US_2_PORT=4502
  export TEMPO_PORT=3305
  export OTLP_PORT=4422
  export KURA_US_URL="http://localhost:${KURA_US_PORT}"
  export KURA_US_2_URL="http://localhost:${KURA_US_2_PORT}"
  dc down -v --remove-orphans >/dev/null 2>&1 || true
  dc build kura-us kura-us-2
}

setup() {
  dc down -v --remove-orphans >/dev/null 2>&1 || true
  dc up -d kura-us

  wait_for_http "${KURA_US_URL}/up"
  wait_for_contains "${KURA_US_URL}/up" '"ring_members":1'
}

teardown() {
  dc logs --no-color >"${BATS_TEST_TMPDIR}/compose.log" 2>&1 || true
  dc down -v --remove-orphans >/dev/null 2>&1 || true
}

dc() {
  docker compose -f docker-compose.yml -f test/e2e/docker-compose.discovery.yml "$@"
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

@test "bootstrap after delete does not resurrect stale namespace state" {
  run status_only -X PUT \
    "${KURA_US_URL}/api/cache/keyvalue?tenant_id=acme&namespace_id=ios" \
    -H "content-type: application/json" \
    -d '{"cas_id":"cas-delete","entries":[{"value":"stale-after-delete"}]}'
  [ "$status" -eq 0 ]
  [ "$output" = "204" ]

  run status_only -X POST \
    "${KURA_US_URL}/api/cache/cas/artifact-delete?tenant_id=acme&namespace_id=ios" \
    -H "content-type: application/octet-stream" \
    --data-binary "stale-binary"
  [ "$status" -eq 0 ]
  [ "$output" = "204" ]

  run status_only -X DELETE \
    "${KURA_US_URL}/api/cache/clean?tenant_id=acme&namespace_id=ios"
  [ "$status" -eq 0 ]
  [ "$output" = "204" ]

  dc up -d kura-us-2

  wait_for_http "${KURA_US_2_URL}/up"
  wait_for_contains "${KURA_US_URL}/up" '"ring_members":2'
  wait_for_contains "${KURA_US_2_URL}/up" '"ring_members":2'

  run status_only \
    "${KURA_US_2_URL}/api/cache/keyvalue/cas-delete?tenant_id=acme&namespace_id=ios"
  [ "$status" -eq 0 ]
  [ "$output" = "404" ]

  run status_only \
    "${KURA_US_2_URL}/api/cache/cas/artifact-delete?tenant_id=acme&namespace_id=ios"
  [ "$status" -eq 0 ]
  [ "$output" = "404" ]
}

@test "rejoining node converges after bootstrap and queued replication retries" {
  dc up -d kura-us-2

  wait_for_http "${KURA_US_2_URL}/up"
  wait_for_contains "${KURA_US_URL}/up" '"ring_members":2'
  wait_for_contains "${KURA_US_2_URL}/up" '"ring_members":2'

  dc stop kura-us-2 >/dev/null

  run status_only -X PUT \
    "${KURA_US_URL}/api/cache/keyvalue?tenant_id=acme&namespace_id=ios" \
    -H "content-type: application/json" \
    -d '{"cas_id":"cas-rejoin","entries":[{"value":"from-outage"},{"value":"from-retry"}]}'
  [ "$status" -eq 0 ]
  [ "$output" = "204" ]

  run status_only -X POST \
    "${KURA_US_URL}/api/cache/cas/artifact-rejoin?tenant_id=acme&namespace_id=ios" \
    -H "content-type: application/octet-stream" \
    --data-binary "rejoin-binary"
  [ "$status" -eq 0 ]
  [ "$output" = "204" ]

  dc up -d kura-us-2

  wait_for_http "${KURA_US_2_URL}/up"
  wait_for_contains "${KURA_US_URL}/up" '"ring_members":2'
  wait_for_contains "${KURA_US_2_URL}/up" '"ring_members":2'

  run wait_for_contains \
    "${KURA_US_2_URL}/api/cache/keyvalue/cas-rejoin?tenant_id=acme&namespace_id=ios" \
    '"from-outage"'
  [ "$status" -eq 0 ]
  [[ "$output" == *'"from-retry"'* ]]

  run wait_for_contains \
    "${KURA_US_2_URL}/api/cache/cas/artifact-rejoin?tenant_id=acme&namespace_id=ios" \
    "rejoin-binary"
  [ "$status" -eq 0 ]
  [ "$output" = "rejoin-binary" ]

  run status_only -X PUT \
    "${KURA_US_2_URL}/api/cache/gradle/gradle-rejoin?tenant_id=acme&namespace_id=android" \
    -H "content-type: application/octet-stream" \
    --data-binary "healthy-after-rejoin"
  [ "$status" -eq 0 ]
  [ "$output" = "201" ]

  run wait_for_contains \
    "${KURA_US_URL}/api/cache/gradle/gradle-rejoin?tenant_id=acme&namespace_id=android" \
    "healthy-after-rejoin"
  [ "$status" -eq 0 ]
  [ "$output" = "healthy-after-rejoin" ]
}
