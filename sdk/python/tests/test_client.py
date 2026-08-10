"""Tests for `Client`: env-based construction, HTTP-backed methods, and

`exec()`'s open -> subscribe -> wait -> close sequencing, against small
stdlib-only fake servers (no real `bsdkrund` needed).
"""

import base64
import json
import os
import socket
import sys
import threading
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "src"))

from bsdkrun.client import Client  # noqa: E402
from bsdkrun.transport import (  # noqa: E402
    OP_CLOSE,
    OP_TEXT,
    WSTransport,
    build_frame,
    compute_accept,
    parse_frame,
)


class TestFromEnv(unittest.TestCase):
    def setUp(self):
        self._saved = {k: os.environ.get(k) for k in ("BSDKRUN_URL", "BSDKRUN_TOKEN")}
        os.environ.pop("BSDKRUN_URL", None)
        os.environ.pop("BSDKRUN_TOKEN", None)

    def tearDown(self):
        for k, v in self._saved.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v

    def test_no_url_raises(self):
        with self.assertRaises(ValueError):
            Client.from_env()

    def test_url_without_token_raises(self):
        os.environ["BSDKRUN_URL"] = "http://localhost:50052"
        with self.assertRaises(ValueError):
            Client.from_env()

    def test_blank_token_is_treated_as_unset(self):
        os.environ["BSDKRUN_URL"] = "http://localhost:50052"
        os.environ["BSDKRUN_TOKEN"] = "   "
        with self.assertRaises(ValueError):
            Client.from_env()

    def test_url_and_token_build_a_normalized_client(self):
        os.environ["BSDKRUN_URL"] = "localhost:50052"
        os.environ["BSDKRUN_TOKEN"] = "secret"
        client = Client.from_env()
        self.assertEqual(client.url, "http://localhost:50052/graphql")
        self.assertEqual(client.token, "secret")


# ---------------------------------------------------------------------------
# fake servers
# ---------------------------------------------------------------------------


class _FakeHttpHandler(BaseHTTPRequestHandler):
    def log_message(self, *args):  # noqa: D401 - silence test server logging
        pass

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        body = json.loads(self.rfile.read(length))
        status, data = self.server.handle(body.get("query", ""), body.get("variables") or {})  # type: ignore[attr-defined]
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps({"data": data}).encode("utf-8"))


class _FakeHttpServer(HTTPServer):
    """A GraphQL-shaped fake: dispatches on substrings in the query text

    rather than actually parsing GraphQL, which is all a unit test needs.
    """

    def __init__(self) -> None:
        super().__init__(("127.0.0.1", 0), _FakeHttpHandler)
        self.calls: list[tuple[str, dict]] = []
        self.machines: list[dict] = []
        self.open_shell_session_id = "sess-1"
        self._thread = threading.Thread(target=self.serve_forever, daemon=True)
        self._thread.start()

    @property
    def url(self) -> str:
        host, port = self.server_address
        return f"http://{host}:{port}/graphql"

    def set_machines(self, machines: list[dict]) -> None:
        self.machines = machines

    def handle(self, query: str, variables: dict) -> tuple[int, dict]:
        self.calls.append((query, variables))
        if "openShell" in query:
            return 200, {
                "openShell": {
                    "id": self.open_shell_session_id,
                    "machineId": variables.get("machineId"),
                    "finished": False,
                    "truncated": False,
                }
            }
        if "closeShell" in query:
            return 200, {"closeShell": True}
        if "machine(" in query:
            found = next((m for m in self.machines if m["id"] == variables.get("id")), None)
            return 200, {"machine": found}
        if "machines(" in query:
            return 200, {"machines": self.machines}
        return 200, {}

    def stop(self) -> None:
        self.shutdown()
        self._thread.join(timeout=5)
        self.server_close()


class _FakeWsServer:
    """A minimal graphql-transport-ws server that auto-acks connection_init

    and, on `subscribe`, replays a scripted sequence of `shellOutput` chunks
    followed by an exit code — exactly the shape `exec()` waits on.
    """

    def __init__(self, chunks: list[bytes], exit_code: int) -> None:
        self._chunks = chunks
        self._exit_code = exit_code
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.sock.bind(("127.0.0.1", 0))
        self.sock.listen(1)
        self.conn: socket.socket | None = None
        self.received: list[dict] = []
        self._lock = threading.Lock()
        threading.Thread(target=self._run, daemon=True).start()

    @property
    def url(self) -> str:
        host, port = self.sock.getsockname()
        return f"ws://{host}:{port}/graphql/ws"

    def _run(self) -> None:
        try:
            conn, _ = self.sock.accept()
        except OSError:
            return
        self.conn = conn
        if not self._handshake(conn):
            return
        self._read_loop(conn)

    def _handshake(self, conn: socket.socket) -> bool:
        buf = b""
        while b"\r\n\r\n" not in buf:
            chunk = conn.recv(4096)
            if not chunk:
                return False
            buf += chunk
        head, _, rest = buf.partition(b"\r\n\r\n")
        headers = {}
        for line in head.decode("iso-8859-1").split("\r\n")[1:]:
            if ":" in line:
                k, _, v = line.partition(":")
                headers[k.strip().lower()] = v.strip()
        accept = compute_accept(headers.get("sec-websocket-key", ""))
        conn.sendall(
            (
                "HTTP/1.1 101 Switching Protocols\r\n"
                "Upgrade: websocket\r\n"
                "Connection: Upgrade\r\n"
                f"Sec-WebSocket-Accept: {accept}\r\n"
                "Sec-WebSocket-Protocol: graphql-transport-ws\r\n"
                "\r\n"
            ).encode("ascii")
        )
        self._leftover = rest
        return True

    def _send(self, obj: dict) -> None:
        assert self.conn is not None
        self.conn.sendall(build_frame(json.dumps(obj).encode("utf-8"), opcode=OP_TEXT, mask=False))

    def _read_loop(self, conn: socket.socket) -> None:
        buf = bytearray(self._leftover)
        try:
            while True:
                frame, consumed = parse_frame(bytes(buf))
                if frame is None:
                    chunk = conn.recv(4096)
                    if not chunk:
                        break
                    buf += chunk
                    continue
                del buf[:consumed]
                if frame.opcode == OP_TEXT:
                    msg = json.loads(frame.payload.decode("utf-8"))
                    with self._lock:
                        self.received.append(msg)
                    self._react(msg)
                elif frame.opcode == OP_CLOSE:
                    break
        except OSError:
            pass

    def _react(self, msg: dict) -> None:
        if msg.get("type") == "connection_init":
            self._send({"type": "connection_ack"})
        elif msg.get("type") == "subscribe":
            sub_id = msg["id"]
            for chunk in self._chunks:
                self._send(
                    {
                        "type": "next",
                        "id": sub_id,
                        "payload": {
                            "data": {
                                "shellOutput": {
                                    "dataBase64": base64.b64encode(chunk).decode("ascii"),
                                    "exitCode": None,
                                }
                            }
                        },
                    }
                )
            self._send(
                {
                    "type": "next",
                    "id": sub_id,
                    "payload": {
                        "data": {"shellOutput": {"dataBase64": None, "exitCode": self._exit_code}}
                    },
                }
            )

    def close(self) -> None:
        if self.conn is not None:
            try:
                self.conn.close()
            except OSError:
                pass
        try:
            self.sock.close()
        except OSError:
            pass


# ---------------------------------------------------------------------------
# tests
# ---------------------------------------------------------------------------


class TestClientHttpMethods(unittest.TestCase):
    def test_list_and_get_map_graphql_fields(self):
        server = _FakeHttpServer()
        self.addCleanup(server.stop)
        server.set_machines(
            [
                {
                    "id": "abc123",
                    "name": "web",
                    "image": "alpine",
                    "kind": "linux",
                    "command": "sleep 1",
                    "status": "running",
                    "running": True,
                    "exitCode": None,
                    "pid": 42,
                    "detached": True,
                    "cpus": 2,
                    "mem": 512,
                    "volume": None,
                    "stateDir": "/s",
                    "createdAt": "1700000000",
                    "finishedAt": None,
                    "network": None,
                    "netIp": None,
                    "ports": [{"bind": "127.0.0.1", "host": 2222, "guest": 22}],
                }
            ]
        )
        client = Client(url=server.url, token="tok")

        machines = client.list(all=True)
        self.assertEqual(len(machines), 1)
        self.assertEqual(machines[0].id, "abc123")
        self.assertEqual(machines[0].created_at, 1700000000)
        self.assertEqual(machines[0].pid, 42)
        self.assertEqual(machines[0].ports[0].host, 2222)

        found = client.get("abc123")
        self.assertIsNotNone(found)
        self.assertEqual(found.name, "web")

        self.assertIsNone(client.get("nope"))


class TestExecSequencing(unittest.TestCase):
    def test_exec_opens_subscribes_waits_then_closes(self):
        http_server = _FakeHttpServer()
        self.addCleanup(http_server.stop)
        ws_server = _FakeWsServer(chunks=[b"hello ", b"world\n"], exit_code=7)
        self.addCleanup(ws_server.close)

        client = Client(url=http_server.url, token="tok")
        # Point the WS side at the fake WS server directly, bypassing the
        # normal URL derivation (already covered by test_transport_url.py)
        # so this test is purely about exec()'s open/subscribe/wait/close
        # sequencing.
        client._ws = WSTransport(ws_server.url, "tok")

        result = client.exec("machine123", ["echo", "hello world"])

        self.assertEqual(result.exit_code, 7)
        self.assertEqual(result.output, b"hello world\n")

        queries = [q for q, _ in http_server.calls]
        self.assertTrue(any("openShell" in q for q in queries), queries)
        self.assertTrue(any("closeShell" in q for q in queries), queries)
        open_idx = next(i for i, q in enumerate(queries) if "openShell" in q)
        close_idx = next(i for i, q in enumerate(queries) if "closeShell" in q)
        self.assertLess(open_idx, close_idx, "openShell must run before closeShell")

        sub_msgs = [m for m in ws_server.received if m.get("type") == "subscribe"]
        self.assertEqual(len(sub_msgs), 1)
        self.assertEqual(sub_msgs[0]["payload"]["variables"]["sessionId"], "sess-1")

        # The open call carried the command through unmodified.
        open_call = next(v for q, v in http_server.calls if "openShell" in q)
        self.assertEqual(open_call["command"], ["echo", "hello world"])
        self.assertEqual(open_call["machineId"], "machine123")


if __name__ == "__main__":
    unittest.main()
