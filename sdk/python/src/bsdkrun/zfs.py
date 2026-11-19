"""ZFS volumes: one zpool per volume, branched copy-on-write in the guest."""

from __future__ import annotations

import builtins
import json
from collections.abc import Sequence

from .process import run_checked
from .types import ZfsBranch, ZfsVolumeInfo

__all__ = ["list", "branches", "create", "branch", "attach", "detach", "destroy"]


def list() -> builtins.list[ZfsVolumeInfo]:
    """List ZFS volumes, with their branches and holder."""
    res = run_checked(["zfs", "ls", "--json"], "bsdkrun zfs ls")
    rows = json.loads(res.stdout or "[]")
    return [ZfsVolumeInfo.from_row(row) for row in rows]


def branches(volume: str) -> builtins.list[ZfsBranch]:
    """The branches of one volume."""
    res = run_checked(["zfs", "ls", volume, "--json"], "bsdkrun zfs ls")
    rows = json.loads(res.stdout or "[]")
    return [ZfsBranch.from_row(row) for row in rows]


def create(name: str, *, size: str | None = None) -> str:
    """Create a volume: a sparse image that becomes a zpool with a ``main``
    branch the first time a guest imports it. ``size`` like ``"10G"``."""
    args = ["zfs", "create", name]
    if size:
        args += ["--size", size]
    return run_checked(args, "bsdkrun zfs create").stdout.strip()


def branch(volume: str, name: str, *, from_branch: str | None = None) -> str:
    """Fork a branch copy-on-write (``zfs snapshot`` + ``zfs clone``) from
    ``from_branch`` (``main`` by default). Runs in the machine holding the
    volume or in a short-lived FreeBSD helper, so it can take as long as a
    BSD boot. Returns the new ``VOL@BRANCH``."""
    args = ["zfs", "branch", volume, name]
    if from_branch:
        args += ["--from", from_branch]
    return run_checked(args, "bsdkrun zfs branch").stdout.strip()


def attach(target: str, machine: str, *, mount: str | None = None) -> str:
    """Attach ``VOL`` or ``VOL@BRANCH`` to a machine at ``mount`` (``/mnt/VOL``
    or ``/mnt/VOL-BRANCH`` by default). A pool is imported by one machine at
    a time; a stopped machine picks it up on start, a running holder mounts
    the branch live. Returns the CLI's one-line report."""
    args = ["zfs", "attach", target, machine]
    if mount:
        args += ["--mount", mount]
    return run_checked(args, "bsdkrun zfs attach").stdout.strip()


def detach(target: str, machine: str) -> str:
    """Detach one branch (``VOL@BRANCH``) or the whole volume (``VOL``)."""
    return run_checked(["zfs", "detach", target, machine], "bsdkrun zfs detach").stdout.strip()


def destroy(targets: str | Sequence[str], *, force: bool = False) -> None:
    """Destroy branches (``VOL@BRANCH``) or whole volumes (``VOL``)."""
    target_list = [targets] if isinstance(targets, str) else [*targets]
    args = ["zfs", "destroy"]
    if force:
        args.append("--force")
    args += target_list
    run_checked(args, "bsdkrun zfs destroy")
