"""Unit tests for JSON-row mapping and the Result helper."""

import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "src"))

from bsdkrun.errors import CommandFailed  # noqa: E402
from bsdkrun.types import (  # noqa: E402
    NetworkInfo,
    Result,
    SandboxInfo,
    VolumeInfo,
)


class TestMapping(unittest.TestCase):
    def test_sandbox_info_running(self):
        info = SandboxInfo.from_row(
            {
                "id": "abc123def456",
                "name": None,
                "image": "alpine",
                "kind": "linux",
                "command": "sleep 300",
                "running": True,
                "exit_code": None,
                "pid": 4242,
                "detached": True,
                "cpus": 2,
                "mem": 1024,
                "volume": None,
                "state_dir": "/var/lib/bsdkrun/abc123",
                "network": "devnet",
                "net_ip": "192.168.127.3",
                "created_at": 1700000000,
                "finished_at": None,
            }
        )
        self.assertEqual(info.status, "running")
        self.assertTrue(info.running)
        self.assertIsNone(info.exit_code)
        self.assertEqual(info.pid, 4242)
        self.assertEqual(info.network, "devnet")

    def test_sandbox_info_exited(self):
        info = SandboxInfo.from_row(
            {
                "id": "abc",
                "image": "alpine",
                "kind": "linux",
                "running": False,
                "exit_code": 0,
                "detached": True,
                "cpus": 1,
                "mem": 512,
                "state_dir": "/s",
                "created_at": 1,
                "finished_at": 2,
            }
        )
        self.assertEqual(info.status, "exited")
        self.assertEqual(info.command, "")
        self.assertEqual(info.finished_at, 2)

    def test_volume_and_network(self):
        vol = VolumeInfo.from_row({"name": "web", "path": "/p", "size": "1G", "tracked": True})
        self.assertEqual(vol.name, "web")
        self.assertIsNone(vol.guest)
        self.assertIsNone(vol.created_at)

        net = NetworkInfo.from_row(
            {
                "name": "devnet",
                "subnet": "192.168.127.0/24",
                "gateway": "192.168.127.1",
                "members": 2,
                "running": 1,
                "up": True,
            }
        )
        self.assertEqual(net.members, 2)
        self.assertTrue(net.up)


class TestResult(unittest.TestCase):
    def test_ok_and_text(self):
        r = Result("hello\n\n", "", 0, "echo")
        self.assertTrue(r.ok)
        self.assertEqual(r.text(), "hello")
        self.assertEqual(r.lines(), ["hello"])

    def test_throw_if_failed(self):
        r = Result("", "boom", 1, "false")
        self.assertFalse(r.ok)
        with self.assertRaises(CommandFailed):
            r.throw_if_failed()


if __name__ == "__main__":
    unittest.main()
