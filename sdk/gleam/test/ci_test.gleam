//// The YAML the builder emits is consumed by tangled's own workflow parser
//// (inside `bsdkrun ci`), so these tests pin the emitted shape — a change
//// here is a change to what spindle would receive.

import bsdkrun/ci
import gleam/dict
import gleam/string
import gleeunit/should

pub fn full_workflow_shape_test() {
  let y =
    ci.workflow("test")
    |> ci.on_push(["main"])
    |> ci.on_pull_request(["main", "develop"])
    |> ci.deps(["gleam", "erlang"])
    |> ci.deps_from("github:nix-community/fenix/abc123", ["stable.default"])
    |> ci.env("CI_FROM", "sdk")
    |> ci.clone_depth(100)
    |> ci.step("deps", "gleam deps download")
    |> ci.step_env("test", "gleam test", dict.from_list([#("WARNINGS", "all")]))
    |> ci.yaml()

  string.contains(y, "  - event: [\"push\"]\n    branch: \"main\"")
  |> should.be_true()
  string.contains(y, "branch: [\"main\", \"develop\"]") |> should.be_true()
  string.contains(y, "engine: nixery") |> should.be_true()
  string.contains(y, "\"nixpkgs\":\n    - \"gleam\"\n    - \"erlang\"")
  |> should.be_true()
  string.contains(y, "\"github:nix-community/fenix/abc123\":")
  |> should.be_true()
  string.contains(y, "CI_FROM: \"sdk\"") |> should.be_true()
  string.contains(y, "depth: 100") |> should.be_true()
  string.contains(
    y,
    "- name: \"deps\"\n    command: |\n      gleam deps download",
  )
  |> should.be_true()
  string.contains(y, "environment:\n      WARNINGS: \"all\"")
  |> should.be_true()
}

pub fn block_unsafe_command_falls_back_to_json_test() {
  // Trailing spaces do not survive a literal block scalar; the emitter must
  // switch representation rather than silently altering the command.
  let y =
    ci.workflow("edge")
    |> ci.step("tricky", "echo 'a'  \necho b")
    |> ci.yaml()
  string.contains(y, "command: \"echo 'a'  \\necho b\"") |> should.be_true()
}

pub fn file_name_test() {
  ci.workflow("build") |> ci.file_name() |> should.equal("build.yml")
  ci.workflow("build.yaml") |> ci.file_name() |> should.equal("build.yaml")
}
