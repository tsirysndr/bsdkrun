# frozen_string_literal: true

require "minitest/autorun"
require "bsdkrun"

# Unit tests for the argv builder — no VM / binary needed.
class TestArgs < Minitest::Test
  def build(opts)
    Bsdkrun::Args.build_create_args(opts)
  end

  def test_linux_minimal
    assert_equal(["linux", "alpine", "-d"], build(os: "linux", image: "alpine"))
  end

  def test_linux_full_ordering
    got = build(
      os: "linux",
      image: "ghcr.io/o/n:tag",
      kernel: "vmlinux",
      kernel_version: "6.6",
      initramfs: true,
      volume: "web",
      mounts: ["~/a:/a", "~/b:/b:ro"],
      entrypoint: "/init",
      console: "hvc0",
      net: { ports: ["8080:80", { host: 2222, guest: 22 }], mac: "AA:BB", network: "devnet" },
      name: "api",
      cpus: 2,
      mem: 1024,
      command: ["node", "server.js"]
    )
    expected = [
      "linux", "ghcr.io/o/n:tag", "-d",
      "--kernel", "vmlinux",
      "--kernel-version", "6.6",
      "--initramfs",
      "-v", "web",
      "--mount", "~/a:/a", "--mount", "~/b:/b:ro",
      "--entrypoint", "/init",
      "--console", "hvc0",
      "--port", "8080:80", "--port", "2222:22",
      "--mac", "AA:BB",
      "--network", "devnet",
      "--name", "api",
      "--cpus", "2",
      "--mem", "1024",
      "--", "node", "server.js"
    ]
    assert_equal(expected, got)
  end

  def test_freebsd
    got = build(os: "freebsd", version: "14.3", firmware: "KRUN_EFI.fd",
                force: true, persist: true, volume: "db", mem: 2048)
    expected = [
      "freebsd", "-d",
      "--version", "14.3",
      "--firmware", "KRUN_EFI.fd",
      "--force",
      "--persist",
      "-v", "db",
      "--mem", "2048"
    ]
    assert_equal(expected, got)
  end

  def test_netbsd_with_attach_disks
    got = build(os: "netbsd", version: "10.1", attach_disk: ["a.raw", "b.raw:ro"], cpus: 4)
    expected = [
      "netbsd", "-d",
      "--version", "10.1",
      "--attach-disk", "a.raw", "--attach-disk", "b.raw:ro",
      "--cpus", "4"
    ]
    assert_equal(expected, got)
  end

  def test_firmware
    got = build(os: "firmware", firmware: "KRUN_EFI.fd", disk: "disk.raw",
                net: { disabled: true })
    expected = [
      "firmware", "--firmware", "KRUN_EFI.fd", "--disk", "disk.raw", "-d",
      "--no-net"
    ]
    assert_equal(expected, got)
  end

  def test_kernel
    got = build(os: "kernel", kernel: "netbsd", format: "elf",
                initramfs: "initrd.img", cmdline: "root=ld0a", disk: "root.raw")
    expected = [
      "kernel", "--kernel", "netbsd", "-d",
      "--format", "elf",
      "--initramfs", "initrd.img",
      "--cmdline", "root=ld0a",
      "--disk", "root.raw"
    ]
    assert_equal(expected, got)
  end

  def test_string_os_value
    # build_create_args accepts a String or Symbol :os value.
    assert_equal(["netbsd", "-d"], build(os: "netbsd"))
    assert_equal(["netbsd", "-d"], build(os: :netbsd))
  end

  def test_unknown_os_raises
    assert_raises(ArgumentError) { build(os: "plan9") }
  end

  def test_no_net_when_net_absent
    assert_equal([], Bsdkrun::Args.net_args(nil))
  end

  def test_result_helpers
    ok = Bsdkrun::Result.new(stdout: "hello\n\n", stderr: "", exit_code: 0, command: "x")
    assert ok.ok?
    assert_equal("hello", ok.text)

    bad = Bsdkrun::Result.new(stdout: "", stderr: "boom", exit_code: 1, command: "x")
    refute bad.ok?
    assert_raises(Bsdkrun::CommandFailed) { bad.throw_if_failed! }
  end

  def test_sandbox_info_from_row
    info = Bsdkrun::SandboxInfo.from_row(
      "id" => "abc123", "name" => "api", "image" => "alpine", "kind" => "linux",
      "command" => "sleep 300", "running" => true, "exit_code" => nil, "pid" => 42,
      "detached" => true, "cpus" => 2, "mem" => 512, "volume" => nil,
      "state_dir" => "/s", "network" => "devnet", "net_ip" => "10.0.0.2",
      "created_at" => 1_700_000_000, "finished_at" => nil,
      "ports" => [{ "bind" => "127.0.0.1", "host" => 8080, "guest" => 80 }]
    )
    assert_equal("abc123", info.id)
    assert_equal("running", info.status)
    assert info.running
    assert_nil info.exit_code
    assert_equal(42, info.pid)
    assert_equal("devnet", info.network)
    assert_equal(1, info.ports.length)
    assert_equal("127.0.0.1", info.ports.first.bind)
    assert_equal(8080, info.ports.first.host)
    assert_equal(80, info.ports.first.guest)
  end

  def test_sandbox_info_from_row_defaults_ports_to_empty_array
    info = Bsdkrun::SandboxInfo.from_row("id" => "abc123")
    assert_equal([], info.ports)
  end
end
