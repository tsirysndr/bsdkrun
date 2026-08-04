# frozen_string_literal: true

require "open3"

module Bsdkrun
  # Spawns the +bsdkrun+ CLI and captures its output.
  #
  # Every invocation is prefixed with +--log-level+ (default 0) so the SDK's
  # captured output stays clean.
  module Process
    # Captured output of a raw CLI invocation.
    #
    # @!attribute [r] stdout
    #   @return [String]
    # @!attribute [r] stderr
    #   @return [String]
    # @!attribute [r] exit_code
    #   @return [Integer]
    RawResult = Struct.new(:stdout, :stderr, :exit_code, keyword_init: true)

    module_function

    # Run +bsdkrun --log-level <n> <args>+ to completion, buffering output.
    #
    # @param args [Array<String>] CLI arguments (without the binary).
    # @param env [Hash] extra environment merged onto the process env.
    # @param stdin [String, nil] data piped to the child's stdin.
    # @param log_level [Integer] bsdkrun global log level (0=off .. 5=trace).
    # @return [RawResult]
    def run(args, env: {}, stdin: nil, log_level: 0)
      bin = Binary.resolve
      full = ["--log-level", log_level.to_s, *args]
      merged_env = env.to_h.transform_keys(&:to_s).transform_values(&:to_s)
      out, err, status = Open3.capture3(merged_env, bin, *full, stdin_data: stdin || "")
      RawResult.new(stdout: out, stderr: err, exit_code: status.exitstatus || 0)
    end

    # Run and raise {CommandFailed} on a non-zero exit.
    #
    # @param args [Array<String>]
    # @param label [String] human label used in the error.
    # @return [RawResult]
    # @raise [CommandFailed]
    def run!(args, label:, **opts)
      res = run(args, **opts)
      unless res.exit_code.zero?
        raise CommandFailed.new(
          exit_code: res.exit_code, stdout: res.stdout, stderr: res.stderr, command: label
        )
      end
      res
    end

    # Spawn an interactive +bsdkrun+ command inheriting the parent's stdio and
    # wait for it (for +shell+). Returns the child's exit status boolean.
    #
    # @param args [Array<String>]
    # @param log_level [Integer]
    # @return [Boolean] true if the command exited zero.
    def spawn_interactive(args, log_level: 0)
      bin = Binary.resolve
      full = ["--log-level", log_level.to_s, *args]
      system(bin, *full)
    end
  end
end
