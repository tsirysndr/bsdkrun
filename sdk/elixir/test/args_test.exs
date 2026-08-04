defmodule Bsdkrun.ArgsTest do
  use ExUnit.Case, async: true

  alias Bsdkrun.Args

  describe "linux" do
    test "minimal image is detached" do
      assert Args.build_create(os: :linux, image: "alpine") == ["linux", "alpine", "-d"]
    end

    test "full option set, in the documented order" do
      argv =
        Args.build_create(
          os: :linux,
          image: "ghcr.io/owner/name:tag",
          kernel: "vmlinux",
          kernel_version: "6.6",
          initramfs: true,
          volume: "web",
          mounts: ["~/project:/src", "~/data:/data:ro"],
          entrypoint: "/init",
          console: "hvc0",
          cpus: 2,
          mem: 1024,
          name: "web",
          net: %{ports: ["8080:80", %{host: 2222, guest: 22}], network: "devnet"},
          command: ["node", "server.js"]
        )

      assert argv == [
               "linux",
               "ghcr.io/owner/name:tag",
               "-d",
               "--kernel",
               "vmlinux",
               "--kernel-version",
               "6.6",
               "--initramfs",
               "-v",
               "web",
               "--mount",
               "~/project:/src",
               "--mount",
               "~/data:/data:ro",
               "--entrypoint",
               "/init",
               "--console",
               "hvc0",
               "--port",
               "8080:80",
               "--port",
               "2222:22",
               "--network",
               "devnet",
               "--name",
               "web",
               "--cpus",
               "2",
               "--mem",
               "1024",
               "--",
               "node",
               "server.js"
             ]
    end

    test "initramfs is a flag, not a value" do
      argv = Args.build_create(os: :linux, image: "alpine", initramfs: true)
      assert "--initramfs" in argv
      # no path follows the flag
      assert Enum.at(argv, Enum.find_index(argv, &(&1 == "--initramfs")) + 1) == nil
    end
  end

  describe "net" do
    test "disabled comes first, then ports, mac, network" do
      argv =
        Args.build_create(
          os: :linux,
          image: "alpine",
          net: %{disabled: true, ports: ["1:1"], mac: "aa:bb", network: "n"}
        )

      tail = Enum.drop(argv, 3)

      assert tail == [
               "--no-net",
               "--port",
               "1:1",
               "--mac",
               "aa:bb",
               "--network",
               "n"
             ]
    end

    test "port accepts tuple, map, and string" do
      argv =
        Args.build_create(
          os: :linux,
          image: "alpine",
          net: %{ports: [{2222, 22}, %{host: 80, guest: 8080}, "5:6"]}
        )

      assert Enum.drop(argv, 3) == [
               "--port",
               "2222:22",
               "--port",
               "80:8080",
               "--port",
               "5:6"
             ]
    end
  end

  describe "freebsd / netbsd" do
    test "freebsd with version, firmware, force and disk persistence" do
      argv =
        Args.build_create(
          os: :freebsd,
          version: "14.3",
          firmware: "KRUN_EFI.fd",
          force: true,
          persist: true,
          volume: "db",
          attach_disk: ["extra.raw", "ro.raw:ro"],
          cpus: 4,
          mem: 2048
        )

      assert argv == [
               "freebsd",
               "-d",
               "--version",
               "14.3",
               "--firmware",
               "KRUN_EFI.fd",
               "--force",
               "--persist",
               "-v",
               "db",
               "--attach-disk",
               "extra.raw",
               "--attach-disk",
               "ro.raw:ro",
               "--cpus",
               "4",
               "--mem",
               "2048"
             ]
    end

    test "netbsd has no firmware flag" do
      argv = Args.build_create(os: :netbsd, version: "10.1", volume: "db")
      assert argv == ["netbsd", "-d", "--version", "10.1", "-v", "db"]
    end
  end

  describe "firmware / kernel" do
    test "firmware places required flags before disk args" do
      argv =
        Args.build_create(os: :firmware, firmware: "KRUN_EFI.fd", disk: "disk.raw", persist: true)

      assert argv == [
               "firmware",
               "--firmware",
               "KRUN_EFI.fd",
               "--disk",
               "disk.raw",
               "-d",
               "--persist"
             ]
    end

    test "kernel initramfs is a path value here" do
      argv =
        Args.build_create(
          os: :kernel,
          kernel: "netbsd",
          format: "elf",
          initramfs: "initrd.img",
          cmdline: "root=ld0a",
          disk: "root.raw"
        )

      assert argv == [
               "kernel",
               "--kernel",
               "netbsd",
               "-d",
               "--format",
               "elf",
               "--initramfs",
               "initrd.img",
               "--cmdline",
               "root=ld0a",
               "--disk",
               "root.raw"
             ]
    end
  end

  test "accepts a map with string keys and a string os" do
    argv = Args.build_create(%{"os" => "linux", "image" => "alpine", "name" => "x"})
    assert argv == ["linux", "alpine", "-d", "--name", "x"]
  end
end
