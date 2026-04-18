# This file is responsible for configuring your application
# and its dependencies with the aid of the Config module.
#
# This configuration file is loaded before any dependency and
# is restricted to this project.

# General application configuration
import Config

config :cache_next,
  tenant: "demo-tenant",
  region: "local",
  tmp_dir: "tmp/cache-next",
  data_dir: "tmp/cache-next-data",
  store_impl: CacheNext.Store.Riak,
  multipart_uploads_impl: CacheNext.MultipartUploads.Riak,
  riak_http_endpoints: ["http://localhost:8098"],
  riak_pb_host: "127.0.0.1",
  riak_pb_port: 8087,
  riak_pb_pool_size: 8,
  riak_chunk_size_bytes: 1_048_576,
  store_request_timeout_ms: 30_000,
  generators: [timestamp_type: :utc_datetime]

config :cache_next, CacheNextWeb.Endpoint,
  url: [host: "localhost"],
  adapter: Bandit.PhoenixAdapter,
  render_errors: [
    formats: [json: CacheNextWeb.ErrorJSON],
    layout: false
  ],
  pubsub_server: CacheNext.PubSub

config :logger, :default_formatter,
  format: "$time $metadata[$level] $message\n",
  metadata: [
    :request_id,
    :region,
    :tenant,
    :account_handle,
    :project_handle,
    :artifact_kind,
    :artifact_key,
    :trace_id,
    :span_id
  ]

config :phoenix, :json_library, Jason

config :opentelemetry,
  span_processor: :batch,
  traces_exporter: :none

config :opentelemetry_exporter,
  otlp_protocol: :http_protobuf,
  otlp_endpoint: "http://localhost:4318"

import_config "#{config_env()}.exs"
