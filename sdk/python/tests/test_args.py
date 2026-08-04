"""Unit tests for the create-argv builder (no VM needed)."""

import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "src"))

from bsdkrun.args import build_create_args  # noqa: E402
from bsdkrun.types import PortForward  # noqa: E402


class TestBuildCreateArgs(unittest.TestCase):
    def test_linux_minimal(self):
        self.assertEqual(
            build_create_args(os="linux", image="alpine"),
            ["linux", "alpine", "-d"],
        )

    def test_linux_full(self):
        args = build_create_args(
            os="linux",
            image="ghcr.io/owner/name:tag",
            kernel="vmlinux",
            kernel_version="6.6",
            initramfs=True,
            volume="web",
            mounts=["~/project:/src", "~/data:/data:ro"],
            entrypoint="/bin/sh",
            console="hvc0",
            net={"ports": ["8080:80", {"host": 2222, "guest": 22}], "network": "devnet"},
            name="api",
            cpus=2,
            mem=1024,
            command=["node", "server.js"],
        )
        self.assertEqual(
            args,
            [
                "linux",
                "ghcr.io/owner/name:tag",
                "-d",
                "--kernel",
                "vmlinux",
                "--kernel-version",
                "6.6",
                "--initramfs",
                "-v",
                "web",
                "--mount",
                "~/project:/src",
                "--mount",
                "~/data:/data:ro",
                "--entrypoint",
                "/bin/sh",
                "--console",
                "hvc0",
                "--port",
                "8080:80",
                "--port",
                "2222:22",
                "--network",
                "devnet",
                "--name",
                "api",
                "--cpus",
                "2",
                "--mem",
                "1024",
                "--",
                "node",
                "server.js",
            ],
        )

    def test_port_forward_dataclass(self):
        args = build_create_args(
            os="linux",
            image="alpine",
            net={"ports": [PortForward(2222, 22)]},
        )
        self.assertIn("--port", args)
        self.assertEqual(args[args.index("--port") + 1], "2222:22")

    def test_net_disabled_ordering(self):
        # --no-net, then ports, then --mac, then --network.
        args = build_create_args(
            os="linux",
            image="alpine",
            net={
                "disabled": True,
                "ports": ["2222:22"],
                "mac": "de:ad:be:ef:00:01",
                "network": "devnet",
            },
        )
        self.assertEqual(
            args,
            [
                "linux",
                "alpine",
                "-d",
                "--no-net",
                "--port",
                "2222:22",
                "--mac",
                "de:ad:be:ef:00:01",
                "--network",
                "devnet",
            ],
        )

    def test_freebsd(self):
        args = build_create_args(
            os="freebsd",
            version="14.3",
            firmware="KRUN_EFI.fd",
            force=True,
            persist=True,
            volume="db",
            attach_disk=["extra.raw", "ro.raw:ro"],
            mem=2048,
            name="bsd",
        )
        self.assertEqual(
            args,
            [
                "freebsd",
                "-d",
                "--version",
                "14.3",
                "--firmware",
                "KRUN_EFI.fd",
                "--force",
                "--persist",
                "-v",
                "db",
                "--attach-disk",
                "extra.raw",
                "--attach-disk",
                "ro.raw:ro",
                "--name",
                "bsd",
                "--mem",
                "2048",
            ],
        )

    def test_netbsd(self):
        self.assertEqual(
            build_create_args(os="netbsd", version="10.1", volume="db"),
            ["netbsd", "-d", "--version", "10.1", "-v", "db"],
        )

    def test_firmware(self):
        self.assertEqual(
            build_create_args(os="firmware", firmware="KRUN_EFI.fd", disk="disk.raw"),
            ["firmware", "--firmware", "KRUN_EFI.fd", "--disk", "disk.raw", "-d"],
        )

    def test_kernel(self):
        # For kernel, initramfs is a path (value), not a bare flag.
        self.assertEqual(
            build_create_args(
                os="kernel",
                kernel="netbsd",
                format="elf",
                initramfs="initrd.img",
                cmdline="root=ld0a",
                disk="root.raw",
            ),
            [
                "kernel",
                "--kernel",
                "netbsd",
                "-d",
                "--format",
                "elf",
                "--initramfs",
                "initrd.img",
                "--cmdline",
                "root=ld0a",
                "--disk",
                "root.raw",
            ],
        )

    def test_missing_required_raises(self):
        with self.assertRaises(ValueError):
            build_create_args(os="linux")
        with self.assertRaises(ValueError):
            build_create_args(os="firmware", firmware="fw.fd")
        with self.assertRaises(ValueError):
            build_create_args(os="kernel")

    def test_unknown_os_raises(self):
        with self.assertRaises(ValueError):
            build_create_args(os="plan9")


if __name__ == "__main__":
    unittest.main()
