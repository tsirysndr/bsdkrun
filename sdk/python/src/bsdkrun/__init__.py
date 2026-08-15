"""bsdkrun — a Python SDK for `bsdkrun <https://github.com/tsirysndr/bsdkrun>`_,
a Firecracker-style microVM launcher for BSD, Linux, and unikernel guests.

A thin, dependency-free wrapper around the ``bsdkrun`` CLI: it builds argv,
shells out, and parses the JSON output.

    from bsdkrun import Sandbox, networks

    sbx = Sandbox.create(os="linux", image="alpine")
    print(sbx.exec(["uname", "-a"]).text())
    sbx.stop()

Host-level operations live in the :mod:`bsdkrun.images`, :mod:`bsdkrun.volumes`,
:mod:`bsdkrun.networks`, and :mod:`bsdkrun.system` namespaces.
"""

from __future__ import annotations

from . import caches, images, networks, system, volumes
from .args import build_create_args
from .binary import reset_binary_cache, resolve_binary, set_binary_path
from .cache import Cache, CacheEntry, RestoreResult
from .client import Client, ShellSession
from .errors import (
    AuthError,
    BinaryNotFound,
    BsdkrunError,
    CommandFailed,
    GraphQLError,
    SandboxNotFound,
)
from .filesystem import FileSystem, FileTransferError
from .process import BinaryResult, RawResult, run, run_binary, run_checked, spawn
from .sandbox import Sandbox
from .types import (
    CommandResult,
    ExecResult,
    ImageInfo,
    NetworkInfo,
    PortForward,
    Result,
    SandboxInfo,
    ShellSessionInfo,
    VolumeInfo,
)

__version__ = "0.2.0"

__all__ = [
    # sandbox (local CLI)
    "Sandbox",
    # client (remote daemon)
    "Client",
    "ShellSession",
    # namespaces
    "images",
    "volumes",
    "networks",
    "system",
    "caches",
    # binary resolution
    "set_binary_path",
    "resolve_binary",
    "reset_binary_cache",
    # low-level process helpers
    "run",
    "run_binary",
    "run_checked",
    "spawn",
    "RawResult",
    "BinaryResult",
    # guest filesystem
    "FileSystem",
    # guest directory cache
    "Cache",
    "CacheEntry",
    "RestoreResult",
    # argv builder
    "build_create_args",
    # data types
    "SandboxInfo",
    "ImageInfo",
    "VolumeInfo",
    "NetworkInfo",
    "PortForward",
    "Result",
    "CommandResult",
    "ShellSessionInfo",
    "ExecResult",
    # errors
    "BsdkrunError",
    "BinaryNotFound",
    "CommandFailed",
    "SandboxNotFound",
    "FileTransferError",
    "GraphQLError",
    "AuthError",
    # metadata
    "__version__",
]
