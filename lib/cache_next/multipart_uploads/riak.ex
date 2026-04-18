defmodule CacheNext.MultipartUploads.Riak do
  @moduledoc false

  @uploads_bucket "cache-next-uploads"
  @upload_chunks_bucket "cache-next-upload-chunks"

  def start_upload(_account_handle, project_handle, category, hash, name) do
    upload_id = upload_id(project_handle, category, hash, name)

    upload = %{
      "project_handle" => project_handle,
      "category" => category,
      "hash" => hash,
      "name" => name,
      "parts" => %{},
      "total_bytes" => 0,
      "updated_at_ms" => System.system_time(:millisecond)
    }

    case put_upload(upload_id, upload) do
      :ok -> {:ok, upload_id}
      {:error, reason} -> {:error, reason}
    end
  end

  def add_part(upload_id, part_number, tmp_path, size_bytes) do
    with {:ok, upload, vclock} <- fetch_upload(upload_id),
         {:ok, new_chunk_keys} <- store_part_chunks(upload_id, part_number, tmp_path),
         :ok <-
           update_upload_part(upload_id, upload, vclock, part_number, size_bytes, new_chunk_keys) do
      File.rm(tmp_path)
      :ok
    else
      {:error, :upload_not_found} ->
        File.rm(tmp_path)
        {:error, :upload_not_found}

      {:error, :total_size_exceeded} ->
        File.rm(tmp_path)
        {:error, :total_size_exceeded}

      {:error, reason} ->
        File.rm(tmp_path)
        {:error, reason}
    end
  end

  def complete_upload(upload_id) do
    case fetch_upload(upload_id) do
      {:ok, upload, _vclock} -> {:ok, normalize_upload(upload)}
      {:error, :not_found} -> {:error, :not_found}
      {:error, reason} -> {:error, reason}
    end
  end

  def abort_upload(upload_id) do
    case fetch_upload(upload_id) do
      {:ok, upload, _vclock} ->
        upload
        |> Map.get("parts", %{})
        |> Enum.each(fn {_part_number, part} ->
          Enum.each(Map.get(part, "chunk_keys", []), &delete_chunk/1)
        end)

        delete_upload(upload_id)

      {:error, :not_found} ->
        :ok

      {:error, _reason} ->
        :ok
    end

    :ok
  end

  def tmp_storage_size, do: 0

  defp update_upload_part(upload_id, upload, vclock, part_number, size_bytes, new_chunk_keys) do
    existing_part = upload["parts"][Integer.to_string(part_number)]
    existing_size = if existing_part, do: existing_part["size"], else: 0
    total_bytes = upload["total_bytes"] - existing_size + size_bytes

    if total_bytes > CacheNext.Config.module_total_max_upload_bytes() do
      rollback_chunk_keys(new_chunk_keys)
      {:error, :total_size_exceeded}
    else
      updated_upload =
        upload
        |> put_in(["parts", Integer.to_string(part_number)], %{
          "size" => size_bytes,
          "chunk_keys" => new_chunk_keys
        })
        |> Map.put("total_bytes", total_bytes)
        |> Map.put("updated_at_ms", System.system_time(:millisecond))

      case put_upload(upload_id, updated_upload, vclock) do
        :ok ->
          existing_part
          |> case do
            nil -> :ok
            part -> Enum.each(part["chunk_keys"], &delete_chunk/1)
          end

          :ok

        {:error, reason} ->
          rollback_chunk_keys(new_chunk_keys)
          {:error, reason}
      end
    end
  end

  defp fetch_upload(upload_id) do
    case CacheNext.Riak.get_object(@uploads_bucket, upload_id) do
      {:ok, %{status: 200, headers: headers, body: body}} ->
        case Jason.decode(body) do
          {:ok, upload} -> {:ok, upload, headers["x-riak-vclock"]}
          {:error, reason} -> {:error, {:invalid_json, reason}}
        end

      {:ok, %{status: 404}} ->
        {:error, :not_found}

      {:ok, %{status: status}} ->
        {:error, {:unexpected_status, status}}

      {:error, reason} ->
        {:error, reason}
    end
  end

  defp put_upload(upload_id, upload, vclock \\ nil) do
    case CacheNext.Riak.put_object(
           @uploads_bucket,
           upload_id,
           Jason.encode!(upload),
           "application/json",
           vclock: vclock
         ) do
      {:ok, %{status: status}} when status in 200..299 -> :ok
      {:ok, %{status: status}} -> {:error, {:unexpected_status, status}}
      {:error, reason} -> {:error, reason}
    end
  end

  defp delete_upload(upload_id) do
    case CacheNext.Riak.delete_object(@uploads_bucket, upload_id) do
      {:ok, %{status: status}} when status in [204, 404] -> :ok
      {:ok, %{status: _status}} -> :ok
      {:error, _reason} -> :ok
    end
  end

  defp store_part_chunks(upload_id, part_number, tmp_path) do
    File.open(tmp_path, [:read, :binary], fn device ->
      do_store_part_chunks(device, upload_id, part_number, 0, [])
    end)
    |> case do
      {:ok, {:ok, chunk_keys}} -> {:ok, chunk_keys}
      {:ok, {:error, reason}} -> {:error, reason}
      {:error, reason} -> {:error, reason}
    end
  end

  defp do_store_part_chunks(device, upload_id, part_number, index, acc) do
    case IO.binread(device, CacheNext.Config.riak_chunk_size_bytes()) do
      :eof ->
        {:ok, Enum.reverse(acc)}

      {:error, reason} ->
        rollback_chunk_keys(acc)
        {:error, reason}

      chunk ->
        chunk_key = "#{upload_id}:#{part_number}:#{index}"

        case CacheNext.Riak.put_object(
               @upload_chunks_bucket,
               chunk_key,
               chunk,
               "application/octet-stream"
             ) do
          {:ok, %{status: status}} when status in 200..299 ->
            do_store_part_chunks(device, upload_id, part_number, index + 1, [chunk_key | acc])

          {:ok, %{status: status}} ->
            rollback_chunk_keys(acc)
            {:error, {:unexpected_status, status}}

          {:error, reason} ->
            rollback_chunk_keys(acc)
            {:error, reason}
        end
    end
  end

  defp rollback_chunk_keys(chunk_keys) do
    Enum.each(chunk_keys, &delete_chunk/1)
  end

  defp delete_chunk(chunk_key) do
    _ = CacheNext.Riak.delete_object(@upload_chunks_bucket, chunk_key)
    :ok
  end

  defp normalize_upload(upload) do
    %{
      project_handle: upload["project_handle"],
      category: upload["category"],
      hash: upload["hash"],
      name: upload["name"],
      total_bytes: upload["total_bytes"],
      parts:
        upload["parts"]
        |> Enum.into(%{}, fn {part_number, part} ->
          {String.to_integer(part_number),
           %{
             size: part["size"],
             chunk_keys: part["chunk_keys"]
           }}
        end)
    }
  end

  defp upload_id(project_handle, category, hash, name) do
    CacheNext.Config.hash(
      "#{CacheNext.Config.tenant()}:#{project_handle}:#{category}:#{hash}:#{name}:#{System.unique_integer()}",
      32
    )
  end
end
