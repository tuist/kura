#!/usr/bin/env bats

setup_file() {
  export COMPOSE_PROJECT_NAME="cache-next-e2e"
  export CACHE_US_PORT=4201
  export CACHE_EU_PORT=4202
  export CACHE_AP_PORT=4203
  export GRAFANA_PORT=3300
  export PROMETHEUS_PORT=9190
  export LOKI_PORT=3201
  export TEMPO_PORT=3301
  export OTLP_PORT=4418
  export RIAK_HTTP_PORT=8198
  export RIAK_PB_PORT=8187
  export CACHE_US_URL="http://localhost:${CACHE_US_PORT}"
  export CACHE_EU_URL="http://localhost:${CACHE_EU_PORT}"
  export CACHE_AP_URL="http://localhost:${CACHE_AP_PORT}"
  export GRAFANA_URL="http://localhost:${GRAFANA_PORT}"
  export PROMETHEUS_URL="http://localhost:${PROMETHEUS_PORT}"
  dc down -v --remove-orphans >/dev/null 2>&1 || true
  dc up --build -d

  wait_for_http "${CACHE_US_URL}/up"
  wait_for_http "${CACHE_EU_URL}/up"
  wait_for_http "${CACHE_AP_URL}/up"
  wait_for_contains "${CACHE_US_URL}/up" '"ring_members":3'
  wait_for_contains "${CACHE_EU_URL}/up" '"ring_members":3'
  wait_for_contains "${CACHE_AP_URL}/up" '"ring_members":3'
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
    "${CACHE_US_URL}/api/cache/keyvalue?account_handle=acme&project_handle=ios" \
    -H "content-type: application/json" \
    -d '{"cas_id":"cas-1","entries":[{"value":"hello"},{"value":"world"}]}'
  [ "$status" -eq 0 ]
  [ "$output" = "204" ]

  run wait_for_contains \
    "${CACHE_EU_URL}/api/cache/keyvalue/cas-1?account_handle=acme&project_handle=ios" \
    '"hello"'
  [ "$status" -eq 0 ]
  [[ "$output" == *'"world"'* ]]
}

@test "xcode artifacts persist in riak and survive api node restart" {
  run status_only -X POST \
    "${CACHE_US_URL}/api/cache/cas/artifact-1?account_handle=acme&project_handle=ios" \
    -H "content-type: application/octet-stream" \
    --data-binary "xcode-binary"
  [ "$status" -eq 0 ]
  [ "$output" = "204" ]

  dc restart cache-eu >/dev/null
  wait_for_http "${CACHE_EU_URL}/up"
  wait_for_contains "${CACHE_EU_URL}/up" '"ring_members":3'

  run wait_for_contains \
    "${CACHE_EU_URL}/api/cache/cas/artifact-1?account_handle=acme&project_handle=ios" \
    "xcode-binary"
  [ "$status" -eq 0 ]
  [ "$output" = "xcode-binary" ]
}

@test "gradle artifacts sync to another region" {
  run status_only -X PUT \
    "${CACHE_EU_URL}/api/cache/gradle/gradle-key-1?account_handle=acme&project_handle=android" \
    -H "content-type: application/octet-stream" \
    --data-binary "gradle-cache"
  [ "$status" -eq 0 ]
  [ "$output" = "201" ]

  run wait_for_contains \
    "${CACHE_AP_URL}/api/cache/gradle/gradle-key-1?account_handle=acme&project_handle=android" \
    "gradle-cache"
  [ "$status" -eq 0 ]
}

@test "multipart module uploads are visible from another node" {
  run curl -fsS -X POST \
    "${CACHE_US_URL}/api/cache/module/start?account_handle=acme&project_handle=ios&hash=hash-1&name=Module.framework&cache_category=builds"
  [ "$status" -eq 0 ]
  upload_id="$(printf '%s' "$output" | sed -E 's/.*"upload_id":"([^"]+)".*/\1/')"
  [ -n "$upload_id" ]

  run status_only -X POST \
    "${CACHE_US_URL}/api/cache/module/part?upload_id=${upload_id}&part_number=1" \
    -H "content-type: application/octet-stream" \
    --data-binary "part-one-"
  [ "$status" -eq 0 ]
  [ "$output" = "204" ]

  run status_only -X POST \
    "${CACHE_US_URL}/api/cache/module/part?upload_id=${upload_id}&part_number=2" \
    -H "content-type: application/octet-stream" \
    --data-binary "part-two"
  [ "$status" -eq 0 ]
  [ "$output" = "204" ]

  run status_only -X POST \
    "${CACHE_US_URL}/api/cache/module/complete?upload_id=${upload_id}" \
    -H "content-type: application/json" \
    -d '{"parts":[1,2]}'
  [ "$status" -eq 0 ]
  [ "$output" = "204" ]

  run status_only -I \
    "${CACHE_EU_URL}/api/cache/module/module-1?account_handle=acme&project_handle=ios&hash=hash-1&name=Module.framework&cache_category=builds"
  [ "$status" -eq 0 ]
  [ "$output" = "204" ]

  run wait_for_contains \
    "${CACHE_EU_URL}/api/cache/module/module-1?account_handle=acme&project_handle=ios&hash=hash-1&name=Module.framework&cache_category=builds" \
    "part-one-part-two"
  [ "$status" -eq 0 ]
}

@test "clean removes project artifacts across the cluster" {
  run status_only -X DELETE \
    "${CACHE_AP_URL}/api/cache/clean?account_handle=acme&project_handle=ios"
  [ "$status" -eq 0 ]
  [ "$output" = "204" ]

  run status_only \
    "${CACHE_US_URL}/api/cache/cas/artifact-1?account_handle=acme&project_handle=ios"
  [ "$status" -eq 0 ]
  [ "$output" = "404" ]

  run status_only \
    "${CACHE_EU_URL}/api/cache/keyvalue/cas-1?account_handle=acme&project_handle=ios"
  [ "$status" -eq 0 ]
  [ "$output" = "404" ]
}

@test "observability stack is reachable" {
  run curl -fsS "${GRAFANA_URL}/api/health"
  [ "$status" -eq 0 ]
  [[ "$output" == *'"database":"ok"'* ]]

  run wait_for_contains \
    "${PROMETHEUS_URL}/api/v1/query?query=cache_next_node_info" \
    "us-east"
  [ "$status" -eq 0 ]
  [[ "$output" == *"us-east"* ]]
  [[ "$output" == *"eu-west"* ]]
  [[ "$output" == *"ap-south"* ]]

  run curl -fsS "${CACHE_US_URL}/metrics"
  [ "$status" -eq 0 ]
  [[ "$output" == *"cache_next_http_requests_total"* ]]
}
