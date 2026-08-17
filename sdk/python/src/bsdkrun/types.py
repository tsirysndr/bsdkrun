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
    "CommandResult",
    "ShellSessionInfo",
    "ExecResult",
    "SnapshotInfo",
    "DockerStatus",
    "DockerContainer",
]


def _num(value: Any) -> int | None:
    """Coerce to int, mapping ``None`` through."""
    return None if value is None else int(value)


@dataclass(frozen=True)
class PortForward:
    """A host to guest TCP port forward, e.g. ``PortForward(2222, 22)``.

    ``bind`` is the host interface the forward is bound to (``127.0.0.1`` by
    default, or ``0.0.0.0`` for a LAN-reachable forward). It is always
    populated on :attr:`SandboxInfo.ports`; it's optional when constructing a
    ``PortForward`` to pass as input, since the CLI defaults it too.
    """

    host: int
    guest: int
    bind: str = "127.0.0.1"

    @classmethod
    def from_row(cls, row: Mapping[str, Any]) -> PortForward:
        return cls(
            host=int(row.get("host") or 0),
            guest=int(row.get("guest") or 0),
            bind=str(row.get("bind") or "127.0.0.1"),
        )


@dataclass(frozen=True)
class SnapshotInfo:
    """A machine snapshot: one machine's disk state, captured under a name.

    A copy-on-write clone rather than a memory image — the files the guest
    wrote, not what it was executing. Boot a new machine from it with
    :meth:`~bsdkrun.client.Client.branch`, or put it back over the machine it
    came from with :meth:`~bsdkrun.client.Client.restore`.
    """

    id: str
    name: str
    machine_id: str
    machine_name: str
    kind: str
    image: str
    path: str
    parent: str | None
    description: str
    cpus: int
    mem: int
    ports: list[PortForward]
    size: str | None
    created_at: int

    @classmethod
    def from_graphql(cls, s: Mapping[str, Any]) -> SnapshotInfo:
        return cls(
            id=str(s.get("id")),
            name=str(s.get("name")),
            machine_id=str(s.get("machineId") or ""),
            machine_name=str(s.get("machineName") or ""),
            kind=str(s.get("kind") or ""),
            image=str(s.get("image") or ""),
            path=str(s.get("path") or ""),
            parent=s.get("parent"),
            description=str(s.get("description") or ""),
            cpus=int(s.get("cpus") or 0),
            mem=int(s.get("mem") or 0),
            ports=[PortForward.from_row(p) for p in s.get("ports") or []],
            size=s.get("size"),
            created_at=int(s.get("createdAt") or 0),
        )

    @classmethod
    def from_row(cls, row: Mapping[str, Any]) -> SnapshotInfo:
        """Build from ``bsdkrun snapshots --json`` (snake_case)."""
        return cls(
            id=str(row.get("id")),
            name=str(row.get("name")),
            machine_id=str(row.get("machine_id") or ""),
            machine_name=str(row.get("machine_name") or ""),
            kind=str(row.get("kind") or ""),
            image=str(row.get("image") or ""),
            path=str(row.get("path") or ""),
            parent=row.get("parent"),
            description=str(row.get("description") or ""),
            cpus=int(row.get("cpus") or 0),
            mem=int(row.get("mem") or 0),
            ports=[PortForward.from_row(p) for p in row.get("ports") or []],
            size=row.get("size"),
            created_at=int(row.get("created_at") or 0),
        )


@dataclass(frozen=True)
class DockerStatus:
    """The Docker engine VM: whether it is up, and how to reach it.

    bsdkrun runs one ``docker:dind`` microVM and serves its API on a host unix
    socket, so the host's own ``docker`` CLI drives it.
    """

    running: bool
    machine_id: str | None
    machine_running: bool
    #: The unix socket the ``docker`` CLI talks to.
    socket: str
    socket_ready: bool
    api_port: int | None
    version: str | None
    containers: int | None
    images: int | None
    #: Host directories shared into the VM, each ``HOST:GUEST``.
    mounts: list[str]
    #: The dedicated image-store disk, when the VM has one, and its size in
    #: bytes — sparse, so the cap rather than the usage.
    disk: str | None
    disk_size: int | None

    @classmethod
    def from_graphql(cls, s: Mapping[str, Any]) -> DockerStatus:
        return cls(
            running=bool(s.get("running")),
            machine_id=s.get("machineId"),
            machine_running=bool(s.get("machineRunning")),
            socket=str(s.get("socket") or ""),
            socket_ready=bool(s.get("socketReady")),
            api_port=_num(s.get("apiPort")),
            version=s.get("version"),
            containers=_num(s.get("containers")),
            images=_num(s.get("images")),
            mounts=list(s.get("mounts") or []),
            disk=s.get("disk"),
            disk_size=_num(s.get("diskSize")),
        )

    @classmethod
    def from_row(cls, row: Mapping[str, Any]) -> DockerStatus:
        """Build from ``bsdkrun docker status --json`` (snake_case)."""
        return cls(
            running=bool(row.get("running")),
            machine_id=row.get("machine_id"),
            machine_running=bool(row.get("machine_running")),
            socket=str(row.get("socket") or ""),
            socket_ready=bool(row.get("socket_ready")),
            api_port=_num(row.get("api_port")),
            version=row.get("version"),
            containers=_num(row.get("containers")),
            images=_num(row.get("images")),
            mounts=list(row.get("mounts") or []),
            disk=row.get("disk"),
            disk_size=_num(row.get("disk_size")),
        )


@dataclass(frozen=True)
class DockerContainer:
    """A container in the Docker engine VM — a trimmed ``docker ps`` row."""

    id: str
    name: str
    image: str
    command: str
    #: "running" | "exited" | "created" | "paused" | ...
    state: str
    #: Docker's human status, e.g. "Up 3 minutes".
    status: str
    #: Published forwards, each ``HOST:GUEST/proto`` — mirrored onto the host.
    ports: list[str]
    created: int

    @property
    def running(self) -> bool:
        return self.state == "running"

    @classmethod
    def from_graphql(cls, c: Mapping[str, Any]) -> DockerContainer:
        return cls(
            id=str(c.get("id") or ""),
            name=str(c.get("name") or ""),
            image=str(c.get("image") or ""),
            command=str(c.get("command") or ""),
            state=str(c.get("state") or ""),
            status=str(c.get("status") or ""),
            ports=list(c.get("ports") or []),
            created=int(c.get("created") or 0),
        )

    #: ``bsdkrun docker ps --json`` uses the same field names.
    from_row = from_graphql


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
    ports: list[PortForward]
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
            ports=[PortForward.from_row(p) for p in row.get("ports") or []],
            created_at=int(row.get("created_at") or 0),
            finished_at=_num(row.get("finished_at")),
        )

    @classmethod
    def from_graphql(cls, m: Mapping[str, Any]) -> SandboxInfo:
        """Build from a GraphQL ``Machine`` (the ``MACHINE_FIELDS`` selection:
        ``id name image kind command status running exitCode pid detached
        cpus mem volume stateDir createdAt finishedAt network netIp
        ports{bind host guest}``).

        The schema is camelCase; ``created_at``/``finished_at`` arrive as
        decimal-string unix timestamps (the daemon passes the CLI's own text
        through unchanged) rather than numbers, so they're parsed here same
        as everything else.
        """
        running = bool(m.get("running"))
        return cls(
            id=str(m.get("id")),
            name=m.get("name"),
            image=str(m.get("image")),
            kind=str(m.get("kind")),
            command=str(m.get("command") or ""),
            status=str(m.get("status") or ("running" if running else "exited")),
            running=running,
            exit_code=_num(m.get("exitCode")),
            pid=_num(m.get("pid")),
            detached=bool(m.get("detached")),
            cpus=int(m.get("cpus") or 0),
            mem=int(m.get("mem") or 0),
            volume=m.get("volume"),
            state_dir=str(m.get("stateDir") or ""),
            network=m.get("network"),
            net_ip=m.get("netIp"),
            ports=[PortForward.from_row(p) for p in m.get("ports") or []],
            created_at=int(m.get("createdAt") or 0),
            finished_at=_num(m.get("finishedAt")),
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


@dataclass(frozen=True)
class CommandResult:
    """The outcome of a remote lifecycle mutation (stop/start/remove/update/commit).

    Mirrors the GraphQL ``CommandResult`` type. A non-zero ``exit_code`` is
    reported rather than raised: for some underlying commands (``ssh
    status``, ``tailscale status``) it is a legitimate state to display, not
    a transport failure.
    """

    exit_code: int
    stdout: str
    stderr: str

    @classmethod
    def from_graphql(cls, r: Mapping[str, Any]) -> CommandResult:
        return cls(
            exit_code=int(r.get("exitCode") or 0),
            stdout=str(r.get("stdout") or ""),
            stderr=str(r.get("stderr") or ""),
        )


@dataclass(frozen=True)
class ShellSessionInfo:
    """A shell session as reported by ``openShell`` / ``shellSessions``."""

    id: str
    machine_id: str
    finished: bool
    truncated: bool

    @classmethod
    def from_graphql(cls, s: Mapping[str, Any]) -> ShellSessionInfo:
        return cls(
            id=str(s.get("id")),
            machine_id=str(s.get("machineId")),
            finished=bool(s.get("finished")),
            truncated=bool(s.get("truncated")),
        )


@dataclass(frozen=True)
class ExecResult:
    """The captured result of :meth:`bsdkrun.client.Client.exec`.

    Unlike :class:`Result` (the local CLI's captured stdout/stderr as text),
    a remote exec's output is a single interleaved byte stream — the shell
    agent's ``shellOutput`` subscription does not distinguish stdout from
    stderr — so this carries raw ``bytes`` instead.
    """

    exit_code: int
    output: bytes
