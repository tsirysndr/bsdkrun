# frozen_string_literal: true

require "json"

module Bsdkrun
  # ZFS volumes: one zpool per volume on a disk image, held by one machine at
  # a time (Linux, FreeBSD, NetBSD). Branches are datasets cloned
  # copy-on-write inside the pool (+zfs snapshot+ + +zfs clone+), so a branch
  # costs nothing until it diverges.
  module Zfs
    module_function

    # List ZFS volumes, with their branches and holder.
    # @return [Array<ZfsVolumeInfo>]
    def list
      res = Process.run!(["zfs", "ls", "--json"], label: "bsdkrun zfs ls")
      JSON.parse(res.stdout.empty? ? "[]" : res.stdout).map { |row| ZfsVolumeInfo.from_row(row) }
    end

    # The branches of one volume.
    # @param volume [String]
    # @return [Array<ZfsBranch>]
    def branches(volume)
      res = Process.run!(["zfs", "ls", volume, "--json"], label: "bsdkrun zfs ls")
      JSON.parse(res.stdout.empty? ? "[]" : res.stdout).map { |row| ZfsBranch.from_row(row) }
    end

    # Create a volume: a sparse image that becomes a zpool with a +main+
    # branch the first time a guest imports it.
    # @param name [String]
    # @param size [String, nil] e.g. "10G" (the default)
    # @return [String] the volume name
    def create(name, size: nil)
      args = ["zfs", "create", name]
      args.push("--size", size) if size
      Process.run!(args, label: "bsdkrun zfs create").stdout.strip
    end

    # Fork a branch copy-on-write from +from+ (+main+ by default). Runs in the
    # machine holding the volume or in a short-lived FreeBSD helper, so it can
    # take as long as a BSD boot.
    # @return [String] the new +VOL@BRANCH+
    def branch(volume, name, from: nil)
      args = ["zfs", "branch", volume, name]
      args.push("--from", from) if from
      Process.run!(args, label: "bsdkrun zfs branch").stdout.strip
    end

    # Attach +VOL+ or +VOL@BRANCH+ to a machine at +mount+ (+/mnt/VOL+, or
    # +/mnt/VOL-BRANCH+, by default). A pool is imported by one machine at a
    # time; a stopped machine picks it up on start, a running holder mounts
    # the branch live.
    # @return [String] the CLI's one-line report
    def attach(target, machine, mount: nil)
      args = ["zfs", "attach", target, machine]
      args.push("--mount", mount) if mount
      Process.run!(args, label: "bsdkrun zfs attach").stdout.strip
    end

    # Detach one branch (+VOL@BRANCH+) or the whole volume (+VOL+).
    # @return [String]
    def detach(target, machine)
      Process.run!(["zfs", "detach", target, machine], label: "bsdkrun zfs detach").stdout.strip
    end

    # Destroy branches (+VOL@BRANCH+) or whole volumes (+VOL+).
    # @param targets [String, Array<String>]
    # @param force [Boolean] destroy a volume a stopped machine still has attached
    # @return [void]
    def destroy(targets, force: false)
      args = ["zfs", "destroy"]
      args << "--force" if force
      args.concat(Array(targets))
      Process.run!(args, label: "bsdkrun zfs destroy")
      nil
    end
  end
end
