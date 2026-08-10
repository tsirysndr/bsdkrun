# frozen_string_literal: true

require "socket"
require "json"
require "digest/sha1"
require "base64"
require "bsdkrun"

# A from-scratch GraphQL-over-HTTP-and-websocket server for tests, built on a
# raw TCPServer — no WEBrick (not bundled in modern Ruby) and no gems, per
# this SDK's "runtime is stdlib only" constraint, which extends to how it is
# tested.
#
# One TCPServer accepts everything; each connection is classified by the
# presence of an `Upgrade: websocket` header, exactly like the real
# `bsdkrund` (`POST /graphql` and `/graphql/ws` share one bind address). This
# lets tests exercise `Bsdkrun::Client` against something that looks, from
# the wire, like the real daemon.
class FakeGraphQLServer
  GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

  attr_reader :port, :requests

  # @param http_handler [#call] called with (query, variables, headers),
  #   returns [status_line, body_hash] e.g. ["200 OK", {"data" => {...}}].
  # @param ws_handler [#call, nil] called with (client_socket, message_hash)
  #   for every parsed text frame received after the handshake.
  def initialize(http_handler:, ws_handler: nil)
    @server = TCPServer.new("127.0.0.1", 0)
    @port = @server.addr[1]
    @http_handler = http_handler
    @ws_handler = ws_handler
    @requests = Queue.new
    @threads = []
    @accept_thread = Thread.new { run }
  end

  # @return [String] a full GraphQL endpoint URL pointing at this server.
  def url
    "http://127.0.0.1:#{@port}/graphql"
  end

  # @return [void]
  def stop
    @server.close
  rescue StandardError
    nil
  ensure
    @accept_thread&.kill
    @threads.each(&:kill)
  end

  # Send a websocket text frame to a connected client socket (unmasked, as a
  # server frame must be).
  def self.send_json(client, obj)
    client.write(Bsdkrun::WebSocketFrame.build(opcode: :text, payload: JSON.generate(obj).b, mask: false))
  end

  private

  def run
    loop do
      client = @server.accept
      @threads << Thread.new { handle(client) }
    rescue IOError, Errno::EBADF
      break
    end
  end

  def handle(client)
    request_line = client.gets
    return client.close unless request_line

    headers = {}
    while (line = client.gets) && line.strip != ""
      k, v = line.split(":", 2)
      headers[k.strip.downcase] = v.strip if k && v
    end

    if headers["upgrade"]&.downcase == "websocket"
      handle_ws(client, headers)
    else
      handle_http(client, headers)
    end
  ensure
    client.close
  end

  def handle_http(client, headers)
    length = headers["content-length"].to_i
    raw = length.positive? ? client.read(length) : "{}"
    payload = JSON.parse(raw)
    @requests << { query: payload["query"], variables: payload["variables"] || {}, headers: headers }

    status, body = @http_handler.call(payload["query"], payload["variables"] || {}, headers)
    json = JSON.generate(body)
    client.write("HTTP/1.1 #{status}\r\n")
    client.write("content-type: application/json\r\n")
    client.write("Content-Length: #{json.bytesize}\r\n")
    client.write("Connection: close\r\n\r\n")
    client.write(json)
  end

  def handle_ws(client, headers)
    key = headers["sec-websocket-key"]
    accept = Base64.strict_encode64(Digest::SHA1.digest(key + GUID))
    client.write("HTTP/1.1 101 Switching Protocols\r\n")
    client.write("Upgrade: websocket\r\n")
    client.write("Connection: Upgrade\r\n")
    client.write("Sec-WebSocket-Accept: #{accept}\r\n\r\n")

    loop do
      _fin, opcode, payload = Bsdkrun::WebSocketFrame.read(client)
      case opcode
      when Bsdkrun::WebSocketFrame::OPCODES[:text]
        @ws_handler&.call(client, JSON.parse(payload))
      when Bsdkrun::WebSocketFrame::OPCODES[:close]
        break
      end
    end
  rescue EOFError, IOError, Errno::ECONNRESET
    nil
  end
end
