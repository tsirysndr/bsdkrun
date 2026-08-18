defmodule Bsdkrun.CI do
  @moduledoc """
  CI workflows defined in code instead of YAML.

  The builder produces exactly the file `bsdkrun ci` (and tangled's spindle)
  consumes — `yaml/1` is that file, `save/2` commits it to
  `.tangled/workflows/`, and `run/1` executes it in a microVM without a file
  ever touching the repository:

      Bsdkrun.CI.workflow("test")
      |> Bsdkrun.CI.on_push("main")
      |> Bsdkrun.CI.deps(["elixir", "erlang"])
      |> Bsdkrun.CI.env("MIX_ENV", "test")
      |> Bsdkrun.CI.step("deps", "mix deps.get")
      |> Bsdkrun.CI.step("test", "mix test")
      |> Bsdkrun.CI.run()

  Code is the source of truth and YAML the wire format, in that order — which
  is why `save/2` writes a generated-file header: a hand-edit there will be
  overwritten by the next save.
  """

  defstruct name: nil,
            engine: "nixery",
            when_: [],
            deps: %{},
            env: %{},
            steps: [],
            clone_depth: nil,
            clone_skip: false

  @type t :: %__MODULE__{}

  @doc "Start a CI workflow definition."
  @spec workflow(String.t()) :: t()
  def workflow(name), do: %__MODULE__{name: name}

  @doc "Override the engine (`nixery` by default)."
  @spec engine(t(), String.t()) :: t()
  def engine(%__MODULE__{} = wf, engine), do: %{wf | engine: engine}

  @doc "Add a push trigger for the given branch(es)."
  @spec on_push(t(), String.t() | [String.t()]) :: t()
  def on_push(%__MODULE__{} = wf, branches),
    do: %{wf | when_: wf.when_ ++ [{["push"], List.wrap(branches)}]}

  @doc "Add a pull_request trigger targeting the given branch(es)."
  @spec on_pull_request(t(), String.t() | [String.t()]) :: t()
  def on_pull_request(%__MODULE__{} = wf, branches),
    do: %{wf | when_: wf.when_ ++ [{["pull_request"], List.wrap(branches)}]}

  @doc "Add nixpkgs dependencies — the toolchain the steps run against."
  @spec deps(t(), [String.t()]) :: t()
  def deps(%__MODULE__{} = wf, packages),
    do: %{wf | deps: Map.update(wf.deps, "nixpkgs", packages, &(&1 ++ packages))}

  @doc "Add dependencies from a custom registry (a flake reference)."
  @spec deps_from(t(), String.t(), [String.t()]) :: t()
  def deps_from(%__MODULE__{} = wf, registry, packages),
    do: %{wf | deps: Map.update(wf.deps, registry, packages, &(&1 ++ packages))}

  @doc "Set a workflow-level environment variable."
  @spec env(t(), String.t(), String.t()) :: t()
  def env(%__MODULE__{} = wf, key, value), do: %{wf | env: Map.put(wf.env, key, value)}

  @doc "Append a step; steps run serially in one VM, from the workspace root."
  @spec step(t(), String.t(), String.t(), %{String.t() => String.t()}) :: t()
  def step(%__MODULE__{} = wf, name, command, step_env \\ %{}),
    do: %{wf | steps: wf.steps ++ [%{name: name, command: command, env: step_env}]}

  @doc "Set the clone depth (default 1)."
  @spec clone_depth(t(), pos_integer()) :: t()
  def clone_depth(%__MODULE__{} = wf, depth), do: %{wf | clone_depth: depth}

  @doc "Skip the checkout entirely."
  @spec skip_clone(t()) :: t()
  def skip_clone(%__MODULE__{} = wf), do: %{wf | clone_skip: true}

  @doc "The workflow file name `save/2` writes: `<name>.yml`."
  @spec file_name(t()) :: String.t()
  def file_name(%__MODULE__{name: name}) do
    if String.ends_with?(name, [".yml", ".yaml"]), do: name, else: name <> ".yml"
  end

  @doc """
  Render the workflow file.

  Scalars are emitted as JSON strings — valid YAML by construction — and
  commands as literal blocks when safe, so the SDK needs no YAML dependency.
  """
  @spec yaml(t()) :: String.t()
  def yaml(%__MODULE__{} = wf) do
    sections =
      [
        when_section(wf),
        "engine: #{wf.engine}",
        deps_section(wf),
        env_section(wf),
        clone_section(wf),
        steps_section(wf)
      ]
      |> Enum.reject(&is_nil/1)

    Enum.join(sections, "\n\n") <> "\n"
  end

  defp when_section(%{when_: []}), do: nil

  defp when_section(%{when_: constraints}) do
    lines =
      Enum.flat_map(constraints, fn {events, branches} ->
        head = "  - event: [#{Enum.map_join(events, ", ", &q/1)}]"

        case branches do
          [] -> [head]
          [one] -> [head, "    branch: #{q(one)}"]
          many -> [head, "    branch: [#{Enum.map_join(many, ", ", &q/1)}]"]
        end
      end)

    Enum.join(["when:" | lines], "\n")
  end

  defp deps_section(%{deps: deps}) when map_size(deps) == 0, do: nil

  defp deps_section(%{deps: deps}) do
    lines =
      deps
      |> Map.keys()
      |> Enum.sort()
      |> Enum.flat_map(fn reg ->
        ["  #{q(reg)}:" | Enum.map(deps[reg], &"    - #{q(&1)}")]
      end)

    Enum.join(["dependencies:" | lines], "\n")
  end

  defp env_section(%{env: env}) when map_size(env) == 0, do: nil

  defp env_section(%{env: env}) do
    lines = env |> Map.keys() |> Enum.sort() |> Enum.map(&"  #{&1}: #{q(env[&1])}")
    Enum.join(["environment:" | lines], "\n")
  end

  defp clone_section(%{clone_skip: false, clone_depth: nil}), do: nil

  defp clone_section(%{clone_skip: skip, clone_depth: depth}) do
    lines =
      ["clone:"] ++
        if(skip, do: ["  skip: true"], else: []) ++
        if(depth, do: ["  depth: #{depth}"], else: [])

    Enum.join(lines, "\n")
  end

  defp steps_section(%{steps: steps}) do
    lines =
      Enum.flat_map(steps, fn s ->
        ["  - name: #{q(s.name)}"] ++
          command_lines(s.command) ++ step_env_lines(s.env)
      end)

    Enum.join(["steps:" | lines], "\n")
  end

  # Literal blocks read well in a committed file, but cannot carry trailing
  # spaces or carriage returns byte-for-byte; fall back to a JSON string
  # rather than silently altering the command.
  defp command_lines(command) do
    block_safe? =
      command != "" and
        not String.contains?(command, "\r") and
        command |> String.split("\n") |> Enum.all?(&(&1 == String.trim_trailing(&1, " ")))

    if block_safe? do
      body =
        command
        |> String.trim_trailing("\n")
        |> String.split("\n")
        |> Enum.map(&"      #{&1}")

      ["    command: |" | body]
    else
      ["    command: #{q(command)}"]
    end
  end

  defp step_env_lines(env) when map_size(env) == 0, do: []

  defp step_env_lines(env) do
    lines = env |> Map.keys() |> Enum.sort() |> Enum.map(&"      #{&1}: #{q(env[&1])}")
    ["    environment:" | lines]
  end

  # A JSON string literal, which is a valid YAML scalar by construction.
  defp q(s) when is_binary(s) do
    inner =
      s
      |> String.to_charlist()
      |> Enum.map_join(fn
        ?" -> "\\\""
        ?\\ -> "\\\\"
        ?\n -> "\\n"
        ?\r -> "\\r"
        ?\t -> "\\t"
        c when c < 0x20 -> "\\u" <> String.pad_leading(Integer.to_string(c, 16), 4, "0")
        c -> <<c::utf8>>
      end)

    "\"" <> inner <> "\""
  end

  @doc "Write into `<repo>/.tangled/workflows/` and return the path."
  @spec save(t(), Path.t()) :: {:ok, Path.t()} | {:error, term()}
  def save(%__MODULE__{} = wf, repo) do
    dir = Path.join([repo, ".tangled", "workflows"])

    with :ok <- File.mkdir_p(dir) do
      path = Path.join(dir, file_name(wf))

      content =
        "# Generated by the bsdkrun SDK — edit the code that save()d it instead.\n" <> yaml(wf)

      with :ok <- File.write(path, content), do: {:ok, path}
    end
  end

  @doc """
  Execute the workflow in a microVM, streaming output.

  The YAML never touches the repository — it goes to a temp file and
  `bsdkrun ci run -f`. Returns `:ok`, or `{:error, exit_code}` when a step
  fails.
  """
  @spec run(t(), keyword()) :: :ok | {:error, term()}
  def run(%__MODULE__{} = wf, opts \\ []) do
    tmp = Path.join(System.tmp_dir!(), "bsdkrun-ci-#{System.unique_integer([:positive])}")

    with :ok <- File.mkdir_p(tmp) do
      file = Path.join(tmp, file_name(wf))

      try do
        with :ok <- File.write(file, yaml(wf)) do
          args =
            ["ci", "run", "-f", file] ++
              case Keyword.get(opts, :dir) do
                nil -> []
                dir -> ["-w", dir]
              end

          bin = Bsdkrun.Binary.resolve!()
          {_out, code} = System.cmd(bin, args, into: IO.stream(:stdio, :line))
          if code == 0, do: :ok, else: {:error, code}
        end
      after
        File.rm_rf(tmp)
      end
    end
  end
end
