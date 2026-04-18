defmodule CacheNextWeb.Router do
  use CacheNextWeb, :router

  pipeline :api do
    plug CacheNextWeb.Plugs.ObservabilityContextPlug
  end

  scope "/", CacheNextWeb do
    get "/up", UpController, :index
    get "/metrics", MetricsController, :index
  end

  scope "/api/cache", CacheNextWeb do
    pipe_through :api

    get "/keyvalue/:cas_id", KeyValueController, :get_value
    put "/keyvalue", KeyValueController, :put_value

    get "/cas/:id", XcodeController, :download
    post "/cas/:id", XcodeController, :save

    head "/module/:id", XcodeModuleController, :exists
    get "/module/:id", XcodeModuleController, :download
    post "/module/start", XcodeModuleController, :start_multipart
    post "/module/part", XcodeModuleController, :upload_part
    post "/module/complete", XcodeModuleController, :complete_multipart

    delete "/clean", CleanController, :clean

    get "/gradle/:cache_key", GradleController, :download
    put "/gradle/:cache_key", GradleController, :save
  end

  scope "/api/registry/swift", CacheNextWeb do
    pipe_through :api

    get "/", RegistryController, :availability
    get "/availability", RegistryController, :availability
    get "/identifiers", RegistryController, :identifiers
    post "/login", RegistryController, :login
    get "/:scope/:name", RegistryController, :list_releases
    get "/:scope/:name/:version", RegistryController, :show_release
    get "/:scope/:name/:version/Package.swift", RegistryController, :show_manifest
  end
end
