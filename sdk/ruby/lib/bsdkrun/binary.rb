# frozen_string_literal: true

require "pathname"

module Bsdkrun
  # Locates the +bsdkrun+ binary on the host and caches the result.
  #
  # Resolution order (first match wins):
  #   1. an explicit override set via {Bsdkrun.binary_path=}
  #   2. the +BSDKRUN_BIN+ environment variable
  #   3. +bsdkrun+ on +PATH+
  #   4. an in-repo dev build: +<repo>/target/release/bsdkrun+ then +debug+
  module Binary
    @override = nil
    @resolved = nil

    class << self
      # Force the SDK to use a specific +bsdkrun+ binary, bypassing discovery.
      #
      # @param path [String, nil]
      # @return [void]
      def override=(path)
        @override = path
        @resolved = nil
      end

      # @return [String, nil] the current explicit override, if any.
      attr_reader :override

      # Reset cached discovery state (mainly for tests).
      # @return [void]
      def reset!
        @resolved = nil
        @override = nil
      end

      # Resolve (and cache) the path to the +bsdkrun+ binary.
      #
      # @return [String] the resolved absolute path or bare command name.
      # @raise [BinaryNotFound] when nothing matched.
      def resolve
        return @resolved if @resolved

        searched = candidates
        searched.each do |candidate|
          if path_like?(candidate)
            return @resolved = candidate if File.exist?(candidate)
          else
            found = which(candidate)
            return @resolved = found if found
          end
        end
        raise BinaryNotFound, searched
      end

      private

      def path_like?(candidate)
        candidate.include?(File::SEPARATOR) ||
          (File::ALT_SEPARATOR && candidate.include?(File::ALT_SEPARATOR))
      end

      # Candidate locations, in priority order.
      def candidates
        out = []
        out << @override if @override

        env = ENV["BSDKRUN_BIN"]
        out << env if env && !env.empty?

        # A `bsdkrun` already on PATH wins over in-repo builds.
        on_path = which("bsdkrun")
        out << on_path if on_path

        # This file lives at sdk/ruby/lib/bsdkrun/binary.rb, so the repo root is
        # four levels up. Prefer a release build, fall back to debug.
        repo_root = File.expand_path("../../../..", __dir__)
        out << File.join(repo_root, "target", "release", "bsdkrun")
        out << File.join(repo_root, "target", "debug", "bsdkrun")
        out
      end

      # Cross-platform PATH lookup for an executable.
      def which(name)
        exts =
          if File::ALT_SEPARATOR
            (ENV["PATHEXT"] || ".EXE;.CMD;.BAT").split(";")
          else
            [""]
          end
        (ENV["PATH"] || "").split(File::PATH_SEPARATOR).each do |dir|
          next if dir.empty?

          exts.each do |ext|
            full = File.join(dir, name + ext)
            return full if File.file?(full) && File.executable?(full)
          end
        end
        nil
      end
    end
  end
end
