defmodule Server.Application do
  @moduledoc false

  use Application

  @impl true
  def start(_type, _args) do
    children = [
      {Plug.Cowboy, scheme: :http, plug: Server, options: [ip: {0, 0, 0, 0}, port: 3000]}
    ]

    opts = [strategy: :one_for_one, name: Server.Supervisor]

    case Supervisor.start_link(children, opts) do
      {:ok, pid} ->
        # Printed rather than logged: the release starts :logger, but a
        # unikernel's console is the only place output can go, and a plain
        # write to stdout is the one thing guaranteed to reach it.
        IO.puts("Elixir listening on port 3000")
        {:ok, pid}

      other ->
        other
    end
  end
end
