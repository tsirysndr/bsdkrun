defmodule Bsdkrun.MixProject do
  use Mix.Project

  @version "0.2.1"
  @source_url "https://github.com/tsirysndr/bsdkrun"

  def project do
    [
      app: :bsdkrun_ex,
      version: @version,
      elixir: "~> 1.15",
      start_permanent: Mix.env() == :prod,
      elixirc_paths: elixirc_paths(Mix.env()),
      deps: deps(),
      description:
        "Elixir SDK for bsdkrun — a Firecracker-style microVM launcher for BSD and Linux guests.",
      package: package(),
      name: "bsdkrun_ex",
      source_url: @source_url,
      docs: docs()
    ]
  end

  # The CLI-backed half of the SDK (Bsdkrun.Sandbox et al.) is a thin,
  # stateless wrapper with no runtime of its own. Bsdkrun.Client — the remote
  # GraphQL client — needs :inets (HTTP) and :ssl (HTTP/S and wss://), plus a
  # tiny supervision tree (Bsdkrun.Application) that gives each Client's
  # shared graphql-transport-ws socket somewhere to live.
  def application do
    [extra_applications: [:logger, :inets, :ssl], mod: {Bsdkrun.Application, []}]
  end

  defp deps do
    [
      {:jason, "~> 1.4"},
      {:ex_doc, "~> 0.31", only: :dev, runtime: false}
    ]
  end

  # test/support holds fake HTTP/WS servers (built on :gen_tcp, no hex
  # dependency) used to exercise Bsdkrun.GraphQL and Bsdkrun.GraphQLSocket
  # end-to-end; keep them out of the published package.
  defp elixirc_paths(:test), do: ["lib", "test/support"]
  defp elixirc_paths(_env), do: ["lib"]

  # Published to Hex as `bsdkrun_ex`: the Gleam SDK already claims `bsdkrun`
  # there, and Hex is a single namespace shared by both. The modules stay
  # `Bsdkrun.*`.
  defp package do
    [
      name: "bsdkrun_ex",
      licenses: ["MIT"],
      links: %{"GitHub" => @source_url},
      files: ~w(lib mix.exs README.md)
    ]
  end

  defp docs do
    [
      main: "readme",
      extras: ["README.md"],
      # Monorepo: the package lives at sdk/elixir and is tagged separately from
      # the CLI's own v-series.
      source_ref: "elixir-sdk-v#{@version}",
      source_url_pattern: "#{@source_url}/blob/elixir-sdk-v#{@version}/sdk/elixir/%{path}#L%{line}"
    ]
  end
end
