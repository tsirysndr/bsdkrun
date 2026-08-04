# frozen_string_literal: true

module Bsdkrun
  # Host-level toolchain / image operations.
  module System
    module_function

    # Sanity-check the toolchain: verify libkrun links and a context can be
    # created/configured. Does not boot.
    # @return [Boolean] true on success.
    def probe
      Process.run(["probe"]).exit_code.zero?
    end

    # Download + prepare a BSD image ahead of time.
    #
    # @param os [String] "freebsd" or "netbsd".
    # @param version [String, nil]
    # @param force [Boolean] re-download even if cached.
    # @return [String] the command output.
    def fetch_image(os, version: nil, force: false)
      args = ["fetch", "--os", os.to_s]
      args.push("--version", version.to_s) if version
      args << "--force" if force
      Process.run!(args, label: "bsdkrun fetch").stdout
    end

    # List the builds available to fetch for a BSD, one version per line.
    #
    # @param os [String] "freebsd" or "netbsd".
    # @return [Array<String>] version strings (non-empty output lines).
    def versions(os)
      res = Process.run!(["versions", "--os", os.to_s], label: "bsdkrun versions")
      res.stdout.split("\n").map(&:strip).reject(&:empty?)
    end

    # Grow a raw disk image (the guest expands its root FS on next boot).
    #
    # @param disk [String]
    # @param size [String]
    # @return [void]
    def grow_disk(disk, size)
      Process.run!(["grow", "--disk", disk, "--size", size], label: "bsdkrun grow")
      nil
    end
  end
end
