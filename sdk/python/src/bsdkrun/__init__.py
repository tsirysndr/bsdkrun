"""bsdkrun — a Python SDK for `bsdkrun <https://github.com/tsirysndr/bsdkrun>`_,
a Firecracker-style microVM launcher for BSD and Linux guests.

A thin, dependency-free wrapper around the ``bsdkrun`` CLI: it builds argv,
shells out, and parses the JSON output.

    from bsdkrun import Sandbox, networks

    box = Sandbox.create(os="linux", image="alpine")
    print(box.exec(["uname", "-a"]).text())
    box.stop()

Host-level operations live in the :mod:`bsdkrun.images`, :mod:`bsdkrun.volumes`,
:mod:`bsdkrun.networks`, and :mod:`bsdkrun.system` namespaces.
"""

from __future__ import annotations

from . import images, networks, system, volumes
from .args import build_create_args
from .binary import reset_binary_cache, resolve_binary, set_binary_path
from .errors import (
    BinaryNotFound,
    BsdkrunError,
    CommandFailed,
    SandboxNotFound,
)
from .process import RawResult, run, run_checked, spawn
from .sandbox import Sandbox
from .types import (
    ImageInfo,
    NetworkInfo,
    PortForward,
    Result,
    SandboxInfo,
    VolumeInfo,
)

__version__ = "0.1.0"

__all__ = [
    # sandbox
    "Sandbox",
    # namespaces
    "images",
    "volumes",
    "networks",
    "system",
    # binary resolution
    "set_binary_path",
    "resolve_binary",
    "reset_binary_cache",
    # low-level process helpers
    "run",
    "run_checked",
    "spawn",
    "RawResult",
    # argv builder
    "build_create_args",
    # data types
    "SandboxInfo",
    "ImageInfo",
    "VolumeInfo",
    "NetworkInfo",
    "PortForward",
    "Result",
    # errors
    "BsdkrunError",
    "BinaryNotFound",
    "CommandFailed",
    "SandboxNotFound",
    # metadata
    "__version__",
]
