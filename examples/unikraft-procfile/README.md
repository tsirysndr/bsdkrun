# Procfile on Unikraft

A project whose start command comes from a `Procfile`, and whose environment
comes from `railpack.json` — neither of which the provider could infer.

## Build

```sh
bsdkrun pack .
```

## Run

```sh
bsdkrun unikraft . --cmdline "python -- /opt/python/bin/python3 -u /src/serve.py"
```

## Try it

```sh
curl http://<vm-ip>:8080/
```

```json
{
  "message": "Hello from a Procfile on Unikraft!",
  "python": "3.12.13",
  "greeting": "hello from deploy.variables",
  "port": 8080
}
```

## What the Procfile decides

The Python provider looks for `main.py`, `app.py` or `server.py` and finds none
of them — the entry point here is `serve.py`. The `Procfile` is what says so.

Process types are chosen in railpack's order: **`web`**, then **`worker`**, then
whatever was declared first. That middle step matters for a Procfile carrying
only background processes: falling straight to "first declared" would pick
`release: migrate`, a command that exits immediately and leaves the guest dead.

A unikernel runs exactly one program, so the others are named as ignored rather
than silently dropped.

### Commands need absolute guest paths

`web: python serve.py` is the Heroku spelling and it will **not** work here.
There is no shell to resolve `python` against `PATH`, and the working directory
is not the source tree. Write what the guest will actually execute:

```
web: /opt/python/bin/python3 -u /src/serve.py
```

`bsdkrun pack` prints the exact argv it will boot with, so a wrong path shows up
before you boot rather than after.

## What railpack.json decides

```json
{
  "packages": { "python": "3.12", "jq": "latest" },
  "deploy": { "variables": { "GREETING": "...", "PORT": "8080" } }
}
```

| Field | Effect |
| ----- | ------ |
| `packages.python` | The provider's own version pin |
| `packages.jq` | An extra build-time tool, installed with mise |
| `deploy.variables` | The guest's environment |

A unikernel has no shell to export anything, so `deploy.variables` are compiled
into the image as kconfig. The indices are **allocated, not fixed** — the Python
provider already holds ENVP4 through ENVP6 for `PYTHONHOME` and friends, so these
land at ENVP7 and ENVP8:

```
CONFIG_LIBPOSIX_ENVIRON_ENVP6: "PYTHONDONTWRITEBYTECODE=1"
CONFIG_LIBPOSIX_ENVIRON_ENVP7: "GREETING=hello from deploy.variables"
CONFIG_LIBPOSIX_ENVIRON_ENVP8: "PORT=8080"
```

Setting a variable that already exists replaces it in place instead of adding a
second entry, so overriding `PATH` or `HOME` does what you would expect.

## Secrets

`railpack.json`'s `secrets` names values the build may read:

```json
{ "secrets": ["NPM_TOKEN"] }
```

Each is mounted at `/run/secrets/<name>` for the command that needs it, with the
value taken from the environment variable of the same name — so `export
NPM_TOKEN=...` locally and a repository secret in CI reach the build the same
way. A secret mount is not a layer: a token used to fetch a private dependency
does not stay readable in the finished image, which matters here because the
image is a unikernel pushed to a registry whole.
