defmodule CacheNextWeb.ControllerHelpers do
  @moduledoc false

  import Plug.Conn
  import Phoenix.Controller

  def error(conn, status, message) do
    conn
    |> put_status(status)
    |> json(%{message: message})
  end

  def required_query(params, keys) do
    Enum.reduce_while(keys, {:ok, %{}}, fn key, {:ok, acc} ->
      case Map.get(params, key) do
        value when is_binary(value) and value != "" -> {:cont, {:ok, Map.put(acc, key, value)}}
        _ -> {:halt, {:error, key}}
      end
    end)
  end

  def parse_integer(nil), do: :error

  def parse_integer(value) when is_integer(value), do: {:ok, value}

  def parse_integer(value) when is_binary(value) do
    case Integer.parse(value) do
      {integer, ""} -> {:ok, integer}
      _ -> :error
    end
  end

  def send_octet_stream(conn, status, body) do
    conn
    |> put_resp_content_type("application/octet-stream")
    |> send_resp(status, body)
  end

  def send_artifact(conn, status, %{body: body} = artifact, default_content_type) do
    conn
    |> put_resp_content_type(Map.get(artifact, :content_type, default_content_type))
    |> send_resp(status, body)
  end

  def send_artifact(conn, status, %{chunk_keys: _chunk_keys} = artifact, default_content_type) do
    CacheNext.Store.stream_artifact(conn, status, artifact, default_content_type)
  end

  def send_artifact(conn, status, %{path: path} = artifact, default_content_type) do
    conn
    |> put_resp_content_type(Map.get(artifact, :content_type, default_content_type))
    |> send_file(status, path)
  end
end
