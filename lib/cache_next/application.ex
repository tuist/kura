defmodule CacheNext.Application do
  @moduledoc false

  use Application
  require Logger

  @impl true
  def start(_type, _args) do
    setup_observability()

    children =
      [
        CacheNextWeb.Telemetry,
        {Phoenix.PubSub, name: CacheNext.PubSub},
        maybe_riak_child(),
        maybe_child(CacheNext.Store.backend()),
        maybe_child(CacheNext.MultipartUploads.backend()),
        CacheNextWeb.Endpoint
      ]
      |> Enum.reject(&is_nil/1)

    opts = [strategy: :one_for_one, name: :cache_next_sup]
    Supervisor.start_link(children, opts)
  end

  @impl true
  def config_change(changed, _new, removed) do
    CacheNextWeb.Endpoint.config_change(changed, removed)
    :ok
  end

  defp setup_observability do
    Logger.metadata(region: CacheNext.Config.region(), tenant: CacheNext.Config.tenant())

    OpentelemetryLoggerMetadata.setup()
    OpentelemetryBandit.setup()
    OpentelemetryPhoenix.setup(adapter: :bandit)
  end

  defp maybe_child(module) do
    Code.ensure_loaded(module)

    if function_exported?(module, :child_spec, 1) do
      {module, []}
    else
      nil
    end
  end

  defp maybe_riak_child do
    store_backend = CacheNext.Store.backend()
    multipart_backend = CacheNext.MultipartUploads.backend()

    if store_backend == CacheNext.Store.Riak or multipart_backend == CacheNext.MultipartUploads.Riak do
      {CacheNext.Riak, []}
    else
      nil
    end
  end
end
