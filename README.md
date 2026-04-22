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
- `KURA_ROCKSDB_BLOCK_CACHE_BYTES` caps RocksDB's block cache so index and SST warming do not grow RSS without bound
- `KURA_ROCKSDB_WRITE_BUFFER_MANAGER_BYTES` caps total RocksDB memtable memory across column families
- `KURA_ROCKSDB_WRITE_BUFFER_SIZE_BYTES` controls the size of each memtable before flush
- `KURA_ROCKSDB_MAX_WRITE_BUFFER_NUMBER` controls how many memtables each column family may keep in memory

Prometheus also exposes the live RocksDB memory gauges:

- `kura_rocksdb_block_cache_usage_bytes`
- `kura_rocksdb_block_cache_pinned_usage_bytes`
- `kura_rocksdb_block_cache_capacity_bytes`
- `kura_rocksdb_write_buffer_usage_bytes`
- `kura_rocksdb_write_buffer_capacity_bytes`

## 🗺️ Project Areas

Kura is easier to read by subsystem than by tutorial step. The sections below group the project by the main areas you operate or extend.

- 🔌 [Protocol surfaces](#-protocol-surfaces)
- 🗄️ [Storage and replication](#-storage-and-replication)
- ⚙️ [Runtime model and limits](#-runtime-model-and-limits)
- 📊 [Observability and analytics](#-observability-and-analytics)
- ☸️ [Deployment options](#-deployment-options)
- 🧩 [Extensions and policy](#-extensions-and-policy)

## 🔌 Protocol Surfaces

Kura exposes multiple cache protocols behind one service:

- 🍎 `Xcode CAS`: `POST/GET /api/cache/cas/{id}?tenant_id=...&namespace_id=...`
- 🗂️ `Keyvalue / action-cache style entries`: `PUT /api/cache/keyvalue?tenant_id=...&namespace_id=...`
- 🐘 `Gradle`: `PUT/GET /api/cache/gradle/{cache_key}?tenant_id=...&namespace_id=...`
- 📦 `Multipart module cache uploads`:
  - `POST /api/cache/module/start?...`
  - `POST /api/cache/module/part?...`
  - `POST /api/cache/module/complete?...`
  - `HEAD/GET /api/cache/module/{id}?...`
- 🧱 `Nx`: `PUT/GET /v1/cache/{hash}`
- 📱 `Metro`: `PUT/GET /api/metro/cache/{cache_key}`
- 🛠️ `Bazel` and `Buck2`: REAPI over gRPC on `KURA_GRPC_PORT`

The local compose stack is still the quickest way to exercise all of those surfaces together:

```bash
docker compose up --build -d
```

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

References:

- `src/http.rs`
- `src/reapi/mod.rs`
- `test/e2e/kura_cluster.bats`
- `test/e2e/kura_clients.bats`

## 🗄️ Storage And Replication

Kura splits storage into two planes:

- 🪨 RocksDB stores metadata, keyvalue payloads, multipart state, tombstones, segment lifecycle state, and the replication outbox.
- 📦 Segment files store large immutable binary artifacts for the hot path.

Replication is leaderless and eventually consistent:

- 🔁 local writes become durable together with their outbox work
- 🌍 peers bootstrap by pulling manifests, tombstones, and artifact bodies
- 🔎 DNS discovery can expand the peer set automatically
- 🧠 the outbox is processed incrementally so queue depth does not blow up heap usage during backlog

Peer-to-peer mTLS is available for the internal plane:

- `KURA_INTERNAL_PORT`
- `KURA_INTERNAL_TLS_CA_CERT_PATH`
- `KURA_INTERNAL_TLS_CERT_PATH`
- `KURA_INTERNAL_TLS_KEY_PATH`

When peer mTLS is enabled:

- 🔒 `KURA_NODE_URL` and every value in `KURA_PEERS` must use `https://...:<KURA_INTERNAL_PORT>`
- 🌍 the public API still stays on `KURA_PORT`
- 🧱 `/_internal/*` is only served on the internal mTLS listener
- 🪪 the certificate configured through `KURA_INTERNAL_TLS_CERT_PATH` should be valid for both server and client auth
- 🏷️ the certificate SANs must cover the hostname used in `KURA_NODE_URL`

References:

- `src/store.rs`
- `src/replication/`
- `src/peer_tls.rs`
- `test/e2e/kura_discovery.bats`
- `test/e2e/kura_mtls.bats`

## ⚙️ Runtime Model And Limits

Kura is designed around explicit resource budgets instead of relying on ambient process limits.

Important runtime configuration:

- 💾 `KURA_FILE_DESCRIPTOR_POOL_SIZE` controls how many application-managed file operations can hold a descriptor at once
- ⏳ `KURA_FILE_DESCRIPTOR_ACQUIRE_TIMEOUT_MS` controls how long requests wait before backpressure fails the checkout
- 🧷 `KURA_SEGMENT_HANDLE_CACHE_SIZE` caps how many long-lived segment read handles can stay pinned in the process, and it must stay below the FD pool size so transient operations keep headroom
- 🧠 `KURA_MEMORY_SOFT_LIMIT_BYTES` marks the point where Kura starts shedding optional memory use
- 🚫 `KURA_MEMORY_HARD_LIMIT_BYTES` marks the point where Kura pauses outbox replication and trims hot caches aggressively
- 🗂️ `KURA_MANIFEST_CACHE_MAX_BYTES` caps the in-memory manifest hot cache and must stay below the soft memory limit so cache warming does not consume the whole heap
- 📏 `KURA_MAX_KEYVALUE_BYTES` bounds per-request keyvalue payload memory on both public and replication APIs
- 🪨 `KURA_ROCKSDB_MAX_OPEN_FILES` controls RocksDB's own SST/WAL descriptor budget
- 🛠️ `KURA_ROCKSDB_MAX_BACKGROUND_JOBS` controls RocksDB flush and compaction concurrency
- 📚 `KURA_ROCKSDB_BLOCK_CACHE_BYTES` caps RocksDB's block cache so index and SST warming do not grow RSS without bound
- 🧮 `KURA_ROCKSDB_WRITE_BUFFER_MANAGER_BYTES` caps total RocksDB memtable memory across column families
- 🧱 `KURA_ROCKSDB_WRITE_BUFFER_SIZE_BYTES` controls the size of each memtable before flush
- 🔢 `KURA_ROCKSDB_MAX_WRITE_BUFFER_NUMBER` controls how many memtables each column family may keep in memory

A minimal direct-binary deployment still looks like:

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
KURA_ROCKSDB_BLOCK_CACHE_BYTES=67108864 \
KURA_ROCKSDB_WRITE_BUFFER_MANAGER_BYTES=67108864 \
KURA_ROCKSDB_WRITE_BUFFER_SIZE_BYTES=16777216 \
KURA_ROCKSDB_MAX_WRITE_BUFFER_NUMBER=4 \
KURA_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT=http://otel-collector:4318/v1/traces \
KURA_OTEL_SERVICE_NAME=kura-eu-central \
KURA_OTEL_DEPLOYMENT_ENVIRONMENT=production \
./target/release/kura
```

References:

- `src/config.rs`
- `src/memory/`
- `src/io/`

## 📊 Observability And Analytics

Kura ships with a fairly complete local observability story:

- 📈 Prometheus metrics
- 📉 Grafana dashboards
- 🪵 Loki and Promtail logs
- 🧭 Tempo traces

Prometheus exposes the live RocksDB memory gauges:

- `kura_rocksdb_block_cache_usage_bytes`
- `kura_rocksdb_block_cache_pinned_usage_bytes`
- `kura_rocksdb_block_cache_capacity_bytes`
- `kura_rocksdb_write_buffer_usage_bytes`
- `kura_rocksdb_write_buffer_capacity_bytes`

Kura also exports:

- 📦 artifact read and write counters by `kind`, `client`, `artifact_class`, and `result`
- 🔁 replication latency and result metrics
- 💾 file descriptor pool pressure metrics
- 🧠 manifest cache occupancy and admission metrics
- 📣 analytics queue, batch, and circuit-breaker metrics

Analytics webhooks are a separate optional subsystem that mirrors the older Tuist cache contract for Xcode and Gradle traffic.

When enabled:

- 🍎 Xcode upload and download events are sent to `/webhooks/cache`
- 🐘 Gradle upload and download events are sent to `/webhooks/gradle-cache`
- ✍️ requests are signed with `x-cache-signature`
- 🧭 requests also include `x-cache-endpoint`
- 🪶 delivery stays in-memory and best-effort, so analytics never block the hot path
- 🧯 a per-pipeline circuit breaker opens after repeated delivery failures so Kura sheds analytics instead of backing up under a misbehaving upstream

Configure it with:

- `KURA_ANALYTICS_SERVER_URL`
- `KURA_ANALYTICS_SIGNING_KEY`
- optional `KURA_ANALYTICS_BATCH_SIZE` default `100`
- optional `KURA_ANALYTICS_BATCH_TIMEOUT_MS` default `5000`
- optional `KURA_ANALYTICS_QUEUE_CAPACITY` default `1000`
- optional `KURA_ANALYTICS_REQUEST_TIMEOUT_MS` default `5000`
- optional `KURA_ANALYTICS_CIRCUIT_BREAKER_FAILURE_THRESHOLD` default `5`
- optional `KURA_ANALYTICS_CIRCUIT_BREAKER_OPEN_MS` default `30000`

References:

- `src/metrics.rs`
- `src/analytics.rs`
- `ops/grafana/`

## ☸️ Deployment Options

### Helm And Kubernetes

The repository includes a Helm chart at `ops/helm/kura` that deploys Kura as a `StatefulSet` with:

- 💾 one PVC per pod for RocksDB state and segment storage
- 🧭 a headless service for stable pod DNS and peer discovery
- 🌐 a regular service exposing both HTTP and gRPC
- 🚪 optional ingress for the HTTP API
- 🧩 optional inline extension script mounting through a `ConfigMap`
- 🔐 optional peer mTLS for `/_internal/*` traffic via a mounted Kubernetes `Secret`

Lint and render the chart:

```bash
helm lint ops/helm/kura
helm template kura ops/helm/kura --namespace kura
```

Install it on a generic cluster:

```bash
helm upgrade --install kura ./ops/helm/kura \
  --namespace kura \
  --create-namespace \
  --set image.repository=ghcr.io/tuist/kura \
  --set image.tag=latest \
  --set config.region=fr-par \
  --set config.telemetry.otlpTracesEndpoint=http://otel-collector.monitoring.svc.cluster.local:4318/v1/traces
```

For a local kind smoke test, the repo includes:

```bash
./test/e2e/kura_helm_kind.sh
```

To enable peer mTLS in Kubernetes, set:

- `peerTls.enabled=true`
- `peerTls.internalPort=<port>`
- `peerTls.secretName=<secret-with-ca-cert-and-key-material>`

The referenced secret should contain the files configured by:

- `peerTls.caCertFileName`
- `peerTls.certFileName`
- `peerTls.keyFileName`

When enabled, the chart advertises peer URLs over `https` on the internal port and mounts the secret into `/etc/kura/peer-tls`.

### Scaleway Kapsule

For Scaleway, start from the bundled overrides in `ops/helm/kura/values-scaleway.yaml`:

```bash
helm upgrade --install kura ./ops/helm/kura \
  --namespace kura \
  --create-namespace \
  -f ./ops/helm/kura/values-scaleway.yaml \
  --set image.repository=ghcr.io/tuist/kura \
  --set image.tag=latest \
  --set config.region=fr-par \
  --set config.telemetry.otlpTracesEndpoint=http://otel-collector.monitoring.svc.cluster.local:4318/v1/traces
```

That values file does two important things:

- 🚪 uses a `LoadBalancer` service, which is the simplest way to expose Kura on Kapsule
- 💾 pins persistence to `scw-bssd`, which Scaleway documents as the default block storage class for Kapsule multi-AZ clusters

References:

- `ops/helm/kura`
- `ops/helm/kura/values-scaleway.yaml`
- `test/e2e/kura_helm_kind.sh`

## 🧩 Extensions And Policy

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

- ✍️ signers:
  - `KURA_EXTENSION_SIGNER_<ID>_ALGORITHM`
  - `KURA_EXTENSION_SIGNER_<ID>_SECRET`
- 🪪 JWT verifiers:
  - `KURA_EXTENSION_JWT_VERIFIER_<ID>_ALGORITHM`
  - `KURA_EXTENSION_JWT_VERIFIER_<ID>_SECRET`
  - `KURA_EXTENSION_JWT_VERIFIER_<ID>_ISSUER`
  - `KURA_EXTENSION_JWT_VERIFIER_<ID>_AUDIENCES`
- 🌐 HTTP clients:
  - `KURA_EXTENSION_HTTP_CLIENT_<ID>_BASE_URL`
  - `KURA_EXTENSION_HTTP_CLIENT_<ID>_CONNECT_TIMEOUT_MS`
  - `KURA_EXTENSION_HTTP_CLIENT_<ID>_REQUEST_TIMEOUT_MS`

The script may define these hooks:

- `authenticate(ctx)`
- `authorize(ctx, principal)`
- `response_headers(ctx, principal)`

The runtime keeps decision caching, metrics, timeouts, and cryptographic primitives in Rust, while the script supplies policy.

References:

- `src/extension/`
- `test/e2e/kura_extension.bats`

<a id="reference"></a>
## 📖 Reference

### ⚙️ Runtime config reference

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
- `KURA_ROCKSDB_BLOCK_CACHE_BYTES` caps RocksDB's block cache so index and SST warming do not grow RSS without bound
- `KURA_ROCKSDB_WRITE_BUFFER_MANAGER_BYTES` caps total RocksDB memtable memory across column families
- `KURA_ROCKSDB_WRITE_BUFFER_SIZE_BYTES` controls the size of each memtable before flush
- `KURA_ROCKSDB_MAX_WRITE_BUFFER_NUMBER` controls how many memtables each column family may keep in memory

### 📊 Metrics reference

Prometheus exposes the live RocksDB memory gauges:

- `kura_rocksdb_block_cache_usage_bytes`
- `kura_rocksdb_block_cache_pinned_usage_bytes`
- `kura_rocksdb_block_cache_capacity_bytes`
- `kura_rocksdb_write_buffer_usage_bytes`
- `kura_rocksdb_write_buffer_capacity_bytes`

Kura also exports:

- 📦 artifact read and write counters by `kind`, `client`, `artifact_class`, and `result`
- 🔁 replication latency and result metrics
- 💾 file descriptor pool pressure metrics
- 🧠 manifest cache occupancy and admission metrics
- 📣 analytics queue, batch, and circuit-breaker metrics

### 🌍 Deployment references

For Scaleway, start from the bundled overrides in `ops/helm/kura/values-scaleway.yaml`:

```bash
helm upgrade --install kura ./ops/helm/kura \
  --namespace kura \
  --create-namespace \
  -f ./ops/helm/kura/values-scaleway.yaml \
  --set image.repository=ghcr.io/tuist/kura \
  --set image.tag=latest \
  --set config.region=fr-par \
  --set config.telemetry.otlpTracesEndpoint=http://otel-collector.monitoring.svc.cluster.local:4318/v1/traces
```

That values file does two important things:

- 🚪 uses a `LoadBalancer` service, which is the simplest way to expose Kura on Kapsule
- 💾 pins persistence to `scw-bssd`, which Scaleway documents as the default block storage class for Kapsule multi-AZ clusters

Useful external references:

- [Scaleway multi-AZ storage guidance](https://www.scaleway.com/en/docs/kubernetes/reference-content/multi-az-clusters/)
- [Scaleway LoadBalancer annotations](https://www.scaleway.com/en/docs/kubernetes/reference-content/using-load-balancer-annotations/)
- [Scaleway NGINX ingress with Kapsule](https://www.scaleway.com/en/docs/kubernetes/reference-content/lb-ingress-controller/)
