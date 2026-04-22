#!/usr/bin/env bats

setup_file() {
  export COMPOSE_PROJECT_NAME="kura-discovery"
  export KURA_US_PORT=4401
  export KURA_US_2_PORT=4402
  export TEMPO_PORT=3303
  export OTLP_PORT=4420
  export KURA_US_URL="http://localhost:${KURA_US_PORT}"
  export KURA_US_2_URL="http://localhost:${KURA_US_2_PORT}"
  dc down -v --remove-orphans >/dev/null 2>&1 || true
  dc build kura-us kura-us-2
  dc up -d kura-us

  wait_for_http "${KURA_US_URL}/up"
  wait_for_contains "${KURA_US_URL}/up" '"ring_members":1'
}

teardown_file() {
  dc logs --no-color >"${BATS_FILE_TMPDIR}/compose.log" 2>&1 || true
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

@test "dns discovery bootstraps and replicates a new node in the same region" {
  run status_only -X PUT \
    "${KURA_US_URL}/api/cache/keyvalue?tenant_id=acme&namespace_id=ios" \
    -H "content-type: application/json" \
    -d '{"cas_id":"cas-1","entries":[{"value":"from-singleton"},{"value":"ready-for-join"}]}'
  [ "$status" -eq 0 ]
  [ "$output" = "204" ]

  run status_only -X POST \
    "${KURA_US_URL}/api/cache/cas/artifact-1?tenant_id=acme&namespace_id=ios" \
    -H "content-type: application/octet-stream" \
    --data-binary "xcode-binary"
  [ "$status" -eq 0 ]
  [ "$output" = "204" ]

  dc up -d kura-us-2

  wait_for_http "${KURA_US_2_URL}/up"
  wait_for_contains "${KURA_US_URL}/up" '"ring_members":2'
  wait_for_contains "${KURA_US_2_URL}/up" '"ring_members":2'

  run wait_for_contains \
    "${KURA_US_2_URL}/api/cache/keyvalue/cas-1?tenant_id=acme&namespace_id=ios" \
    '"from-singleton"'
  [ "$status" -eq 0 ]
  [[ "$output" == *'"ready-for-join"'* ]]

  run wait_for_contains \
    "${KURA_US_2_URL}/api/cache/cas/artifact-1?tenant_id=acme&namespace_id=ios" \
    "xcode-binary"
  [ "$status" -eq 0 ]
  [ "$output" = "xcode-binary" ]

  run status_only -X PUT \
    "${KURA_US_2_URL}/api/cache/gradle/gradle-key-1?tenant_id=acme&namespace_id=android" \
    -H "content-type: application/octet-stream" \
    --data-binary "from-new-node"
  [ "$status" -eq 0 ]
  [ "$output" = "201" ]

  run wait_for_contains \
    "${KURA_US_URL}/api/cache/gradle/gradle-key-1?tenant_id=acme&namespace_id=android" \
    "from-new-node"
  [ "$status" -eq 0 ]
  [ "$output" = "from-new-node" ]
}
