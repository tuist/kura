defmodule CacheNextWeb.XcodeController do
  use CacheNextWeb, :controller

  import CacheNextWeb.ControllerHelpers
  import Plug.Conn

  alias CacheNext.BodyReader
  alias CacheNext.Store

  require Logger

  def download(conn, %{"id" => id} = params) do
    with {:ok, %{"account_handle" => account_handle, "project_handle" => project_handle}} <-
           required_query(params, ["account_handle", "project_handle"]) do
      Logger.metadata(artifact_kind: "xcode", artifact_key: id)

      case Store.fetch(:xcode, account_handle, project_handle, id) do
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

  def save(conn, %{"id" => id} = params) do
    with {:ok, %{"account_handle" => account_handle, "project_handle" => project_handle}} <-
           required_query(params, ["account_handle", "project_handle"]) do
      Logger.metadata(artifact_kind: "xcode", artifact_key: id)

      if Store.exists?(:xcode, account_handle, project_handle, id) do
        {_, conn_after} =
          BodyReader.drain(conn, max_bytes: CacheNext.Config.xcode_max_upload_bytes())

        send_resp(conn_after, :no_content, "")
      else
        persist_upload(
          conn,
          :xcode,
          account_handle,
          project_handle,
          id,
          CacheNext.Config.xcode_max_upload_bytes(),
          :no_content
        )
      end
    else
      {:error, missing} ->
        error(conn, :bad_request, "Missing #{missing}")
    end
  end

  defp persist_upload(conn, kind, account_handle, project_handle, key, max_bytes, success_status) do
    case BodyReader.read(conn, max_bytes: max_bytes) do
      {:ok, data, conn_after} ->
        case Store.put(
               kind,
               account_handle,
               project_handle,
               key,
               data,
               "application/octet-stream"
             ) do
          :ok ->
            send_resp(conn_after, success_status, "")

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
