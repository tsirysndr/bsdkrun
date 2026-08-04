# frozen_string_literal: true

require "json"

module Bsdkrun
  # Host-level volume operations.
  module Volumes
    module_function

    # List persistent volumes.
    # @return [Array<VolumeInfo>]
    def list
      res = Process.run!(["volume", "ls", "--json"], label: "bsdkrun volume ls")
      rows = JSON.parse(res.stdout.empty? ? "[]" : res.stdout)
      rows.map { |row| VolumeInfo.from_row(row) }
    end

    # Remove one or more volumes (and their data).
    #
    # @param names [String, Array<String>]
    # @param force [Boolean]
    # @return [void]
    def remove(names, force: false)
      args = ["volume", "rm"]
      args << "--force" if force
      args.concat(Array(names))
      Process.run!(args, label: "bsdkrun volume rm")
      nil
    end
  end
end
