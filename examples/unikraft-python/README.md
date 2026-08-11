# Python on Unikraft

A CPython HTTP service running as a Unikraft unikernel, built with `bsdkrun pack`.

There is no `Dockerfile`, no `Kraftfile` and no `build.sh` here — `pack` detects
the project and generates all three internally.

## Build

```sh
bsdkrun pack .
```

## Run

```sh
bsdkrun unikraft . --cmdline "python -- /opt/python/bin/python3 -u /src/main.py"
```

`pack` prints that exact command when it finishes, so there is no need to
remember it.

## Try it

```sh
curl http://<vm-ip>:8080/
```

```json
{
  "message": "Hello from Python on Unikraft!",
  "python": "3.12.11",
  "machine": "arm64",
  "path": "/"
}
```

## How the interpreter gets there

Unlike the other interpreted examples here, this one does not start from an
official language image. `pack`'s Python provider installs the interpreter with
[mise](https://mise.jdx.dev) into a single prefix, `/opt/python`, and copies that
prefix into the rootfs whole.

That matters because Python finds its own standard library *relative to the
executable*: `bin/python3` implies `lib/python3.x/` beside it. Splitting the
interpreter across `/usr/bin` and `/usr/lib`, the way the Ruby and PHP providers
do, would leave it unable to locate its own `os.py`.

mise's builds help twice over. They come precompiled, so no C toolchain is
needed in the build image, and they link OpenSSL, SQLite and zlib statically —
so what lands in the rootfs is that one prefix plus libc, instead of a scatter
of system libraries.

The version comes from `mise.toml`. A `.tool-versions` file works too, as does
no pin at all (3.12 by default).

## Dependencies

This example is deliberately stdlib-only, so it has nothing to install. A real
project's manifest is picked up automatically, most-specific first:

| File                     | Installed with                        |
| ------------------------ | ------------------------------------- |
| `uv.lock`                | `uv export` → `uv pip install`        |
| `Pipfile` / `Pipfile.lock` | `pipenv requirements` → `uv pip install` |
| `pyproject.toml`         | `uv pip install .`                    |
| `requirements.txt`       | `uv pip install -r`                   |

Packages install into `/opt/python`'s own `site-packages` rather than a
virtualenv — one prefix for the guest to find, not two.

## Guest environment

The provider sets three variables in the generated `Kraftfile`:

| Variable                  | Why                                                        |
| ------------------------- | ---------------------------------------------------------- |
| `PYTHONHOME`              | Points at `/opt/python`, so the prefix needs no deducing    |
| `PYTHONUNBUFFERED`        | Buffered output on a serial console is lost in a crash      |
| `PYTHONDONTWRITEBYTECODE` | The rootfs is a read-only ramdisk; a `.pyc` write can only fail |
