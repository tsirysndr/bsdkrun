# frozen_string_literal: true

require "minitest/autorun"
require "bsdkrun"
require_relative "support/fake_graphql_server"

# The HTTP transport (queries/mutations): URL normalization, config
# resolution, and the response -> exception mapping. `WsClient`/subscriptions
# are covered separately in test_ws_client.rb.
class TestClientHttp < Minitest::Test
  def teardown
    ENV.delete("BSDKRUN_URL")
    ENV.delete("BSDKRUN_TOKEN")
  end

  # ---- URL normalization ----------------------------------------------------

  def test_normalize_url_adds_scheme_and_suffix
    assert_equal("http://localhost:50052/graphql", Bsdkrun::Client.normalize_url("localhost:50052"))
  end

  def test_normalize_url_strips_trailing_slashes
    assert_equal("http://host:50052/graphql", Bsdkrun::Client.normalize_url("http://host:50052///"))
  end

  def test_normalize_url_does_not_double_append_graphql
    assert_equal("https://host:50052/graphql", Bsdkrun::Client.normalize_url("https://host:50052/graphql/"))
  end

  def test_normalize_url_preserves_https
    assert_equal("https://host/graphql", Bsdkrun::Client.normalize_url("https://host"))
  end

  def test_normalize_url_trims_whitespace
    assert_equal("http://host/graphql", Bsdkrun::Client.normalize_url("  host  "))
  end

  # ---- from_env ---------------------------------------------------------

  def test_from_env_raises_without_url
    ENV.delete("BSDKRUN_URL")
    err = assert_raises(Bsdkrun::Error) { Bsdkrun::Client.from_env }
    assert_match(/BSDKRUN_URL/, err.message)
  end

  def test_from_env_raises_with_url_but_no_token
    ENV["BSDKRUN_URL"] = "http://host:50052"
    ENV.delete("BSDKRUN_TOKEN")
    err = assert_raises(Bsdkrun::Error) { Bsdkrun::Client.from_env }
    assert_match(/BSDKRUN_TOKEN/, err.message)
  end

  def test_from_env_builds_a_client
    ENV["BSDKRUN_URL"] = "host:50052"
    ENV["BSDKRUN_TOKEN"] = "tok"
    client = Bsdkrun::Client.from_env
    assert_equal("http://host:50052/graphql", client.url)
  end

  # ---- request() error mapping -------------------------------------------

  def with_server(http_handler)
    server = FakeGraphQLServer.new(http_handler: http_handler)
    yield server
  ensure
    server&.stop
  end

  def test_request_returns_data_on_success
    with_server(lambda { |_q, _v, _h|
      ["200 OK", { "data" => { "machines" => [] } }]
    }) do |server|
      client = Bsdkrun::Client.new(url: server.url, token: "tok")
      data = client.request("{ machines { id } }")
      assert_equal({ "machines" => [] }, data)
    end
  end

  def test_request_sends_bearer_header_and_json_body
    with_server(lambda { |q, v, h|
      assert_equal("Bearer tok", h["authorization"])
      assert_equal("application/json", h["content-type"])
      assert_equal("{ x }", q)
      assert_equal({ "a" => 1 }, v)
      ["200 OK", { "data" => { "ok" => true } }]
    }) do |server|
      client = Bsdkrun::Client.new(url: server.url, token: "tok")
      client.request("{ x }", { a: 1 })
    end
  end

  def test_request_401_raises_auth_error
    with_server(->(_q, _v, _h) { ["401 Unauthorized", { "errors" => [{ "message" => "nope" }] }] }) do |server|
      client = Bsdkrun::Client.new(url: server.url, token: "bad")
      assert_raises(Bsdkrun::AuthError) { client.request("{ x }") }
    end
  end

  def test_request_unauthenticated_extension_raises_auth_error
    with_server(lambda { |_q, _v, _h|
      ["200 OK", { "errors" => [{ "message" => "bad token", "extensions" => { "code" => "UNAUTHENTICATED" } }] }]
    }) do |server|
      client = Bsdkrun::Client.new(url: server.url, token: "bad")
      err = assert_raises(Bsdkrun::AuthError) { client.request("{ x }") }
      assert_equal("bad token", err.message)
    end
  end

  def test_request_other_graphql_error_raises_graphql_error_with_code
    with_server(lambda { |_q, _v, _h|
      ["200 OK", { "errors" => [{ "message" => "bad argument", "extensions" => { "code" => "INVALID_ARGUMENT" } }] }]
    }) do |server|
      client = Bsdkrun::Client.new(url: server.url, token: "tok")
      err = assert_raises(Bsdkrun::GraphQLError) { client.request("{ x }") }
      refute_kind_of(Bsdkrun::AuthError, err)
      assert_equal("bad argument", err.message)
      assert_equal("INVALID_ARGUMENT", err.code)
    end
  end

  def test_request_unreachable_daemon_raises_graphql_error
    # Nothing listening on this port.
    client = Bsdkrun::Client.new(url: "http://127.0.0.1:1/graphql", token: "tok")
    err = assert_raises(Bsdkrun::GraphQLError) { client.request("{ x }") }
    assert_match(/cannot reach the bsdkrun daemon at/, err.message)
    assert_match(%r{http://127\.0\.0\.1:1/graphql}, err.message)
  end

  # ---- typed methods: field-name wiring ----------------------------------

  def test_list_maps_graphql_machine_into_sandbox_info
    machine = {
      "id" => "abc123", "name" => "api", "image" => "alpine", "kind" => "linux",
      "command" => "sleep 300", "status" => "running", "running" => true,
      "exitCode" => nil, "pid" => 42, "detached" => true, "cpus" => 2, "mem" => 512,
      "volume" => nil, "stateDir" => "/s", "network" => "devnet", "netIp" => "10.0.0.2",
      "createdAt" => "1700000000", "finishedAt" => nil,
      "ports" => [{ "bind" => "127.0.0.1", "host" => 8080, "guest" => 80 }]
    }
    with_server(lambda { |q, v, _h|
      assert_match(/machines\(all:\$all\)/, q)
      assert_equal({ "all" => true }, v)
      ["200 OK", { "data" => { "machines" => [machine] } }]
    }) do |server|
      client = Bsdkrun::Client.new(url: server.url, token: "tok")
      list = client.list(all: true)
      assert_equal(1, list.length)
      info = list.first
      assert_kind_of(Bsdkrun::SandboxInfo, info)
      assert_equal("abc123", info.id)
      assert_equal(42, info.pid)
      assert_equal("running", info.status)
      assert_equal(1700_000_000, info.created_at)
      assert_nil info.finished_at
      assert_equal(1, info.ports.length)
      assert_equal(8080, info.ports.first.host)
    end
  end

  def test_get_returns_nil_when_machine_is_null
    with_server(->(_q, _v, _h) { ["200 OK", { "data" => { "machine" => nil } }] }) do |server|
      client = Bsdkrun::Client.new(url: server.url, token: "tok")
      assert_nil client.get("missing")
    end
  end

  def test_run_linux_sends_camel_case_input_and_returns_id
    with_server(lambda { |q, v, _h|
      assert_match(/RunLinuxInput/, q)
      input = v["i"]
      assert_equal("alpine", input["image"])
      assert_equal("6.6", input["kernelVersion"])
      assert_equal(false, input["net"]["noNet"])
      assert_equal(["8080:80"], input["net"]["ports"])
      ["200 OK", { "data" => { "runLinux" => "deadbeef0001" } }]
    }) do |server|
      client = Bsdkrun::Client.new(url: server.url, token: "tok")
      id = client.run_linux(image: "alpine", kernel_version: "6.6", net: { ports: ["8080:80"] })
      assert_equal("deadbeef0001", id)
    end
  end

  def test_run_bsd_maps_os_enum_and_disk_fields
    with_server(lambda { |_q, v, _h|
      input = v["i"]
      assert_equal("NETBSD", input["os"])
      assert_equal(["a.raw"], input["attachDisk"])
      assert_equal("20G", input["diskSize"])
      ["200 OK", { "data" => { "runBsd" => "cafebabe0002" } }]
    }) do |server|
      client = Bsdkrun::Client.new(url: server.url, token: "tok")
      id = client.run_bsd(os: "netbsd", attach_disk: ["a.raw"], disk_size: "20G")
      assert_equal("cafebabe0002", id)
    end
  end

  def test_stop_returns_command_result
    with_server(lambda { |_q, v, _h|
      assert_equal("m1", v["id"])
      ["200 OK", { "data" => { "stopMachine" => { "exitCode" => 0, "stdout" => "ok", "stderr" => "" } } }]
    }) do |server|
      client = Bsdkrun::Client.new(url: server.url, token: "tok")
      result = client.stop("m1")
      assert_kind_of(Bsdkrun::CommandResult, result)
      assert_equal(0, result.exit_code)
      assert_equal("ok", result.stdout)
    end
  end

  def test_logs_returns_string
    with_server(lambda { |_q, v, _h|
      assert_equal(true, v["boot"])
      ["200 OK", { "data" => { "machineLogs" => "boot log\n" } }]
    }) do |server|
      client = Bsdkrun::Client.new(url: server.url, token: "tok")
      assert_equal("boot log\n", client.logs("m1", boot: true))
    end
  end
end
