//// CI workflows defined in code instead of YAML.
////
//// The builder produces exactly the file `bsdkrun ci` (and tangled's
//// spindle) consumes — `yaml` is that file, `save` commits it to
//// `.tangled/workflows/`, and `run` executes it in a microVM without a file
//// ever touching the repository:
////
//// ```gleam
//// ci.workflow("test")
//// |> ci.on_push(["main"])
//// |> ci.deps(["gleam", "erlang"])
//// |> ci.env("CI_FROM", "sdk")
//// |> ci.step("deps", "gleam deps download")
//// |> ci.step("test", "gleam test")
//// |> ci.run()
//// ```
////
//// Code is the source of truth and YAML the wire format, in that order —
//// which is why `save` writes a generated-file header: a hand-edit there
//// will be overwritten by the next save.

import bsdkrun/cli
import bsdkrun/error.{type Error}
import gleam/dict.{type Dict}
import gleam/int
import gleam/json
import gleam/list
import gleam/option.{type Option, None, Some}
import gleam/string
import simplifile

/// A workflow under construction.
pub type Workflow {
  Workflow(
    name: String,
    engine: String,
    when: List(#(List(String), List(String))),
    deps: Dict(String, List(String)),
    env: Dict(String, String),
    steps: List(Step),
    clone_depth: Option(Int),
    clone_skip: Bool,
  )
}

pub type Step {
  Step(name: String, command: String, env: Dict(String, String))
}

/// Start a CI workflow definition.
pub fn workflow(name: String) -> Workflow {
  Workflow(
    name: name,
    engine: "nixery",
    when: [],
    deps: dict.new(),
    env: dict.new(),
    steps: [],
    clone_depth: None,
    clone_skip: False,
  )
}

/// Override the engine (`nixery` by default).
pub fn engine(wf: Workflow, engine: String) -> Workflow {
  Workflow(..wf, engine: engine)
}

/// Add a push trigger for the given branches.
pub fn on_push(wf: Workflow, branches: List(String)) -> Workflow {
  Workflow(..wf, when: list.append(wf.when, [#(["push"], branches)]))
}

/// Add a pull_request trigger targeting the given branches.
pub fn on_pull_request(wf: Workflow, branches: List(String)) -> Workflow {
  Workflow(..wf, when: list.append(wf.when, [#(["pull_request"], branches)]))
}

/// Add nixpkgs dependencies — the toolchain the steps run against.
pub fn deps(wf: Workflow, packages: List(String)) -> Workflow {
  deps_from(wf, "nixpkgs", packages)
}

/// Add dependencies from a custom registry (a flake reference).
pub fn deps_from(
  wf: Workflow,
  registry: String,
  packages: List(String),
) -> Workflow {
  let updated = case dict.get(wf.deps, registry) {
    Ok(existing) -> list.append(existing, packages)
    Error(_) -> packages
  }
  Workflow(..wf, deps: dict.insert(wf.deps, registry, updated))
}

/// Set a workflow-level environment variable.
pub fn env(wf: Workflow, key: String, value: String) -> Workflow {
  Workflow(..wf, env: dict.insert(wf.env, key, value))
}

/// Append a step; steps run serially in one VM, from the workspace root.
pub fn step(wf: Workflow, name: String, command: String) -> Workflow {
  Workflow(
    ..wf,
    steps: list.append(wf.steps, [Step(name, command, dict.new())]),
  )
}

/// Append a step with step-scoped environment variables.
pub fn step_env(
  wf: Workflow,
  name: String,
  command: String,
  env: Dict(String, String),
) -> Workflow {
  Workflow(..wf, steps: list.append(wf.steps, [Step(name, command, env)]))
}

/// Set the clone depth (default 1).
pub fn clone_depth(wf: Workflow, depth: Int) -> Workflow {
  Workflow(..wf, clone_depth: Some(depth))
}

/// Skip the checkout entirely.
pub fn skip_clone(wf: Workflow) -> Workflow {
  Workflow(..wf, clone_skip: True)
}

/// The workflow file name `save` writes: `<name>.yml`.
pub fn file_name(wf: Workflow) -> String {
  case string.ends_with(wf.name, ".yml") || string.ends_with(wf.name, ".yaml") {
    True -> wf.name
    False -> wf.name <> ".yml"
  }
}

/// Render the workflow file.
///
/// Scalars are emitted as JSON strings — valid YAML by construction — and
/// commands as literal blocks when safe, so no YAML library is needed.
pub fn yaml(wf: Workflow) -> String {
  [
    when_section(wf),
    Some("engine: " <> wf.engine),
    deps_section(wf),
    env_section(wf),
    clone_section(wf),
    Some(steps_section(wf)),
  ]
  |> list.filter_map(fn(s) { option.to_result(s, Nil) })
  |> string.join("\n\n")
  |> string.append("\n")
}

fn q(s: String) -> String {
  json.to_string(json.string(s))
}

fn when_section(wf: Workflow) -> Option(String) {
  case wf.when {
    [] -> None
    constraints -> {
      let lines =
        list.flat_map(constraints, fn(c) {
          let #(events, branches) = c
          let head =
            "  - event: [" <> string.join(list.map(events, q), ", ") <> "]"
          case branches {
            [] -> [head]
            [one] -> [head, "    branch: " <> q(one)]
            many -> [
              head,
              "    branch: [" <> string.join(list.map(many, q), ", ") <> "]",
            ]
          }
        })
      Some(string.join(["when:", ..lines], "\n"))
    }
  }
}

fn deps_section(wf: Workflow) -> Option(String) {
  case dict.size(wf.deps) {
    0 -> None
    _ -> {
      let lines =
        wf.deps
        |> dict.keys()
        |> list.sort(string.compare)
        |> list.flat_map(fn(reg) {
          let packages = case dict.get(wf.deps, reg) {
            Ok(p) -> p
            Error(_) -> []
          }
          [
            "  " <> q(reg) <> ":",
            ..list.map(packages, fn(p) { "    - " <> q(p) })
          ]
        })
      Some(string.join(["dependencies:", ..lines], "\n"))
    }
  }
}

fn env_section(wf: Workflow) -> Option(String) {
  case dict.size(wf.env) {
    0 -> None
    _ -> {
      let lines =
        wf.env
        |> dict.keys()
        |> list.sort(string.compare)
        |> list.map(fn(k) {
          let value = case dict.get(wf.env, k) {
            Ok(v) -> v
            Error(_) -> ""
          }
          "  " <> k <> ": " <> q(value)
        })
      Some(string.join(["environment:", ..lines], "\n"))
    }
  }
}

fn clone_section(wf: Workflow) -> Option(String) {
  case wf.clone_skip, wf.clone_depth {
    False, None -> None
    skip, depth -> {
      let lines =
        ["clone:"]
        |> list.append(case skip {
          True -> ["  skip: true"]
          False -> []
        })
        |> list.append(case depth {
          Some(d) -> ["  depth: " <> int.to_string(d)]
          None -> []
        })
      Some(string.join(lines, "\n"))
    }
  }
}

fn steps_section(wf: Workflow) -> String {
  let lines =
    list.flat_map(wf.steps, fn(s) {
      ["  - name: " <> q(s.name)]
      |> list.append(command_lines(s.command))
      |> list.append(step_env_lines(s.env))
    })
  string.join(["steps:", ..lines], "\n")
}

/// A literal block when it round-trips byte-for-byte; a JSON string when it
/// cannot (trailing spaces, carriage returns) — never a silent alteration.
fn command_lines(command: String) -> List(String) {
  let block_safe =
    command != ""
    && !string.contains(command, "\r")
    && command
    |> string.split("\n")
    |> list.all(fn(l) { !string.ends_with(l, " ") })

  case block_safe {
    False -> ["    command: " <> q(command)]
    True -> {
      let body =
        command
        |> trim_trailing_newlines()
        |> string.split("\n")
        |> list.map(fn(l) { "      " <> l })
      ["    command: |", ..body]
    }
  }
}

fn trim_trailing_newlines(s: String) -> String {
  case string.ends_with(s, "\n") {
    True -> trim_trailing_newlines(string.drop_end(s, 1))
    False -> s
  }
}

fn step_env_lines(env: Dict(String, String)) -> List(String) {
  case dict.size(env) {
    0 -> []
    _ -> {
      let lines =
        env
        |> dict.keys()
        |> list.sort(string.compare)
        |> list.map(fn(k) {
          let value = case dict.get(env, k) {
            Ok(v) -> v
            Error(_) -> ""
          }
          "      " <> k <> ": " <> q(value)
        })
      ["    environment:", ..lines]
    }
  }
}

/// Write into `<repo>/.tangled/workflows/` and return the path.
pub fn save(
  wf: Workflow,
  repo: String,
) -> Result(String, simplifile.FileError) {
  let dir = repo <> "/.tangled/workflows"
  case simplifile.create_directory_all(dir) {
    Error(e) -> Error(e)
    Ok(_) -> {
      let path = dir <> "/" <> file_name(wf)
      let content =
        "# Generated by the bsdkrun SDK — edit the code that save()d it instead.\n"
        <> yaml(wf)
      case simplifile.write(path, content) {
        Ok(_) -> Ok(path)
        Error(e) -> Error(e)
      }
    }
  }
}

/// Execute the workflow in a microVM against the current directory,
/// streaming output. The YAML never touches the repository — it goes to a
/// temp file and `bsdkrun ci run -f`.
pub fn run(wf: Workflow) -> Result(Nil, Error) {
  run_in(wf, None)
}

/// `run` against an explicit repository directory.
pub fn run_in(wf: Workflow, dir: Option(String)) -> Result(Nil, Error) {
  let tmp = "/tmp/bsdkrun-ci-" <> wf.name
  let _ = simplifile.create_directory_all(tmp)
  let file = tmp <> "/" <> file_name(wf)
  let _ = simplifile.write(file, yaml(wf))

  let args =
    ["ci", "run", "-f", file]
    |> list.append(case dir {
      Some(d) -> ["-w", d]
      None -> []
    })

  let opts =
    cli.options()
    |> cli.with_stdout(fn(_line) { Nil })
  let result = cli.checked(args, "bsdkrun ci run", opts)
  let _ = simplifile.delete(tmp)
  case result {
    Ok(_) -> Ok(Nil)
    Error(e) -> Error(e)
  }
}
