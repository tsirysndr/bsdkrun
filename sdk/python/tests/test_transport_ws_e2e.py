"""End-to-end tests for `WSTransport` against a minimal real WebSocket server.

The server is hand-rolled from the same stdlib primitives (`socket`,
`threading`) and reuses `compute_accept`/`build_frame`/`parse_frame` from
`bsdkrun.transport` — a natural byproduct of writing that exact handshake and
framing code for the client, per the task's suggestion. It speaks just enough
`graphql-transport-ws` to drive `WSTransport` through a full
connection_init -> connection_ack -> subscribe -> next -> complete cycle, plus
the ping/pong and close-before/after-ack edge cases.
"""

import json
import os
import socket
import sys
import threading
import time
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "src"))

from bsdkrun.errors import AuthError, GraphQLError  # noqa: E402
from bsdkrun.transport import (  # noqa: E402
    OP_CLOSE,
    OP_TEXT,
    WSTransport,
    build_frame,
    compute_accept,
    parse_frame,
)


class _TestWSServer:
    """Accepts one connection, performs the RFC 6455 handshake, and records

    every text message it receives while letting the test script replies.
    """

    def __init__(self) -> None:
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.sock.bind(("127.0.0.1", 0))
        self.sock.listen(1)
        self.conn: socket.socket | None = None
        self.received: list[dict] = []
        self._lock = threading.Lock()
        self._leftover = b""
        self.thread = threading.Thread(target=self._run, daemon=True)
        self.thread.start()

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
        key = headers.get("sec-websocket-key", "")
        accept = compute_accept(key)
        response = (
            "HTTP/1.1 101 Switching Protocols\r\n"
            "Upgrade: websocket\r\n"
            "Connection: Upgrade\r\n"
            f"Sec-WebSocket-Accept: {accept}\r\n"
            "Sec-WebSocket-Protocol: graphql-transport-ws\r\n"
            "\r\n"
        )
        conn.sendall(response.encode("ascii"))
        self._leftover = rest
        return True

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
                elif frame.opcode == OP_CLOSE:
                    break
        except OSError:
            pass

    def send(self, obj: dict) -> None:
        # Server->client frames are unmasked, per RFC 6455.
        assert self.conn is not None
        self.conn.sendall(build_frame(json.dumps(obj).encode("utf-8"), opcode=OP_TEXT, mask=False))

    def wait_for(self, predicate, timeout: float = 5.0) -> dict:
        deadline = time.time() + timeout
        while time.time() < deadline:
            with self._lock:
                for m in self.received:
                    if predicate(m):
                        return m
            time.sleep(0.02)
        raise AssertionError(f"expected message not seen within {timeout}s: {self.received!r}")

    def close(self) -> None:
        if self.conn is not None:
            # shutdown() first: the server's own `_read_loop` thread may
            # still be blocked in `conn.recv()` on this exact fd, and
            # closing a socket concurrently with a blocking read on it from
            # another thread is not reliably safe or immediate across
            # platforms (a classic POSIX footgun — the read may not unblock
            # promptly, or a since-reused fd number could even be affected).
            # shutdown(SHUT_RDWR) is specifically designed to deliver EOF to
            # a concurrent blocking read and to send the peer a clean FIN
            # right away, which close() alone does not guarantee.
            try:
                self.conn.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            try:
                self.conn.close()
            except OSError:
                pass
        try:
            self.sock.close()
        except OSError:
            pass


class TestWsTransportE2E(unittest.TestCase):
    def test_full_subscribe_next_complete_cycle(self):
        server = _TestWSServer()
        self.addCleanup(server.close)
        transport = WSTransport(server.url, "tok")
        self.addCleanup(transport.close)

        events: list[tuple] = []
        done = threading.Event()

        sub_id = transport.subscribe(
            "subscription{x}",
            {},
            on_next=lambda data: events.append(("next", data)),
            on_complete=lambda: (events.append(("complete",)), done.set()),
        )

        init_msg = server.wait_for(lambda m: m.get("type") == "connection_init")
        self.assertEqual(init_msg["payload"]["authorization"], "Bearer tok")

        # Nothing is sent to the daemon before the ack — the subscribe above
        # was queued, not sent, until this point.
        server.send({"type": "connection_ack"})

        sub_msg = server.wait_for(lambda m: m.get("type") == "subscribe" and m.get("id") == sub_id)
        self.assertEqual(sub_msg["payload"]["query"], "subscription{x}")

        server.send({"type": "next", "id": sub_id, "payload": {"data": {"x": 1}}})
        server.send({"type": "complete", "id": sub_id})

        self.assertTrue(done.wait(5), "on_complete never fired")
        self.assertIn(("next", {"x": 1}), events)
        self.assertIn(("complete",), events)

    def test_error_message_routes_to_on_error(self):
        server = _TestWSServer()
        self.addCleanup(server.close)
        transport = WSTransport(server.url, "tok")
        self.addCleanup(transport.close)

        caught: list[Exception] = []
        done = threading.Event()

        def on_error(e: Exception) -> None:
            caught.append(e)
            done.set()

        sub_id = transport.subscribe(
            "subscription{x}", {}, on_next=lambda d: None, on_error=on_error
        )
        server.wait_for(lambda m: m.get("type") == "connection_init")
        server.send({"type": "connection_ack"})
        server.wait_for(lambda m: m.get("type") == "subscribe" and m.get("id") == sub_id)
        server.send({"type": "error", "id": sub_id, "payload": [{"message": "boom"}]})

        self.assertTrue(done.wait(5))
        self.assertIsInstance(caught[0], GraphQLError)
        self.assertIn("boom", str(caught[0]))

    def test_ping_gets_pong_reply(self):
        server = _TestWSServer()
        self.addCleanup(server.close)
        transport = WSTransport(server.url, "tok")
        self.addCleanup(transport.close)

        transport.subscribe("subscription{x}", {}, on_next=lambda d: None)
        server.wait_for(lambda m: m.get("type") == "connection_init")
        server.send({"type": "connection_ack"})
        server.wait_for(lambda m: m.get("type") == "subscribe")

        server.send({"type": "ping"})
        server.wait_for(lambda m: m.get("type") == "pong")

    def test_close_before_ack_is_auth_error(self):
        server = _TestWSServer()
        self.addCleanup(server.close)
        transport = WSTransport(server.url, "tok")
        self.addCleanup(transport.close)

        caught: list[Exception] = []
        done = threading.Event()

        transport.subscribe(
            "subscription{x}",
            {},
            on_next=lambda d: None,
            on_error=lambda e: (caught.append(e), done.set()),
        )
        server.wait_for(lambda m: m.get("type") == "connection_init")
        server.close()  # hang up without ever acking

        self.assertTrue(done.wait(5))
        self.assertIsInstance(caught[0], AuthError)

    def test_close_after_ack_is_generic_graphql_error(self):
        server = _TestWSServer()
        self.addCleanup(server.close)
        transport = WSTransport(server.url, "tok")
        self.addCleanup(transport.close)

        caught: list[Exception] = []
        done = threading.Event()

        transport.subscribe(
            "subscription{x}",
            {},
            on_next=lambda d: None,
            on_error=lambda e: (caught.append(e), done.set()),
        )
        server.wait_for(lambda m: m.get("type") == "connection_init")
        server.send({"type": "connection_ack"})
        server.wait_for(lambda m: m.get("type") == "subscribe")
        server.close()

        self.assertTrue(done.wait(5))
        self.assertIsInstance(caught[0], GraphQLError)
        self.assertNotIsInstance(caught[0], AuthError)

    def test_unsubscribe_sends_complete_and_closes_when_empty(self):
        server = _TestWSServer()
        self.addCleanup(server.close)
        transport = WSTransport(server.url, "tok")
        self.addCleanup(transport.close)

        sub_id = transport.subscribe("subscription{x}", {}, on_next=lambda d: None)
        server.wait_for(lambda m: m.get("type") == "connection_init")
        server.send({"type": "connection_ack"})
        server.wait_for(lambda m: m.get("type") == "subscribe")

        transport.unsubscribe(sub_id)
        server.wait_for(lambda m: m.get("type") == "complete" and m.get("id") == sub_id)


if __name__ == "__main__":
    unittest.main()
