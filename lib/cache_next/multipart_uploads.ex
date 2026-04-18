defmodule CacheNext.MultipartUploads do
  @moduledoc false

  def backend, do: CacheNext.Config.multipart_uploads_impl()

  def start_upload(account_handle, project_handle, category, hash, name) do
    backend().start_upload(account_handle, project_handle, category, hash, name)
  end

  def add_part(upload_id, part_number, tmp_path, size_bytes) do
    backend().add_part(upload_id, part_number, tmp_path, size_bytes)
  end

  def complete_upload(upload_id) do
    backend().complete_upload(upload_id)
  end

  def abort_upload(upload_id) do
    backend().abort_upload(upload_id)
  end

  def tmp_storage_size do
    backend().tmp_storage_size()
  end
end
