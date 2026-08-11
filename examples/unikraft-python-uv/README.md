# Python + uv on Unikraft

A uv-managed Python service running as a Unikraft unikernel, built with
`bsdkrun pack`.

Where [`../unikraft-python`](../unikraft-python) is stdlib-only, this one has a
real locked dependency — so it exercises the part of the provider that resolves
and installs third-party packages.

## Build

```sh
bsdkrun pack .
```

## Run

```sh
bsdkrun unikraft . --cmdline "python -- /opt/python/bin/python3 -u /src/main.py"
```

## Try it

```sh
curl http://<vm-ip>:8080/
```

## How the lock file is used

A `uv.lock` is the most specific manifest a Python project can carry, so it wins
over `Pipfile`, `pyproject.toml` and `requirements.txt`:

```sh
uv export --frozen --no-dev --no-emit-project --format requirements-txt -o /tmp/requirements.txt
uv pip install --python "$PY" -r /tmp/requirements.txt
```

`--frozen` means the lock is obeyed rather than re-resolved: a build that
silently upgraded a dependency would not be reproducible, which is most of the
point of committing a lock file.

Packages install into `/opt/python`'s own `site-packages` rather than a
virtualenv — one prefix for the guest to find, not two. uv itself is copied in
from its official image, so no installer is fetched mid-build.

Bottle serves on the standard library's `wsgiref` server, so what reaches the
guest is one extra pure-Python package, not a second web server.
