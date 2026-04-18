# Cache Next

`cache-next` is a Phoenix API node that mirrors the existing `tuist/cache` HTTP surface and uses a full Riak KV cluster as the storage and replication layer. Phoenix no longer owns shard placement or replication itself; it stores manifests, multipart state, and blob chunks in Riak and lets Riak handle cluster membership, replica placement, handoff, and rebalancing. Each regional container now launches both the Phoenix API process and a colocated Riak node from the same image.

## What is implemented

- Same cache-facing API shape for key-value, Xcode CAS, Gradle cache, multipart module uploads, project clean, and Swift registry read endpoints.
- No authentication or authorization yet.
- Three regional nodes, each running Phoenix plus its local Riak node:
  - `cache-us`
  - `cache-eu`
  - `cache-ap`
- Phoenix talks to its colocated Riak over loopback TCP instead of hopping across container boundaries.
- Large artifact storage via Riak manifests plus chunk objects. Chunks are intentionally kept small because Riak KV documentation recommends avoiding large single objects.
- Multipart upload state stored in Riak instead of local process memory.
- Local observability stack:
  - `prometheus`
  - `loki`
  - `tempo`
  - `grafana`
  - `promtail`
- End-to-end verification in `test/e2e/cache_cluster.bats` and `test/e2e/cache_handoff.bats`
- GitHub Actions workflow in `.github/workflows/e2e.yml`

## Toolchain

`mise.toml` is pinned to:

- Elixir `1.19.4`
- Erlang/OTP `28.4.2`

## Local development

```bash
mise trust mise.toml
mise install
mise exec -- mix setup
mise exec -- mix phx.server
```

For local `mix test`, the app uses in-memory store backends. Runtime and e2e flows use Riak.

## Cluster simulation

The Compose stack boots the three combined regional nodes and the observability stack:

```bash
docker compose up --build -d
```

Useful endpoints:

- `http://localhost:4101/up`
- `http://localhost:4102/up`
- `http://localhost:4103/up`
- `http://localhost:8098/ping` for the exposed Riak HTTP endpoint on `cache-us`
- `http://localhost:3000` for Grafana (`admin` / `admin`)
- `http://localhost:9090` for Prometheus
- `http://localhost:3100` for Loki
- `http://localhost:3200` for Tempo

The provisioned Grafana dashboard is `Cache Next Cluster`.

## Storage model

- API requests can land on any Phoenix node.
- Phoenix stores artifact manifests, multipart state, and blob chunks in Riak.
- Final artifacts are stored as:
  - one manifest object with metadata and chunk references
  - many chunk objects for the payload
- Multipart uploads are stored as:
  - one upload manifest object
  - chunked part objects
- Project clean looks up artifact manifests through Riak secondary indexes keyed by `project_handle`.
- Riak handles cluster replication and topology change handoff.

## End-to-end tests

The Bats suite boots the Compose stack, waits for Riak-backed health to converge, exercises the APIs across regions, and verifies synchronization:

```bash
bats test/e2e/*.bats
```

The suite covers:

- key-value synchronization
- Xcode CAS persistence across API node restart
- Gradle cache synchronization
- multipart module upload completion and remote visibility
- project-wide clean
- Riak-backed topology growth from a singleton node to a three-node cluster
- observability stack reachability
