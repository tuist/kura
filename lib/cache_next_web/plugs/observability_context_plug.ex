defmodule CacheNextWeb.Plugs.ObservabilityContextPlug do
  @moduledoc false

  require Logger

  def init(opts), do: opts

  def call(conn, _opts) do
    account_handle = conn.params["account_handle"]
    project_handle = conn.params["project_handle"]

    Logger.metadata(
      region: CacheNext.Config.region(),
      tenant: CacheNext.Config.tenant(),
      account_handle: account_handle,
      project_handle: project_handle
    )

    OpenTelemetry.Tracer.set_attributes([
      {"cache.region", CacheNext.Config.region()},
      {"cache.tenant", CacheNext.Config.tenant()},
      {"cache.account_handle", account_handle || ""},
      {"cache.project_handle", project_handle || ""}
    ])

    conn
  end
end
