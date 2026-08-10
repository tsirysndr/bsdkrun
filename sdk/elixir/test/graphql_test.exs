defmodule Bsdkrun.GraphQLTest do
  use ExUnit.Case, async: true

  alias Bsdkrun.Error
  alias Bsdkrun.GraphQL
  alias Bsdkrun.Test.FakeDaemon

  describe "request/4 against a real server" do
    test "success: posts the right method/headers/body and returns data" do
      test_pid = self()

      url =
        FakeDaemon.start(
          http: fn req ->
            send(test_pid, {:request, req})
            {200, Jason.encode!(%{data: %{machines: []}})}
          end
        )

      assert {:ok, %{"machines" => []}} = GraphQL.request(url, "s3cr3t", "{ machines { id } }", %{"a" => 1})

      assert_receive {:request, req}
      assert req.request_line =~ ~r/^POST .* HTTP\/1\.1$/
      assert Map.get(req.headers, "content-type") == "application/json"
      assert Map.get(req.headers, "authorization") == "Bearer s3cr3t"
      assert Jason.decode!(req.body) == %{"query" => "{ machines { id } }", "variables" => %{"a" => 1}}
    end

    test "401 is an auth_error, regardless of body" do
      url = FakeDaemon.start(http: fn _req -> {401, "unauthorized"} end)

      assert {:error, %Error{kind: :auth_error}} = GraphQL.request(url, "bad-token", "{ x }")
    end

    test "a GraphQL error with extensions.code UNAUTHENTICATED is an auth_error" do
      body =
        Jason.encode!(%{
          errors: [%{message: "bad token", extensions: %{code: "UNAUTHENTICATED"}}]
        })

      url = FakeDaemon.start(http: fn _req -> {200, body} end)

      assert {:error, %Error{kind: :auth_error, message: "bad token"}} = GraphQL.request(url, "t", "{ x }")
    end

    test "any other GraphQL error is a graphql_error, carrying the extensions.code" do
      body =
        Jason.encode!(%{
          errors: [%{message: "machine not found", extensions: %{code: "INVALID_ARGUMENT"}}]
        })

      url = FakeDaemon.start(http: fn _req -> {200, body} end)

      assert {:error, %Error{kind: :graphql_error, message: "machine not found", code: "INVALID_ARGUMENT"}} =
               GraphQL.request(url, "t", "{ x }")
    end

    test "a GraphQL error with no extensions is still a graphql_error" do
      body = Jason.encode!(%{errors: [%{message: "boom"}]})
      url = FakeDaemon.start(http: fn _req -> {200, body} end)

      assert {:error, %Error{kind: :graphql_error, message: "boom", code: nil}} = GraphQL.request(url, "t", "{ x }")
    end

    test "a non-JSON response is a graphql_error" do
      url = FakeDaemon.start(http: fn _req -> {200, "not json"} end)
      assert {:error, %Error{kind: :graphql_error}} = GraphQL.request(url, "t", "{ x }")
    end
  end

  describe "request/4 against an unreachable daemon" do
    test "a transport-level failure is a graphql_error naming the url" do
      # Nothing is listening on this port.
      url = "http://127.0.0.1:1"

      assert {:error, %Error{kind: :graphql_error, message: message}} = GraphQL.request(url, "t", "{ x }")
      assert message =~ "cannot reach the bsdkrun daemon at #{url}"
    end
  end
end
