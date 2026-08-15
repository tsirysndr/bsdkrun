"""Host-level cache operations — listing and removing stored entries.

Mirrors :mod:`bsdkrun.images` / :mod:`bsdkrun.volumes`: the per-sandbox half
lives on :attr:`bsdkrun.Sandbox.cache`.
"""

from __future__ import annotations

from .cache import CacheEntry, list_caches, remove_cache

__all__ = ["CacheEntry", "ls", "rm"]

#: Every stored cache entry, newest first.
ls = list_caches
#: Remove entries by key, or every one of them with ``all=True``.
rm = remove_cache
