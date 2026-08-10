"""Exception types raised by the bsdkrun SDK."""

from __future__ import annotations

__all__ = [
    "BsdkrunError",
    "BinaryNotFound",
    "CommandFailed",
    "SandboxNotFound",
    "GraphQLError",
    "AuthError",
]


class BsdkrunError(Exception):
    """Base class for every error the SDK raises."""


class BinaryNotFound(BsdkrunError):
    """The ``bsdkrun`` binary could not be located on the host."""

    def __init__(self, searched: list[str]) -> None:
        self.searched = list(searched)
        super().__init__(
            'could not find the "bsdkrun" binary. Set BSDKRUN_BIN, add it to '
            "PATH, or call set_binary_path(). Looked in: " + ", ".join(self.searched)
        )


class CommandFailed(BsdkrunError):
    """A ``bsdkrun`` invocation exited non-zero.

    Carries the process ``exit_code`` and the captured ``stdout`` / ``stderr``.
    """

    def __init__(
        self,
        exit_code: int,
        stdout: str,
        stderr: str,
        command: str,
    ) -> None:
        self.exit_code = exit_code
        self.stdout = stdout
        self.stderr = stderr
        self.command = command
        message = f"command failed (exit {exit_code}): {command}"
        if stderr.strip():
            message += "\n" + stderr.strip()
        super().__init__(message)


class SandboxNotFound(BsdkrunError):
    """No machine matched the given id / prefix."""

    def __init__(self, sandbox_id: str) -> None:
        self.sandbox_id = sandbox_id
        super().__init__(f"no sandbox found matching id {sandbox_id!r}")


class GraphQLError(BsdkrunError):
    """A GraphQL- or transport-level failure talking to a remote ``bsdkrund``.

    Raised by :class:`~bsdkrun.client.Client`. ``code`` carries the
    response's ``extensions.code`` when the daemon set one (e.g.
    ``"INVALID_ARGUMENT"``, ``"FAILED"``); it is ``None`` for a transport
    failure (the daemon was unreachable) or a malformed response.
    """

    def __init__(self, message: str, code: str | None = None) -> None:
        self.code = code
        super().__init__(message)


class AuthError(GraphQLError):
    """The daemon rejected the bearer token.

    Raised on an HTTP 401, a GraphQL error with
    ``extensions.code == "UNAUTHENTICATED"``, or the WebSocket closing before
    ``connection_ack`` was ever received.
    """

    def __init__(self, message: str = "the daemon rejected this token") -> None:
        super().__init__(message, "UNAUTHENTICATED")
