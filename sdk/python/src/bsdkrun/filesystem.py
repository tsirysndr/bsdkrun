"""Files in a running sandbox — ``sandbox.fs``.

Every call goes through the guest's exec agent, so the sandbox has to be
running; there is no offline write.
"""

from __future__ import annotations

import os
import re
from pathlib import Path

from .errors import BsdkrunError
from .process import run, run_binary

__all__ = ["FileSystem", "FileTransferError"]

_ERROR_PREFIX = re.compile(r"^Error:\s*")


class FileTransferError(BsdkrunError):
    """A filesystem operation the guest refused."""

    def __init__(self, message: str, path: str) -> None:
        #: The path that could not be transferred.
        self.path = path
        super().__init__(message)


class FileSystem:
    """Read and write files inside a running microVM.

    Reached as :attr:`~bsdkrun.sandbox.Sandbox.fs`::

        sbx.fs.write_file("/app/main.py", "print('hi')")
        text = sbx.fs.read_text("/app/out.json")
        sbx.fs.upload("./src", "/app/src")
        sbx.fs.download("/app/dist", "./dist", recursive=True)
    """

    def __init__(self, sandbox_id: str) -> None:
        self.id = sandbox_id

    def __repr__(self) -> str:
        return f"FileSystem(id={self.id!r})"

    def write_file(self, path: str, data: str | bytes) -> None:
        """Write ``data`` to ``path`` in the guest, creating parent directories.

        ``str`` is encoded as UTF-8; ``bytes`` is written as-is.
        """
        payload = data.encode("utf-8") if isinstance(data, str) else data
        result = run(["cp", "-", f"{self.id}:{path}"], stdin=payload)
        if result.exit_code != 0:
            raise FileTransferError(_message(result.stderr, path), path)

    def read_file(self, path: str) -> bytes:
        """Read ``path`` from the guest as bytes."""
        result = run_binary(["cp", f"{self.id}:{path}", "-"])
        if result.exit_code != 0:
            raise FileTransferError(_message(result.stderr, path), path)
        return result.stdout

    def read_text(self, path: str, encoding: str = "utf-8") -> str:
        """Read ``path`` from the guest and decode it."""
        return self.read_file(path).decode(encoding)

    def upload(self, local_path: str | os.PathLike[str], remote_path: str) -> None:
        """Copy a host file or directory into the guest.

        A directory's *contents* land in ``remote_path``, so
        ``upload("./src", "/app/src")`` leaves the guest's ``/app/src`` holding
        what ``./src`` holds. Whether it recurses is decided by looking at the
        local path, so callers do not have to say which kind of thing it is.
        """
        local = Path(local_path)
        if not local.exists():
            raise FileTransferError(f"cannot upload {local}: no such file or directory", str(local))
        args = ["cp"]
        if local.is_dir():
            args.append("-r")
        args += [str(local), f"{self.id}:{remote_path}"]
        result = run(args)
        if result.exit_code != 0:
            raise FileTransferError(_message(result.stderr, str(local)), str(local))

    def download(
        self,
        remote_path: str,
        local_path: str | os.PathLike[str],
        *,
        recursive: bool = False,
    ) -> None:
        """Copy a file or directory out of the guest onto the host.

        Pass ``recursive=True`` for a directory. Unlike :meth:`upload` this
        cannot be detected for you: the path lives in the guest, and answering
        it would cost an extra round trip on every call.
        """
        args = ["cp"]
        if recursive:
            args.append("-r")
        args += [f"{self.id}:{remote_path}", str(local_path)]
        result = run(args)
        if result.exit_code != 0:
            raise FileTransferError(_message(result.stderr, remote_path), remote_path)


def _message(stderr: str, path: str) -> str:
    """The CLI already explains these well; strip its ``Error:`` prefix."""
    text = _ERROR_PREFIX.sub("", stderr.strip())
    return text or f"file transfer failed for {path}"
