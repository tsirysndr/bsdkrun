# frozen_string_literal: true

require "minitest/autorun"
require "base64"
require "bsdkrun"
require_relative "support/fake_graphql_server"

# `Client#shell`: a live ShellSession whose on_output/on_exit callbacks fire
# from the background reader thread, and whose write/resize/close send the
# mutations daemon/README.md:118-124 documents.
class TestClientShell < Minitest::Test
  def test_shell_delivers_output_and_exit_then_write_resize_close_hit_the_wire
    session_id = "sess-live"
    mutations = Queue.new
    ws_client_socket = nil
    subscribe_id = nil

    http_handler = lambda do |query, variables, _h|
      case query
      when /openShell/
        ["200 OK", { "data" => { "openShell" => { "id" => session_id } } }]
      when /sendShellInput/
        mutations << [:write, variables["d"]]
        ["200 OK", { "data" => { "sendShellInput" => true } }]
      when /resizeShell/
        mutations << [:resize, variables["r"], variables["c"]]
        ["200 OK", { "data" => { "resizeShell" => true } }]
      when /closeShell/
        mutations << [:close, variables["s"]]
        ["200 OK", { "data" => { "closeShell" => true } }]
      else
        ["200 OK", { "data" => {} }]
      end
    end

    ws_handler = lambda do |client, msg|
      case msg["type"]
      when "connection_init"
        FakeGraphQLServer.send_json(client, { type: "connection_ack" })
      when "subscribe"
        ws_client_socket = client
        subscribe_id = msg["id"]
        chunk = Base64.strict_encode64("$ ")
        FakeGraphQLServer.send_json(
          client,
          { type: "next", id: msg["id"],
            payload: { data: { "shellOutput" => { "dataBase64" => chunk, "exitCode" => nil } } } }
        )
      end
    end

    server = FakeGraphQLServer.new(http_handler: http_handler, ws_handler: ws_handler)
    client = Bsdkrun::Client.new(url: server.url, token: "tok")

    output_chunks = Queue.new
    exit_codes = Queue.new

    session = client.shell("m1", rows: 40, cols: 120)
    assert_equal(session_id, session.id)
    session.on_output { |bytes| output_chunks << bytes }
    session.on_exit { |code| exit_codes << code }

    assert_equal("$ ", output_chunks.pop)

    session.write("ls\n")
    assert_equal([:write, Base64.strict_encode64("ls\n")], mutations.pop)

    session.resize(50, 100)
    assert_equal([:resize, 50, 100], mutations.pop)

    # Simulate the guest command exiting.
    FakeGraphQLServer.send_json(
      ws_client_socket,
      { type: "next", id: subscribe_id, payload: { data: { "shellOutput" => { "dataBase64" => nil, "exitCode" => 0 } } } }
    )
    assert_equal(0, exit_codes.pop)

    session.close
    assert_equal([:close, session_id], mutations.pop)
  ensure
    server&.stop
  end
end
