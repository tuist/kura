import Config

config :cache_next,
  tenant: "test-tenant",
  region: "test",
  tmp_dir: "tmp/test",
  data_dir: "tmp/test-data",
  store_impl: CacheNext.Store.Memory,
  multipart_uploads_impl: CacheNext.MultipartUploads.Memory

config :cache_next, CacheNextWeb.Endpoint,
  http: [ip: {127, 0, 0, 1}, port: 4002],
  secret_key_base: "jGWsgIjwOEiq73kbe2Nz6KbgvufUGzit+LIHxmjHt9pEfk1nbraY0dtEa/Y7zHeX",
  server: false

config :logger, level: :warning

config :phoenix, :plug_init_mode, :runtime

config :phoenix,
  sort_verified_routes_query_params: true
