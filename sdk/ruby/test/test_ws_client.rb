# frozen_string_literal: true

require "minitest/autorun"
require "bsdkrun"
require_relative "support/fake_graphql_server"

# End-to-end tests for the hand-rolled `graphql-transport-ws` client, against
# a real TCP server speaking the protocol (see support/fake_graphql_server.rb
# — a natural byproduct of writing the same handshake/framing code for the
# client, so a matching test-only server was cheap to add). Frame-level
# parsing/masking is unit-tested separately in test_websocket_frame.rb; this
# file is about the protocol state machine: init/ack gating, next/error/
# complete routing, ping/pong, and close semantics.
class TestWsClient < Minitest::Test
  def start_server(&ws_handler)
    FakeGraphQLServer.new(
      http_handler: ->(_q, _v, _h) { ["200 OK", { "data" => {} }] },
      ws_handler: ws_handler
    )
  end

  def test_connection_init_carries_the_bearer_token_and_ack_unblocks_subscribe
    received_init = Queue.new
    server = start_server do |client, msg|
      case msg["type"]
      when "connection_init"
        received_init << msg
        FakeGraphQLServer.send_json(client, { type: "connection_ack" })
      when "subscribe"
        FakeGraphQLServer.send_json(client, { type: "next", id: msg["id"], payload: { data: { hello: "world" } } })
        FakeGraphQLServer.send_json(client, { type: "complete", id: msg["id"] })
      end
    end

    ws = Bsdkrun::WsClient.new(ws_url: "ws://127.0.0.1:#{server.port}/graphql/ws", token: "secret-token")
    results = Queue.new
    ws.subscribe("subscription{hello}", {}, on_next: ->(d) { results << [:next, d] },
                                             on_complete: -> { results << [:complete] })

    init_msg = received_init.pop
    assert_equal({ "authorization" => "Bearer secret-token" }, init_msg["payload"])

    assert_equal([:next, { "hello" => "world" }], results.pop)
    assert_equal([:complete], results.pop)
  ensure
    ws&.close
    server&.stop
  end

  def test_subscribe_before_ack_is_queued_and_flushed_on_ack
    subscribe_received_at = Queue.new
    server = start_server do |client, msg|
      case msg["type"]
      when "connection_init"
        # Delay the ack so a subscribe() called immediately after connecting
        # is guaranteed to still be pending when this fires.
        Thread.new do
          sleep 0.15
          FakeGraphQLServer.send_json(client, { type: "connection_ack" })
        end
      when "subscribe"
        subscribe_received_at << Time.now
        FakeGraphQLServer.send_json(client, { type: "complete", id: msg["id"] })
      end
    end

    ws = Bsdkrun::WsClient.new(ws_url: "ws://127.0.0.1:#{server.port}/graphql/ws", token: "t")
    started_at = Time.now
    done = Queue.new
    ws.subscribe("subscription{x}", {}, on_next: ->(_d) {}, on_complete: -> { done << true })

    done.pop
    elapsed = subscribe_received_at.pop - started_at
    assert_operator elapsed, :>=, 0.1, "the daemon should not see `subscribe` before it acked connection_init"
  ensure
    ws&.close
    server&.stop
  end

  def test_error_message_routes_to_on_error_and_removes_the_subscription
    server = start_server do |client, msg|
      case msg["type"]
      when "connection_init"
        FakeGraphQLServer.send_json(client, { type: "connection_ack" })
      when "subscribe"
        FakeGraphQLServer.send_json(client, { type: "error", id: msg["id"], payload: [{ "message" => "boom" }] })
      end
    end

    ws = Bsdkrun::WsClient.new(ws_url: "ws://127.0.0.1:#{server.port}/graphql/ws", token: "t")
    errors = Queue.new
    ws.subscribe("subscription{x}", {}, on_next: ->(_d) {}, on_error: ->(e) { errors << e })

    err = errors.pop
    assert_kind_of(Bsdkrun::GraphQLError, err)
    assert_equal("boom", err.message)
  ensure
    ws&.close
    server&.stop
  end

  def test_ping_is_answered_with_pong
    pong_received = Queue.new
    server = start_server do |client, msg|
      case msg["type"]
      when "connection_init"
        FakeGraphQLServer.send_json(client, { type: "connection_ack" })
        FakeGraphQLServer.send_json(client, { type: "ping" })
      when "pong"
        pong_received << true
      end
    end

    ws = Bsdkrun::WsClient.new(ws_url: "ws://127.0.0.1:#{server.port}/graphql/ws", token: "t")
    ws.subscribe("subscription{x}", {}, on_next: ->(_d) {})

    assert pong_received.pop
  ensure
    ws&.close
    server&.stop
  end

  def test_unsubscribe_sends_complete_and_closes_socket_when_idle
    complete_received = Queue.new
    server = start_server do |client, msg|
      case msg["type"]
      when "connection_init"
        FakeGraphQLServer.send_json(client, { type: "connection_ack" })
      when "complete"
        complete_received << msg["id"]
      end
    end

    ws = Bsdkrun::WsClient.new(ws_url: "ws://127.0.0.1:#{server.port}/graphql/ws", token: "t")
    unsubscribe = ws.subscribe("subscription{x}", {}, on_next: ->(_d) {})
    sleep 0.05 # let connection_ack land so the subscribe message actually went out
    unsubscribe.call

    assert complete_received.pop
    sleep 0.05
    refute ws.connected?, "the socket should close once the last subscription ends"
  ensure
    server&.stop
  end

  def test_close_before_ack_delivers_auth_error
    server = start_server do |client, msg|
      client.close if msg["type"] == "connection_init" # never ack; just hang up
    end

    ws = Bsdkrun::WsClient.new(ws_url: "ws://127.0.0.1:#{server.port}/graphql/ws", token: "t")
    errors = Queue.new
    ws.subscribe("subscription{x}", {}, on_next: ->(_d) {}, on_error: ->(e) { errors << e })

    err = errors.pop
    assert_kind_of(Bsdkrun::AuthError, err)
  ensure
    ws&.close
    server&.stop
  end

  def test_close_after_ack_delivers_generic_graphql_error
    server = start_server do |client, msg|
      if msg["type"] == "connection_init"
        FakeGraphQLServer.send_json(client, { type: "connection_ack" })
      elsif msg["type"] == "subscribe"
        client.close # drop the connection mid-subscription, after having acked
      end
    end

    ws = Bsdkrun::WsClient.new(ws_url: "ws://127.0.0.1:#{server.port}/graphql/ws", token: "t")
    errors = Queue.new
    ws.subscribe("subscription{x}", {}, on_next: ->(_d) {}, on_error: ->(e) { errors << e })

    err = errors.pop
    assert_kind_of(Bsdkrun::GraphQLError, err)
    refute_kind_of(Bsdkrun::AuthError, err)
    assert_match(/closed/, err.message)
  ensure
    ws&.close
    server&.stop
  end
end
