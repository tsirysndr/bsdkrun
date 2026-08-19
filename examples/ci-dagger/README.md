# ci-dagger

A [Dagger](https://dagger.io) module, run locally by `bsdkrun ci` with nothing
installed but bsdkrun.

```sh
bsdkrun ci run -w .
```

There is no CI configuration here — the module *is* the pipeline. bsdkrun
detects it (`dagger.json`; `dagger.toml` and `dagger-module.toml` on the 1.0
line are detected too), boots a microVM with a Docker daemon, installs the
dagger CLI, pulls the engine and calls a function. The `ci` function runs a
container through the engine and prints `dagger-example-ok`.

With no function named it takes the first of `ci`, `test`, `build`, `all`
the module exposes; otherwise name one:

```sh
bsdkrun ci run --dagger-call container-echo -w .
```

## From a tangled workflow

The same environment is available to a workflow that asks for it, and there
the steps are dagger functions — the engine has already been named, so
repeating `dagger call` on every line would be noise:

```yaml
when:
  - event: ["manual"]

engine: dagger

steps:
  - name: functions
    command: dagger functions   # anything starting with `dagger` runs as-is
  - name: ci
    command: ci                 # → dagger call ci
```

## What it costs

The engine is a container image of some hundreds of megabytes, pulled into a
fresh VM on every run, so expect a few minutes end to end. Two things shorten
it: `BSDKRUN_CI_DAGGER_IMAGE=ghcr.io/tsirysndr/bsdkrun-flavor-dagger:latest`
boots an image with the CLI already in it, and `BSDKRUN_CI_DAGGER_VERSION`
pins the CLI so it matches whatever engine you already have cached.
