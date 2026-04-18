defmodule CacheNext.Store do
  @moduledoc false

  def backend, do: CacheNext.Config.store_impl()

  def exists?(kind, account_handle, project_handle, key) do
    backend().exists?(kind, account_handle, project_handle, key)
  end

  def fetch(kind, account_handle, project_handle, key) do
    backend().fetch(kind, account_handle, project_handle, key)
  end

  def put(kind, account_handle, project_handle, key, data, content_type) do
    backend().put(kind, account_handle, project_handle, key, data, content_type)
  end

  def delete_project(account_handle, project_handle) do
    backend().delete_project(account_handle, project_handle)
  end

  def ring_members do
    backend().ring_members()
  end

  def stream_artifact(conn, status, artifact, default_content_type) do
    backend().stream_artifact(conn, status, artifact, default_content_type)
  end

  def fetch_registry_metadata(scope, name) do
    fetch(:keyvalue, "_registry", "#{scope}--#{name}", "metadata.json")
  end

  def fetch_registry_archive(scope, name, version) do
    fetch(:xcode, "_registry", "#{scope}--#{name}", "#{version}/source_archive.zip")
  end

  def fetch_registry_manifest(scope, name, version, filename) do
    fetch(:module, "_registry", "#{scope}--#{name}", "#{version}/#{filename}")
  end
end
