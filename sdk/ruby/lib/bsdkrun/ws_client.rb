# frozen_string_literal: true

require "socket"
require "openssl"
require "uri"
require "json"
require "digest/sha1"
require "base64"

require_relative "websocket_frame"
require_relative "errors"

module Bsdkrun
  # A hand-rolled +graphql-transport-ws+ client (the graphql-ws project's
  # protocol; the negotiated subprotocol string is literally
  # +"graphql-transport-ws"+), because the Ruby standard library has no
  # WebSocket client.
  #
  # One socket per instance, opened lazily on the first {#subscribe} and
  # closed once the last subscription ends. A background reader +Thread+
  # pumps frames off the socket and dispatches to each subscription's
  # callbacks — this SDK is otherwise synchronous (see
  # {Bsdkrun::Sandbox}, which shells out via +Open3+), so the public surface
  # here stays callback-based and lets callers like {Bsdkrun::Client#exec}
  # block on a +Queue+ instead of needing their own event loop.
  #
  # Two mutexes, deliberately not one: +@state_mutex+ guards the small bits
  # of bookkeeping (+@subs+, +@pending+, +@acked+) so the reader thread and
  # callers never race on them, while +@write_mutex+ only serializes actual
  # socket writes. Holding one lock across a blocking socket write while the
  # other is needed just for a hash lookup would let a slow write stall
  # unrelated bookkeeping.
  class WsClient
    GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
    private_constant :GUID

    # @param ws_url [String] e.g. "ws://host:50052/graphql/ws".
    # @param token [String]
    def initialize(ws_url:, token:)
      @ws_url = ws_url
      @token = token

      @connect_mutex = Mutex.new
      @write_mutex = Mutex.new
      @state_mutex = Mutex.new

      @socket = nil
      @reader_thread = nil
      @acked = false
      @subs = {}
      @pending = []
      @next_id = 0
    end

    # Start a subscription.
    #
    # Connects (and sends +connection_init+) on the first call. If
    # +connection_ack+ has not arrived yet, the +subscribe+ message is queued
    # and flushed once it does — starting a subscription before the ack
    # would be sent to a daemon not yet ready to accept operations.
    #
    # @param query [String]
    # @param variables [Hash]
    # @param on_next [#call] invoked with the +data+ hash for each +next+.
    # @param on_error [#call, nil] invoked with an {Error} exception once.
    # @param on_complete [#call, nil] invoked with no args once, on a clean end.
    # @return [Proc] call to unsubscribe (sends +complete+, then closes the
    #   socket if this was the last subscription).
    def subscribe(query, variables, on_next:, on_error: nil, on_complete: nil)
      ensure_connected

      id = (@state_mutex.synchronize { @next_id += 1 }).to_s
      sub = {
        on_next: on_next,
        on_error: on_error || ->(_e) {},
        on_complete: on_complete || -> {}
      }
      start = lambda do
        send_json({ id: id, type: "subscribe", payload: { query: query, variables: variables } })
      end

      ready = @state_mutex.synchronize do
        @subs[id] = sub
        if @acked
          true
        else
          @pending << start
          false
        end
      end
      start.call if ready

      unsubscribed = false
      lambda do
        next if unsubscribed

        unsubscribed = true
        existed = @state_mutex.synchronize { !!@subs.delete(id) }
        next unless existed

        send_json({ id: id, type: "complete" }) if connected?
        close_if_idle
      end
    end

    # @return [Boolean] whether the socket is currently open.
    def connected?
      !@socket.nil?
    end

    # Close the socket immediately, regardless of open subscriptions. Used by
    # {Bsdkrun::Client} teardown paths; normal use tears itself down when the
    # last subscription unsubscribes.
    # @return [void]
    def close
      @connect_mutex.synchronize { close_socket }
    end

    private

    def ensure_connected
      @connect_mutex.synchronize do
        return if @socket

        uri = URI.parse(@ws_url)
        port = uri.port || (uri.scheme == "wss" ? 443 : 80)
        tcp = TCPSocket.new(uri.host, port)
        socket = uri.scheme == "wss" ? wrap_tls(tcp, uri.host) : tcp

        path = uri.path.to_s
        path = "/" if path.empty?
        path = "#{path}?#{uri.query}" if uri.query

        handshake(socket, uri.host, port, path)

        @socket = socket
        @state_mutex.synchronize do
          @acked = false
          @pending = []
        end
        start_reader
        send_json({ type: "connection_init", payload: { authorization: "Bearer #{@token}" } })
      end
    end

    def wrap_tls(tcp, hostname)
      ctx = OpenSSL::SSL::SSLContext.new
      ssl = OpenSSL::SSL::SSLSocket.new(tcp, ctx)
      ssl.hostname = hostname
      ssl.connect
      ssl
    end

    # The RFC 6455 opening handshake: an HTTP/1.1 Upgrade request, and the
    # server's +Sec-WebSocket-Accept+ verified against the client's key
    # (SHA-1 of key+GUID, base64-encoded — the one bit of cryptography this
    # protocol asks of a client).
    def handshake(socket, host, port, path)
      key = Base64.strict_encode64(Random.bytes(16))
      request = +"GET #{path} HTTP/1.1\r\n"
      request << "Host: #{host}:#{port}\r\n"
      request << "Upgrade: websocket\r\n"
      request << "Connection: Upgrade\r\n"
      request << "Sec-WebSocket-Key: #{key}\r\n"
      request << "Sec-WebSocket-Version: 13\r\n"
      request << "Sec-WebSocket-Protocol: graphql-transport-ws\r\n"
      request << "\r\n"
      socket.write(request)

      status_line = socket.gets
      unless status_line
        raise GraphQLError, "cannot reach the bsdkrun daemon at #{@ws_url} — connection closed during handshake"
      end
      unless status_line.include?(" 101 ")
        raise GraphQLError, "websocket handshake with #{@ws_url} failed: #{status_line.strip}"
      end

      headers = {}
      while (line = socket.gets) && line.strip != ""
        k, v = line.split(":", 2)
        headers[k.strip.downcase] = v.strip if k && v
      end

      expected = Base64.strict_encode64(Digest::SHA1.digest(key + GUID))
      return if headers["sec-websocket-accept"] == expected

      raise GraphQLError, "websocket handshake with #{@ws_url} failed: bad Sec-WebSocket-Accept"
    end

    def start_reader
      socket = @socket
      @reader_thread = Thread.new { read_loop(socket) }
      @reader_thread.report_on_exception = false
    end

    def read_loop(socket)
      loop do
        _fin, opcode, payload = WebSocketFrame.read(socket)
        case opcode
        when WebSocketFrame::OPCODES[:text]
          dispatch(payload)
        when WebSocketFrame::OPCODES[:ping]
          write_frame(opcode: :pong, payload: payload)
        when WebSocketFrame::OPCODES[:close]
          break
        else
          # binary / pong / continuation: nothing this protocol sends that we
          # need to act on. Fragmented text messages would arrive as
          # continuation frames, which is the documented unsupported case.
        end
      end
    rescue EOFError, IOError, Errno::ECONNRESET, OpenSSL::SSL::SSLError
      # The socket closed — handled uniformly below.
    ensure
      handle_disconnect(socket)
    end

    def dispatch(raw)
      msg = JSON.parse(raw)
      case msg["type"]
      when "connection_ack"
        flush_pending
      when "next"
        sub = @state_mutex.synchronize { @subs[msg["id"]] }
        sub&.[](:on_next)&.call(msg.dig("payload", "data"))
      when "error"
        sub = @state_mutex.synchronize { @subs.delete(msg["id"]) }
        sub&.[](:on_error)&.call(GraphQLError.new(error_detail(msg["payload"])))
        close_if_idle
      when "complete"
        sub = @state_mutex.synchronize { @subs.delete(msg["id"]) }
        sub&.[](:on_complete)&.call
        close_if_idle
      when "ping"
        send_json({ type: "pong" })
      end
    rescue JSON::ParserError
      nil
    end

    def error_detail(payload)
      if payload.is_a?(Array)
        payload.map { |e| e["message"] }.compact.join("; ")
      else
        payload.to_s
      end
    end

    def flush_pending
      queued = @state_mutex.synchronize do
        @acked = true
        q = @pending
        @pending = []
        q
      end
      queued.each(&:call)
    end

    # The reader thread's terminal state: fan out a close reason to every
    # still-open subscription and drop the socket. Whether a +connection_ack+
    # was ever received decides the reason — an unacked close means the
    # daemon rejected our token; an acked close is just "the connection
    # dropped."
    def handle_disconnect(socket)
      acked, subs = @state_mutex.synchronize do
        next [@acked, {}] unless @socket.equal?(socket)

        result = [@acked, @subs.dup]
        @subs.clear
        @pending = []
        @acked = false
        result
      end

      @connect_mutex.synchronize { close_socket if @socket.equal?(socket) }

      return if subs.empty?

      err = acked ? GraphQLError.new("the connection to the daemon was closed") : AuthError.new
      subs.each_value { |sub| sub[:on_error]&.call(err) }
    end

    def close_if_idle
      close if @state_mutex.synchronize { @subs.empty? }
    end

    def close_socket
      return unless @socket

      socket = @socket
      @socket = nil
      begin
        @write_mutex.synchronize { socket.write(WebSocketFrame.build(opcode: :close, payload: "".b)) }
      rescue StandardError
        nil
      end
      begin
        socket.close
      rescue StandardError
        nil
      end
    end

    def send_json(obj)
      write_frame(opcode: :text, payload: JSON.generate(obj).b)
    end

    def write_frame(opcode:, payload:)
      frame = WebSocketFrame.build(opcode: opcode, payload: payload)
      @write_mutex.synchronize { @socket&.write(frame) }
    rescue StandardError
      nil
    end
  end
end
