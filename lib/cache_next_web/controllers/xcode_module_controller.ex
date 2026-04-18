defmodule CacheNextWeb.XcodeModuleController do
  use CacheNextWeb, :controller

  import CacheNextWeb.ControllerHelpers
  import Plug.Conn

  alias CacheNext.BodyReader
  alias CacheNext.MultipartUploads
  alias CacheNext.Store

  require Logger

  def exists(conn, params) do
    with {:ok, query} <-
           required_query(params, ["account_handle", "project_handle", "hash", "name"]) do
      key = module_key(params, query)
      Logger.metadata(artifact_kind: "module", artifact_key: key)

      if Store.exists?(:module, query["account_handle"], query["project_handle"], key) do
        send_resp(conn, :no_content, "")
      else
        send_resp(conn, :not_found, "")
      end
    else
      {:error, missing} ->
        error(conn, :bad_request, "Missing #{missing}")
    end
  end

  def download(conn, params) do
    with {:ok, query} <-
           required_query(params, ["account_handle", "project_handle", "hash", "name"]) do
      key = module_key(params, query)
      Logger.metadata(artifact_kind: "module", artifact_key: key)

      case Store.fetch(:module, query["account_handle"], query["project_handle"], key) do
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

  def start_multipart(conn, params) do
    with {:ok, query} <-
           required_query(params, ["account_handle", "project_handle", "hash", "name"]) do
      key = module_key(params, query)

      if Store.exists?(:module, query["account_handle"], query["project_handle"], key) do
        json(conn, %{upload_id: nil})
      else
        category = Map.get(params, "cache_category", "builds")

        case MultipartUploads.start_upload(
               query["account_handle"],
               query["project_handle"],
               category,
               query["hash"],
               query["name"]
             ) do
          {:ok, upload_id} ->
            json(conn, %{upload_id: upload_id})

          {:error, reason} ->
            error(conn, :internal_server_error, "Failed to start upload: #{inspect(reason)}")
        end
      end
    else
      {:error, missing} ->
        error(conn, :bad_request, "Missing #{missing}")
    end
  end

  def upload_part(conn, params) do
    with {:ok, %{"upload_id" => upload_id}} <- required_query(params, ["upload_id"]),
         {:ok, part_number} <- parse_integer(Map.get(params, "part_number")) do
      case BodyReader.read(conn,
             max_bytes: CacheNext.Config.module_part_max_upload_bytes(),
             tmp_dir: Path.join(CacheNext.Config.tmp_dir(), "parts")
           ) do
        {:ok, {:file, tmp_path}, conn_after} ->
          persist_part(conn_after, upload_id, part_number, tmp_path)

        {:ok, data, conn_after} ->
          tmp_dir = Path.join(CacheNext.Config.tmp_dir(), "parts")
          File.mkdir_p!(tmp_dir)
          tmp_path = Path.join(tmp_dir, "part-#{System.unique_integer([:positive])}")
          File.write!(tmp_path, data)
          persist_part(conn_after, upload_id, part_number, tmp_path)

        {:error, :too_large, conn_after} ->
          error(conn_after, :request_entity_too_large, "Part exceeds 10MB limit")

        {:error, :timeout, conn_after} ->
          error(conn_after, :request_timeout, "Request body read timed out")

        {:error, _reason, conn_after} ->
          error(conn_after, :internal_server_error, "Failed to persist multipart upload part")
      end
    else
      {:error, missing} ->
        error(conn, :bad_request, "Missing #{missing}")

      :error ->
        error(conn, :bad_request, "Invalid part_number")
    end
  end

  def complete_multipart(conn, params) do
    with {:ok, %{"upload_id" => upload_id}} <- required_query(params, ["upload_id"]) do
      case MultipartUploads.complete_upload(upload_id) do
        {:ok, upload} ->
          with {:ok, parts} <-
                 normalize_parts(conn.body_params["parts"] || conn.body_params[:parts]),
               {:ok, validated_parts} <- validate_parts(upload.parts, parts),
               :ok <-
               Store.put(
                   :module,
                   Map.get(upload, :account_handle, ""),
                   upload.project_handle,
                   module_key(upload.category, upload.hash, upload.name),
                   {:multipart_upload, upload, validated_parts},
                   "application/octet-stream"
                 ),
               :ok <- MultipartUploads.abort_upload(upload_id) do
            send_resp(conn, :no_content, "")
          else
            {:error, :parts_mismatch} ->
              error(conn, :bad_request, "Parts mismatch or missing parts")

            {:error, reason} ->
              error(
                conn,
                :internal_server_error,
                "Failed to complete multipart upload: #{inspect(reason)}"
              )
          end

        {:error, :not_found} ->
          error(conn, :not_found, "Upload not found")
      end
    else
      {:error, missing} ->
        error(conn, :bad_request, "Missing #{missing}")
    end
  end

  defp persist_part(conn, upload_id, part_number, tmp_path) do
    size =
      case File.stat(tmp_path) do
        {:ok, %File.Stat{size: value}} -> value
        _ -> 0
      end

    case MultipartUploads.add_part(upload_id, part_number, tmp_path, size) do
      :ok ->
        :telemetry.execute(
          [:cache_next, :multipart, :part],
          %{count: 1, size: size},
          %{result: "ok", region: CacheNext.Config.region()}
        )

        send_resp(conn, :no_content, "")

      {:error, :upload_not_found} ->
        File.rm(tmp_path)
        error(conn, :not_found, "Upload not found")

      {:error, :total_size_exceeded} ->
        File.rm(tmp_path)
        error(conn, :unprocessable_entity, "Total upload size exceeds 2GB limit")

      {:error, reason} ->
        File.rm(tmp_path)

        error(
          conn,
          :internal_server_error,
          "Failed to store multipart upload part: #{inspect(reason)}"
        )
    end
  end

  defp validate_parts(uploaded_parts, client_parts) do
    uploaded_numbers = uploaded_parts |> Map.keys() |> Enum.sort()
    client_numbers = client_parts |> Enum.sort()

    if uploaded_numbers == client_numbers and uploaded_numbers != [] do
      {:ok, client_numbers}
    else
      {:error, :parts_mismatch}
    end
  end

  defp normalize_parts(parts) when is_list(parts) do
    Enum.reduce_while(parts, {:ok, []}, fn value, {:ok, acc} ->
      case parse_integer(value) do
        {:ok, integer} -> {:cont, {:ok, [integer | acc]}}
        :error -> {:halt, {:error, :parts_mismatch}}
      end
    end)
    |> case do
      {:ok, values} -> {:ok, Enum.reverse(values)}
      error -> error
    end
  end

  defp normalize_parts(_parts), do: {:error, :parts_mismatch}

  defp module_key(params, query) do
    module_key(
      Map.get(params, "cache_category", "builds"),
      query["hash"],
      query["name"]
    )
  end

  defp module_key(category, hash, name), do: Enum.join([category, hash, name], "/")
end
