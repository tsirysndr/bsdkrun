"""Unit tests for JSON-row mapping and the Result helper."""

import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "src"))

from bsdkrun.errors import CommandFailed  # noqa: E402
from bsdkrun.types import (  # noqa: E402
    CommandResult,
    ExecResult,
    NetworkInfo,
    PortForward,
    Result,
    SandboxInfo,
    ShellSessionInfo,
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


class TestSandboxInfoFromGraphql(unittest.TestCase):
    def test_maps_camel_case_fields(self):
        info = SandboxInfo.from_graphql(
            {
                "id": "abc123def456",
                "name": None,
                "image": "alpine",
                "kind": "linux",
                "command": "sleep 300",
                "status": "running",
                "running": True,
                "exitCode": None,
                "pid": 4242,
                "detached": True,
                "cpus": 2,
                "mem": 1024,
                "volume": None,
                "stateDir": "/var/lib/bsdkrun/abc123",
                "network": "devnet",
                "netIp": "192.168.127.3",
                "ports": [{"bind": "0.0.0.0", "host": 2222, "guest": 22}],
                "createdAt": "1700000000",
                "finishedAt": None,
            }
        )
        self.assertEqual(info.status, "running")
        self.assertTrue(info.running)
        self.assertIsNone(info.exit_code)
        self.assertEqual(info.pid, 4242)
        self.assertEqual(info.state_dir, "/var/lib/bsdkrun/abc123")
        self.assertEqual(info.net_ip, "192.168.127.3")
        self.assertEqual(info.created_at, 1700000000)
        self.assertIsNone(info.finished_at)
        self.assertEqual(info.ports, [PortForward(host=2222, guest=22, bind="0.0.0.0")])

    def test_pid_and_mem_arrive_as_floats_over_the_wire(self):
        # GraphQL widens these to Float to avoid a 32-bit Int overflow; the
        # mapping must still coerce them back to plain ints.
        info = SandboxInfo.from_graphql(
            {
                "id": "abc",
                "image": "alpine",
                "kind": "linux",
                "command": "",
                "status": "exited",
                "running": False,
                "exitCode": 0,
                "pid": 4242.0,
                "detached": True,
                "cpus": 1,
                "mem": 512.0,
                "stateDir": "/s",
                "createdAt": "1",
                "finishedAt": "2",
            }
        )
        self.assertEqual(info.pid, 4242)
        self.assertEqual(info.mem, 512)
        self.assertEqual(info.finished_at, 2)


class TestCommandResult(unittest.TestCase):
    def test_from_graphql(self):
        r = CommandResult.from_graphql({"exitCode": 1, "stdout": "out", "stderr": "err"})
        self.assertEqual(r.exit_code, 1)
        self.assertEqual(r.stdout, "out")
        self.assertEqual(r.stderr, "err")

    def test_missing_fields_default_sanely(self):
        r = CommandResult.from_graphql({})
        self.assertEqual(r.exit_code, 0)
        self.assertEqual(r.stdout, "")
        self.assertEqual(r.stderr, "")


class TestShellSessionInfo(unittest.TestCase):
    def test_from_graphql(self):
        s = ShellSessionInfo.from_graphql(
            {"id": "sess-1", "machineId": "abc123", "finished": False, "truncated": True}
        )
        self.assertEqual(s.id, "sess-1")
        self.assertEqual(s.machine_id, "abc123")
        self.assertFalse(s.finished)
        self.assertTrue(s.truncated)


class TestExecResult(unittest.TestCase):
    def test_is_a_plain_bytes_carrying_dataclass(self):
        r = ExecResult(exit_code=0, output=b"hello\n")
        self.assertEqual(r.exit_code, 0)
        self.assertEqual(r.output, b"hello\n")


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
