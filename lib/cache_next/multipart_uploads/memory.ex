defmodule CacheNext.MultipartUploads.Memory do
  @moduledoc false

  use Agent

  def start_link(_opts) do
    Agent.start_link(fn -> %{} end, name: __MODULE__)
  end

  def child_spec(_opts) do
    %{id: __MODULE__, start: {__MODULE__, :start_link, [[]]}}
  end

  def start_upload(_account_handle, project_handle, category, hash, name) do
    upload_id = upload_id(project_handle, category, hash, name)

    Agent.update(__MODULE__, fn state ->
      Map.put(state, upload_id, %{
        project_handle: project_handle,
        category: category,
        hash: hash,
        name: name,
        parts: %{},
        total_bytes: 0
      })
    end)

    {:ok, upload_id}
  end

  def add_part(upload_id, part_number, tmp_path, size_bytes) do
    body = File.read!(tmp_path)
    File.rm(tmp_path)

    Agent.get_and_update(__MODULE__, fn state ->
      case Map.get(state, upload_id) do
        nil ->
          {{:error, :upload_not_found}, state}

        upload ->
          existing = Map.get(upload.parts, part_number)

          total_bytes =
            upload.total_bytes -
              if(existing, do: existing.size, else: 0) +
              size_bytes

          if total_bytes > CacheNext.Config.module_total_max_upload_bytes() do
            {{:error, :total_size_exceeded}, state}
          else
            updated_upload = %{
              upload
              | parts: Map.put(upload.parts, part_number, %{body: body, size: size_bytes}),
                total_bytes: total_bytes
            }

            {:ok, Map.put(state, upload_id, updated_upload)}
          end
      end
    end)
  end

  def complete_upload(upload_id) do
    Agent.get(__MODULE__, fn state ->
      case Map.get(state, upload_id) do
        nil -> {:error, :not_found}
        upload -> {:ok, upload}
      end
    end)
  end

  def abort_upload(upload_id) do
    Agent.update(__MODULE__, &Map.delete(&1, upload_id))
    :ok
  end

  def tmp_storage_size do
    case Process.whereis(__MODULE__) do
      nil ->
        0

      _pid ->
        Agent.get(__MODULE__, fn state ->
          Enum.reduce(state, 0, fn {_upload_id, upload}, acc ->
            acc + upload.total_bytes
          end)
        end)
    end
  end

  defp upload_id(project_handle, category, hash, name) do
    CacheNext.Config.hash(
      "#{CacheNext.Config.tenant()}:#{project_handle}:#{category}:#{hash}:#{name}:#{System.unique_integer()}",
      32
    )
  end
end
