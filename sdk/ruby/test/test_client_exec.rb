# frozen_string_literal: true

require "minitest/autorun"
require "base64"
require "bsdkrun"
require_relative "support/fake_graphql_server"

# `Client#exec`'s three-operation sequencing (daemon/README.md:112-137):
# openShell (HTTP mutation), THEN subscribe to shellOutput (WS), THEN wait
# for an exit code — closeShell always runs afterward, regardless of outcome.
class TestClientExec < Minitest::Test
  def build_server
    events = Queue.new
    session_id = "sess-1"

    http_handler = lambda do |query, variables, _headers|
      if query.include?("openShell")
        events << :open_shell
        assert_equal(["echo", "hi"], variables["c"])
        ["200 OK", { "data" => { "openShell" => { "id" => session_id } } }]
      elsif query.include?("closeShell")
        events << :close_shell
        assert_equal(session_id, variables["s"])
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
        events << :subscribe
        next unless msg.dig("payload", "variables", "s") == session_id

        chunk = Base64.strict_encode64("hello world\n")
        FakeGraphQLServer.send_json(
          client,
          { type: "next", id: msg["id"], payload: { data: { "shellOutput" => { "dataBase64" => chunk, "exitCode" => nil } } } }
        )
        FakeGraphQLServer.send_json(
          client,
          { type: "next", id: msg["id"], payload: { data: { "shellOutput" => { "dataBase64" => nil, "exitCode" => 0 } } } }
        )
      end
    end

    [FakeGraphQLServer.new(http_handler: http_handler, ws_handler: ws_handler), events]
  end

  def test_exec_runs_open_subscribe_wait_close_in_order_and_collects_output
    server, events = build_server
    client = Bsdkrun::Client.new(url: server.url, token: "tok")

    result = client.exec("m1", ["echo", "hi"])

    assert_kind_of(Bsdkrun::ExecResult, result)
    assert_equal(0, result.exit_code)
    assert_equal("hello world\n", result.output)

    order = []
    order << events.pop until events.empty?
    assert_equal(%i[open_shell subscribe close_shell], order)
  ensure
    server&.stop
  end

  def test_exec_closes_the_session_even_when_the_subscription_errors
    session_id = "sess-err"
    close_called = Queue.new

    http_handler = lambda do |query, variables, _h|
      if query.include?("openShell")
        ["200 OK", { "data" => { "openShell" => { "id" => session_id } } }]
      elsif query.include?("closeShell")
        close_called << variables["s"]
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
        FakeGraphQLServer.send_json(client, { type: "error", id: msg["id"], payload: [{ "message" => "session vanished" }] })
      end
    end

    server = FakeGraphQLServer.new(http_handler: http_handler, ws_handler: ws_handler)
    client = Bsdkrun::Client.new(url: server.url, token: "tok")

    err = assert_raises(Bsdkrun::GraphQLError) { client.exec("m1", ["true"]) }
    assert_match(/session vanished/, err.message)
    assert_equal(session_id, close_called.pop)
  ensure
    server&.stop
  end
end
