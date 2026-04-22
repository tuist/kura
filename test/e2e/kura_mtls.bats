#!/usr/bin/env bats

setup_file() {
  export COMPOSE_PROJECT_NAME="kura-mtls"
  export KURA_US_PORT=4501
  export KURA_EU_PORT=4502
  export KURA_AP_PORT=4503
  export GRAFANA_PORT=3400
  export PROMETHEUS_PORT=9290
  export LOKI_PORT=3210
  export TEMPO_PORT=3310
  export OTLP_PORT=4425
  export KURA_US_URL="http://localhost:${KURA_US_PORT}"
  export KURA_EU_URL="http://localhost:${KURA_EU_PORT}"
  export KURA_AP_URL="http://localhost:${KURA_AP_PORT}"
  export KURA_MTLS_CERT_DIR="${BATS_FILE_TMPDIR}/mtls"

  generate_peer_tls_material

  dc down -v --remove-orphans >/dev/null 2>&1 || true
  dc up --build -d

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
  docker compose -f docker-compose.yml -f test/e2e/docker-compose.mtls.yml "$@"
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

generate_peer_tls_material() {
  mkdir -p "${KURA_MTLS_CERT_DIR}"

  cat >"${KURA_MTLS_CERT_DIR}/openssl.cnf" <<'EOF'
[req]
distinguished_name = req_distinguished_name
prompt = no

[req_distinguished_name]
CN = kura-peer

[peer_cert]
basicConstraints = CA:false
keyUsage = critical, digitalSignature, keyEncipherment
extendedKeyUsage = serverAuth, clientAuth
subjectAltName = @alt_names

[alt_names]
DNS.1 = kura-us.kura.internal
DNS.2 = kura-eu.kura.internal
DNS.3 = kura-ap.kura.internal
DNS.4 = kura-ring.kura.internal
EOF

  openssl genrsa -out "${KURA_MTLS_CERT_DIR}/ca.key" 2048 >/dev/null 2>&1
  openssl req -x509 -new -nodes \
    -key "${KURA_MTLS_CERT_DIR}/ca.key" \
    -sha256 \
    -days 3650 \
    -out "${KURA_MTLS_CERT_DIR}/ca.pem" \
    -subj "/CN=kura-peer-ca" >/dev/null 2>&1

  openssl genrsa -out "${KURA_MTLS_CERT_DIR}/peer.key" 2048 >/dev/null 2>&1
  openssl req -new \
    -key "${KURA_MTLS_CERT_DIR}/peer.key" \
    -out "${KURA_MTLS_CERT_DIR}/peer.csr" \
    -config "${KURA_MTLS_CERT_DIR}/openssl.cnf" >/dev/null 2>&1
  openssl x509 -req \
    -in "${KURA_MTLS_CERT_DIR}/peer.csr" \
    -CA "${KURA_MTLS_CERT_DIR}/ca.pem" \
    -CAkey "${KURA_MTLS_CERT_DIR}/ca.key" \
    -CAcreateserial \
    -out "${KURA_MTLS_CERT_DIR}/peer.pem" \
    -days 3650 \
    -sha256 \
    -extfile "${KURA_MTLS_CERT_DIR}/openssl.cnf" \
    -extensions peer_cert >/dev/null 2>&1
}

@test "internal endpoints require client certificates and replication still works" {
  run status_only "${KURA_US_URL}/_internal/status"
  [ "$status" -eq 0 ]
  [ "$output" = "404" ]

  run dc exec -T kura-us sh -lc \
    "curl --fail --silent --show-error --cacert /etc/kura/mtls/ca.pem https://kura-eu.kura.internal:7443/_internal/status"
  [ "$status" -ne 0 ]

  run dc exec -T kura-us sh -lc \
    "curl --fail --silent --show-error --cacert /etc/kura/mtls/ca.pem --cert /etc/kura/mtls/peer.pem --key /etc/kura/mtls/peer.key https://kura-eu.kura.internal:7443/_internal/status"
  [ "$status" -eq 0 ]
  [[ "$output" == *'"node_url":"https://kura-eu.kura.internal:7443"'* ]]

  run status_only -X POST \
    "${KURA_US_URL}/api/cache/cas/mtls-artifact?tenant_id=acme&namespace_id=ios" \
    -H "content-type: application/octet-stream" \
    --data-binary "mtls-binary"
  [ "$status" -eq 0 ]
  [ "$output" = "204" ]

  run wait_for_contains \
    "${KURA_EU_URL}/api/cache/cas/mtls-artifact?tenant_id=acme&namespace_id=ios" \
    "mtls-binary"
  [ "$status" -eq 0 ]
  [ "$output" = "mtls-binary" ]
}
