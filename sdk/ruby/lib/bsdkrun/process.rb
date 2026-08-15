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
    # @param binary [Boolean] keep stdout as bytes (ASCII-8BIT) instead of text.
    #   Needed by {FileSystem#read_file}: appending a chunk of arbitrary bytes to
    #   a UTF-8 buffer raises Encoding::CompatibilityError, so a PNG read back
    #   out of a guest would blow up mid-transfer rather than return.
    # @return [RawResult]
    def run(args, env: {}, stdin: nil, log_level: 0, on_stdout: nil, on_stderr: nil, binary: false)
      bin = Binary.resolve
      full = ["--log-level", log_level.to_s, *args]
      merged_env = env.to_h.transform_keys(&:to_s).transform_values(&:to_s)
      out = binary ? (+"").b : +""
      err = +""
      status = nil
      Open3.popen3(merged_env, bin, *full) do |child_in, child_out, child_err, wait|
        if binary
          child_in.binmode
          child_out.binmode
        end
        writer = Thread.new { child_in.write(stdin) if stdin; child_in.close }
        stdout_reader = Thread.new do
          while (chunk = child_out.readpartial(8192) rescue nil)
            out << chunk
            on_stdout&.call(chunk)
          end
        end
        stderr_reader = Thread.new do
          while (chunk = child_err.readpartial(8192) rescue nil)
            err << chunk
            on_stderr&.call(chunk)
          end
        end
        [writer, stdout_reader, stderr_reader].each(&:join)
        status = wait.value
      end
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
