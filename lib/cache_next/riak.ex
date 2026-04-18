defmodule CacheNext.Riak do
  @moduledoc false

  use Supervisor

  @default_http_headers [{"accept", "*/*"}]

  def start_link(_opts) do
    Supervisor.start_link(__MODULE__, [], name: __MODULE__)
  end

  @impl true
  def init(_opts) do
    pool_size = max(CacheNext.Config.riak_pb_pool_size(), 1)

    children =
      for index <- 0..(pool_size - 1) do
        %{
          id: {__MODULE__, index},
          start: {__MODULE__, :start_connection, [index]}
        }
      end

    Supervisor.init(children, strategy: :one_for_one)
  end

  def start_connection(index) do
    host = CacheNext.Config.riak_pb_host() |> to_charlist()
    port = CacheNext.Config.riak_pb_port()

    case :riakc_pb_socket.start_link(
           host,
           port,
           [
             :auto_reconnect,
             :queue_if_disconnected,
             {:connect_timeout, CacheNext.Config.store_request_timeout_ms()}
           ]
         ) do
      {:ok, pid} ->
        true = Process.register(pid, connection_name(index))
        {:ok, pid}

      other ->
        other
    end
  end

  def get_object(bucket, key, opts \\ []) do
    get_via_pb(bucket, key, opts, false)
  end

  def head_object(bucket, key, opts \\ []) do
    get_via_pb(bucket, key, opts, true)
  end

  def put_object(bucket, key, body, content_type, opts \\ []) do
    with {:ok, object} <- build_object(bucket, key, body, content_type),
         {:ok, object} <- apply_put_options(object, opts),
         {:ok, response} <- put_via_pb(object, timeout_from_opts(opts)) do
      {:ok, response}
    end
  end

  def delete_object(bucket, key) do
    case :riakc_pb_socket.delete(connection_for_key(key), pb_bucket(bucket), pb_key(key), timeout()) do
      :ok -> {:ok, %{status: 204, headers: %{}, body: <<>>}}
      {:error, reason} -> {:error, normalize_error(reason)}
    end
  end

  def query_index(bucket, index, value, opts \\ []) do
    pb_opts =
      []
      |> maybe_add_option(:continuation, opts[:continuation])
      |> maybe_add_option(:max_results, opts[:max_results])
      |> maybe_add_option(:timeout, opts[:timeout] || timeout())

    case :riakc_pb_socket.get_index_eq(
           connection_for_query(),
           pb_bucket(bucket),
           pb_secondary_index(index),
           pb_key(value),
           pb_opts
         ) do
      {:ok, {:index_results_v1, keys, _terms, continuation}} ->
        {:ok,
         %{
           status: 200,
           keys: keys || [],
           continuation: normalize_continuation(continuation)
         }}

      {:ok, {:index_body_results_v1, objects, continuation}} ->
        {:ok,
         %{
           status: 200,
           objects: objects,
           continuation: normalize_continuation(continuation)
         }}

      {:error, reason} ->
        {:error, normalize_error(reason)}
    end
  end

  def ping do
    case :riakc_pb_socket.ping(connection_for_query()) do
      :pong -> {:ok, %{status: 200, headers: %{}, body: "pong"}}
      {:error, reason} -> {:error, normalize_error(reason)}
    end
  end

  def stats do
    with {:ok, response} <- http_request(:get, "/stats", headers: @default_http_headers),
         {:ok, %{status: 200} = response} <- {:ok, response},
         {:ok, decoded} <- decode_json(response) do
      {:ok, decoded.body}
    else
      {:ok, %{status: status}} -> {:error, {:unexpected_status, status}}
      {:error, reason} -> {:error, reason}
    end
  end

  def cluster_status do
    case stats() do
      {:ok, %{"ring_members" => ring_members, "connected_nodes" => connected_nodes}}
      when is_list(ring_members) and is_list(connected_nodes) ->
        %{
          ring_members: Enum.map(ring_members, &normalize_member/1),
          connected_nodes: Enum.map(connected_nodes, &normalize_member/1)
        }

      _ ->
        connected_nodes =
          case :riakc_pb_socket.peer_discovery(connection_for_query()) do
            {:ok, nodes} -> Enum.map(nodes, &normalize_pb_node/1)
            _ -> []
          end

        %{
          ring_members: connected_nodes,
          connected_nodes: connected_nodes
        }
    end
  end

  def members do
    cluster_status().ring_members
  end

  def decode_json(%{body: body} = response) when is_binary(body) do
    case Jason.decode(body) do
      {:ok, value} -> {:ok, Map.put(response, :body, value)}
      {:error, reason} -> {:error, {:invalid_json, reason}}
    end
  end

  defp get_via_pb(bucket, key, opts, head?) do
    get_opts =
      []
      |> maybe_add_option(:head, head?)

    case :riakc_pb_socket.get(
           connection_for_key(key),
           pb_bucket(bucket),
           pb_key(key),
           get_opts,
           timeout_from_opts(opts)
         ) do
      {:ok, object} ->
        {:ok, response_from_object(object, head?)}

      {:error, :notfound} ->
        {:ok, %{status: 404, headers: %{}, body: <<>>}}

      {:error, :notfound, vclock} ->
        {:ok, %{status: 404, headers: %{"x-riak-vclock" => vclock}, body: <<>>}}

      {:error, reason} ->
        {:error, normalize_error(reason)}
    end
  end

  defp build_object(bucket, key, body, content_type) do
    case :riakc_obj.new(pb_bucket(bucket), pb_key(key), body, content_type) do
      {:error, reason} -> {:error, reason}
      object -> {:ok, object}
    end
  end

  defp apply_put_options(object, opts) do
    with {:ok, object} <- maybe_set_vclock(object, opts[:vclock]),
         {:ok, object} <- maybe_apply_headers(object, Keyword.get(opts, :headers, [])) do
      {:ok, object}
    end
  end

  defp maybe_set_vclock(object, nil), do: {:ok, object}

  defp maybe_set_vclock(object, vclock) when is_binary(vclock) do
    {:ok, :riakc_obj.set_vclock(object, vclock)}
  end

  defp maybe_apply_headers(object, headers) do
    Enum.reduce_while(headers, {:ok, object}, fn
      {"content-type", _value}, {:ok, object} ->
        {:cont, {:ok, object}}

      {"x-riak-vclock", _value}, {:ok, object} ->
        {:cont, {:ok, object}}

      {"x-riak-index-" <> index_name, value}, {:ok, object} ->
        case put_secondary_index(object, index_name, value) do
          {:ok, object} -> {:cont, {:ok, object}}
          {:error, reason} -> {:halt, {:error, reason}}
        end

      {_name, _value}, {:ok, object} ->
        {:cont, {:ok, object}}
    end)
  end

  defp put_secondary_index(object, index_name, value) do
    metadata = :riakc_obj.get_update_metadata(object)

    case parse_secondary_index(index_name, value) do
      {:ok, index} ->
        {:ok, :riakc_obj.update_metadata(object, :riakc_obj.set_secondary_index(metadata, index))}

      {:error, reason} ->
        {:error, reason}
    end
  end

  defp parse_secondary_index(index_name, value) do
    cond do
      String.ends_with?(index_name, "_bin") ->
        {:ok,
         {{:binary_index, String.trim_trailing(index_name, "_bin")}, [pb_key(value)]}}

      String.ends_with?(index_name, "_int") ->
        case Integer.parse(to_string(value)) do
          {integer, ""} ->
            {:ok,
             {{:integer_index, String.trim_trailing(index_name, "_int")}, [integer]}}

          _ ->
            {:error, {:invalid_secondary_index, index_name, value}}
        end

      true ->
        {:error, {:unsupported_secondary_index, index_name}}
    end
  end

  defp put_via_pb(object, request_timeout) do
    case :riakc_pb_socket.put(connection_for_key(:riakc_obj.key(object)), object, request_timeout) do
      :ok ->
        {:ok, %{status: 204, headers: %{}, body: <<>>}}

      {:ok, returned_object} when is_tuple(returned_object) ->
        {:ok, response_from_object(returned_object, false)}

      {:ok, _generated_key} ->
        {:ok, %{status: 201, headers: %{}, body: <<>>}}

      {:error, reason} ->
        {:error, normalize_error(reason)}
    end
  end

  defp response_from_object(object, head?) do
    body =
      if head? do
        <<>>
      else
        object
        |> :riakc_obj.get_value()
        |> normalize_body()
      end

    headers =
      %{}
      |> maybe_put_response_header("content-type", safe_content_type(object))
      |> maybe_put_response_header("x-riak-vclock", :riakc_obj.vclock(object))

    %{status: 200, headers: headers, body: body}
  end

  defp safe_content_type(object) do
    try do
      :riakc_obj.get_content_type(object)
    catch
      :throw, :siblings -> nil
    end
  end

  defp maybe_put_response_header(headers, _name, nil), do: headers

  defp maybe_put_response_header(headers, name, value) do
    Map.put(headers, name, value)
  end

  defp connection_for_key(key) do
    pool_size = max(CacheNext.Config.riak_pb_pool_size(), 1)
    index = :erlang.phash2(key, pool_size)

    connection_name(index)
    |> Process.whereis()
    |> case do
      nil -> raise "Riak protobuf connection #{index} is not available"
      pid -> pid
    end
  end

  defp connection_for_query do
    connection_for_key("query")
  end

  defp connection_name(index), do: :"cache_next_riak_pb_#{index}"

  defp pb_bucket({type, bucket}), do: {pb_key(type), pb_key(bucket)}
  defp pb_bucket(bucket), do: pb_key(bucket)

  defp pb_key(value) when is_binary(value), do: value
  defp pb_key(value), do: value |> to_string() |> IO.iodata_to_binary()

  defp pb_secondary_index(index) do
    index
    |> to_string()
    |> pb_key()
  end

  defp normalize_continuation(nil), do: nil
  defp normalize_continuation(:undefined), do: nil
  defp normalize_continuation(value), do: value

  defp timeout do
    CacheNext.Config.store_request_timeout_ms()
  end

  defp timeout_from_opts(opts) do
    Keyword.get(opts, :timeout, timeout())
  end

  defp maybe_add_option(options, _name, nil), do: options
  defp maybe_add_option(options, _name, false), do: options
  defp maybe_add_option(options, name, true), do: [name | options]
  defp maybe_add_option(options, name, value), do: [{name, value} | options]

  defp normalize_error({:req_timedout, _request_id}), do: :timeout
  defp normalize_error(reason), do: reason

  defp http_request(method, path, opts) do
    CacheNext.Config.riak_http_endpoints()
    |> Enum.reduce_while({:error, :no_available_endpoint}, fn endpoint, _acc ->
      case do_http_request(endpoint, method, path, opts) do
        {:error, reason} ->
          if reason in [:timeout, :econnrefused, :socket_closed_remotely, :nxdomain] do
            {:cont, {:error, reason}}
          else
            {:halt, {:error, reason}}
          end

        other ->
          {:halt, other}
      end
    end)
  end

  defp do_http_request(endpoint, method, path, opts) do
    headers =
      opts
      |> Keyword.get(:headers, [])
      |> Enum.map(fn {name, value} ->
        {String.to_charlist(name), String.to_charlist(value)}
      end)

    url = String.to_charlist(endpoint <> path)
    request = {url, headers}

    case :httpc.request(method, request, [timeout: timeout()], [body_format: :binary]) do
      {:ok, {{_version, status, _reason}, response_headers, body}} ->
        {:ok,
         %{
           status: status,
           headers: normalize_headers(response_headers),
           body: normalize_body(body)
         }}

      {:error, reason} ->
        {:error, reason}
    end
  end

  defp normalize_headers(headers) do
    Enum.into(headers, %{}, fn {name, value} ->
      {name |> to_string() |> String.downcase(), to_string(value)}
    end)
  end

  defp normalize_body(body) when is_binary(body), do: body
  defp normalize_body(body) when is_list(body), do: IO.iodata_to_binary(body)
  defp normalize_body(_body), do: <<>>

  defp normalize_member(member) do
    member
    |> to_string()
    |> String.split("@")
    |> List.last()
    |> String.split(".")
    |> List.first()
  end

  defp normalize_pb_node({ip, port}) do
    "#{List.to_string(:inet.ntoa(ip))}:#{port}"
  end
end
