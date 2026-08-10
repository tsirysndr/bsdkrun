defmodule Bsdkrun.ClientTest do
  # System.put_env/2 is process-global; these tests mutate BSDKRUN_URL /
  # BSDKRUN_TOKEN, so this file cannot run concurrently with itself.
  use ExUnit.Case, async: false

  alias Bsdkrun.Client
  alias Bsdkrun.Error
  alias Bsdkrun.Test.FakeDaemon

  # Bsdkrun.Client.Registry keys a shared GraphQLSocket by {url, token}. Two
  # FakeDaemon instances can land on the same OS-reassigned ephemeral port in
  # quick succession, so a token reused across tests risks one test's
  # ensure_conn/1 finding a *different* test's (already-dead-or-dying) socket
  # under the same key. A unique token per connecting test sidesteps that —
  # it is a test-isolation concern, not something Bsdkrun.Client needs to
  # guard against for real daemon usage.
  defp unique_token(label), do: "#{label}-#{System.unique_integer([:positive])}"

  describe "normalize_url/1" do
    test "adds http:// when no scheme is given" do
      assert Client.normalize_url("localhost:50052") == "http://localhost:50052/graphql"
    end

    test "keeps an explicit https:// scheme" do
      assert Client.normalize_url("https://vps.example.com:50052") == "https://vps.example.com:50052/graphql"
    end

    test "strips trailing slashes before appending /graphql" do
      assert Client.normalize_url("http://host:50052///") == "http://host:50052/graphql"
    end

    test "does not double up /graphql" do
      assert Client.normalize_url("http://host:50052/graphql") == "http://host:50052/graphql"
      assert Client.normalize_url("http://host:50052/graphql/") == "http://host:50052/graphql"
    end

    test "trims surrounding whitespace" do
      assert Client.normalize_url("  localhost:50052  ") == "http://localhost:50052/graphql"
    end
  end

  describe "new/1" do
    test "normalizes the url and does not connect" do
      client = Client.new(url: "localhost:50052", token: "tok")
      assert %Client{url: "http://localhost:50052/graphql", token: "tok"} = client
    end
  end

  describe "from_env/0 and from_env!/0" do
    setup do
      before_url = System.get_env("BSDKRUN_URL")
      before_token = System.get_env("BSDKRUN_TOKEN")
      System.delete_env("BSDKRUN_URL")
      System.delete_env("BSDKRUN_TOKEN")

      on_exit(fn ->
        if before_url, do: System.put_env("BSDKRUN_URL", before_url), else: System.delete_env("BSDKRUN_URL")
        if before_token, do: System.put_env("BSDKRUN_TOKEN", before_token), else: System.delete_env("BSDKRUN_TOKEN")
      end)

      :ok
    end

    test "BSDKRUN_URL unset is a config_error" do
      assert {:error, %Error{kind: :config_error, message: message}} = Client.from_env()
      assert message =~ "BSDKRUN_URL"
    end

    test "BSDKRUN_URL set without BSDKRUN_TOKEN is a config_error, not a silent fallback" do
      System.put_env("BSDKRUN_URL", "http://host:50052")
      assert {:error, %Error{kind: :config_error, message: message}} = Client.from_env()
      assert message =~ "BSDKRUN_TOKEN"
    end

    test "both set builds a normalized client" do
      System.put_env("BSDKRUN_URL", "host:50052")
      System.put_env("BSDKRUN_TOKEN", "s3cret")

      assert {:ok, %Client{url: "http://host:50052/graphql", token: "s3cret"}} = Client.from_env()
    end

    test "from_env!/0 raises Bsdkrun.Error when unset" do
      assert_raise Bsdkrun.Error, fn -> Client.from_env!() end
    end

    test "from_env!/0 returns the client when set" do
      System.put_env("BSDKRUN_URL", "host:50052")
      System.put_env("BSDKRUN_TOKEN", "s3cret")
      assert %Client{token: "s3cret"} = Client.from_env!()
    end
  end

  describe "exec/4 (openShell -> shellOutput subscription -> closeShell)" do
    test "opens, subscribes, waits for exitCode, closes, and returns the collected output" do
      test_pid = self()
      token = unique_token("s3cret")

      url =
        FakeDaemon.start(
          http: fn req ->
            body = Jason.decode!(req.body)

            cond do
              String.contains?(req.body, "openShell(") ->
                send(test_pid, {:http, :open_shell, body})
                {200, Jason.encode!(%{data: %{openShell: %{id: "sess-1"}}})}

              String.contains?(req.body, "closeShell(") ->
                send(test_pid, {:http, :close_shell, body})
                {200, Jason.encode!(%{data: %{closeShell: true}})}

              true ->
                {200, Jason.encode!(%{data: %{}})}
            end
          end,
          ws: fn sock ->
            {init_frame, buf} = FakeDaemon.recv_frame(sock)
            send(test_pid, {:ws, :connection_init, Jason.decode!(init_frame.payload)})
            FakeDaemon.send_text(sock, Jason.encode!(%{type: "connection_ack"}))

            {sub_frame, _rest} = FakeDaemon.recv_frame(sock, buf)
            sub_msg = Jason.decode!(sub_frame.payload)
            send(test_pid, {:ws, :subscribe, sub_msg})
            sub_id = sub_msg["id"]

            next = fn payload -> FakeDaemon.send_text(sock, Jason.encode!(%{id: sub_id, type: "next", payload: payload})) end

            next.(%{data: %{shellOutput: %{dataBase64: Base.encode64("hello "), exitCode: nil}}})
            next.(%{data: %{shellOutput: %{dataBase64: Base.encode64("world"), exitCode: nil}}})
            next.(%{data: %{shellOutput: %{dataBase64: nil, exitCode: 0}}})
            :ok
          end
        )

      client = Client.new(url: url, token: token)

      assert {:ok, %{exit_code: 0, output: "hello world"}} = Client.exec(client, "machine-1", ["echo", "hi"])

      assert_receive {:http, :open_shell, open_body}, 2_000
      assert open_body["variables"]["machineId"] == "machine-1"
      assert open_body["variables"]["command"] == ["echo", "hi"]

      assert_receive {:ws, :connection_init, init_msg}, 2_000
      assert init_msg["type"] == "connection_init"
      assert init_msg["payload"]["authorization"] == "Bearer #{token}"

      assert_receive {:ws, :subscribe, sub_msg}, 2_000
      assert sub_msg["type"] == "subscribe"
      assert sub_msg["payload"]["variables"]["sessionId"] == "sess-1"

      assert_receive {:http, :close_shell, close_body}, 2_000
      assert close_body["variables"]["sessionId"] == "sess-1"
    end

    test "a GraphQL error while waiting for exitCode is returned, not raised" do
      test_pid = self()

      url =
        FakeDaemon.start(
          http: fn req ->
            cond do
              String.contains?(req.body, "openShell(") -> {200, Jason.encode!(%{data: %{openShell: %{id: "sess-2"}}})}
              String.contains?(req.body, "closeShell(") -> {200, Jason.encode!(%{data: %{closeShell: true}})}
              true -> {200, Jason.encode!(%{data: %{}})}
            end
          end,
          ws: fn sock ->
            {_init_frame, buf} = FakeDaemon.recv_frame(sock)
            FakeDaemon.send_text(sock, Jason.encode!(%{type: "connection_ack"}))

            {sub_frame, _rest} = FakeDaemon.recv_frame(sock, buf)
            sub_id = Jason.decode!(sub_frame.payload)["id"]
            send(test_pid, :subscribed)

            FakeDaemon.send_text(
              sock,
              Jason.encode!(%{id: sub_id, type: "error", payload: [%{message: "machine vanished"}]})
            )

            :ok
          end
        )

      client = Client.new(url: url, token: unique_token("tok"))

      assert {:error, %Error{kind: :graphql_error, message: "machine vanished"}} =
               Client.exec(client, "machine-1", "uptime")

      assert_receive :subscribed, 2_000
    end
  end

  describe "subscribe/4 — socket closes before connection_ack" do
    test "delivers an auth_error to the subscription" do
      url =
        FakeDaemon.start(
          http: fn _req -> {200, Jason.encode!(%{data: %{}})} end,
          ws: fn sock ->
            {_init_frame, _rest} = FakeDaemon.recv_frame(sock)
            # Never ack — give the client a moment to have called subscribe/4
            # (which happens synchronously right after the socket is up)
            # before dropping the connection.
            Process.sleep(200)
            :ok
          end
        )

      client = Client.new(url: url, token: unique_token("tok"))
      assert {:ok, _subscription} = Client.subscribe(client, "subscription { x }", %{})

      assert_receive {:bsdkrun_subscription, _id, {:error, %Error{kind: :auth_error}}}, 2_000
    end
  end
end
