defmodule CacheNextWeb.KeyValueController do
  use CacheNextWeb, :controller

  import CacheNextWeb.ControllerHelpers
  import Plug.Conn

  alias CacheNext.Store

  def get_value(conn, %{"cas_id" => cas_id} = params) do
    with {:ok, %{"account_handle" => account_handle, "project_handle" => project_handle}} <-
           required_query(params, ["account_handle", "project_handle"]) do
      case Store.fetch(:keyvalue, account_handle, project_handle, cas_id) do
        {:ok, artifact} ->
          send_artifact(conn, 200, artifact, "application/json")

        {:error, :not_found} ->
          error(conn, :not_found, "No entries found for CAS ID #{cas_id}.")

        {:error, reason} ->
          error(conn, :service_unavailable, "Failed to fetch key-value entry: #{inspect(reason)}")
      end
    else
      {:error, missing} ->
        error(conn, :bad_request, "Missing #{missing}")
    end
  end

  def put_value(conn, params) do
    with {:ok, %{"account_handle" => account_handle, "project_handle" => project_handle}} <-
           required_query(params, ["account_handle", "project_handle"]),
         %{"cas_id" => cas_id, "entries" => entries} <- conn.body_params,
         true <- is_list(entries) do
      values =
        entries
        |> Enum.map(&(Map.get(&1, "value") || Map.get(&1, :value)))
        |> Enum.filter(&is_binary/1)

      payload =
        Jason.encode!(%{"cas_id" => cas_id, "entries" => Enum.map(values, &%{"value" => &1})})

      case Store.put(
             :keyvalue,
             account_handle,
             project_handle,
             cas_id,
             payload,
             "application/json"
           ) do
        :ok ->
          send_resp(conn, :no_content, "")

        {:error, reason} ->
          error(
            conn,
            :service_unavailable,
            "Failed to persist key-value entry: #{inspect(reason)}"
          )
      end
    else
      {:error, missing} ->
        error(conn, :bad_request, "Missing #{missing}")

      _ ->
        error(conn, :bad_request, "Invalid key-value payload")
    end
  end
end
