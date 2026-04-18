defmodule CacheNextWeb.UpController do
  use CacheNextWeb, :controller

  def index(conn, _params) do
    cluster_status = CacheNext.Riak.cluster_status()
    members = Enum.sort(cluster_status.ring_members)
    connected_nodes = Enum.sort(cluster_status.connected_nodes)

    json(conn, %{
      status: "ok",
      tenant: CacheNext.Config.tenant(),
      region: CacheNext.Config.region(),
      node: CacheNext.Config.region(),
      connected_nodes: connected_nodes,
      ring_members: length(members),
      members: members
    })
  end
end
