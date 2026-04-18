defmodule CacheNextWeb.XcodeModuleControllerTest do
  use CacheNextWeb.ConnCase, async: false

  alias CacheNext.Store

  test "HEAD /api/cache/module/:id returns 204 when the module exists", %{conn: conn} do
    {account_handle, project_handle, hash, name} = unique_module_parts()
    key = "builds/#{hash}/#{name}"

    assert :ok =
             Store.put(
               :module,
               account_handle,
               project_handle,
               key,
               "module-payload",
               "application/octet-stream"
             )

    conn =
      head(
        conn,
        "/api/cache/module/module-1?account_handle=#{account_handle}&project_handle=#{project_handle}&hash=#{hash}&name=#{name}"
      )

    assert conn.status == 204
    assert conn.resp_body == ""
  end

  test "GET /api/cache/module/:id returns the stored module blob", %{conn: conn} do
    {account_handle, project_handle, hash, name} = unique_module_parts()
    key = "builds/#{hash}/#{name}"

    assert :ok =
             Store.put(
               :module,
               account_handle,
               project_handle,
               key,
               "module-payload",
               "application/octet-stream"
             )

    conn =
      get(
        conn,
        "/api/cache/module/module-1?account_handle=#{account_handle}&project_handle=#{project_handle}&hash=#{hash}&name=#{name}"
      )

    assert conn.status == 200
    assert conn.resp_body == "module-payload"
    assert ["application/octet-stream" <> _suffix] = get_resp_header(conn, "content-type")
  end

  defp unique_module_parts do
    suffix = System.unique_integer([:positive, :monotonic])

    {
      "account-#{suffix}",
      "project-#{suffix}",
      "hash-#{suffix}",
      "Module-#{suffix}.framework"
    }
  end
end
