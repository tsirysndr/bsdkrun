"""Host toolchain / image operations that aren't tied to a single machine."""

from __future__ import annotations

from .process import run, run_checked

__all__ = ["probe", "fetch_image", "versions", "grow_disk"]


def probe() -> bool:
    """Sanity-check the toolchain (libkrun links, a context is creatable).

    Does not boot. Returns ``True`` on success.
    """
    return run(["probe"]).exit_code == 0


def fetch_image(
    os: str,
    *,
    version: str | None = None,
    dir: str | None = None,
    force: bool = False,
) -> str:
    """Download + prepare a BSD image ahead of time. Returns the command output.

    ``os`` is ``"freebsd"`` or ``"netbsd"``.
    """
    args = ["fetch", "--os", os]
    if version:
        args += ["--version", version]
    if dir:
        args += ["--dir", dir]
    if force:
        args.append("--force")
    return run_checked(args, "bsdkrun fetch").stdout


def versions(os: str) -> list[str]:
    """List the arm64 builds available to fetch for a BSD (``freebsd``/``netbsd``).

    Returns the non-empty output lines.
    """
    out = run_checked(["versions", "--os", os], "bsdkrun versions").stdout
    return [line.strip() for line in out.split("\n") if line.strip()]


def grow_disk(disk: str, size: str) -> None:
    """Grow a raw disk image (the guest expands its root FS on next boot)."""
    run_checked(["grow", "--disk", disk, "--size", size], "bsdkrun grow")
