defmodule CacheNextWeb.Telemetry do
  use Supervisor
  import Telemetry.Metrics
  alias TelemetryMetricsPrometheus.Core

  def start_link(arg) do
    Supervisor.start_link(__MODULE__, arg, name: __MODULE__)
  end

  @impl true
  def init(_arg) do
    children = [
      {Core,
       name: CacheNext.Config.prometheus_reporter(), metrics: metrics(), start_async: false},
      {:telemetry_poller, measurements: periodic_measurements(), period: 10_000}
    ]

    Supervisor.init(children, strategy: :one_for_one)
  end

  def metrics do
    [
      counter("cache_next.http.requests.total",
        event_name: [:phoenix, :router_dispatch, :stop],
        measurement: fn _, _ -> 1 end,
        tags: [:route, :method, :status, :region],
        tag_values: &http_tags/1
      ),
      distribution("cache_next.http.request.duration.seconds",
        event_name: [:phoenix, :router_dispatch, :stop],
        measurement: :duration,
        unit: {:native, :second},
        tags: [:route, :method, :status, :region],
        tag_values: &http_tags/1,
        reporter_options: [buckets: [0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2, 5]]
      ),
      counter("cache_next.http.exceptions.total",
        event_name: [:phoenix, :router_dispatch, :exception],
        measurement: fn _, _ -> 1 end,
        tags: [:route, :method, :kind, :region],
        tag_values: &exception_tags/1
      ),
      counter("cache_next.artifact.reads.total",
        event_name: [:cache_next, :artifact, :read],
        measurement: :count,
        tags: [:kind, :result, :region]
      ),
      distribution("cache_next.artifact.read.duration.seconds",
        event_name: [:cache_next, :artifact, :read],
        measurement: :duration,
        unit: {:native, :second},
        tags: [:kind, :result, :region],
        reporter_options: [buckets: [0.001, 0.01, 0.05, 0.1, 0.25, 0.5, 1, 2, 5]]
      ),
      counter("cache_next.artifact.read.bytes.total",
        event_name: [:cache_next, :artifact, :read],
        measurement: :size,
        tags: [:kind, :result, :region]
      ),
      counter("cache_next.artifact.writes.total",
        event_name: [:cache_next, :artifact, :write],
        measurement: :count,
        tags: [:kind, :result, :region]
      ),
      distribution("cache_next.artifact.write.duration.seconds",
        event_name: [:cache_next, :artifact, :write],
        measurement: :duration,
        unit: {:native, :second},
        tags: [:kind, :result, :region],
        reporter_options: [buckets: [0.001, 0.01, 0.05, 0.1, 0.25, 0.5, 1, 2, 5]]
      ),
      counter("cache_next.artifact.write.bytes.total",
        event_name: [:cache_next, :artifact, :write],
        measurement: :size,
        tags: [:kind, :result, :region]
      ),
      counter("cache_next.remote.requests.total",
        event_name: [:cache_next, :remote, :request],
        measurement: :count,
        tags: [:operation, :result, :region]
      ),
      distribution("cache_next.remote.request.duration.seconds",
        event_name: [:cache_next, :remote, :request],
        measurement: :duration,
        unit: {:native, :second},
        tags: [:operation, :result, :region],
        reporter_options: [buckets: [0.001, 0.01, 0.05, 0.1, 0.25, 0.5, 1, 2, 5]]
      ),
      counter("cache_next.multipart.parts.total",
        event_name: [:cache_next, :multipart, :part],
        measurement: :count,
        tags: [:result, :region]
      ),
      counter("cache_next.multipart.bytes.total",
        event_name: [:cache_next, :multipart, :part],
        measurement: :size,
        tags: [:result, :region]
      ),
      last_value("cache_next.node.info",
        event_name: [:cache_next, :node, :info],
        measurement: :value,
        tags: [:region, :tenant]
      ),
      last_value("cache_next.tmp_storage.bytes",
        event_name: [:cache_next, :tmp_storage, :bytes],
        measurement: :bytes,
        tags: [:region]
      ),
      last_value("vm.memory.total.bytes",
        event_name: [:vm, :memory],
        measurement: :total,
        unit: :byte
      ),
      last_value("vm.total_run_queue_lengths.total",
        event_name: [:vm, :total_run_queue_lengths],
        measurement: :total
      ),
      last_value("vm.total_run_queue_lengths.cpu",
        event_name: [:vm, :total_run_queue_lengths],
        measurement: :cpu
      ),
      last_value("vm.total_run_queue_lengths.io",
        event_name: [:vm, :total_run_queue_lengths],
        measurement: :io
      )
    ]
  end

  defp periodic_measurements do
    [
      {__MODULE__, :emit_node_info, []},
      {__MODULE__, :emit_tmp_storage_metrics, []}
    ]
  end

  def emit_node_info do
    :telemetry.execute(
      [:cache_next, :node, :info],
      %{value: 1},
      %{region: CacheNext.Config.region(), tenant: CacheNext.Config.tenant()}
    )
  end

  def emit_tmp_storage_metrics do
    :telemetry.execute(
      [:cache_next, :tmp_storage, :bytes],
      %{bytes: CacheNext.MultipartUploads.tmp_storage_size()},
      %{region: CacheNext.Config.region()}
    )
  end

  defp http_tags(metadata) do
    %{
      route: Map.get(metadata, :route, "unknown"),
      method: metadata.conn.method,
      status: Integer.to_string(metadata.conn.status || 0),
      region: CacheNext.Config.region()
    }
  end

  defp exception_tags(metadata) do
    %{
      route: Map.get(metadata, :route, "unknown"),
      method: metadata.conn.method,
      kind: metadata.kind |> to_string(),
      region: CacheNext.Config.region()
    }
  end
end
