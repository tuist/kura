defmodule CacheNextWeb.MetricsController do
  use CacheNextWeb, :controller

  import Plug.Conn
  alias TelemetryMetricsPrometheus.Core

  def index(conn, _params) do
    conn
    |> put_resp_header("content-type", "text/plain; version=0.0.4")
    |> send_resp(200, Core.scrape(CacheNext.Config.prometheus_reporter()))
  end
end
