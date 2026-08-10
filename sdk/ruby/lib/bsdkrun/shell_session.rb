# frozen_string_literal: true

require "base64"

module Bsdkrun
  # A live interactive session opened by {Bsdkrun::Client#shell}.
  #
  # Output arrives on the {Bsdkrun::WsClient}'s background reader thread, so
  # {#on_output} / {#on_exit} callbacks run on that thread, not the caller's —
  # the same tradeoff +web/src/lib/graphql.ts+ makes with its +onNext+
  # handlers, just single-threaded there because JS has no threads to worry
  # about.
  class ShellSession
    # @return [String] the session id (+openShell+'s return value).
    attr_reader :id

    # @!visibility private
    #
    # Constructed by {Bsdkrun::Client#shell} — not meant to be built directly.
    # +unsubscribe+ is assigned right after construction, before the
    # +shellOutput+ subscription can possibly deliver anything, so callbacks
    # closing over +self+ never observe it unset.
    def initialize(client:, id:)
      @client = client
      @id = id
      @unsubscribe = nil
      @callback_mutex = Mutex.new
      @on_output = nil
      @on_exit = nil
      # The shellOutput subscription starts synchronously inside
      # Client#shell, on the WsClient's background reader thread — a real,
      # genuinely concurrent thread here (unlike JS's single-threaded event
      # loop), so output/exit can arrive before the caller gets around to
      # calling #on_output/#on_exit. The daemon buffers from the moment the
      # session opens specifically so nothing is lost in that window (see
      # daemon/README.md); buffer here too so a late registration still sees
      # everything, exactly like write-side of this same race in the other
      # SDKs' shell()/ShellSession.
      @buffered_output = []
      @exit_delivered = false
      @buffered_exit = nil
    end

    # @!visibility private
    def unsubscribe=(proc)
      @unsubscribe = proc
    end

    # Register a callback for output chunks (already base64-decoded). Any
    # output that arrived before this was called is replayed immediately.
    # @yieldparam bytes [String] binary-safe.
    # @return [void]
    def on_output(&block)
      buffered = @callback_mutex.synchronize do
        @on_output = block
        buf = @buffered_output
        @buffered_output = []
        buf
      end
      buffered.each { |bytes| block.call(bytes) }
    end

    # Register a callback for the session's exit code. Fires once — right
    # away if the session had already exited before this was called.
    # @yieldparam exit_code [Integer, nil] nil if the session ended without
    #   ever reporting one (e.g. the connection dropped).
    # @return [void]
    def on_exit(&block)
      already_exited, code = @callback_mutex.synchronize do
        @on_exit = block
        [@exit_delivered, @buffered_exit]
      end
      block.call(code) if already_exited
    end

    # Send keystrokes / input bytes.
    # @param data [String]
    # @return [void]
    def write(data)
      @client.request(
        "mutation($s:String!,$d:String!){ sendShellInput(sessionId:$s, dataBase64:$d) }",
        { s: @id, d: Base64.strict_encode64(data.to_s.b) }
      )
      nil
    end

    # Resize the pseudo-terminal.
    # @param rows [Integer]
    # @param cols [Integer]
    # @return [void]
    def resize(rows, cols)
      @client.request(
        "mutation($s:String!,$r:Int!,$c:Int!){ resizeShell(sessionId:$s, rows:$r, cols:$c) }",
        { s: @id, r: rows, c: cols }
      )
      nil
    end

    # Unsubscribe and close the session. Idempotent on the wire (+closeShell+
    # is safe to call on an already-closed session) — a request failure here
    # (already gone, machine removed, etc.) is swallowed rather than raised,
    # matching the other SDKs' `close`/`closeShell` handling.
    # @return [void]
    def close
      @unsubscribe&.call
      begin
        @client.request("mutation($s:String!){ closeShell(sessionId:$s) }", { s: @id })
      rescue GraphQLError
        nil
      end
      nil
    end

    # @!visibility private — invoked from the {WsClient} reader thread.
    def deliver_output(bytes)
      cb = @callback_mutex.synchronize do
        cb = @on_output
        @buffered_output << bytes if cb.nil?
        cb
      end
      cb&.call(bytes)
    end

    # @!visibility private — invoked from the {WsClient} reader thread. Only
    # the first call has any effect, whether that arrives as a real exit code
    # or (from {Bsdkrun::Client#shell}'s `on_error`) as +nil+ for a dropped
    # connection.
    def deliver_exit(exit_code)
      cb = @callback_mutex.synchronize do
        next nil if @exit_delivered

        @exit_delivered = true
        @buffered_exit = exit_code
        @on_exit
      end
      cb&.call(exit_code)
    end
  end
end
