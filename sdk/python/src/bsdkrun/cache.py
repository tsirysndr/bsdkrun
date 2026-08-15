"""Cached guest directories — ``sandbox.cache``, plus host-level listing.

Entries are keyed, so a rebuild can pick up where the last one left off::

    hit = box.cache.restore(key=key, restore_keys=["deps-"])
    if not hit.restored:
        box.exec(["npm", "ci"])
        box.cache.save("/app/node_modules", key=key)

Where entries live — host disk or S3 — is host configuration, not an SDK
concern: set ``BSDKRUN_CACHE_BACKEND`` / ``BSDKRUN_CACHE_S3_*``, or write
``~/.config/bsdkrun/cache.toml``.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any, Literal

from .errors import CommandFailed
from .process import run

__all__ = ["Cache", "CacheEntry", "RestoreResult", "Compression", "list_caches", "remove_cache"]

#: An archive format a cache entry can be stored in.
Compression = Literal["gzip", "zstd", "estargz", "none"]


@dataclass(frozen=True)
class CacheEntry:
    """A stored cache entry, as ``cache ls`` reports it."""

    key: str
    #: Guest path the tree came from.
    path: str
    compression: str
    #: Archive size in bytes.
    size: int
    #: Unix seconds when it was saved.
    created: int
    #: ``sha256:…`` over the archive.
    digest: str = ""

    @staticmethod
    def _from(row: dict[str, Any]) -> CacheEntry:
        return CacheEntry(
            key=str(row.get("key", "")),
            path=str(row.get("path", "")),
            compression=str(row.get("compression", "")),
            size=int(row.get("size", 0)),
            created=int(row.get("created", 0)),
            digest=str(row.get("digest", "")),
        )


@dataclass(frozen=True)
class RestoreResult:
    """What a restore did. A miss is not an error — check :attr:`restored`."""

    restored: bool
    #: The key asked for.
    requested_key: str
    #: The entry actually used; differs from :attr:`requested_key` when a
    #: ``restore_keys`` prefix matched, and is None on a miss.
    key: str | None = None
    #: Guest path it was restored into.
    path: str | None = None
    size: int | None = None
    compression: str | None = None
    created: int | None = None


def _json(args: list[str], label: str) -> dict[str, Any]:
    result = run(args)
    if result.exit_code != 0:
        raise CommandFailed(result.exit_code, result.stdout, result.stderr, label)
    decoded: Any = json.loads(result.stdout or "{}")
    if not isinstance(decoded, dict):
        raise CommandFailed(
            result.exit_code,
            result.stdout,
            f"expected a JSON object from {label}, got {type(decoded).__name__}",
            label,
        )
    return decoded


class Cache:
    """Save and restore guest directories under a key."""

    def __init__(self, sandbox_id: str) -> None:
        self.id = sandbox_id

    def __repr__(self) -> str:
        return f"Cache(id={self.id!r})"

    def save(
        self,
        path: str,
        *,
        key: str,
        compression: Compression = "gzip",
        force: bool = False,
    ) -> CacheEntry:
        """Archive the guest directory at ``path`` under ``key``."""
        args = ["cache", "save", f"{self.id}:{path}", "--key", key, "--json"]
        if compression != "gzip":
            args += ["--compression", compression]
        if force:
            args.append("--force")
        return CacheEntry._from(_json(args, "bsdkrun cache save"))

    def restore(
        self,
        *,
        key: str,
        path: str | None = None,
        restore_keys: list[str] | None = None,
    ) -> RestoreResult:
        """Restore a stored tree.

        ``path`` defaults to the directory the entry was saved from.
        ``restore_keys`` are prefixes tried in order when ``key`` misses; within
        a prefix the newest matching entry wins.
        """
        target = f"{self.id}:{path}" if path else self.id
        args = ["cache", "restore", target, "--key", key, "--json"]
        if restore_keys:
            args += ["--restore-keys", *restore_keys]
        row = _json(args, "bsdkrun cache restore")
        return RestoreResult(
            restored=bool(row.get("restored")),
            requested_key=str(row.get("requested_key", key)),
            key=row.get("key"),
            path=row.get("path"),
            size=row.get("size"),
            compression=row.get("compression"),
            created=row.get("created"),
        )


def list_caches() -> list[CacheEntry]:
    """Every stored cache entry, newest first."""
    result = run(["cache", "ls", "--json"])
    if result.exit_code != 0:
        raise CommandFailed(result.exit_code, result.stdout, result.stderr, "bsdkrun cache ls")
    decoded: Any = json.loads(result.stdout or "[]")
    if not isinstance(decoded, list):
        raise CommandFailed(
            result.exit_code,
            result.stdout,
            f"expected a JSON array from bsdkrun cache ls, got {type(decoded).__name__}",
            "bsdkrun cache ls",
        )
    return [CacheEntry._from(row) for row in decoded]


def remove_cache(keys: str | list[str] | None = None, *, all: bool = False) -> None:
    """Remove entries by key, or every one of them with ``all=True``."""
    args = ["cache", "rm"]
    if all:
        args.append("--all")
    else:
        args += [keys] if isinstance(keys, str) else list(keys or [])
    result = run(args)
    if result.exit_code != 0:
        raise CommandFailed(result.exit_code, result.stdout, result.stderr, "bsdkrun cache rm")
