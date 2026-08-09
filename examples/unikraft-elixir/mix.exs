defmodule Server.MixProject do
  use Mix.Project

  def project do
    [
      app: :server,
      version: "0.1.0",
      elixir: "~> 1.14",
      start_permanent: Mix.env() == :prod,
      deps: deps(),
      releases: [
        server: [
          # The unikernel already carries ERTS -- the Dockerfile copies it out
          # of the same elixir image, built for the target architecture. Baking
          # a second copy into the release would only make the image bigger,
          # and the image is resident twice in the guest (see the README).
          include_erts: false,
          # Every .beam in the release is Erlang bytecode and therefore
          # architecture-independent, which is what lets the build stage run on
          # the *host* arch while the runtime stage is pulled for the target.
          strip_beams: true
        ]
      ]
    ]
  end

  def application do
    [
      extra_applications: [:logger],
      mod: {Server.Application, []}
    ]
  end

  defp deps do
    [
      {:plug_cowboy, "~> 2.7"},
      {:jason, "~> 1.4"}
    ]
  end
end
