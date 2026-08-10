defmodule Bsdkrun.GraphQL do
  @moduledoc """
  The HTTP transport for `Bsdkrun.Client`: one `POST` per query or mutation,
  over Erlang's built-in `:httpc` (part of `:inets` — no hex dependency).

  Mirrors `web/src/lib/graphql.ts`'s `gql()`: same headers
  (`content-type: application/json`, `authorization: Bearer <token>`), same
  body shape (`{"query": ..., "variables": ...}`), and the same error
  semantics — a 401 or a GraphQL error with `extensions.code ==
  "UNAUTHENTICATED"` is an auth failure (`Bsdkrun.Error` with `kind:
  :auth_error`); anything else that goes wrong (unreachable daemon, a
  non-JSON response, any other GraphQL error) is `kind: :graphql_error`.
  """

  alias Bsdkrun.Error

  @doc """
  POST `query`/`variables` as a GraphQL request to `url`, authenticated with
  the bearer `token`. Returns `{:ok, data}` — the decoded `data` field of the
  response — or `{:error, %Bsdkrun.Error{}}`.
  """
  @spec request(String.t(), String.t(), String.t(), map()) :: {:ok, term()} | {:error, Error.t()}
  def request(url, token, query, variables \\ %{}) do
    _ = Application.ensure_all_started(:inets)
    _ = Application.ensure_all_started(:ssl)

    body = Jason.encode!(%{query: query, variables: variables})

    headers = [
      {~c"content-type", ~c"application/json"},
      {~c"authorization", String.to_charlist("Bearer " <> token)}
    ]

    http_request = {String.to_charlist(url), headers, ~c"application/json", body}
    http_options = http_options(url)

    case :httpc.request(:post, http_request, http_options, body_format: :binary) do
      {:ok, {{_version, 401, _reason}, _headers, _body}} ->
        {:error, Error.auth_error()}

      {:ok, {{_version, _status, _reason}, _headers, resp_body}} ->
        decode_response(resp_body)

      {:error, reason} ->
        {:error,
         Error.graphql_error("cannot reach the bsdkrun daemon at #{url} — #{inspect(reason)}")}
    end
  end

  # Full system-CA verification by default — the same policy
  # `daemon/src/client.rs`'s own remote client uses
  # (`ClientTlsConfig::new().with_native_roots()`) and what every other
  # language's HTTP client (`fetch`, `httpx`, `net/http`, ...) does out of
  # the box. A loopback daemon's typically self-signed certificate (see
  # daemon/README.md) therefore needs a real certificate — or a reverse
  # proxy terminating TLS with one — to be reached over `https://` from this
  # SDK, exactly as it would from a browser; skipping verification by
  # default would silently drop MITM protection for every other `https://`
  # host this client is pointed at.
  defp http_options(url) do
    if String.starts_with?(url, "https://") do
      [ssl: [verify: :verify_peer, cacerts: :public_key.cacerts_get()]]
    else
      []
    end
  end

  defp decode_response(resp_body) do
    case Jason.decode(resp_body) do
      {:error, _reason} ->
        {:error, Error.graphql_error("the daemon returned a non-JSON response")}

      {:ok, body} ->
        case body["errors"] do
          [first | _rest] ->
            code = get_in(first, ["extensions", "code"])
            message = first["message"] || "unknown GraphQL error"

            if code == "UNAUTHENTICATED" do
              {:error, Error.auth_error(message)}
            else
              {:error, Error.graphql_error(message, code)}
            end

          _no_errors ->
            {:ok, body["data"]}
        end
    end
  end
end
