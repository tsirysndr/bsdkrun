# frozen_string_literal: true

module Bsdkrun
  # Base class for every error the SDK raises.
  class Error < StandardError; end

  # The +bsdkrun+ binary could not be located on the host.
  class BinaryNotFound < Error
    # @param searched [Array<String>] the paths that were probed, in order.
    def initialize(searched)
      super(
        'could not find the "bsdkrun" binary. Set BSDKRUN_BIN, add it to PATH, ' \
        "or set Bsdkrun.binary_path=. Looked in: #{Array(searched).join(', ')}"
      )
    end
  end

  # A +bsdkrun+ invocation exited non-zero.
  class CommandFailed < Error
    # @return [Integer] the process exit status.
    attr_reader :exit_code
    # @return [String] the captured standard output.
    attr_reader :stdout
    # @return [String] the captured standard error.
    attr_reader :stderr
    # @return [String] a human label for the command that failed.
    attr_reader :command

    # @param exit_code [Integer]
    # @param stdout [String]
    # @param stderr [String]
    # @param command [String] label describing the invocation.
    def initialize(exit_code:, stdout:, stderr:, command:)
      @exit_code = exit_code
      @stdout = stdout
      @stderr = stderr
      @command = command
      message = "command failed (exit #{exit_code}): #{command}"
      trimmed = stderr.to_s.strip
      message += "\n#{trimmed}" unless trimmed.empty?
      super(message)
    end
  end

  # No machine matched the given id / prefix.
  class SandboxNotFound < Error
    # @param id [String] the id or prefix that matched nothing.
    def initialize(id)
      super("no sandbox found matching id #{id.inspect}")
    end
  end

  # A GraphQL request to a remote +bsdkrund+ daemon failed — a transport
  # failure, a non-JSON response, or a +body["errors"]+ entry that was not an
  # auth failure. Mirrors +web/src/lib/graphql.ts+'s +GraphQLError+.
  class GraphQLError < Error
    # @return [String, nil] the error's +extensions.code+, when the daemon set one.
    attr_reader :code

    # @param message [String]
    # @param code [String, nil]
    def initialize(message, code = nil)
      super(message)
      @code = code
    end
  end

  # The daemon rejected our token — an HTTP 401, a GraphQL error whose
  # +extensions.code+ is +"UNAUTHENTICATED"+, or a websocket that closed
  # before +connection_ack+ ever arrived.
  class AuthError < GraphQLError
    # @param message [String]
    def initialize(message = "the daemon rejected this token")
      super(message, "UNAUTHENTICATED")
    end
  end
end
