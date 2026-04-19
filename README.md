# cache

`cache` is a Rust cache node for Tuist and Bazel action + binary cache traffic.

## Summary ✨

- ⚡ Hot reads come from local disk
- 🪨 Metadata, multipart state, and the replication outbox live in RocksDB
- 🔁 Blobs and cache metadata replicate to peer nodes with eventual consistency
- 📦 The HTTP API covers key value entries, Xcode CAS artifacts, Gradle artifacts, multipart module uploads, and project clean
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
