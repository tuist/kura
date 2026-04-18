defmodule CacheNext.Registry do
  @moduledoc false

  def get_package(scope, name) do
    case CacheNext.Store.fetch_registry_metadata(scope, name) do
      {:ok, %{body: body}} -> Jason.decode(body)
      {:error, :not_found} -> {:error, :not_found}
      {:error, reason} -> {:error, reason}
    end
  end

  def get_archive(scope, name, version) do
    CacheNext.Store.fetch_registry_archive(scope, name, version)
  end

  def get_manifest(scope, name, version, filename) do
    CacheNext.Store.fetch_registry_manifest(scope, name, version, filename)
  end

  def manifest_candidates(nil), do: ["Package.swift"]

  def manifest_candidates(swift_version) do
    case String.split(swift_version, ".", parts: 3) do
      [major, minor | _] ->
        ["Package@swift-#{major}.#{minor}.swift", "Package@swift-#{major}.swift", "Package.swift"]

      [major] ->
        ["Package@swift-#{major}.swift", "Package.swift"]

      _ ->
        ["Package.swift"]
    end
  end
end
