# frozen_string_literal: true

require "json"

module Bsdkrun
  # Host-level image operations.
  module Images
    module_function

    # List downloaded images (pulled OCI images + fetched BSD images).
    # @return [Array<ImageInfo>]
    def list
      res = Process.run!(["images", "--json"], label: "bsdkrun images")
      rows = JSON.parse(res.stdout.empty? ? "[]" : res.stdout)
      rows.map { |row| ImageInfo.from_row(row) }
    end
  end
end
