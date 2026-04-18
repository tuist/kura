defmodule CacheNextWeb.CleanController do
  use CacheNextWeb, :controller

  import CacheNextWeb.ControllerHelpers
  import Plug.Conn

  alias CacheNext.Store

  def clean(conn, params) do
    with {:ok, %{"account_handle" => account_handle, "project_handle" => project_handle}} <-
           required_query(params, ["account_handle", "project_handle"]) do
      case Store.delete_project(account_handle, project_handle) do
        :ok ->
          send_resp(conn, :no_content, "")

        {:error, reason} ->
          error(conn, :internal_server_error, "Failed to clean cache: #{inspect(reason)}")
      end
    else
      {:error, missing} ->
        error(conn, :bad_request, "Missing #{missing}")
    end
  end
end
