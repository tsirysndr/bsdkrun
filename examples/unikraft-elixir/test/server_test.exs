defmodule ServerTest do
  # Plug.Test drives the router directly — the same two routes the unikernel
  # e2e asserts, without a listener.
  use ExUnit.Case, async: true
  use Plug.Test

  @opts Server.init([])

  test "/ greets" do
    conn = Server.call(conn(:get, "/"), @opts)
    assert conn.status == 200
    assert conn.resp_body == "Hello from Elixir on Unikraft!\n"
  end

  test "/info reports the runtime" do
    conn = Server.call(conn(:get, "/info"), @opts)
    assert conn.status == 200
    assert Jason.decode!(conn.resp_body)["runtime"] == "elixir"
  end
end
