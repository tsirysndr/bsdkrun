defmodule Bsdkrun.MixProject do
  use Mix.Project

  @version "0.1.0"
  @source_url "https://github.com/tsirysndr/bsdkrun"

  def project do
    [
      app: :bsdkrun_ex,
      version: @version,
      elixir: "~> 1.15",
      start_permanent: Mix.env() == :prod,
      deps: deps(),
      description:
        "Elixir SDK for bsdkrun — a Firecracker-style microVM launcher for BSD and Linux guests.",
      package: package(),
      name: "bsdkrun_ex",
      source_url: @source_url,
      docs: docs()
    ]
  end

  # No application runtime — this is a thin, stateless wrapper around the CLI.
  def application do
    [extra_applications: [:logger]]
  end

  defp deps do
    [
      {:jason, "~> 1.4"},
      {:ex_doc, "~> 0.31", only: :dev, runtime: false}
    ]
  end

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
