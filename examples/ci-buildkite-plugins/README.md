# ci-buildkite-plugins — real Buildkite plugins, run locally

A Buildkite plugin is a git repository of shell hooks, and `bsdkrun ci`
runs them for real: the plugin is cloned at its ref, its configuration is
exported as `BUILDKITE_PLUGIN_<NAME>_<KEY>` (nested maps flattened, arrays
indexed — Buildkite's own scheme), and its hooks wrap the command in the
agent's own order: `environment` sourced, `pre-command`, the command,
`post-command` with the exit status preserved.

This example uses the real
[`improbable-eng/metahook`](https://github.com/improbable-eng/metahook-buildkite-plugin)
plugin: its `environment` hook runs the configured snippet, and the
command asserts the snippet's side effect actually happened — the proof
that the whole lifecycle executed. (Hooks that `exec`, as metahook's do,
run in a subshell: their commands execute for real, while an exec can
never replace the step's own shell.)

Plugins that shell out to `docker` will fail honestly in the guest (a
microVM runs no Docker daemon) — that is the plugin's own output, not a
silent skip.

CI runs the repository's **HEAD commit**, so the example needs its own git
repository:

```sh
cp -r examples/ci-buildkite-plugins /tmp/ci-buildkite-plugins
cd /tmp/ci-buildkite-plugins
git init -q && git add -A && git commit -qm init
bsdkrun ci run
```
