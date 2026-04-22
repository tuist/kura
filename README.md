<p align="center">
  <img src=".github/assets/kura-logo.png" alt="Kura logo" width="420" />
</p>

# Kura

`Kura` is a Rust server for building low-latency cache meshes for tenants, handling distributed cache traffic for binary artifacts and metadata.

> [!NOTE]
> `Kura` comes from the Japanese word `蔵` (`kura`), which refers to a storehouse or warehouse. The name fits the system's role: keeping build artifacts and cache metadata stored durably and close at hand so they can be served with low latency.

## Summary ✨

- ⚡ Hot reads come from local disk
- 🪨 Local metadata, multipart state, and the replication outbox live in RocksDB
- 🔁 Blobs and cache metadata replicate to peer nodes with eventual consistency
- 🔎 Nodes can discover peers through DNS and bootstrap themselves from already-running nodes
- 📦 The HTTP API covers key value entries, Xcode CAS artifacts, Gradle artifacts, multipart module uploads, Nx self-hosted cache artifacts, Metro cache artifacts, and namespace clean
- 🧰 The gRPC API exposes the Bazel Remote Execution cache services used by Bazel and Buck2
- 📊 The local stack includes Grafana, Prometheus, Loki, Promtail, and Tempo traces

## Local stack 🧪

Run:

```bash
docker compose up --build -d
```

Useful endpoints:

- `http://localhost:4101/up`
- `http://localhost:4102/up`
- `http://localhost:4103/up`
- `grpc://localhost:5101` for Bazel/Buck2 REAPI against `kura-us`
- `grpc://localhost:5102` for Bazel/Buck2 REAPI against `kura-eu`
- `grpc://localhost:5103` for Bazel/Buck2 REAPI against `kura-ap`
- `http://localhost:3000` for Grafana with `admin` / `admin`
- `http://localhost:9090` for Prometheus
- `http://localhost:3100` for Loki
- `http://localhost:3200` for Tempo

Supported cache protocols:

- `Bazel` and `Buck2`: Bazel Remote Execution API v2 over gRPC on `KURA_GRPC_PORT`
- `Nx`: self-hosted remote cache API on `GET/PUT /v1/cache/{hash}`
- `React Native Metro`: `HttpStore` / `HttpGetStore` on `GET/PUT /api/metro/cache/{cache_key}`

## Toolchain 🛠️

Install Rust from `mise.toml`:

```bash
mise trust mise.toml
mise install
```

Run tests:

```bash
mise x rust@1.94.1 -- cargo test
bats test/e2e/*.bats
```

Important runtime configuration:

- `KURA_FILE_DESCRIPTOR_POOL_SIZE` controls how many application-managed file operations can hold a descriptor at once
- `KURA_FILE_DESCRIPTOR_ACQUIRE_TIMEOUT_MS` controls how long requests wait before backpressure fails the checkout
- `KURA_PEERS` provides seed peers that Kura can use immediately, even before DNS discovery has converged
- `KURA_DISCOVERY_DNS_NAME` optionally enables DNS-based node discovery. Every node resolved behind that name is probed and, if healthy, becomes a replication and bootstrap target automatically
- `KURA_SEGMENT_HANDLE_CACHE_SIZE` caps how many long-lived segment read handles can stay pinned in the process, and it must stay below the FD pool size so transient operations keep headroom
- `KURA_MEMORY_SOFT_LIMIT_BYTES` marks the point where Kura starts shedding optional memory use
- `KURA_MEMORY_HARD_LIMIT_BYTES` marks the point where Kura pauses outbox replication and trims hot caches aggressively
- `KURA_MANIFEST_CACHE_MAX_BYTES` caps the in-memory manifest hot cache and must stay below the soft memory limit so cache warming does not consume the whole heap
- `KURA_MAX_KEYVALUE_BYTES` bounds per-request keyvalue payload memory on both public and replication APIs
- `KURA_ROCKSDB_MAX_OPEN_FILES` controls RocksDB's own SST/WAL descriptor budget
- `KURA_ROCKSDB_MAX_BACKGROUND_JOBS` controls RocksDB flush and compaction concurrency

## Using Kura

Kura exposes multiple cache protocols behind one service:

- `Xcode CAS`: `POST/GET /api/cache/cas/{id}?tenant_id=...&namespace_id=...`
- `Keyvalue / action-cache style entries`: `PUT /api/cache/keyvalue?tenant_id=...&namespace_id=...`
- `Gradle`: `PUT/GET /api/cache/gradle/{cache_key}?tenant_id=...&namespace_id=...`
- `Multipart module cache uploads`:
  - `POST /api/cache/module/start?...`
  - `POST /api/cache/module/part?...`
  - `POST /api/cache/module/complete?...`
  - `HEAD/GET /api/cache/module/{id}?...`
- `Nx`: `PUT/GET /v1/cache/{hash}`
- `Metro`: `PUT/GET /api/metro/cache/{cache_key}`
- `Bazel` and `Buck2`: REAPI over gRPC on `KURA_GRPC_PORT`

Example Xcode artifact round trip:

```bash
curl -X POST \
  "http://localhost:4101/api/cache/cas/artifact-1?tenant_id=acme&namespace_id=ios" \
  -H "content-type: application/octet-stream" \
  --data-binary "xcode-binary"

curl \
  "http://localhost:4102/api/cache/cas/artifact-1?tenant_id=acme&namespace_id=ios"
```

Example keyvalue entry round trip:

```bash
curl -X PUT \
  "http://localhost:4101/api/cache/keyvalue?tenant_id=acme&namespace_id=ios" \
  -H "content-type: application/json" \
  -d '{"cas_id":"cas-1","entries":[{"value":"hello"},{"value":"world"}]}'

curl \
  "http://localhost:4103/api/cache/keyvalue/cas-1?tenant_id=acme&namespace_id=ios"
```

## Self-Hosting Kura

At minimum, a node needs:

- writable directories for `KURA_TMP_DIR` and `KURA_DATA_DIR`
- a stable `KURA_NODE_URL`
- seed peers in `KURA_PEERS`
- a unique `KURA_REGION`
- a shared `KURA_TENANT_ID`

A minimal single-node example:

```bash
KURA_PORT=4000 \
KURA_GRPC_PORT=50051 \
KURA_TENANT_ID=default \
KURA_REGION=eu-central \
KURA_TMP_DIR=/tmp/kura \
KURA_DATA_DIR=/var/cache/kura \
KURA_NODE_URL=http://cache-1.internal:4000 \
KURA_PEERS=http://cache-1.internal:4000 \
KURA_FILE_DESCRIPTOR_POOL_SIZE=128 \
KURA_FILE_DESCRIPTOR_ACQUIRE_TIMEOUT_MS=5000 \
KURA_SEGMENT_HANDLE_CACHE_SIZE=32 \
KURA_MEMORY_SOFT_LIMIT_BYTES=536870912 \
KURA_MEMORY_HARD_LIMIT_BYTES=805306368 \
KURA_MANIFEST_CACHE_MAX_BYTES=67108864 \
KURA_MAX_KEYVALUE_BYTES=1048576 \
KURA_ROCKSDB_MAX_OPEN_FILES=1024 \
KURA_ROCKSDB_MAX_BACKGROUND_JOBS=4 \
KURA_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT=http://otel-collector:4318/v1/traces \
KURA_OTEL_SERVICE_NAME=kura-eu-central \
KURA_OTEL_DEPLOYMENT_ENVIRONMENT=production \
./target/release/kura
```

For multi-node deployments:

- keep `KURA_PEERS` populated with at least a seed set of nodes
- optionally set `KURA_DISCOVERY_DNS_NAME` to a DNS name that resolves to all healthy nodes
- ensure every node can reach every other node's HTTP port
- keep clocks reasonably in sync because replication and tombstones use version timestamps

## Extension Scripts

Kura can load one operator-provided extension script at startup to customize authentication, authorization, and response headers without recompiling the binary.

Core env vars:

- `KURA_EXTENSION_ENABLED=true`
- `KURA_EXTENSION_SCRIPT_PATH=/etc/kura/extensions/hooks.lua`
- `KURA_EXTENSION_HOOK_TIMEOUT_MS=25`
- `KURA_EXTENSION_AUTH_CACHE_ALLOW_TTL_SECONDS=600`
- `KURA_EXTENSION_AUTH_CACHE_DENY_TTL_SECONDS=3`
- `KURA_EXTENSION_FAIL_CLOSED_AUTHENTICATE=true`
- `KURA_EXTENSION_FAIL_CLOSED_AUTHORIZE=true`
- `KURA_EXTENSION_FAIL_OPEN_RESPONSE_HEADERS=true`

Generic host resources are also env-driven:

- signers:
  - `KURA_EXTENSION_SIGNER_<ID>_ALGORITHM`
  - `KURA_EXTENSION_SIGNER_<ID>_SECRET`
- JWT verifiers:
  - `KURA_EXTENSION_JWT_VERIFIER_<ID>_ALGORITHM`
  - `KURA_EXTENSION_JWT_VERIFIER_<ID>_SECRET`
  - `KURA_EXTENSION_JWT_VERIFIER_<ID>_ISSUER`
  - `KURA_EXTENSION_JWT_VERIFIER_<ID>_AUDIENCES`
- HTTP clients:
  - `KURA_EXTENSION_HTTP_CLIENT_<ID>_BASE_URL`
  - `KURA_EXTENSION_HTTP_CLIENT_<ID>_CONNECT_TIMEOUT_MS`
  - `KURA_EXTENSION_HTTP_CLIENT_<ID>_REQUEST_TIMEOUT_MS`

The script may define these hooks:

- `authenticate(ctx)`
- `authorize(ctx, principal)`
- `response_headers(ctx, principal)`

The runtime keeps decision caching, metrics, timeouts, and cryptographic primitives in Rust, while the script supplies policy.
