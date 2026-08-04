# frozen_string_literal: true

require "json"

module Bsdkrun
  # Global-network operations — shared subnets where members reach each other by
  # IP and by name.
  module Networks
    module_function

    # List global networks and their member counts.
    # @return [Array<NetworkInfo>]
    def list
      res = Process.run!(["network", "ls", "--json"], label: "bsdkrun network ls")
      rows = JSON.parse(res.stdout.empty? ? "[]" : res.stdout)
      rows.map { |row| NetworkInfo.from_row(row) }
    end

    # Create a global network (starts its shared switch).
    # @param name [String]
    # @return [void]
    def create(name)
      Process.run!(["network", "create", name], label: "bsdkrun network create")
      nil
    end

    # Remove one or more networks. +force+ allows removal with running members.
    # @param names [String, Array<String>]
    # @param force [Boolean]
    # @return [void]
    def remove(names, force: false)
      args = ["network", "rm"]
      args << "--force" if force
      args.concat(Array(names))
      Process.run!(args, label: "bsdkrun network rm")
      nil
    end

    # Join or switch a machine to a network. Applies on the machine's next start.
    # @param machine [String] id or name.
    # @param network [String]
    # @return [void]
    def connect(machine, network)
      Process.run!(["network", "connect", machine, network], label: "bsdkrun network connect")
      nil
    end

    # Detach a machine from its network. Applies on its next start.
    # @param machine [String]
    # @return [void]
    def disconnect(machine)
      Process.run!(["network", "disconnect", machine], label: "bsdkrun network disconnect")
      nil
    end

    # Refresh members' +/etc/hosts+ so peers resolve by name (notably NetBSD).
    # @param network [String]
    # @return [void]
    def sync(network)
      Process.run!(["network", "sync", network], label: "bsdkrun network sync")
      nil
    end

    # The machines currently attached to +network+ (running or stopped).
    # @param network [String]
    # @return [Array<SandboxInfo>]
    def members(network)
      Sandbox.list(all: true).select { |m| m.network == network }
    end
  end
end
