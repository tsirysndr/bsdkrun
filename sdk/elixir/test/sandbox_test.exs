defmodule Bsdkrun.SandboxTest do
  # `id/1` and the `Builder` (`new/1` + `with_*/2`) are pure enough to unit
  # test without spawning a real `bsdkrun` binary — everything else in this
  # module shells out.
  use ExUnit.Case, async: true

  alias Bsdkrun.{Args, Sandbox}
  alias Bsdkrun.Sandbox.Builder

  test "id from a sandbox struct" do
    assert Sandbox.id(%Sandbox{id: "abc123", ssh_port: 2222}) == "abc123"
  end

  test "id from a bare id string" do
    assert Sandbox.id("web-1") == "web-1"
  end

  describe "Builder" do
    test "new/1 normalizes into a Builder with an atom-keyed opts map" do
      assert %Builder{opts: %{os: :linux, image: "alpine"}} =
               Sandbox.new(os: :linux, image: "alpine")
    end

    test "with_volume/2, with_mount(s)/2, with_cpus/2, with_mem/2, with_name/2, with_command/2 set flat opts" do
      builder =
        Sandbox.new(os: :linux, image: "alpine")
        |> Sandbox.with_volume("web")
        |> Sandbox.with_mount("~/project:/src")
        |> Sandbox.with_mounts(["~/data:/data:ro"])
        |> Sandbox.with_cpus(2)
        |> Sandbox.with_mem(1024)
        |> Sandbox.with_name("web")
        |> Sandbox.with_command(["node", "server.js"])

      assert builder.opts.volume == "web"
      assert builder.opts.mounts == ["~/project:/src", "~/data:/data:ro"]
      assert builder.opts.cpus == 2
      assert builder.opts.mem == 1024
      assert builder.opts.name == "web"
      assert builder.opts.command == ["node", "server.js"]
    end

    test "with_network/2, with_port(s)/2 and with_disk/2 accumulate under :net / :attach_disk" do
      builder =
        Sandbox.new(os: :linux, image: "alpine")
        |> Sandbox.with_network("devnet")
        |> Sandbox.with_port("8080:80")
        |> Sandbox.with_ports([{2222, 22}])
        |> Sandbox.with_disk("extra.raw")

      assert builder.opts.net == %{network: "devnet", ports: ["8080:80", {2222, 22}]}
      assert builder.opts.attach_disk == ["extra.raw"]
    end

    test "with_opt/3 sets an arbitrary key" do
      builder = Sandbox.new(os: :linux, image: "alpine") |> Sandbox.with_opt(:persist, true)
      assert builder.opts.persist == true
    end

    test "a Builder's opts build the same argv as the equivalent keyword list" do
      builder =
        Sandbox.new(os: :linux, image: "alpine")
        |> Sandbox.with_volume("web")
        |> Sandbox.with_network("devnet")

      from_builder = Args.build_create(builder.opts)

      from_opts =
        Args.build_create(os: :linux, image: "alpine", volume: "web", net: %{network: "devnet"})

      assert from_builder == from_opts
    end
  end
end
