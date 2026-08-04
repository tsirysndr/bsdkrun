"""Global-network operations — reach machines by name on a shared subnet."""

from __future__ import annotations

import builtins
import json
from collections.abc import Sequence

from .process import run_checked
from .sandbox import Sandbox
from .types import NetworkInfo, SandboxInfo

__all__ = [
    "list",
    "create",
    "remove",
    "connect",
    "disconnect",
    "sync",
    "members",
]


def list() -> builtins.list[NetworkInfo]:
    """List global networks and their member counts."""
    res = run_checked(["network", "ls", "--json"], "bsdkrun network ls")
    rows = json.loads(res.stdout or "[]")
    return [NetworkInfo.from_row(row) for row in rows]


def create(name: str) -> None:
    """Create a global network (starts its shared switch)."""
    run_checked(["network", "create", name], "bsdkrun network create")


def remove(names: str | Sequence[str], *, force: bool = False) -> None:
    """Remove one or more networks. ``force`` allows removal with live members."""
    name_list = [names] if isinstance(names, str) else [*names]
    args = ["network", "rm"]
    if force:
        args.append("--force")
    args += name_list
    run_checked(args, "bsdkrun network rm")


def connect(machine: str, network: str) -> None:
    """Join or switch a machine (by id or name) to a network (next start)."""
    run_checked(["network", "connect", machine, network], "bsdkrun network connect")


def disconnect(machine: str) -> None:
    """Detach a machine from its network. Applies on its next start."""
    run_checked(["network", "disconnect", machine], "bsdkrun network disconnect")


def sync(network: str) -> None:
    """Refresh members' ``/etc/hosts`` so peers resolve by name (notably NetBSD)."""
    run_checked(["network", "sync", network], "bsdkrun network sync")


def members(network: str) -> builtins.list[SandboxInfo]:
    """The machines currently attached to ``network`` (running or stopped)."""
    return [m for m in Sandbox.list(all=True) if m.network == network]
