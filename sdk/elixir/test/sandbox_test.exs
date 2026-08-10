defmodule Bsdkrun.SandboxTest do
  # Only `id/1` is pure enough to unit test without spawning a real `bsdkrun`
  # binary — everything else in this module shells out.
  use ExUnit.Case, async: true

  alias Bsdkrun.Sandbox

  test "id from a sandbox struct" do
    assert Sandbox.id(%Sandbox{id: "abc123", ssh_port: 2222}) == "abc123"
  end

  test "id from a bare id string" do
    assert Sandbox.id("web-1") == "web-1"
  end
end
