defmodule Bsdkrun.Application do
  @moduledoc false

  # Only exists for Bsdkrun.Client (the remote GraphQL client). The CLI-backed
  # half of the SDK (Bsdkrun.Sandbox et al.) needs none of this — it just
  # shells out via System.cmd/3.
  #
  # A `Bsdkrun.Client` struct is plain, immutable config: it carries no pid.
  # Subscriptions (exec, shell, follow_logs, subscribe) need a live
  # graphql-transport-ws socket, though, and every caller sharing the same
  # `Client` (same url + token) should share one. That needs somewhere for
  # the socket process to live and a way to find it again — hence a
  # `Registry` keyed by `{url, token}` and a `DynamicSupervisor` that starts
  # `Bsdkrun.GraphQLSocket` processes on demand. See `Bsdkrun.Client.ensure_conn/1`.
  use Application

  @impl true
  def start(_type, _args) do
    # Defensive: normally already running via `extra_applications`, but this
    # keeps `Bsdkrun.Client` usable even if something started this app
    # without going through a full OTP release/dependency boot.
    _ = Application.ensure_all_started(:inets)
    _ = Application.ensure_all_started(:ssl)

    children = [
      {Registry, keys: :unique, name: Bsdkrun.Client.Registry},
      {DynamicSupervisor, name: Bsdkrun.Client.SocketSupervisor, strategy: :one_for_one}
    ]

    Supervisor.start_link(children, strategy: :one_for_one, name: Bsdkrun.Supervisor)
  end
end
