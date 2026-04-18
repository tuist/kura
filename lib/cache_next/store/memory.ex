defmodule CacheNext.Store.Memory do
  @moduledoc false

  use Agent

  import Plug.Conn

  def start_link(_opts) do
    Agent.start_link(fn -> %{artifacts: %{}, project_refs: %{}} end, name: __MODULE__)
  end

  def child_spec(_opts) do
    %{id: __MODULE__, start: {__MODULE__, :start_link, [[]]}}
  end

  def exists?(kind, account_handle, project_handle, key) do
    Agent.get(__MODULE__, fn state ->
      Map.has_key?(state.artifacts, artifact_key(kind, account_handle, project_handle, key))
    end)
  end

  def fetch(kind, account_handle, project_handle, key) do
    Agent.get(__MODULE__, fn state ->
      case Map.get(state.artifacts, artifact_key(kind, account_handle, project_handle, key)) do
        nil -> {:error, :not_found}
        artifact -> {:ok, artifact}
      end
    end)
  end

  def put(
        kind,
        account_handle,
        project_handle,
        key,
        {:multipart_upload, upload, parts},
        content_type
      ) do
    body =
      parts
      |> Enum.map(fn part_number ->
        upload.parts |> Map.fetch!(part_number) |> Map.fetch!(:body)
      end)
      |> IO.iodata_to_binary()

    put(kind, account_handle, project_handle, key, body, content_type)
  end

  def put(kind, account_handle, project_handle, key, data, content_type) when is_binary(data) do
    store_key = artifact_key(kind, account_handle, project_handle, key)
    project_key = project_key(account_handle, project_handle)

    Agent.update(__MODULE__, fn state ->
      refs = Map.get(state.project_refs, project_key, MapSet.new())

      state
      |> put_in([:artifacts, store_key], %{
        body: data,
        content_type: content_type,
        size: byte_size(data)
      })
      |> put_in([:project_refs, project_key], MapSet.put(refs, store_key))
    end)

    :ok
  end

  def put(kind, account_handle, project_handle, key, {:file, path}, content_type) do
    data = File.read!(path)
    File.rm(path)
    put(kind, account_handle, project_handle, key, data, content_type)
  end

  def delete_project(account_handle, project_handle) do
    project_key = project_key(account_handle, project_handle)

    Agent.update(__MODULE__, fn state ->
      refs = Map.get(state.project_refs, project_key, MapSet.new())

      artifacts =
        Enum.reduce(refs, state.artifacts, fn ref, artifacts ->
          Map.delete(artifacts, ref)
        end)

      %{state | artifacts: artifacts, project_refs: Map.delete(state.project_refs, project_key)}
    end)

    :ok
  end

  def ring_members, do: ["memory"]

  def stream_artifact(conn, status, %{body: body} = artifact, default_content_type) do
    conn
    |> put_resp_content_type(Map.get(artifact, :content_type, default_content_type))
    |> send_resp(status, body)
  end

  defp artifact_key(kind, _account_handle, project_handle, key) do
    {kind, project_handle, key}
  end

  defp project_key(_account_handle, project_handle) do
    project_handle
  end
end
