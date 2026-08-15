#!/usr/bin/env python3
"""An interactive console with the bsdkrun SDK preloaded.

    cd sdk/python && uv run console.py

Starts IPython (it ships in the ``dev`` dependency group) and falls back to the
stdlib REPL if it isn't installed, so the console works from a bare checkout
too.

By default the SDK finds the binary the usual way — ``$BSDKRUN_BIN``, then
``bsdkrun`` on ``$PATH``, then an in-repo dev build. Point it somewhere else
for the session with ``--bin``:

    uv run console.py --bin ../../target/release/bsdkrun
"""

from __future__ import annotations

import argparse
from typing import Any

import bsdkrun
from bsdkrun import (
    Sandbox,
    build_create_args,
    images,
    networks,
    reset_binary_cache,
    resolve_binary,
    run,
    run_checked,
    set_binary_path,
    spawn,
    system,
    volumes,
)
from bsdkrun.errors import (
    BinaryNotFound,
    BsdkrunError,
    CommandFailed,
    SandboxNotFound,
)

BANNER = """\
bsdkrun {version} — interactive console
binary: {binary}

  Sandbox            create / get / list machines
  images, volumes    host-level image and volume operations
  networks, system   global networks; probe, fetch, versions, grow_disk
  run, run_checked   low-level CLI helpers (argv in, RawResult out)
  ps()               shorthand for Sandbox.list(all=True)

  sbx = Sandbox.create(os="linux", image="alpine")
  sbx.exec(["uname", "-a"]).text()
  sbx.stop()
"""


def ps(all: bool = True) -> list[Any]:
    """Every machine, exited ones included — a shorthand for poking around."""
    return Sandbox.list(all=all)


def _binary_line() -> str:
    try:
        return resolve_binary()
    except BinaryNotFound as exc:
        return f"NOT FOUND ({exc})"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--bin",
        metavar="PATH",
        help="use this bsdkrun binary instead of the discovered one",
    )
    args = parser.parse_args()

    if args.bin:
        set_binary_path(args.bin)

    namespace = {
        "bsdkrun": bsdkrun,
        "Sandbox": Sandbox,
        "images": images,
        "volumes": volumes,
        "networks": networks,
        "system": system,
        "run": run,
        "run_checked": run_checked,
        "spawn": spawn,
        "build_create_args": build_create_args,
        "set_binary_path": set_binary_path,
        "resolve_binary": resolve_binary,
        "reset_binary_cache": reset_binary_cache,
        "BsdkrunError": BsdkrunError,
        "BinaryNotFound": BinaryNotFound,
        "CommandFailed": CommandFailed,
        "SandboxNotFound": SandboxNotFound,
        "ps": ps,
    }

    banner = BANNER.format(version=bsdkrun.__version__, binary=_binary_line())

    try:
        from IPython import start_ipython
        from traitlets.config import Config
    except ImportError:
        import code

        code.interact(banner=banner, local=namespace, exitmsg="")
        return

    config = Config()
    config.TerminalInteractiveShell.banner1 = banner
    config.TerminalInteractiveShell.confirm_exit = False
    start_ipython(argv=[], user_ns=namespace, config=config)


if __name__ == "__main__":
    main()
