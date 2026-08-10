defmodule Bsdkrun.TypesTest do
  use ExUnit.Case, async: true

  alias Bsdkrun.Types
  alias Bsdkrun.Types.SandboxInfo

  describe "sandbox_info/1" do
    test "kind is an atom, matching create/1's :os values" do
      row = %{
        "id" => "abc123",
        "image" => "alpine",
        "kind" => "freebsd",
        "running" => true,
        "detached" => true,
        "cpus" => 2,
        "mem" => 1024,
        "state_dir" => "/tmp/abc123",
        "created_at" => 0
      }

      assert %SandboxInfo{kind: :freebsd} = Types.sandbox_info(row)
    end
  end

  describe "sandbox_info_from_graphql/1" do
    test "kind is an atom, matching create/1's :os values" do
      row = %{
        "id" => "abc123",
        "image" => "alpine",
        "kind" => "unikraft",
        "status" => "running",
        "running" => true,
        "detached" => true,
        "cpus" => 2,
        "mem" => 1024
      }

      assert %SandboxInfo{kind: :unikraft} = Types.sandbox_info_from_graphql(row)
    end
  end
end
