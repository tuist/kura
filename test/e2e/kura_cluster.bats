#!/usr/bin/env bats

setup_file() {
  export COMPOSE_PROJECT_NAME="kura-e2e"
  export KURA_US_PORT=4201
  export KURA_EU_PORT=4202
  export KURA_AP_PORT=4203
  export GRAFANA_PORT=3300
  export PROMETHEUS_PORT=9190
  export LOKI_PORT=3201
  export TEMPO_PORT=3301
  export OTLP_PORT=4418
  export KURA_US_URL="http://localhost:${KURA_US_PORT}"
  export KURA_EU_URL="http://localhost:${KURA_EU_PORT}"
  export KURA_AP_URL="http://localhost:${KURA_AP_PORT}"
  export GRAFANA_URL="http://localhost:${GRAFANA_PORT}"
  export PROMETHEUS_URL="http://localhost:${PROMETHEUS_PORT}"
  dc down -v --remove-orphans >/dev/null 2>&1 || true
  dc up --build -d

  wait_for_http "${KURA_US_URL}/up"
  wait_for_http "${KURA_EU_URL}/up"
  wait_for_http "${KURA_AP_URL}/up"
  wait_for_contains "${KURA_US_URL}/up" '"ring_members":3'
  wait_for_contains "${KURA_EU_URL}/up" '"ring_members":3'
  wait_for_contains "${KURA_AP_URL}/up" '"ring_members":3'
  wait_for_http "${GRAFANA_URL}/api/health"
  wait_for_http "${PROMETHEUS_URL}/-/ready"
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

  for _ in $(seq 1 30); do
    local body
    body="$(curl -fsS "$url" 2>/dev/null || true)"
    if [[ "$body" == *"$needle"* ]]; then
      printf '%s' "$body"
      return 0
    fi
    sleep 1
  done

  return 1
}

status_only() {
  curl -sS -o /dev/null -w "%{http_code}" "$@"
}

@test "keyvalue entries sync across regions" {
  run status_only -X PUT \
    "${KURA_US_URL}/api/cache/keyvalue?tenant_id=acme&namespace_id=ios" \
    -H "content-type: application/json" \
    -d '{"cas_id":"cas-1","entries":[{"value":"hello"},{"value":"world"}]}'
  [ "$status" -eq 0 ]
  [ "$output" = "204" ]

  run wait_for_contains \
    "${KURA_EU_URL}/api/cache/keyvalue/cas-1?tenant_id=acme&namespace_id=ios" \
    '"hello"'
  [ "$status" -eq 0 ]
  [[ "$output" == *'"world"'* ]]
}

@test "xcode artifacts persist on disk and survive api node restart" {
  run status_only -X POST \
    "${KURA_US_URL}/api/cache/cas/artifact-1?tenant_id=acme&namespace_id=ios" \
    -H "content-type: application/octet-stream" \
    --data-binary "xcode-binary"
  [ "$status" -eq 0 ]
  [ "$output" = "204" ]

  run wait_for_contains \
    "${KURA_EU_URL}/api/cache/cas/artifact-1?tenant_id=acme&namespace_id=ios" \
    "xcode-binary"
  [ "$status" -eq 0 ]
  [ "$output" = "xcode-binary" ]

  dc restart kura-eu >/dev/null
  wait_for_http "${KURA_EU_URL}/up"
  wait_for_contains "${KURA_EU_URL}/up" '"ring_members":3'

  run wait_for_contains \
    "${KURA_EU_URL}/api/cache/cas/artifact-1?tenant_id=acme&namespace_id=ios" \
    "xcode-binary"
  [ "$status" -eq 0 ]
  [ "$output" = "xcode-binary" ]
}

@test "gradle artifacts sync to another region" {
  run status_only -X PUT \
    "${KURA_EU_URL}/api/cache/gradle/gradle-key-1?tenant_id=acme&namespace_id=android" \
    -H "content-type: application/octet-stream" \
    --data-binary "gradle-cache"
  [ "$status" -eq 0 ]
  [ "$output" = "201" ]

  run wait_for_contains \
    "${KURA_AP_URL}/api/cache/gradle/gradle-key-1?tenant_id=acme&namespace_id=android" \
    "gradle-cache"
  [ "$status" -eq 0 ]
}

@test "multipart module uploads are visible from another node" {
  run curl -fsS -X POST \
    "${KURA_US_URL}/api/cache/module/start?tenant_id=acme&namespace_id=ios&hash=hash-1&name=Module.framework&cache_category=builds"
  [ "$status" -eq 0 ]
  upload_id="$(printf '%s' "$output" | sed -E 's/.*"upload_id":"([^"]+)".*/\1/')"
  [ -n "$upload_id" ]

  run status_only -X POST \
    "${KURA_US_URL}/api/cache/module/part?upload_id=${upload_id}&part_number=1" \
    -H "content-type: application/octet-stream" \
    --data-binary "part-one-"
  [ "$status" -eq 0 ]
  [ "$output" = "204" ]

  run status_only -X POST \
    "${KURA_US_URL}/api/cache/module/part?upload_id=${upload_id}&part_number=2" \
    -H "content-type: application/octet-stream" \
    --data-binary "part-two"
  [ "$status" -eq 0 ]
  [ "$output" = "204" ]

  run status_only -X POST \
    "${KURA_US_URL}/api/cache/module/complete?upload_id=${upload_id}" \
    -H "content-type: application/json" \
    -d '{"parts":[1,2]}'
  [ "$status" -eq 0 ]
  [ "$output" = "204" ]

  run status_only -I \
    "${KURA_EU_URL}/api/cache/module/module-1?tenant_id=acme&namespace_id=ios&hash=hash-1&name=Module.framework&cache_category=builds"
  [ "$status" -eq 0 ]
  [ "$output" = "204" ]

  run wait_for_contains \
    "${KURA_EU_URL}/api/cache/module/module-1?tenant_id=acme&namespace_id=ios&hash=hash-1&name=Module.framework&cache_category=builds" \
    "part-one-part-two"
  [ "$status" -eq 0 ]
}

@test "clean removes namespace artifacts across the cluster" {
  run status_only -X DELETE \
    "${KURA_AP_URL}/api/cache/clean?tenant_id=acme&namespace_id=ios"
  [ "$status" -eq 0 ]
  [ "$output" = "204" ]

  run status_only \
    "${KURA_US_URL}/api/cache/cas/artifact-1?tenant_id=acme&namespace_id=ios"
  [ "$status" -eq 0 ]
  [ "$output" = "404" ]

  run status_only \
    "${KURA_EU_URL}/api/cache/keyvalue/cas-1?tenant_id=acme&namespace_id=ios"
  [ "$status" -eq 0 ]
  [ "$output" = "404" ]
}

@test "observability stack is reachable" {
  run curl -fsS "${GRAFANA_URL}/api/health"
  [ "$status" -eq 0 ]
  [[ "$output" == *'"database":"ok"'* ]]

  run wait_for_contains \
    "${PROMETHEUS_URL}/api/v1/query?query=kura_node_info" \
    "us-east"
  [ "$status" -eq 0 ]
  [[ "$output" == *"us-east"* ]]
  [[ "$output" == *"eu-west"* ]]
  [[ "$output" == *"ap-south"* ]]

  run curl -fsS "${KURA_US_URL}/metrics"
  [ "$status" -eq 0 ]
  [[ "$output" == *"kura_http_requests_total"* ]]
}
