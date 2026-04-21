# Kura

`Kura` is a Rust server for building low-latency cache meshes for tenants, handling distributed cache traffic for binary artifacts and metadata.

> [!NOTE]
> `Kura` comes from the Japanese word `蔵` (`kura`), which refers to a storehouse or warehouse. The name fits the system's role: keeping build artifacts and cache metadata stored durably and close at hand so they can be served with low latency.

## Summary ✨

- ⚡ Hot reads come from local disk
- 🪨 Local metadata, multipart state, and the replication outbox live in RocksDB
- 🔁 Blobs and cache metadata replicate to peer nodes with eventual consistency
- 📦 The HTTP API covers key value entries, Xcode CAS artifacts, Gradle artifacts, multipart module uploads, and namespace clean
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
- `http://localhost:3000` for Grafana with `admin` / `admin`
- `http://localhost:9090` for Prometheus
- `http://localhost:3100` for Loki
- `http://localhost:3200` for Tempo

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
- `KURA_SEGMENT_HANDLE_CACHE_SIZE` caps how many long-lived segment read handles can stay pinned in the process, and it must stay below the FD pool size so transient operations keep headroom
- `KURA_MEMORY_SOFT_LIMIT_BYTES` marks the point where Kura starts shedding optional memory use
- `KURA_MEMORY_HARD_LIMIT_BYTES` marks the point where Kura pauses outbox replication and trims hot caches aggressively
- `KURA_MANIFEST_CACHE_MAX_BYTES` caps the in-memory manifest hot cache and must stay below the soft memory limit so cache warming does not consume the whole heap
- `KURA_MAX_KEYVALUE_BYTES` bounds per-request keyvalue payload memory on both public and replication APIs
- `KURA_ROCKSDB_MAX_OPEN_FILES` controls RocksDB's own SST/WAL descriptor budget
- `KURA_ROCKSDB_MAX_BACKGROUND_JOBS` controls RocksDB flush and compaction concurrency
