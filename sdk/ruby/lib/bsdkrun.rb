# frozen_string_literal: true

require_relative "bsdkrun/version"
require_relative "bsdkrun/errors"
require_relative "bsdkrun/binary"
require_relative "bsdkrun/process"
require_relative "bsdkrun/args"
require_relative "bsdkrun/cache"
require_relative "bsdkrun/filesystem"
require_relative "bsdkrun/types"
require_relative "bsdkrun/sandbox"
require_relative "bsdkrun/images"
require_relative "bsdkrun/volumes"
require_relative "bsdkrun/networks"
require_relative "bsdkrun/system"
require_relative "bsdkrun/websocket_frame"
require_relative "bsdkrun/ws_client"
require_relative "bsdkrun/shell_session"
require_relative "bsdkrun/client"

# bsdkrun — a Ruby SDK for {https://github.com/tsirysndr/bsdkrun bsdkrun}, a
# Firecracker-style microVM launcher for BSD, Linux, and unikernel guests. A thin wrapper
# around the +bsdkrun+ CLI: it builds argv, shells out, and parses JSON output.
#
# @example
#   require "bsdkrun"
#
#   sbx = Bsdkrun::Sandbox.create(os: "linux", image: "alpine")
#   puts sbx.exec(["uname", "-a"]).text
#   sbx.stop
module Bsdkrun
  class << self
    # Force the SDK to use a specific +bsdkrun+ binary, bypassing discovery.
    # @param path [String, nil]
    # @return [void]
    def binary_path=(path)
      Binary.override = path
    end

    # @return [String] the resolved +bsdkrun+ binary path.
    # @raise [BinaryNotFound]
    def binary_path
      Binary.resolve
    end

    # Reset cached binary discovery state (mainly for tests).
    # @return [void]
    def reset_binary!
      Binary.reset!
    end

    # Convenience alias for {Sandbox.create}.
    # @return [Sandbox]
    def create(opts = {}, **kwargs)
      Sandbox.create(opts, **kwargs)
    end

    # Convenience alias for {Sandbox.get}.
    # @return [Sandbox]
    def get(id)
      Sandbox.get(id)
    end

    # Convenience alias for {Sandbox.list}.
    # @return [Array<SandboxInfo>]
    def list(all: false)
      Sandbox.list(all: all)
    end

    # The {Images} namespace.
    # @return [Module]
    def images
      Images
    end

    # The {Volumes} namespace.
    # @return [Module]
    def volumes
      Volumes
    end

    # The {Networks} namespace.
    # @return [Module]
    def networks
      Networks
    end

    # The {System} namespace.
    # @return [Module]
    def system
      System
    end
  end
end
