defmodule Server do
  @moduledoc """
  The HTTP endpoint. Two routes, the same pair the other Unikraft examples in
  this repository serve: `/` greets, `/info` reports the runtime versions.
  """
  use Plug.Router

  plug(:match)
  plug(:dispatch)

  get "/" do
    conn
    |> put_resp_content_type("text/plain")
    |> send_resp(200, "Hello from Elixir on Unikraft!\n")
  end

  get "/info" do
    body =
      Jason.encode!(%{
        runtime: "elixir",
        elixir: System.version(),
        otp: System.otp_release(),
        erts: List.to_string(:erlang.system_info(:version)),
        schedulers: System.schedulers_online()
      })

    conn
    |> put_resp_content_type("application/json")
    |> send_resp(200, body)
  end

  match _ do
    send_resp(conn, 404, "not found\n")
  end
end
