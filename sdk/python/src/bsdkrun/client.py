"""A client for a remote ``bsdkrund`` daemon's GraphQL API.

:class:`Sandbox` (see ``sandbox.py``) talks to a *local* ``bsdkrun`` binary by
shelling out to it. :class:`Client` is the network sibling: it drives the
exact same operations against a daemon over HTTP (queries/mutations) and a
hand-rolled ``graphql-transport-ws`` socket (subscriptions — exec output,
live shells, log follow), so a program can target either a machine with the
CLI installed or a remote host running ``bsdkrund`` with the same calls.

The GraphQL documents below are deliberately minimal string literals rather
than a generated client: this SDK has no code-generation step, and the
schema is small and stable enough (``daemon/src/graphql.rs``) that hand-typed
queries stay easy to keep in sync.
"""

from __future__ import annotations

import base64
import builtins
import os
import threading
from collections.abc import Callable, Mapping
from queue import Queue
from typing import Any

from .errors import GraphQLError
from .transport import TOKEN_ENV, URL_ENV, WSTransport, http_request, normalize_url, ws_url
from .types import CommandResult, ExecResult, PortForward, SandboxInfo, ShellSessionInfo

__all__ = ["Client", "ShellSession"]

# ---------------------------------------------------------------------------
# GraphQL documents
# ---------------------------------------------------------------------------

_MACHINE_FIELDS = (
    "id name image kind command status running exitCode pid detached "
    "cpus mem volume stateDir createdAt finishedAt network netIp "
    "ports { bind host guest }"
)
_CMD_RESULT_FIELDS = "exitCode stdout stderr"
_SESSION_FIELDS = "id machineId finished truncated"

_LIST_QUERY = f"query($all: Boolean!) {{ machines(all: $all) {{ {_MACHINE_FIELDS} }} }}"
_GET_QUERY = f"query($id: String!) {{ machine(id: $id) {{ {_MACHINE_FIELDS} }} }}"
_LOGS_QUERY = "query($id: String!, $boot: Boolean!) { machineLogs(id: $id, boot: $boot) }"

_STOP_MUTATION = f"mutation($id: String!) {{ stopMachine(id: $id) {{ {_CMD_RESULT_FIELDS} }} }}"
_START_MUTATION = f"mutation($id: String!) {{ startMachine(id: $id) {{ {_CMD_RESULT_FIELDS} }} }}"
_REMOVE_MUTATION = (
    f"mutation($ids: [String!]!, $force: Boolean!) {{ "
    f"removeMachines(ids: $ids, force: $force) {{ {_CMD_RESULT_FIELDS} }} }}"
)
_UPDATE_MUTATION = (
    f"mutation($id: String!, $cpus: Int, $mem: Int) {{ "
    f"updateMachine(id: $id, cpus: $cpus, mem: $mem) {{ {_CMD_RESULT_FIELDS} }} }}"
)
_COMMIT_MUTATION = (
    f"mutation($id: String!, $name: String!, $description: String!) {{ "
    f"commitMachine(id: $id, name: $name, description: $description) {{ {_CMD_RESULT_FIELDS} }} }}"
)

_RUN_LINUX_MUTATION = "mutation($input: RunLinuxInput!) { runLinux(input: $input) }"
_RUN_BSD_MUTATION = "mutation($input: RunBsdInput!) { runBsd(input: $input) }"
_RUN_NANOS_MUTATION = "mutation($input: RunNanosInput!) { runNanos(input: $input) }"
_RUN_UNIKRAFT_MUTATION = "mutation($input: RunUnikraftInput!) { runUnikraft(input: $input) }"
_RUN_SOLO5_MUTATION = "mutation($input: RunSolo5Input!) { runSolo5(input: $input) }"
_RUN_OSV_MUTATION = "mutation($input: RunOsvInput!) { runOsv(input: $input) }"
_RUN_FLAVOR_MUTATION = "mutation($input: RunFlavorInput!) { runFlavor(input: $input) }"

_MACHINE_LOGS_SUBSCRIPTION = (
    "subscription($id: String!, $follow: Boolean!, $boot: Boolean!) { "
    "machineLogs(id: $id, follow: $follow, boot: $boot) { dataBase64 exitCode } }"
)

_OPEN_SHELL_MUTATION = (
    "mutation($machineId: String!, $command: [String!]!, $env: [String!]!, "
    "$rows: Int!, $cols: Int!) { "
    "openShell(machineId: $machineId, command: $command, env: $env, "
    f"rows: $rows, cols: $cols) {{ {_SESSION_FIELDS} }} }}"
)
_SHELL_OUTPUT_SUBSCRIPTION = (
    "subscription($sessionId: String!) { "
    "shellOutput(sessionId: $sessionId) { dataBase64 exitCode } }"
)
_SEND_INPUT_MUTATION = (
    "mutation($sessionId: String!, $dataBase64: String!) { "
    "sendShellInput(sessionId: $sessionId, dataBase64: $dataBase64) }"
)
_RESIZE_MUTATION = (
    "mutation($sessionId: String!, $rows: Int!, $cols: Int!) { "
    "resizeShell(sessionId: $sessionId, rows: $rows, cols: $cols) }"
)
_CLOSE_MUTATION = "mutation($sessionId: String!) { closeShell(sessionId: $sessionId) }"


# ---------------------------------------------------------------------------
# RunSpec -> GraphQL input dicts
#
# Mirrors args.py's build_create_args: **opts is a loosely-typed grab bag
# (the convention Sandbox.create() already established), and a per-os
# builder function pulls out the fields it needs with .get()/defaults. Field
# names are exact 1:1 snake_case renderings of the GraphQL input object
# fields in daemon/src/graphql.rs (e.g. kernel_version -> kernelVersion).
# ---------------------------------------------------------------------------


def _port_strings(ports: Any) -> list[str]:
    """Normalize a ports list to ``"HOST:GUEST"`` strings.

    Accepts the same shapes ``args.py``'s ``_net_args`` does: bare strings,
    :class:`~bsdkrun.types.PortForward`, ``{"host":.., "guest":..}`` mappings,
    or ``(host, guest)`` tuples/lists — so a ``net`` dict built for the local
    :meth:`~bsdkrun.sandbox.Sandbox.create` can be reused verbatim here.
    """
    out: list[str] = []
    for port in ports or []:
        if isinstance(port, str):
            out.append(port)
        elif isinstance(port, PortForward):
            out.append(f"{port.host}:{port.guest}")
        elif isinstance(port, Mapping):
            out.append(f"{port['host']}:{port['guest']}")
        elif isinstance(port, (tuple, list)):
            out.append(f"{port[0]}:{port[1]}")
        else:
            raise TypeError(f"invalid port forward: {port!r}")
    return out


def _net_input(net: Mapping[str, Any] | None) -> dict[str, Any] | None:
    """Build a ``NetInput`` from a ``net`` option mapping.

    ``no_net`` is the exact GraphQL field name; ``disabled`` is accepted too,
    as an alias, since that's the key ``args.py``'s local-CLI ``net`` mapping
    convention already uses (so one ``net`` dict works against both
    :meth:`~bsdkrun.sandbox.Sandbox.create` and this remote client).
    """
    if net is None:
        return None
    no_net = net.get("no_net")
    if no_net is None:
        no_net = net.get("disabled")
    return {
        "noNet": bool(no_net),
        "ports": _port_strings(net.get("ports")),
        "mac": net.get("mac"),
        "network": net.get("network"),
        "name": net.get("name"),
    }


def _bsd_os_enum(os_kind: str) -> str:
    value = os_kind.strip().lower()
    if value == "freebsd":
        return "FREEBSD"
    if value == "netbsd":
        return "NETBSD"
    raise ValueError(f"run_bsd requires os='freebsd' or 'netbsd', got {os_kind!r}")


def _require(opts: Mapping[str, Any], field: str, method: str) -> Any:
    value = opts.get(field)
    if not value:
        raise ValueError(f"{method}() requires a {field!r} option")
    return value


def _run_linux_input(opts: Mapping[str, Any]) -> dict[str, Any]:
    return {
        "image": _require(opts, "image", "run_linux"),
        "cpus": opts.get("cpus"),
        "mem": opts.get("mem"),
        "net": _net_input(opts.get("net")),
        "volume": opts.get("volume"),
        "mounts": list(opts.get("mounts") or []),
        "env": list(opts.get("env") or []),
        "entrypoint": opts.get("entrypoint"),
        "initramfs": bool(opts.get("initramfs", False)),
        "kernel": opts.get("kernel"),
        "kernelVersion": opts.get("kernel_version"),
        "console": opts.get("console"),
        "repo": opts.get("repo"),
        "command": list(opts.get("command") or []),
    }


def _run_bsd_input(opts: Mapping[str, Any]) -> dict[str, Any]:
    return {
        "os": _bsd_os_enum(_require(opts, "os", "run_bsd")),
        "version": opts.get("version"),
        "cpus": opts.get("cpus"),
        "mem": opts.get("mem"),
        "net": _net_input(opts.get("net")),
        "volume": opts.get("volume"),
        "persist": bool(opts.get("persist", False)),
        "force": bool(opts.get("force", False)),
        "firmware": opts.get("firmware"),
        "attachDisk": list(opts.get("attach_disk") or []),
        "diskSize": opts.get("disk_size"),
        "repo": opts.get("repo"),
        "command": list(opts.get("command") or []),
    }


def _run_nanos_input(opts: Mapping[str, Any]) -> dict[str, Any]:
    return {
        "image": _require(opts, "image", "run_nanos"),
        "cpus": opts.get("cpus"),
        "mem": opts.get("mem"),
        "net": _net_input(opts.get("net")),
        "kernel": opts.get("kernel"),
        "cmdline": opts.get("cmdline"),
        "persist": bool(opts.get("persist", False)),
    }


def _run_unikraft_input(opts: Mapping[str, Any]) -> dict[str, Any]:
    return {
        "path": opts.get("path"),
        "cpus": opts.get("cpus"),
        "mem": opts.get("mem"),
        "net": _net_input(opts.get("net")),
        "cmdline": opts.get("cmdline"),
        "initramfs": opts.get("initramfs"),
        "mounts": list(opts.get("mounts") or []),
    }


def _run_solo5_input(opts: Mapping[str, Any]) -> dict[str, Any]:
    # Solo5 (MirageOS) runs under the `solo5-hvt` tender rather than libkrun.
    # The unikernel declares its own network and block devices in its MFT1
    # manifest note, so only what the host alone can know is asked for: block
    # backing files ("NAME=FILE") and the unikernel's own args (e.g.
    # "--ipv4=10.0.0.2/24"). Like unikraft, no disk and no agent.
    return {
        "path": opts.get("path"),
        "cpus": opts.get("cpus"),
        "mem": opts.get("mem"),
        "net": _net_input(opts.get("net")),
        "block": list(opts.get("block") or []),
        "args": list(opts.get("args") or []),
    }


def _run_osv_input(opts: Mapping[str, Any]) -> dict[str, Any]:
    return {
        "image": _require(opts, "image", "run_osv"),
        "cpus": opts.get("cpus"),
        "mem": opts.get("mem"),
        "net": _net_input(opts.get("net")),
        "cmdline": opts.get("cmdline"),
        "disk": opts.get("disk"),
        "noDisk": bool(opts.get("no_disk", False)),
        "attachDisk": list(opts.get("attach_disk") or []),
        "gic": opts.get("gic"),
        "persist": bool(opts.get("persist", False)),
        "volume": opts.get("volume"),
    }


def _run_flavor_input(opts: Mapping[str, Any]) -> dict[str, Any]:
    return {
        "name": _require(opts, "name", "run_flavor"),
        "cpus": opts.get("cpus"),
        "mem": opts.get("mem"),
        "ports": _port_strings(opts.get("ports")),
        "volume": opts.get("volume"),
        "repo": opts.get("repo"),
    }


# ---------------------------------------------------------------------------
# Client
# ---------------------------------------------------------------------------


class Client:
    """A client for a remote ``bsdkrund``'s GraphQL API.

    Queries and mutations go over HTTP; subscriptions (used internally by
    :meth:`exec`, :meth:`shell` and :meth:`follow_logs`) share one lazily
    opened ``graphql-transport-ws`` socket per ``Client`` instance, torn down
    once the last subscription ends.
    """

    def __init__(self, *, url: str, token: str) -> None:
        self.url = normalize_url(url)
        self.token = token
        self._ws_lock = threading.Lock()
        self._ws: WSTransport | None = None

    def __repr__(self) -> str:
        return f"Client(url={self.url!r})"

    @classmethod
    def from_env(cls) -> Client:
        """Build a client from ``BSDKRUN_URL``/``BSDKRUN_TOKEN``.

        Raises :class:`ValueError` if ``BSDKRUN_URL`` is unset (nothing to
        connect to), or if it is set but ``BSDKRUN_TOKEN`` is not — a host
        configured without a token is a configuration error, never a silent
        fall-back to an unauthenticated request.
        """
        url = os.environ.get(URL_ENV, "").strip()
        if not url:
            raise ValueError(f"{URL_ENV} is not set; nothing to connect to")
        token = os.environ.get(TOKEN_ENV, "").strip()
        if not token:
            raise ValueError(f"{URL_ENV} is set but {TOKEN_ENV} is not")
        return cls(url=url, token=token)

    # -- transport (escape hatch) --------------------------------------------

    def request(self, query: str, variables: dict[str, Any] | None = None) -> dict[str, Any]:
        """Run a raw query or mutation and return its ``data``."""
        return http_request(self.url, self.token, query, variables)

    def subscribe(
        self,
        query: str,
        variables: dict[str, Any],
        *,
        on_next: Callable[[Any], None],
        on_error: Callable[[Exception], None] | None = None,
        on_complete: Callable[[], None] | None = None,
    ) -> Callable[[], None]:
        """Start a raw subscription. Returns an unsubscribe function."""
        transport = self._ws_transport()
        sub_id = transport.subscribe(query, variables, on_next, on_error, on_complete)
        return lambda: transport.unsubscribe(sub_id)

    def _ws_transport(self) -> WSTransport:
        with self._ws_lock:
            if self._ws is None:
                self._ws = WSTransport(ws_url(self.url), self.token)
            return self._ws

    # -- lifecycle / listing --------------------------------------------------

    def list(self, all: bool = False) -> list[SandboxInfo]:
        """List machines. ``all=True`` includes exited ones."""
        data = self.request(_LIST_QUERY, {"all": all})
        return [SandboxInfo.from_graphql(m) for m in data.get("machines") or []]

    def get(self, id: str) -> SandboxInfo | None:
        """Fetch one machine by id (a unique prefix) or name, or ``None``."""
        data = self.request(_GET_QUERY, {"id": id})
        m = data.get("machine")
        return SandboxInfo.from_graphql(m) if m else None

    def stop(self, id: str) -> CommandResult:
        data = self.request(_STOP_MUTATION, {"id": id})
        return CommandResult.from_graphql(data["stopMachine"])

    def start(self, id: str) -> CommandResult:
        data = self.request(_START_MUTATION, {"id": id})
        return CommandResult.from_graphql(data["startMachine"])

    def remove(self, ids: builtins.list[str], force: bool = False) -> CommandResult:
        data = self.request(_REMOVE_MUTATION, {"ids": ids, "force": force})
        return CommandResult.from_graphql(data["removeMachines"])

    def update(self, id: str, *, cpus: int | None = None, mem: int | None = None) -> CommandResult:
        data = self.request(_UPDATE_MUTATION, {"id": id, "cpus": cpus, "mem": mem})
        return CommandResult.from_graphql(data["updateMachine"])

    def commit(self, id: str, name: str, description: str = "") -> CommandResult:
        data = self.request(_COMMIT_MUTATION, {"id": id, "name": name, "description": description})
        return CommandResult.from_graphql(data["commitMachine"])

    def logs(self, id: str, boot: bool = False) -> str:
        """One-shot read of a machine's console log."""
        data = self.request(_LOGS_QUERY, {"id": id, "boot": boot})
        return str(data.get("machineLogs") or "")

    def follow_logs(
        self,
        id: str,
        *,
        follow: bool = True,
        boot: bool = False,
        on_data: Callable[[bytes], None],
        on_error: Callable[[Exception], None] | None = None,
        on_complete: Callable[[], None] | None = None,
    ) -> Callable[[], None]:
        """Stream a machine's console log live. Returns an unsubscribe function."""
        transport = self._ws_transport()

        def _on_next(data: Any) -> None:
            payload = (data or {}).get("machineLogs")
            if not payload:
                return
            b64 = payload.get("dataBase64")
            if b64:
                on_data(base64.b64decode(b64))
            # exitCode marks the stream's end; graphql-transport-ws follows
            # it with its own "complete" message, which fires on_complete.

        sub_id = transport.subscribe(
            _MACHINE_LOGS_SUBSCRIPTION,
            {"id": id, "follow": follow, "boot": boot},
            _on_next,
            on_error,
            on_complete,
        )
        return lambda: transport.unsubscribe(sub_id)

    # -- booting ----------------------------------------------------------

    def run_linux(self, **opts: Any) -> str:
        data = self.request(_RUN_LINUX_MUTATION, {"input": _run_linux_input(opts)})
        return str(data["runLinux"])

    def run_bsd(self, **opts: Any) -> str:
        data = self.request(_RUN_BSD_MUTATION, {"input": _run_bsd_input(opts)})
        return str(data["runBsd"])

    def run_nanos(self, **opts: Any) -> str:
        data = self.request(_RUN_NANOS_MUTATION, {"input": _run_nanos_input(opts)})
        return str(data["runNanos"])

    def run_unikraft(self, **opts: Any) -> str:
        data = self.request(_RUN_UNIKRAFT_MUTATION, {"input": _run_unikraft_input(opts)})
        return str(data["runUnikraft"])

    def run_solo5(self, **opts: Any) -> str:
        data = self.request(_RUN_SOLO5_MUTATION, {"input": _run_solo5_input(opts)})
        return str(data["runSolo5"])

    def run_osv(self, **opts: Any) -> str:
        data = self.request(_RUN_OSV_MUTATION, {"input": _run_osv_input(opts)})
        return str(data["runOsv"])

    def run_flavor(self, **opts: Any) -> str:
        data = self.request(_RUN_FLAVOR_MUTATION, {"input": _run_flavor_input(opts)})
        return str(data["runFlavor"])

    # -- exec / interactive shell ------------------------------------------

    def exec(
        self, id: str, command: builtins.list[str], env: builtins.list[str] | None = None
    ) -> ExecResult:
        """Run a command to completion via the machine's shell agent.

        Sequenced exactly as ``daemon/README.md`` describes: `openShell`
        (with ``command`` set, so the session runs it instead of a login
        shell), THEN subscribe to `shellOutput` (output is buffered from the
        moment the session opened, so nothing is lost even though the
        subscribe necessarily happens after the mutation), collecting bytes
        until an event carries a non-null exit code, THEN `closeShell` —
        called unconditionally, including on error, since it is idempotent
        and a session must never be left dangling.
        """
        transport = self._ws_transport()
        data = self.request(
            _OPEN_SHELL_MUTATION,
            {
                "machineId": id,
                "command": list(command),
                "env": list(env or []),
                "rows": 24,
                "cols": 80,
            },
        )
        session = ShellSessionInfo.from_graphql(data["openShell"])

        chunks: list[bytes] = []
        # A background reader thread (owned by the WSTransport) delivers
        # `shellOutput` events via callbacks; this queue is how the calling
        # thread blocks until the one it cares about (an exit code, or a
        # terminal error) arrives, keeping exec() a synchronous call.
        done: Queue[tuple[str, Any]] = Queue(maxsize=1)

        def _on_next(payload: Any) -> None:
            p = (payload or {}).get("shellOutput")
            if not p:
                return
            b64 = p.get("dataBase64")
            if b64:
                chunks.append(base64.b64decode(b64))
            code = p.get("exitCode")
            if code is not None:
                done.put(("exit", code))

        def _on_error(err: Exception) -> None:
            done.put(("error", err))

        def _on_complete() -> None:
            # The subscription ended without ever delivering an exit code
            # (e.g. the daemon tore the session down) — surface that instead
            # of blocking forever.
            done.put(("error", GraphQLError("shell session ended before an exit code arrived")))

        sub_id: str | None = None
        try:
            sub_id = transport.subscribe(
                _SHELL_OUTPUT_SUBSCRIPTION,
                {"sessionId": session.id},
                _on_next,
                _on_error,
                _on_complete,
            )
            kind, value = done.get()
        finally:
            if sub_id is not None:
                transport.unsubscribe(sub_id)
            try:
                self.request(_CLOSE_MUTATION, {"sessionId": session.id})
            except GraphQLError:
                pass  # closeShell is idempotent; an already-gone session is not a failure

        if kind == "error":
            raise value
        return ExecResult(exit_code=int(value), output=b"".join(chunks))

    def shell(
        self,
        id: str,
        *,
        command: builtins.list[str] | None = None,
        env: builtins.list[str] | None = None,
        rows: int = 24,
        cols: int = 80,
    ) -> ShellSession:
        """Open a live interactive session. Output/exit arrive via callbacks."""
        transport = self._ws_transport()
        data = self.request(
            _OPEN_SHELL_MUTATION,
            {
                "machineId": id,
                "command": list(command or []),
                "env": list(env or []),
                "rows": rows,
                "cols": cols,
            },
        )
        session_info = ShellSessionInfo.from_graphql(data["openShell"])
        session = ShellSession(self, transport, session_info.id)
        session._start()
        return session


class ShellSession:
    """A live interactive session opened by :meth:`Client.shell`.

    Output and exit events arrive on the shared :class:`~bsdkrun.transport.WSTransport`'s
    background reader thread and are handed to whatever callback is
    registered via :meth:`on_output`/:meth:`on_exit` at the time they arrive.
    Anything that arrives *before* a callback is registered (a real
    possibility — the subscription is started synchronously in
    :meth:`Client.shell` and the daemon can reply before the caller gets
    around to calling `on_output`) is buffered and flushed the moment a
    callback is set, so a caller that does
    ``s = client.shell(id); s.on_output(cb)`` never silently loses a frame.
    """

    def __init__(self, client: Client, transport: WSTransport, session_id: str) -> None:
        self.id = session_id
        self._client = client
        self._transport = transport
        self._sub_id: str | None = None
        self._closed = False

        self._cb_lock = threading.Lock()
        self._output_cb: Callable[[bytes], None] | None = None
        self._exit_cb: Callable[[int], None] | None = None
        self._buffered_output: list[bytes] = []
        self._buffered_exit: int | None = None
        self._exit_fired = False

    def __repr__(self) -> str:
        return f"ShellSession(id={self.id!r})"

    def _start(self) -> None:
        self._sub_id = self._transport.subscribe(
            _SHELL_OUTPUT_SUBSCRIPTION,
            {"sessionId": self.id},
            self._on_next,
            self._on_error,
            self._on_complete,
        )

    def _on_next(self, payload: Any) -> None:
        p = (payload or {}).get("shellOutput")
        if not p:
            return
        b64 = p.get("dataBase64")
        if b64:
            self._emit_output(base64.b64decode(b64))
        code = p.get("exitCode")
        if code is not None:
            self._emit_exit(int(code))

    def _on_error(self, _err: Exception) -> None:
        # A dropped connection ends the session the same way an exit would,
        # so a caller has one place (on_exit) to notice the session is gone.
        # -1 has no exit-code meaning of its own; it just isn't 0.
        self._emit_exit(-1)

    def _on_complete(self) -> None:
        pass

    def _emit_output(self, data: bytes) -> None:
        with self._cb_lock:
            cb = self._output_cb
            if cb is None:
                self._buffered_output.append(data)
        if cb is not None:
            cb(data)

    def _emit_exit(self, code: int) -> None:
        with self._cb_lock:
            if self._exit_fired:
                return
            self._exit_fired = True
            cb = self._exit_cb
            if cb is None:
                self._buffered_exit = code
        if cb is not None:
            cb(code)

    def on_output(self, cb: Callable[[bytes], None]) -> None:
        with self._cb_lock:
            self._output_cb = cb
            buffered = self._buffered_output
            self._buffered_output = []
        for chunk in buffered:
            cb(chunk)

    def on_exit(self, cb: Callable[[int], None]) -> None:
        with self._cb_lock:
            self._exit_cb = cb
            pending = self._buffered_exit
        if pending is not None:
            cb(pending)

    def write(self, data: bytes | str) -> None:
        raw = data.encode("utf-8") if isinstance(data, str) else data
        self._client.request(
            _SEND_INPUT_MUTATION,
            {"sessionId": self.id, "dataBase64": base64.b64encode(raw).decode("ascii")},
        )

    def resize(self, rows: int, cols: int) -> None:
        self._client.request(_RESIZE_MUTATION, {"sessionId": self.id, "rows": rows, "cols": cols})

    def close(self) -> None:
        """Close the session and kill its command. Idempotent."""
        if self._closed:
            return
        self._closed = True
        if self._sub_id is not None:
            self._transport.unsubscribe(self._sub_id)
        try:
            self._client.request(_CLOSE_MUTATION, {"sessionId": self.id})
        except GraphQLError:
            pass
