defmodule CacheNextWeb.RegistryController do
  use CacheNextWeb, :controller

  import CacheNextWeb.ControllerHelpers
  import Plug.Conn

  alias CacheNext.Registry

  def availability(conn, _params) do
    conn
    |> put_resp_header("content-version", "1")
    |> send_resp(:ok, "")
  end

  def identifiers(conn, %{"url" => repository_url}) do
    with {:ok, {scope, name}} <- repo_identifier(repository_url),
         {:ok, _metadata} <- Registry.get_package(scope, name) do
      conn
      |> put_resp_header("content-version", "1")
      |> json(%{identifiers: ["#{scope}.#{name}"]})
    else
      {:error, :invalid_repository_url} ->
        conn
        |> put_resp_header("content-version", "1")
        |> error(:bad_request, "Invalid repository URL: #{repository_url}")

      {:error, :not_found} ->
        conn
        |> put_resp_header("content-version", "1")
        |> error(:not_found, "The package #{repository_url} was not found in the registry.")

      {:error, reason} ->
        conn
        |> put_resp_header("content-version", "1")
        |> error(:service_unavailable, "Registry is temporarily unavailable: #{inspect(reason)}")
    end
  end

  def login(conn, _params) do
    conn
    |> put_resp_header("content-version", "1")
    |> json(%{})
  end

  def list_releases(conn, %{"scope" => scope, "name" => name}) do
    case Registry.get_package(scope, name) do
      {:ok, metadata} ->
        releases =
          metadata
          |> Map.get("releases", %{})
          |> Map.new(fn {version, _data} ->
            {version, %{url: "/api/registry/swift/#{scope}/#{name}/#{version}"}}
          end)

        conn
        |> put_resp_header("content-version", "1")
        |> json(%{releases: releases})

      {:error, :not_found} ->
        conn
        |> put_resp_header("content-version", "1")
        |> error(:not_found, "The package #{scope}/#{name} was not found in the registry.")

      {:error, reason} ->
        conn
        |> put_resp_header("content-version", "1")
        |> error(:service_unavailable, "Registry is temporarily unavailable: #{inspect(reason)}")
    end
  end

  def show_release(conn, %{"scope" => scope, "name" => name, "version" => version}) do
    if String.ends_with?(version, ".zip") do
      download_archive(conn, scope, name, String.trim_trailing(version, ".zip"))
    else
      render_release(conn, scope, name, version)
    end
  end

  def show_manifest(conn, %{"scope" => scope, "name" => name, "version" => version}) do
    result =
      Registry.manifest_candidates(conn.query_params["swift-version"])
      |> Enum.reduce_while(:not_found, fn candidate, _acc ->
        case Registry.get_manifest(scope, name, version, candidate) do
          {:ok, %{body: body}} -> {:halt, {:ok, candidate, body}}
          {:error, :not_found} -> {:cont, :not_found}
          {:error, reason} -> {:halt, {:error, reason}}
        end
      end)

    case result do
      {:ok, _candidate, body} ->
        conn
        |> put_resp_header("content-version", "1")
        |> put_resp_content_type("text/x-swift")
        |> send_resp(:ok, body)

      :not_found ->
        if is_binary(conn.query_params["swift-version"]) do
          conn
          |> put_resp_header("content-version", "1")
          |> put_resp_header(
            "location",
            "/api/registry/swift/#{scope}/#{name}/#{version}/Package.swift"
          )
          |> send_resp(303, "")
        else
          conn
          |> put_resp_header("content-version", "1")
          |> put_status(:not_found)
          |> json(%{})
        end

      {:error, reason} ->
        conn
        |> put_resp_header("content-version", "1")
        |> error(:service_unavailable, "Registry is temporarily unavailable: #{inspect(reason)}")
    end
  end

  defp render_release(conn, scope, name, version) do
    case Registry.get_package(scope, name) do
      {:ok, metadata} ->
        releases = Map.get(metadata, "releases", %{})

        case Map.get(releases, version) do
          nil ->
            conn
            |> put_resp_header("content-version", "1")
            |> error(:not_found, "The package #{scope}/#{name} was not found in the registry.")

          release_data ->
            conn
            |> put_resp_header("content-version", "1")
            |> json(%{
              id: "#{scope}.#{name}",
              version: version,
              resources: [
                %{
                  name: "source-archive",
                  type: "application/zip",
                  checksum: release_data["checksum"]
                }
              ]
            })
        end

      {:error, :not_found} ->
        conn
        |> put_resp_header("content-version", "1")
        |> error(:not_found, "The package #{scope}/#{name} was not found in the registry.")

      {:error, reason} ->
        conn
        |> put_resp_header("content-version", "1")
        |> error(:service_unavailable, "Registry is temporarily unavailable: #{inspect(reason)}")
    end
  end

  defp download_archive(conn, scope, name, version) do
    case Registry.get_archive(scope, name, version) do
      {:ok, %{body: body}} ->
        conn
        |> put_resp_header("content-version", "1")
        |> put_resp_content_type("application/zip")
        |> put_resp_header(
          "content-disposition",
          "attachment; filename=\"#{name}-#{version}.zip\""
        )
        |> send_resp(:ok, body)

      {:error, :not_found} ->
        conn
        |> put_resp_header("content-version", "1")
        |> error(:not_found, "The package #{scope}/#{name} was not found in the registry.")

      {:error, reason} ->
        conn
        |> put_resp_header("content-version", "1")
        |> error(:service_unavailable, "Registry is temporarily unavailable: #{inspect(reason)}")
    end
  end

  defp repo_identifier(repository_url) do
    case URI.parse(repository_url) do
      %URI{host: host, path: path}
      when host in ["github.com", "www.github.com"] and is_binary(path) ->
        case path
             |> String.trim_leading("/")
             |> String.trim_trailing(".git")
             |> String.split("/", trim: true) do
          [scope, name | _] -> {:ok, {scope, name}}
          _ -> {:error, :invalid_repository_url}
        end

      _ ->
        {:error, :invalid_repository_url}
    end
  end
end
