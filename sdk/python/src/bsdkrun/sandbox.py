"""The :class:`Sandbox` handle — a running (or stopped) bsdkrun microVM."""

from __future__ import annotations

import json
import re
from collections.abc import Callable, Mapping, Sequence
from typing import Any

from .args import build_create_args
from .errors import CommandFailed, SandboxNotFound
from .filesystem import FileSystem
from .process import run, run_checked, spawn
from .types import Result, SandboxInfo

__all__ = ["Sandbox"]

_ID_RE = re.compile(r"^[0-9a-f]{6,}$")
_SSH_PORT_RE = re.compile(r"ssh -p (\d+)")


class Sandbox:
    """A handle to a running (or stopped) bsdkrun microVM.

    Create one with :meth:`create`, reconnect with :meth:`get`, or enumerate
    with :meth:`list`::

        box = Sandbox.create(os="linux", image="alpine")
        box.exec(["uname", "-a"])
        box.stop()
    """

    def __init__(self, sandbox_id: str, ssh_port: int | None = None) -> None:
        #: The machine's Docker-style short id.
        self.id = sandbox_id
        #: Host port forwarded to the guest's SSH, if the boot banner reported one.
        self.ssh_port = ssh_port
        #: Read and write files in the guest.
        self.fs = FileSystem(sandbox_id)

    def __repr__(self) -> str:
        return f"Sandbox(id={self.id!r})"

    # -- construction ------------------------------------------------------

    @classmethod
    def create(cls, *, os: str, log_level: int | None = None, **opts: Any) -> Sandbox:
        """Boot a new microVM (detached) and return a handle to it.

        ``os`` selects the guest kind (``linux``/``freebsd``/``netbsd``/
        ``firmware``/``kernel``); the remaining keyword options depend on it —
        see :func:`~bsdkrun.args.build_create_args`. ``log_level`` defaults to 1
        (boot diagnostics), unlike the other calls.
        """
        args = build_create_args(os=os, **opts)
        res = run(args, log_level=1 if log_level is None else log_level)
        if res.exit_code != 0:
            raise CommandFailed(res.exit_code, res.stdout, res.stderr, "bsdkrun create")

        # Detached runs print just the machine id on stdout — take the last
        # line that looks like one.
        machine_id: str | None = None
        for line in res.stdout.split("\n"):
            stripped = line.strip()
            if _ID_RE.match(stripped):
                machine_id = stripped
        if not machine_id:
            raise CommandFailed(
                res.exit_code,
                res.stdout,
                res.stderr,
                "bsdkrun create (no machine id in output)",
            )

        match = _SSH_PORT_RE.search(res.stderr)
        ssh_port = int(match.group(1)) if match else None
        return cls(machine_id, ssh_port)

    @classmethod
    def get(cls, sandbox_id: str) -> Sandbox:
        """Reconnect to an existing machine by id (a unique prefix is enough)."""
        for info in cls.list(all=True):
            if info.id == sandbox_id or info.id.startswith(sandbox_id) or info.name == sandbox_id:
                return cls(info.id)
        raise SandboxNotFound(sandbox_id)

    @classmethod
    def list(cls, all: bool = False) -> list[SandboxInfo]:
        """List machines. ``all=True`` includes exited ones (default: running)."""
        args = ["ps", "--json"]
        if all:
            args.append("--all")
        res = run_checked(args, "bsdkrun ps")
        rows = json.loads(res.stdout or "[]")
        return [SandboxInfo.from_row(row) for row in rows]

    # -- commands ----------------------------------------------------------

    def exec(
        self,
        command: str | Sequence[str],
        *,
        args: Sequence[str] | None = None,
        env: Mapping[str, str] | None = None,
        tty: bool = False,
        stdin: str | bytes | None = None,
        cwd: str | None = None,
        log_level: int | None = None,
        throw_on_error: bool = False,
        on_stdout: Callable[[bytes], None] | None = None,
        on_stderr: Callable[[bytes], None] | None = None,
    ) -> Result:
        """Run a command in the guest through its exec agent.

        ``command`` may be an argv list or a bare program name string (combined
        with ``args``). ``env`` sets per-command variables (``-e K=V``), ``tty``
        allocates a PTY, ``cwd`` runs in a working directory, and ``stdin`` is
        piped to the command. With ``throw_on_error`` a non-zero exit raises
        :class:`CommandFailed`. ``on_stdout`` and ``on_stderr`` receive byte
        chunks in real time while the same output remains available in the
        returned result.

            box.exec(["ls", "-la", "/etc"])
            box.exec("node", args=["-e", "print(1)"], env={"X": "1"})
        """
        if isinstance(command, str):
            argv = [command, *(args or [])]
        else:
            argv = list(command)

        if cwd:
            # Emulate a working directory: cd, drop it, then exec the real argv.
            argv = ["/bin/sh", "-c", 'cd "$1" && shift && exec "$@"', "sh", cwd, *argv]

        cli_args = ["exec"]
        if tty:
            cli_args.append("-t")
        for key, value in (env or {}).items():
            cli_args += ["-e", f"{key}={value}"]
        cli_args.append(self.id)
        cli_args += argv

        res = run(
            cli_args, stdin=stdin, log_level=log_level, on_stdout=on_stdout, on_stderr=on_stderr
        )
        result = Result(res.stdout, res.stderr, res.exit_code, f"exec {' '.join(argv)}")
        if throw_on_error:
            result.throw_if_failed()
        return result

    def run_command(
        self,
        command: str,
        args: Sequence[str] | None = None,
        **opts: Any,
    ) -> Result:
        """Alias for :meth:`exec`: a program plus its args."""
        return self.exec(command, args=args or [], **opts)

    def logs(self, boot: bool = False) -> str:
        """Read the machine's console log (or bsdkrun's boot log if ``boot``)."""
        args = ["logs"]
        if boot:
            args.append("--boot")
        args.append(self.id)
        return run(args).stdout

    def shell(self) -> int:
        """Attach an interactive shell to the machine (inherits the terminal).

        Blocks until the shell exits and returns its exit code.
        """
        return spawn(["shell", self.id])

    # -- inspection --------------------------------------------------------

    def status(self) -> SandboxInfo | None:
        """Fetch this machine's current status row, or ``None`` if it's gone."""
        for info in Sandbox.list(all=True):
            if info.id == self.id:
                return info
        return None

    def is_running(self) -> bool:
        """Whether the machine is currently running."""
        info = self.status()
        return bool(info and info.running)

    # -- lifecycle ---------------------------------------------------------

    def stop(self) -> None:
        """Stop the machine (BSD: clean power-off; Linux: SIGTERM)."""
        run_checked(["stop", self.id], "bsdkrun stop")

    def start(self) -> None:
        """Restart a stopped machine in place — same id, disk/rootfs, network."""
        run_checked(["start", self.id], "bsdkrun start")

    def remove(self, force: bool = False) -> None:
        """Remove the machine and its state. ``force`` stops it first if running."""
        args = ["rm"]
        if force:
            args.append("--force")
        args.append(self.id)
        run_checked(args, "bsdkrun rm")

    def update(self, *, cpus: int | None = None, mem: int | None = None) -> None:
        """Change the recorded vCPU / RAM. Applies on the next :meth:`start`."""
        args = ["update", self.id]
        if cpus is not None:
            args += ["--cpus", str(cpus)]
        if mem is not None:
            args += ["--mem", str(mem)]
        run_checked(args, "bsdkrun update")

    def connect_network(self, network: str) -> None:
        """Join or switch this machine to a global network (next :meth:`start`)."""
        run_checked(["network", "connect", self.id, network], "bsdkrun network connect")

    def disconnect_network(self) -> None:
        """Detach this machine from its network. Applies on the next start."""
        run_checked(["network", "disconnect", self.id], "bsdkrun network disconnect")

    # -- in-guest agent helpers -------------------------------------------

    def _agent(
        self,
        family: str,
        action: Sequence[str],
        env: Mapping[str, str] | None = None,
    ) -> Result:
        res = run([family, self.id, *action], env=env)
        result = Result(res.stdout, res.stderr, res.exit_code, f"{family} {' '.join(action)}")
        return result.throw_if_failed()

    def ssh_setup(
        self,
        *,
        user: str | None = None,
        key: str | Sequence[str] | None = None,
    ) -> Result:
        """Install SSH keys in the guest (``ssh setup``, via the agent).

        With no ``key``, the CLI installs your local ``~/.ssh/*.pub`` keys.
        ``key`` may be a literal ``ssh-...`` string or a local ``.pub`` path (or
        a list of them).
        """
        action = ["setup"]
        if user:
            action += ["--user", user]
        for entry in _key_list(key):
            action += ["--key", entry]
        return self._agent("ssh", action)

    def tailscale_up(
        self,
        *,
        authkey: str | None = None,
        hostname: str | None = None,
        args: Sequence[str] | None = None,
    ) -> Result:
        """Put a guest on your tailnet (``tailscale setup``, via the agent).

        ``authkey`` is forwarded as the ``TS_AUTHKEY`` env var (kept off the arg
        list). ``hostname`` sets the machine name on the tailnet.
        """
        action = ["setup"]
        if hostname:
            action += ["--hostname", hostname]
        if args:
            action += list(args)
        env = {"TS_AUTHKEY": authkey} if authkey else None
        return self._agent("tailscale", action, env)


def _key_list(key: str | Sequence[str] | None) -> list[str]:
    if not key:
        return []
    if isinstance(key, str):
        return [key]
    return list(key)
