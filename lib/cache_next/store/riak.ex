defmodule CacheNext.Store.Riak do
  @moduledoc false

  import Plug.Conn

  @memory_threshold 512_000
  @manifests_bucket "cache-next-manifests"
  @chunks_bucket "cache-next-chunks"
  @project_handle_index "project_handle_bin"

  def exists?(kind, _account_handle, project_handle, key) do
    artifact_key(kind, project_handle, key)
    |> fetch_manifest_head()
    |> case do
      {:ok, _headers} -> true
      _ -> false
    end
  end

  def fetch(kind, _account_handle, project_handle, key) do
    started_at = System.monotonic_time()

    result =
      kind
      |> artifact_key(project_handle, key)
      |> fetch_manifest()
      |> case do
        {:ok, manifest} -> materialize_fetch(manifest)
        {:error, reason} -> {:error, reason}
      end

    emit_read_telemetry(kind, result, started_at)
    result
  end

  def put(kind, _account_handle, project_handle, key, {:multipart_upload, upload, parts}, content_type) do
    started_at = System.monotonic_time()

    result =
      case put_from_upload(kind, project_handle, key, upload, parts, content_type) do
        {:ok, size} -> {:ok, size}
        {:error, reason} -> {:error, reason}
      end

    emit_write_telemetry(kind, result, started_at)

    case result do
      {:ok, _size} -> :ok
      {:error, reason} -> {:error, reason}
    end
  end

  def put(kind, _account_handle, project_handle, key, data, content_type) do
    started_at = System.monotonic_time()

    result =
      case normalize_source(data) do
        {:ok, %{path: path, size: size}, cleanup} ->
          artifact_id = artifact_key(kind, project_handle, key)

          metadata = %{
            "kind" => Atom.to_string(kind),
            "project_handle" => project_handle,
            "content_type" => content_type,
            "size" => size,
            "stored_at_ms" => System.system_time(:millisecond)
          }

          response =
            with {:ok, chunk_keys} <- store_file_chunks(artifact_id, path),
                 :ok <-
                   put_manifest(
                     artifact_id,
                     Map.put(metadata, "chunk_keys", chunk_keys),
                     project_handle
                   ) do
              {:ok, size}
            else
              {:error, _reason} = error ->
                _ = delete_artifact(artifact_id)
                error

              other ->
                _ = delete_artifact(artifact_id)
                {:error, other}
            end

          cleanup.()
          response

        {:error, reason, cleanup} ->
          cleanup.()
          {:error, reason}
      end

    emit_write_telemetry(kind, result, started_at)

    case result do
      {:ok, _size} -> :ok
      {:error, reason} -> {:error, reason}
    end
  end

  def delete_project(_account_handle, project_handle) do
    case fetch_project_artifact_ids(project_handle) do
      {:ok, artifact_ids} ->
        with :ok <-
               Enum.reduce_while(artifact_ids, :ok, fn artifact_id, :ok ->
                 case delete_artifact(artifact_id) do
                   :ok -> {:cont, :ok}
                   {:error, reason} -> {:halt, {:error, reason}}
                 end
               end) do
          :ok
        end

      {:error, reason} ->
        {:error, reason}
    end
  end

  def ring_members do
    CacheNext.Riak.members()
  end

  def stream_artifact(
        conn,
        status,
        %{chunk_keys: chunk_keys, size: size} = artifact,
        default_content_type
      ) do
    content_type = Map.get(artifact, :content_type, default_content_type)

    conn =
      conn
      |> put_resp_content_type(content_type)
      |> put_resp_header("content-length", Integer.to_string(size))
      |> send_chunked(status)

    Enum.reduce_while(chunk_keys, conn, fn chunk_key, conn_acc ->
      case timed_request(:get_chunk, fn ->
             CacheNext.Riak.get_object(@chunks_bucket, chunk_key, fallback_on_not_found: true)
           end) do
        {:ok, %{status: 200, body: body}} ->
          case chunk(conn_acc, body) do
            {:ok, conn_after} -> {:cont, conn_after}
            {:error, _reason} -> {:halt, conn_acc}
          end

        _ ->
          {:halt, conn_acc}
      end
    end)
  end

  def stream_artifact(conn, status, %{body: body} = artifact, default_content_type) do
    conn
    |> put_resp_content_type(Map.get(artifact, :content_type, default_content_type))
    |> send_resp(status, body)
  end

  defp put_from_upload(kind, project_handle, key, upload, parts, content_type) do
    artifact_id = artifact_key(kind, project_handle, key)

    with {:ok, %{chunk_keys: chunk_keys, size: size}} <-
           copy_upload_parts(artifact_id, upload, parts),
         :ok <-
           put_manifest(
             artifact_id,
             %{
               "kind" => Atom.to_string(kind),
               "project_handle" => project_handle,
               "content_type" => content_type,
               "size" => size,
               "stored_at_ms" => System.system_time(:millisecond),
               "chunk_keys" => chunk_keys
             },
             project_handle
           ) do
      {:ok, size}
    else
      {:error, _reason} = error ->
        _ = delete_artifact(artifact_id)
        error
    end
  end

  defp copy_upload_parts(artifact_id, upload, parts) do
    Enum.reduce_while(parts, {:ok, %{chunk_keys: [], size: 0, next_index: 0}}, fn part_number,
                                                                                  {:ok, acc} ->
      case Map.fetch(upload.parts, part_number) do
        :error ->
          {:halt, {:error, :parts_mismatch}}

        {:ok, %{chunk_keys: upload_chunk_keys, size: size}} ->
          case copy_chunk_keys(upload_chunk_keys, artifact_id, acc.next_index) do
            {:ok, %{chunk_keys: chunk_keys, next_index: next_index}} ->
              {:cont,
               {:ok,
                %{
                  chunk_keys: acc.chunk_keys ++ chunk_keys,
                  size: acc.size + size,
                  next_index: next_index
                }}}

            {:error, reason} ->
              rollback_chunks(acc.chunk_keys)
              {:halt, {:error, reason}}
          end
      end
    end)
    |> case do
      {:ok, %{chunk_keys: chunk_keys, size: size}} -> {:ok, %{chunk_keys: chunk_keys, size: size}}
      other -> other
    end
  end

  defp copy_chunk_keys(upload_chunk_keys, artifact_id, start_index) do
    Enum.reduce_while(
      upload_chunk_keys,
      {:ok, %{chunk_keys: [], next_index: start_index}},
      fn upload_chunk_key, {:ok, acc} ->
        chunk_key = chunk_key(artifact_id, acc.next_index)

        case timed_request(:copy_chunk, fn ->
               CacheNext.Riak.get_object("cache-next-upload-chunks", upload_chunk_key)
             end) do
          {:ok, %{status: 200, body: body}} ->
            case timed_request(:put_chunk, fn ->
                   CacheNext.Riak.put_object(
                     @chunks_bucket,
                     chunk_key,
                     body,
                     "application/octet-stream"
                   )
                 end) do
              {:ok, %{status: status}} when status in 200..299 ->
                {:cont,
                 {:ok,
                  %{chunk_keys: acc.chunk_keys ++ [chunk_key], next_index: acc.next_index + 1}}}

              {:ok, %{status: status}} ->
                rollback_chunks(acc.chunk_keys)
                {:halt, {:error, {:unexpected_status, status}}}

              {:error, reason} ->
                rollback_chunks(acc.chunk_keys)
                {:halt, {:error, reason}}
            end

          {:ok, %{status: 404}} ->
            rollback_chunks(acc.chunk_keys)
            {:halt, {:error, :not_found}}

          {:ok, %{status: status}} ->
            rollback_chunks(acc.chunk_keys)
            {:halt, {:error, {:unexpected_status, status}}}

          {:error, reason} ->
            rollback_chunks(acc.chunk_keys)
            {:halt, {:error, reason}}
        end
      end
    )
  end

  defp fetch_manifest_head(artifact_id) do
    case timed_request(:head_manifest, fn ->
           CacheNext.Riak.head_object(
             @manifests_bucket,
             artifact_id,
             fallback_on_not_found: true
           )
         end) do
      {:ok, %{status: 200, headers: headers}} -> {:ok, headers}
      {:ok, %{status: 404}} -> {:error, :not_found}
      {:ok, %{status: status}} -> {:error, {:unexpected_status, status}}
      {:error, reason} -> {:error, reason}
    end
  end

  defp fetch_manifest(artifact_id) do
    case fetch_json(@manifests_bucket, artifact_id, fallback_on_not_found: true) do
      {:ok, manifest} -> {:ok, manifest}
      {:error, reason} -> {:error, reason}
    end
  end

  defp materialize_fetch(%{"chunk_keys" => chunk_keys} = manifest) do
    content_type = manifest["content_type"] || "application/octet-stream"
    size = manifest["size"] || 0

    if size <= @memory_threshold do
      fetch_body(chunk_keys, content_type)
    else
      {:ok,
       %{
         chunk_keys: chunk_keys,
         size: size,
         content_type: content_type
       }}
    end
  end

  defp fetch_body(chunk_keys, content_type) do
    Enum.reduce_while(chunk_keys, {:ok, []}, fn chunk_key, {:ok, acc} ->
      case timed_request(:get_chunk, fn ->
             CacheNext.Riak.get_object(@chunks_bucket, chunk_key, fallback_on_not_found: true)
           end) do
        {:ok, %{status: 200, body: body}} -> {:cont, {:ok, [body | acc]}}
        {:ok, %{status: 404}} -> {:halt, {:error, :not_found}}
        {:ok, %{status: status}} -> {:halt, {:error, {:unexpected_status, status}}}
        {:error, reason} -> {:halt, {:error, reason}}
      end
    end)
    |> case do
      {:ok, bodies} ->
        {:ok,
         %{
           body: bodies |> Enum.reverse() |> IO.iodata_to_binary(),
           content_type: content_type
         }}

      error ->
        error
    end
  end

  defp put_manifest(artifact_id, manifest, project_handle) do
    put_json(
      @manifests_bucket,
      artifact_id,
      manifest,
      nil,
      headers: [{"x-riak-index-#{@project_handle_index}", project_handle}]
    )
  end

  defp store_file_chunks(artifact_id, path) do
    File.open(path, [:read, :binary], fn device ->
      do_store_file_chunks(device, artifact_id, 0, [])
    end)
    |> case do
      {:ok, {:ok, chunk_keys}} -> {:ok, chunk_keys}
      {:ok, {:error, reason}} -> {:error, reason}
      {:error, reason} -> {:error, reason}
    end
  end

  defp do_store_file_chunks(device, artifact_id, index, acc) do
    case IO.binread(device, CacheNext.Config.riak_chunk_size_bytes()) do
      :eof ->
        {:ok, Enum.reverse(acc)}

      {:error, reason} ->
        rollback_chunks(acc)
        {:error, reason}

      chunk ->
        chunk_key = chunk_key(artifact_id, index)

        case timed_request(:put_chunk, fn ->
               CacheNext.Riak.put_object(
                 @chunks_bucket,
                 chunk_key,
                 chunk,
                 "application/octet-stream"
               )
             end) do
          {:ok, %{status: status}} when status in 200..299 ->
            do_store_file_chunks(device, artifact_id, index + 1, [chunk_key | acc])

          {:ok, %{status: status}} ->
            rollback_chunks(acc)
            {:error, {:unexpected_status, status}}

          {:error, reason} ->
            rollback_chunks(acc)
            {:error, reason}
        end
    end
  end

  defp delete_artifact(artifact_id) do
    case fetch_manifest(artifact_id) do
      {:ok, %{"chunk_keys" => chunk_keys}} ->
        rollback_chunks(chunk_keys)

        case delete_json(@manifests_bucket, artifact_id) do
          :ok -> :ok
          {:error, :not_found} -> :ok
          {:error, reason} -> {:error, reason}
        end

      {:error, :not_found} ->
        :ok

      {:error, reason} ->
        {:error, reason}
    end
  end

  defp rollback_chunks(chunk_keys) do
    Enum.each(chunk_keys, fn key ->
      _ = delete_json(@chunks_bucket, key)
    end)
  end

  defp fetch_json(bucket, key, opts) do
    case fetch_json_with_vclock(bucket, key, opts) do
      {:ok, value, _vclock} -> {:ok, value}
      {:error, reason} -> {:error, reason}
    end
  end

  defp fetch_json_with_vclock(bucket, key, opts) do
    case timed_request(:get_json, fn -> CacheNext.Riak.get_object(bucket, key, opts) end) do
      {:ok, %{status: 200, headers: headers, body: body}} ->
        with {:ok, decoded} <- Jason.decode(body) do
          {:ok, decoded, Map.get(headers, "x-riak-vclock")}
        else
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

  defp put_json(bucket, key, value, vclock, opts) do
    body = Jason.encode!(value)

    case timed_request(:put_json, fn ->
           opts =
             opts
             |> Keyword.update(:headers, [], fn headers -> headers end)
             |> Keyword.put(:vclock, vclock)

           CacheNext.Riak.put_object(bucket, key, body, "application/json", opts)
         end) do
      {:ok, %{status: status}} when status in 200..299 -> :ok
      {:ok, %{status: status}} -> {:error, {:unexpected_status, status}}
      {:error, reason} -> {:error, reason}
    end
  end

  defp fetch_project_artifact_ids(project_handle) do
    do_fetch_project_artifact_ids(project_handle, nil, [])
  end

  defp do_fetch_project_artifact_ids(project_handle, continuation, acc) do
    opts = [continuation: continuation, max_results: 500]

    case timed_request(:query_project_index, fn ->
           CacheNext.Riak.query_index(
             @manifests_bucket,
             @project_handle_index,
             project_handle,
             opts
           )
         end) do
      {:ok, %{status: 200, keys: keys, continuation: next_continuation}} ->
        merged = acc ++ keys

        if is_binary(next_continuation) and next_continuation != "" do
          do_fetch_project_artifact_ids(project_handle, next_continuation, merged)
        else
          {:ok, Enum.uniq(merged)}
        end

      {:ok, %{status: 404}} ->
        {:ok, Enum.uniq(acc)}

      {:ok, %{status: status}} ->
        {:error, {:unexpected_status, status}}

      {:error, reason} ->
        {:error, reason}
    end
  end

  defp delete_json(bucket, key) do
    case timed_request(:delete_object, fn -> CacheNext.Riak.delete_object(bucket, key) end) do
      {:ok, %{status: status}} when status in [204, 404] -> :ok
      {:ok, %{status: status}} -> {:error, {:unexpected_status, status}}
      {:error, reason} -> {:error, reason}
    end
  end

  defp timed_request(operation, fun) do
    started_at = System.monotonic_time()
    result = fun.()

    :telemetry.execute(
      [:cache_next, :remote, :request],
      %{count: 1, duration: System.monotonic_time() - started_at},
      %{
        operation: Atom.to_string(operation),
        result: result_label(result),
        region: CacheNext.Config.region()
      }
    )

    result
  end

  defp artifact_key(kind, project_handle, key) do
    CacheNext.Config.hash("#{CacheNext.Config.tenant()}|#{project_handle}|#{kind}|#{key}")
  end

  defp chunk_key(artifact_id, index) do
    "#{artifact_id}:#{index}"
  end

  defp normalize_source({:file, path}) do
    case File.stat(path) do
      {:ok, %File.Stat{size: size}} -> {:ok, %{path: path, size: size}, fn -> File.rm(path) end}
      {:error, reason} -> {:error, reason, fn -> File.rm(path) end}
    end
  end

  defp normalize_source(body) when is_binary(body) do
    tmp_dir = Path.join(CacheNext.Config.tmp_dir(), "store")
    File.mkdir_p!(tmp_dir)
    path = Path.join(tmp_dir, "artifact-#{System.unique_integer([:positive, :monotonic])}")

    case File.write(path, body) do
      :ok -> {:ok, %{path: path, size: byte_size(body)}, fn -> File.rm(path) end}
      {:error, reason} -> {:error, reason, fn -> File.rm(path) end}
    end
  end

  defp emit_read_telemetry(kind, result, started_at) do
    :telemetry.execute(
      [:cache_next, :artifact, :read],
      %{
        count: 1,
        duration: System.monotonic_time() - started_at,
        size: read_size(result)
      },
      %{
        kind: Atom.to_string(kind),
        result: read_result(result),
        region: CacheNext.Config.region()
      }
    )
  end

  defp emit_write_telemetry(kind, result, started_at) do
    :telemetry.execute(
      [:cache_next, :artifact, :write],
      %{
        count: 1,
        duration: System.monotonic_time() - started_at,
        size: write_size(result)
      },
      %{
        kind: Atom.to_string(kind),
        result: write_result(result),
        region: CacheNext.Config.region()
      }
    )
  end

  defp read_result({:ok, _object}), do: "ok"
  defp read_result({:error, :not_found}), do: "not_found"
  defp read_result({:error, _reason}), do: "error"

  defp write_result({:ok, _size}), do: "ok"
  defp write_result({:error, _reason}), do: "error"

  defp result_label({:ok, %{status: status}}) when status in 200..299, do: "ok"
  defp result_label({:ok, %{status: 404}}), do: "not_found"
  defp result_label({:ok, _value}), do: "error"
  defp result_label({:error, :not_found}), do: "not_found"
  defp result_label({:error, _reason}), do: "error"

  defp read_size({:ok, %{size: size}}) when is_integer(size), do: size
  defp read_size({:ok, %{body: body}}), do: byte_size(body)
  defp read_size(_result), do: 0

  defp write_size({:ok, size}) when is_integer(size), do: size
  defp write_size(_result), do: 0
end
