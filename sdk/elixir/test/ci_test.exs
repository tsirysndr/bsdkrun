defmodule Bsdkrun.CITest do
  use ExUnit.Case, async: true

  alias Bsdkrun.CI

  # The YAML the builder emits is consumed by tangled's own workflow parser
  # (inside `bsdkrun ci`), so these tests pin the emitted shape — a change
  # here is a change to what spindle would receive.

  test "renders the full workflow shape" do
    y =
      CI.workflow("test")
      |> CI.on_push("main")
      |> CI.on_pull_request(["main", "develop"])
      |> CI.deps(["elixir", "erlang"])
      |> CI.deps_from("github:nix-community/fenix/abc123", ["stable.default"])
      |> CI.env("MIX_ENV", "test")
      |> CI.clone_depth(100)
      |> CI.step("deps", "mix deps.get")
      |> CI.step("test", "mix test", %{"WARNINGS_AS_ERRORS" => "1"})
      |> CI.yaml()

    assert y =~ "  - event: [\"push\"]\n    branch: \"main\""
    assert y =~ "branch: [\"main\", \"develop\"]"
    assert y =~ "engine: nixery"
    assert y =~ "\"nixpkgs\":\n    - \"elixir\"\n    - \"erlang\""
    assert y =~ "\"github:nix-community/fenix/abc123\":"
    assert y =~ "MIX_ENV: \"test\""
    assert y =~ "depth: 100"
    assert y =~ "- name: \"deps\"\n    command: |\n      mix deps.get"
    assert y =~ "environment:\n      WARNINGS_AS_ERRORS: \"1\""
  end

  test "block-unsafe commands fall back to a JSON string" do
    # Trailing spaces do not survive a literal block scalar; the emitter must
    # switch representation rather than silently altering the command.
    y = CI.workflow("edge") |> CI.step("tricky", "echo 'a'  \necho b") |> CI.yaml()
    assert y =~ "command: \"echo 'a'  \\necho b\""
  end

  test "file names get the yml suffix" do
    assert CI.workflow("build") |> CI.file_name() == "build.yml"
    assert CI.workflow("build.yaml") |> CI.file_name() == "build.yaml"
  end
end
