"""Build the ``bsdkrun create`` argv from keyword options.

This mirrors the TypeScript SDK's ``args.ts`` exactly. Every path ends with
``-d`` (detached) so ``create`` yields a handle.

The public entrypoint is :func:`build_create_args`, keyed on ``os``:

* ``linux``    — run an OCI image as a Linux microVM (``image`` required).
* ``freebsd``  — run FreeBSD.
* ``netbsd``   — run NetBSD.
* ``firmware`` — boot a raw disk through its UEFI loader (``firmware``, ``disk``).
* ``kernel``   — boot a kernel directly (``kernel`` required).
* ``unikraft`` — boot a Unikraft unikernel (``path``: a kraft project dir or an
  image; defaults to ``.``).
"""

from __future__ import annotations

from collections.abc import Mapping
from typing import Any

from .types import PortForward

__all__ = ["build_create_args"]

_VALID_OS = ("linux", "freebsd", "netbsd", "firmware", "kernel", "unikraft")


def _net_args(net: Mapping[str, Any] | None) -> list[str]:
    a: list[str] = []
    if not net:
        return a
    if net.get("disabled"):
        a.append("--no-net")
    for port in net.get("ports") or []:
        if isinstance(port, str):
            a += ["--port", port]
        elif isinstance(port, PortForward):
            a += ["--port", f"{port.host}:{port.guest}"]
        elif isinstance(port, Mapping):
            a += ["--port", f"{port['host']}:{port['guest']}"]
        elif isinstance(port, (tuple, list)):
            a += ["--port", f"{port[0]}:{port[1]}"]
        else:
            raise TypeError(f"invalid port forward: {port!r}")
    if net.get("mac"):
        a += ["--mac", net["mac"]]
    if net.get("network"):
        a += ["--network", net["network"]]
    return a


def _name_args(opts: Mapping[str, Any]) -> list[str]:
    return ["--name", opts["name"]] if opts.get("name") else []


def _vm_args(opts: Mapping[str, Any]) -> list[str]:
    a: list[str] = []
    if opts.get("cpus") is not None:
        a += ["--cpus", str(opts["cpus"])]
    if opts.get("mem") is not None:
        a += ["--mem", str(opts["mem"])]
    return a


def _disk_args(opts: Mapping[str, Any]) -> list[str]:
    a: list[str] = []
    if opts.get("persist"):
        a.append("--persist")
    if opts.get("volume"):
        a += ["-v", opts["volume"]]
    for disk in opts.get("attach_disk") or []:
        a += ["--attach-disk", disk]
    return a


def _require(opts: Mapping[str, Any], field: str, os_kind: str) -> Any:
    value = opts.get(field)
    if not value:
        raise ValueError(f"os={os_kind!r} requires a {field!r} option")
    return value


def build_create_args(*, os: str, **opts: Any) -> list[str]:
    """Build the full ``bsdkrun`` argv (minus the binary and global flags).

    ``os`` selects the guest kind; the remaining keyword options depend on it.
    Common options across kinds: ``net`` (a mapping with ``disabled``/``ports``/
    ``mac``/``network``), ``name``, ``cpus``, ``mem``, and — for the disk-backed
    kinds — ``persist``, ``volume``, ``attach_disk``.
    """
    net = opts.get("net")

    if os == "linux":
        image = _require(opts, "image", os)
        a = ["linux", image, "-d"]
        if opts.get("kernel"):
            a += ["--kernel", opts["kernel"]]
        if opts.get("kernel_version"):
            a += ["--kernel-version", opts["kernel_version"]]
        if opts.get("initramfs"):
            a.append("--initramfs")
        if opts.get("volume"):
            a += ["-v", opts["volume"]]
        for mount in opts.get("mounts") or []:
            a += ["--mount", mount]
        if opts.get("entrypoint"):
            a += ["--entrypoint", opts["entrypoint"]]
        if opts.get("console"):
            a += ["--console", opts["console"]]
        a += _net_args(net) + _name_args(opts) + _vm_args(opts)
        if opts.get("command"):
            a += ["--", *opts["command"]]
        return a

    if os == "freebsd":
        a = ["freebsd", "-d"]
        if opts.get("version"):
            a += ["--version", opts["version"]]
        if opts.get("firmware"):
            a += ["--firmware", opts["firmware"]]
        if opts.get("force"):
            a.append("--force")
        a += _disk_args(opts) + _net_args(net) + _name_args(opts) + _vm_args(opts)
        return a

    if os == "netbsd":
        a = ["netbsd", "-d"]
        if opts.get("version"):
            a += ["--version", opts["version"]]
        if opts.get("force"):
            a.append("--force")
        a += _disk_args(opts) + _net_args(net) + _name_args(opts) + _vm_args(opts)
        return a

    if os == "firmware":
        firmware = _require(opts, "firmware", os)
        disk = _require(opts, "disk", os)
        a = ["firmware", "--firmware", firmware, "--disk", disk, "-d"]
        a += _disk_args(opts) + _net_args(net) + _name_args(opts) + _vm_args(opts)
        return a

    if os == "kernel":
        kernel = _require(opts, "kernel", os)
        a = ["kernel", "--kernel", kernel, "-d"]
        if opts.get("format"):
            a += ["--format", opts["format"]]
        if opts.get("initramfs"):
            a += ["--initramfs", opts["initramfs"]]
        if opts.get("cmdline"):
            a += ["--cmdline", opts["cmdline"]]
        if opts.get("disk"):
            a += ["--disk", opts["disk"]]
        a += _disk_args(opts) + _net_args(net) + _name_args(opts) + _vm_args(opts)
        return a

    if os == "unikraft":
        # No _disk_args: a unikernel has no disk, so there is nothing to
        # persist, attach or clone.
        a = ["unikraft", "-d"]
        if opts.get("cmdline"):
            a += ["--cmdline", opts["cmdline"]]
        if opts.get("initramfs"):
            a += ["--initramfs", opts["initramfs"]]
        a += _net_args(net) + _name_args(opts) + _vm_args(opts)
        a.append(opts.get("path") or ".")
        return a

    raise ValueError(f"unknown os {os!r}; expected one of {', '.join(_VALID_OS)}")
