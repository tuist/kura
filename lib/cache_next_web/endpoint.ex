defmodule CacheNextWeb.Endpoint do
  use Phoenix.Endpoint, otp_app: :cache_next

  plug Plug.Static,
    at: "/",
    from: :cache_next,
    gzip: not code_reloading?,
    only: CacheNextWeb.static_paths(),
    raise_on_missing_only: code_reloading?

  if code_reloading? do
    plug Phoenix.CodeReloader
  end

  plug Plug.RequestId
  plug Plug.Telemetry, event_prefix: [:phoenix, :endpoint]

  plug Plug.Parsers,
    parsers: [:urlencoded, :multipart, :json],
    pass: ["*/*"],
    json_decoder: Phoenix.json_library()

  plug Plug.MethodOverride
  plug CacheNextWeb.Router
end
