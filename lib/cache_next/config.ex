defmodule CacheNext.Config do
  @moduledoc false

  @xcode_max_upload_bytes 25 * 1024 * 1024
  @gradle_max_upload_bytes 100 * 1024 * 1024
  @module_part_max_upload_bytes 10 * 1024 * 1024
  @module_total_max_upload_bytes 2 * 1024 * 1024 * 1024

  def tenant, do: Application.get_env(:cache_next, :tenant, "demo-tenant")
  def region, do: Application.get_env(:cache_next, :region, "local")
  def tmp_dir, do: Application.get_env(:cache_next, :tmp_dir, "tmp/cache-next")
  def data_dir, do: Application.get_env(:cache_next, :data_dir, "tmp/cache-next-data")
  def store_impl, do: Application.get_env(:cache_next, :store_impl, CacheNext.Store.Riak)

  def multipart_uploads_impl,
    do: Application.get_env(:cache_next, :multipart_uploads_impl, CacheNext.MultipartUploads.Riak)

  def riak_http_endpoints,
    do: Application.get_env(:cache_next, :riak_http_endpoints, ["http://127.0.0.1:8098"])

  def riak_pb_host,
    do: Application.get_env(:cache_next, :riak_pb_host, "127.0.0.1")

  def riak_pb_port,
    do: Application.get_env(:cache_next, :riak_pb_port, 8087)

  def riak_pb_pool_size,
    do: Application.get_env(:cache_next, :riak_pb_pool_size, 8)

  def riak_chunk_size_bytes,
    do: Application.get_env(:cache_next, :riak_chunk_size_bytes, 1_048_576)

  def store_request_timeout_ms,
    do: Application.get_env(:cache_next, :store_request_timeout_ms, 30_000)

  def xcode_max_upload_bytes, do: @xcode_max_upload_bytes
  def gradle_max_upload_bytes, do: @gradle_max_upload_bytes
  def module_part_max_upload_bytes, do: @module_part_max_upload_bytes
  def module_total_max_upload_bytes, do: @module_total_max_upload_bytes

  def prometheus_reporter, do: CacheNext.Prometheus

  def hash(value, take \\ 64) do
    value
    |> then(&:crypto.hash(:sha256, &1))
    |> Base.encode16(case: :lower)
    |> binary_part(0, take)
  end
end
