import Config

config :cache_next,
  tmp_dir: "tmp/dev",
  data_dir: "tmp/dev-data",
  node_name: "cache-next-dev"

config :cache_next, CacheNextWeb.Endpoint,
  http: [ip: {127, 0, 0, 1}, port: 4000],
  check_origin: false,
  code_reloader: true,
  debug_errors: true,
  secret_key_base: "uVK6wA5PGJo4gw++Q3LK/EOBOgBG0SNvmM3MHoJcsWYap2/jO64kb7726SF7pR1Z",
  watchers: []

config :cache_next, dev_routes: true

config :phoenix, :stacktrace_depth, 20

config :phoenix, :plug_init_mode, :runtime
