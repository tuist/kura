import Config

if System.get_env("PHX_SERVER") do
  config :cache_next, CacheNextWeb.Endpoint, server: true
end

tenant = System.get_env("TENANT_ID") || Application.get_env(:cache_next, :tenant)
region = System.get_env("CACHE_REGION") || Application.get_env(:cache_next, :region)
tmp_dir = System.get_env("CACHE_TMP_DIR") || Application.get_env(:cache_next, :tmp_dir)
data_dir = System.get_env("CACHE_DATA_DIR") || Application.get_env(:cache_next, :data_dir)

riak_http_endpoints =
  System.get_env("RIAK_HTTP_ENDPOINTS", "")
  |> String.split(",", trim: true)
  |> case do
    [] -> Application.get_env(:cache_next, :riak_http_endpoints, ["http://localhost:8098"])
    values -> values
  end

riak_pb_host =
  System.get_env("RIAK_PB_HOST") || Application.get_env(:cache_next, :riak_pb_host, "127.0.0.1")

riak_pb_port =
  String.to_integer(
    System.get_env(
      "RIAK_PB_PORT",
      Integer.to_string(Application.get_env(:cache_next, :riak_pb_port, 8087))
    )
  )

riak_pb_pool_size =
  String.to_integer(
    System.get_env(
      "RIAK_PB_POOL_SIZE",
      Integer.to_string(Application.get_env(:cache_next, :riak_pb_pool_size, 8))
    )
  )

riak_chunk_size_bytes =
  String.to_integer(
    System.get_env(
      "RIAK_CHUNK_SIZE_BYTES",
      Integer.to_string(Application.get_env(:cache_next, :riak_chunk_size_bytes, 1_048_576))
    )
  )

config :cache_next,
  tenant: tenant,
  region: region,
  tmp_dir: tmp_dir,
  data_dir: data_dir,
  riak_http_endpoints: riak_http_endpoints,
  riak_pb_host: riak_pb_host,
  riak_pb_port: riak_pb_port,
  riak_pb_pool_size: riak_pb_pool_size,
  riak_chunk_size_bytes: riak_chunk_size_bytes

config :cache_next, CacheNextWeb.Endpoint,
  http: [ip: {0, 0, 0, 0}, port: String.to_integer(System.get_env("PORT", "4000"))]

if System.get_env("OTEL_EXPORTER_OTLP_ENDPOINT") ||
     System.get_env("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT") do
  config :opentelemetry, traces_exporter: :otlp
end

if config_env() == :prod do
  secret_key_base =
    System.get_env("SECRET_KEY_BASE") ||
      raise """
      environment variable SECRET_KEY_BASE is missing.
      You can generate one by calling: mix phx.gen.secret
      """

  host = System.get_env("PHX_HOST") || "localhost"

  config :cache_next, CacheNextWeb.Endpoint,
    url: [host: host, port: String.to_integer(System.get_env("PORT", "4000")), scheme: "http"],
    secret_key_base: secret_key_base
end
