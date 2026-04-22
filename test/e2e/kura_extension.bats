#!/usr/bin/env bats

setup_file() {
  export COMPOSE_PROJECT_NAME="kura-extension"
  export KURA_US_PORT=4501
  export KURA_EU_PORT=4502
  export KURA_AP_PORT=4503
  export KURA_US_URL="http://localhost:${KURA_US_PORT}"
  export KURA_EU_URL="http://localhost:${KURA_EU_PORT}"
  export KURA_AP_URL="http://localhost:${KURA_AP_PORT}"
  dc down -v --remove-orphans >/dev/null 2>&1 || true
  dc up --build -d kura-us kura-eu kura-ap

  wait_for_http "${KURA_US_URL}/up"
  wait_for_http "${KURA_EU_URL}/up"
  wait_for_http "${KURA_AP_URL}/up"
  wait_for_contains "${KURA_US_URL}/up" '"ring_members":3'
  wait_for_contains "${KURA_EU_URL}/up" '"ring_members":3'
  wait_for_contains "${KURA_AP_URL}/up" '"ring_members":3'
}

teardown_file() {
  dc logs --no-color >"${BATS_FILE_TMPDIR}/compose.log" 2>&1 || true
  dc down -v --remove-orphans >/dev/null 2>&1 || true
}

dc() {
  docker compose -f docker-compose.yml -f test/e2e/docker-compose.extension.yml "$@"
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

jwt_for_namespace() {
  local namespace_id="$1"
  python3 - "$namespace_id" <<'PY'
import base64
import hashlib
import hmac
import json
import sys

def b64url(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()

namespace_id = sys.argv[1]
header = {"alg": "HS256", "typ": "JWT"}
payload = {"sub": "user-1", "namespace_id": namespace_id, "exp": 4000000000}

header_b64 = b64url(json.dumps(header, separators=(",", ":")).encode())
payload_b64 = b64url(json.dumps(payload, separators=(",", ":")).encode())
signing_input = f"{header_b64}.{payload_b64}".encode()
signature = hmac.new(b"extension-jwt-secret", signing_input, hashlib.sha256).digest()
print(f"{header_b64}.{payload_b64}.{b64url(signature)}")
PY
}

expected_signature() {
  local payload="$1"
  python3 - "$payload" <<'PY'
import base64
import hashlib
import hmac
import sys

payload = sys.argv[1].encode()
signature = hmac.new(b"extension-signing-secret", payload, hashlib.sha256).digest()
print(base64.b64encode(signature).decode())
PY
}

@test "extension enforces authz and signs module cache responses" {
  run status_only -X POST \
    "${KURA_US_URL}/api/cache/cas/artifact-1?tenant_id=acme&namespace_id=ios" \
    -H "content-type: application/octet-stream" \
    --data-binary "xcode-binary"
  [ "$status" -eq 0 ]
  [ "$output" = "401" ]

  ios_token="$(jwt_for_namespace ios)"
  android_token="$(jwt_for_namespace android)"

  run status_only -X POST \
    "${KURA_US_URL}/api/cache/cas/artifact-1?tenant_id=acme&namespace_id=ios" \
    -H "authorization: Bearer ${ios_token}" \
    -H "content-type: application/octet-stream" \
    --data-binary "xcode-binary"
  [ "$status" -eq 0 ]
  [ "$output" = "204" ]

  run status_only \
    "${KURA_EU_URL}/api/cache/cas/artifact-1?tenant_id=acme&namespace_id=ios" \
    -H "authorization: Bearer ${android_token}"
  [ "$status" -eq 0 ]
  [ "$output" = "403" ]

  run curl -fsS -X POST \
    "${KURA_US_URL}/api/cache/module/start?tenant_id=acme&namespace_id=ios&hash=hash-1&name=Module.framework&cache_category=builds" \
    -H "authorization: Bearer ${ios_token}"
  [ "$status" -eq 0 ]
  upload_id="$(printf '%s' "$output" | sed -E 's/.*"upload_id":"([^"]+)".*/\1/')"
  [ -n "$upload_id" ]

  run status_only -X POST \
    "${KURA_US_URL}/api/cache/module/part?upload_id=${upload_id}&part_number=1" \
    -H "authorization: Bearer ${ios_token}" \
    -H "content-type: application/octet-stream" \
    --data-binary "part-one-"
  [ "$status" -eq 0 ]
  [ "$output" = "204" ]

  run status_only -X POST \
    "${KURA_US_URL}/api/cache/module/part?upload_id=${upload_id}&part_number=2" \
    -H "authorization: Bearer ${ios_token}" \
    -H "content-type: application/octet-stream" \
    --data-binary "part-two"
  [ "$status" -eq 0 ]
  [ "$output" = "204" ]

  run status_only -X POST \
    "${KURA_US_URL}/api/cache/module/complete?upload_id=${upload_id}" \
    -H "authorization: Bearer ${ios_token}" \
    -H "content-type: application/json" \
    -d '{"parts":[1,2]}'
  [ "$status" -eq 0 ]
  [ "$output" = "204" ]

  local headers_file="${BATS_TEST_TMPDIR}/module.headers"
  local body_file="${BATS_TEST_TMPDIR}/module.body"
  run curl -fsS \
    -D "${headers_file}" \
    -o "${body_file}" \
    "${KURA_EU_URL}/api/cache/module/module-1?tenant_id=acme&namespace_id=ios&hash=hash-1&name=Module.framework&cache_category=builds" \
    -H "authorization: Bearer ${ios_token}"
  [ "$status" -eq 0 ]

  body="$(cat "${body_file}")"
  [ "$body" = "part-one-part-two" ]

  signature="$(awk 'BEGIN {IGNORECASE=1} /^x-cache-signature:/ {print $2}' "${headers_file}" | tr -d '\r')"
  [ -n "$signature" ]
  [ "$signature" = "$(expected_signature hash-1)" ]
}
