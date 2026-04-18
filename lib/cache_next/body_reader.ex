defmodule CacheNext.BodyReader do
  @moduledoc false

  @default_read_length 262_144
  @default_read_timeout 30_000
  @memory_threshold 512_000

  def read(conn, opts \\ []) do
    max_bytes = Keyword.fetch!(opts, :max_bytes)
    tmp_dir = Keyword.get(opts, :tmp_dir, CacheNext.Config.tmp_dir())
    read_length = Keyword.get(opts, :read_length, @default_read_length)
    read_timeout = Keyword.get(opts, :read_timeout, @default_read_timeout)

    File.mkdir_p!(tmp_dir)

    do_read(
      conn,
      [length: read_length, read_length: read_length, read_timeout: read_timeout],
      max_bytes,
      tmp_dir,
      0,
      [],
      nil,
      nil
    )
  end

  def drain(conn, opts \\ []) do
    max_bytes = Keyword.fetch!(opts, :max_bytes)
    read_length = Keyword.get(opts, :read_length, @default_read_length)
    read_timeout = Keyword.get(opts, :read_timeout, @default_read_timeout)

    do_drain(
      conn,
      [length: read_length, read_length: read_length, read_timeout: read_timeout],
      0,
      max_bytes
    )
  end

  defp do_read(conn, read_opts, max_bytes, tmp_dir, total_bytes, chunks, device, path) do
    case Plug.Conn.read_body(conn, read_opts) do
      {:ok, body, conn_after} ->
        finish_read(conn_after, body, max_bytes, tmp_dir, total_bytes, chunks, device, path)

      {:more, body, conn_after} ->
        with {:ok, total_bytes, chunks, device, path} <-
               append_chunk(body, max_bytes, tmp_dir, total_bytes, chunks, device, path) do
          do_read(conn_after, read_opts, max_bytes, tmp_dir, total_bytes, chunks, device, path)
        else
          {:error, reason, device, path} ->
            cleanup_file(device, path)
            {:error, reason, conn_after}
        end

      {:error, :timeout} ->
        cleanup_file(device, path)
        {:error, :timeout, conn}

      {:error, reason} ->
        cleanup_file(device, path)
        {:error, reason, conn}
    end
  rescue
    error in [Bandit.HTTPError, Bandit.TransportError] ->
      cleanup_file(device, path)
      {:error, normalize_transport_error(error), conn}
  end

  defp finish_read(conn, body, max_bytes, tmp_dir, total_bytes, chunks, device, path) do
    with {:ok, _total_bytes, chunks, device, path} <-
           append_chunk(body, max_bytes, tmp_dir, total_bytes, chunks, device, path) do
      case device do
        nil ->
          {:ok, IO.iodata_to_binary(Enum.reverse(chunks)), conn}

        _ ->
          File.close(device)
          {:ok, {:file, path}, conn}
      end
    else
      {:error, reason, device, path} ->
        cleanup_file(device, path)
        {:error, reason, conn}
    end
  end

  defp append_chunk(body, _max_bytes, _tmp_dir, total_bytes, chunks, device, path)
       when body in ["", <<>>] do
    {:ok, total_bytes, chunks, device, path}
  end

  defp append_chunk(body, max_bytes, tmp_dir, total_bytes, chunks, device, path) do
    new_total = total_bytes + byte_size(body)

    cond do
      new_total > max_bytes ->
        {:error, :too_large, device, path}

      device == nil and new_total <= @memory_threshold ->
        {:ok, new_total, [body | chunks], nil, nil}

      device == nil ->
        path = temp_file_path(tmp_dir)
        {:ok, device} = File.open(path, [:write, :binary])
        Enum.reverse(chunks) |> Enum.each(&IO.binwrite(device, &1))
        IO.binwrite(device, body)
        {:ok, new_total, [], device, path}

      true ->
        IO.binwrite(device, body)
        {:ok, new_total, chunks, device, path}
    end
  end

  defp do_drain(conn, read_opts, total_bytes, max_bytes) do
    case Plug.Conn.read_body(conn, read_opts) do
      {:ok, body, conn_after} ->
        total_bytes = total_bytes + byte_size(body)

        if total_bytes > max_bytes do
          {:error, conn_after}
        else
          {:ok, conn_after}
        end

      {:more, body, conn_after} ->
        total_bytes = total_bytes + byte_size(body)

        if total_bytes > max_bytes do
          {:error, conn_after}
        else
          do_drain(conn_after, read_opts, total_bytes, max_bytes)
        end

      {:error, _reason} ->
        {:error, conn}
    end
  rescue
    _error in [Bandit.HTTPError, Bandit.TransportError] ->
      {:error, conn}
  end

  defp cleanup_file(nil, _path), do: :ok

  defp cleanup_file(device, path) do
    File.close(device)
    File.rm(path)
  end

  defp normalize_transport_error(%Bandit.TransportError{error: :timeout}), do: :timeout
  defp normalize_transport_error(_error), do: :cancelled

  defp temp_file_path(tmp_dir) do
    Path.join(tmp_dir, "upload-#{System.unique_integer([:positive, :monotonic])}")
  end
end
