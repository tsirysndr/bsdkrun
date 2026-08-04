"""Typed data structures returned by the SDK.

The ``*Info`` dataclasses mirror the JSON rows emitted by the ``bsdkrun`` CLI's
``--json`` output (snake_case field names). :class:`Result` is the captured
result of a guest ``exec``.
"""

from __future__ import annotations

import json as _json
from collections.abc import Mapping
from dataclasses import dataclass
from typing import Any

from .errors import CommandFailed

__all__ = [
    "PortForward",
    "Result",
    "SandboxInfo",
    "ImageInfo",
    "VolumeInfo",
    "NetworkInfo",
]


def _num(value: Any) -> int | None:
    """Coerce to int, mapping ``None`` through."""
    return None if value is None else int(value)


@dataclass(frozen=True)
class PortForward:
    """A host to guest TCP port forward, e.g. ``PortForward(2222, 22)``."""

    host: int
    guest: int


@dataclass(frozen=True)
class Result:
    """The captured result of running a command in a guest.

    Returned by :meth:`~bsdkrun.sandbox.Sandbox.exec`.
    """

    stdout: str
    stderr: str
    exit_code: int
    command: str

    @property
    def ok(self) -> bool:
        """Whether the command succeeded (exit 0)."""
        return self.exit_code == 0

    def text(self) -> str:
        """``stdout`` with trailing newlines trimmed — the common case."""
        return self.stdout.rstrip("\n")

    def json(self) -> Any:
        """Parse ``stdout`` as JSON."""
        return _json.loads(self.stdout)

    def lines(self) -> list[str]:
        """Non-empty ``stdout`` lines."""
        return [line for line in self.stdout.split("\n") if line]

    def throw_if_failed(self) -> Result:
        """Raise :class:`CommandFailed` if the command exited non-zero."""
        if self.exit_code != 0:
            raise CommandFailed(self.exit_code, self.stdout, self.stderr, self.command)
        return self


@dataclass
class SandboxInfo:
    """A machine as reported by ``bsdkrun ps --json``."""

    id: str
    name: str | None
    image: str
    kind: str
    command: str
    status: str
    running: bool
    exit_code: int | None
    pid: int | None
    detached: bool
    cpus: int
    mem: int
    volume: str | None
    state_dir: str
    network: str | None
    net_ip: str | None
    created_at: int
    finished_at: int | None

    @classmethod
    def from_row(cls, row: Mapping[str, Any]) -> SandboxInfo:
        running = bool(row.get("running"))
        return cls(
            id=str(row.get("id")),
            name=row.get("name"),
            image=str(row.get("image")),
            kind=str(row.get("kind")),
            command=str(row.get("command") or ""),
            status="running" if running else "exited",
            running=running,
            exit_code=_num(row.get("exit_code")),
            pid=_num(row.get("pid")),
            detached=bool(row.get("detached")),
            cpus=int(row.get("cpus") or 0),
            mem=int(row.get("mem") or 0),
            volume=row.get("volume"),
            state_dir=str(row.get("state_dir")),
            network=row.get("network"),
            net_ip=row.get("net_ip"),
            created_at=int(row.get("created_at") or 0),
            finished_at=_num(row.get("finished_at")),
        )


@dataclass
class ImageInfo:
    """An image as reported by ``bsdkrun images --json``."""

    id: str
    reference: str
    digest: str
    size: int
    rootfs: str
    created_at: int

    @classmethod
    def from_row(cls, row: Mapping[str, Any]) -> ImageInfo:
        return cls(
            id=str(row.get("id")),
            reference=str(row.get("reference")),
            digest=str(row.get("digest")),
            size=int(row.get("size") or 0),
            rootfs=str(row.get("rootfs")),
            created_at=int(row.get("created_at") or 0),
        )


@dataclass
class VolumeInfo:
    """A volume as reported by ``bsdkrun volume ls --json``."""

    name: str
    guest: str | None
    base: str | None
    path: str
    size: str
    created_at: int | None
    tracked: bool

    @classmethod
    def from_row(cls, row: Mapping[str, Any]) -> VolumeInfo:
        return cls(
            name=str(row.get("name")),
            guest=row.get("guest"),
            base=row.get("base"),
            path=str(row.get("path")),
            size=str(row.get("size")),
            created_at=_num(row.get("created_at")),
            tracked=bool(row.get("tracked")),
        )


@dataclass
class NetworkInfo:
    """A global network as reported by ``bsdkrun network ls --json``."""

    name: str
    subnet: str
    gateway: str
    members: int
    running: int
    up: bool
    created_at: int | None

    @classmethod
    def from_row(cls, row: Mapping[str, Any]) -> NetworkInfo:
        return cls(
            name=str(row.get("name")),
            subnet=str(row.get("subnet")),
            gateway=str(row.get("gateway")),
            members=int(row.get("members") or 0),
            running=int(row.get("running") or 0),
            up=bool(row.get("up")),
            created_at=_num(row.get("created_at")),
        )
