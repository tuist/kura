defmodule CacheNextWeb.GradleController do
  use CacheNextWeb, :controller

  import CacheNextWeb.ControllerHelpers
  import Plug.Conn

  alias CacheNext.BodyReader
  alias CacheNext.Store

  require Logger

  def download(conn, %{"cache_key" => cache_key} = params) do
    with {:ok, %{"account_handle" => account_handle, "project_handle" => project_handle}} <-
           required_query(params, ["account_handle", "project_handle"]) do
      Logger.metadata(artifact_kind: "gradle", artifact_key: cache_key)

      case Store.fetch(:gradle, account_handle, project_handle, cache_key) do
        {:ok, artifact} ->
          send_artifact(conn, 200, artifact, "application/octet-stream")

        {:error, :not_found} ->
          send_resp(conn, :not_found, "")

        {:error, reason} ->
          error(conn, :service_unavailable, "Failed to fetch artifact: #{inspect(reason)}")
      end
    else
      {:error, missing} ->
        error(conn, :bad_request, "Missing #{missing}")
    end
  end

  def save(conn, %{"cache_key" => cache_key} = params) do
    with {:ok, %{"account_handle" => account_handle, "project_handle" => project_handle}} <-
           required_query(params, ["account_handle", "project_handle"]) do
      Logger.metadata(artifact_kind: "gradle", artifact_key: cache_key)

      if Store.exists?(:gradle, account_handle, project_handle, cache_key) do
        {_, conn_after} =
          BodyReader.drain(conn, max_bytes: CacheNext.Config.gradle_max_upload_bytes())

        send_resp(conn_after, :ok, "")
      else
        persist_upload(conn, account_handle, project_handle, cache_key)
      end
    else
      {:error, missing} ->
        error(conn, :bad_request, "Missing #{missing}")
    end
  end

  defp persist_upload(conn, account_handle, project_handle, cache_key) do
    case BodyReader.read(conn, max_bytes: CacheNext.Config.gradle_max_upload_bytes()) do
      {:ok, data, conn_after} ->
        case Store.put(
               :gradle,
               account_handle,
               project_handle,
               cache_key,
               data,
               "application/octet-stream"
             ) do
          :ok ->
            send_resp(conn_after, :created, "")

          {:error, reason} ->
            error(
              conn_after,
              :service_unavailable,
              "Failed to persist artifact: #{inspect(reason)}"
            )
        end

      {:error, :too_large, conn_after} ->
        error(conn_after, :request_entity_too_large, "Request body exceeded allowed size")

      {:error, :timeout, conn_after} ->
        error(conn_after, :request_timeout, "Request body read timed out")

      {:error, _reason, conn_after} ->
        error(conn_after, :internal_server_error, "Failed to persist artifact")
    end
  end
end
