"""Unit tests for the HTTP transport (queries/mutations), against a minimal

local `http.server` — no third-party dependency, matching the SDK's own
"stdlib only" rule.
"""

import json
import os
import sys
import threading
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "src"))

from bsdkrun.errors import AuthError, GraphQLError  # noqa: E402
from bsdkrun.transport import http_request  # noqa: E402


class _CannedHandler(BaseHTTPRequestHandler):
    """Replies with whatever `(status, body_dict)` the test queued next."""

    def log_message(self, *args):  # noqa: D401 - silence test server logging
        pass

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        raw = self.rfile.read(length)
        self.server.last_request = json.loads(raw)  # type: ignore[attr-defined]
        self.server.last_headers = dict(self.headers.items())  # type: ignore[attr-defined]
        status, body = self.server.next_response  # type: ignore[attr-defined]
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps(body).encode("utf-8"))


class _FakeServer:
    def __init__(self):
        self.httpd = HTTPServer(("127.0.0.1", 0), _CannedHandler)
        self.httpd.next_response = (200, {"data": {}})
        self.thread = threading.Thread(target=self.httpd.serve_forever, daemon=True)
        self.thread.start()

    @property
    def url(self) -> str:
        host, port = self.httpd.server_address
        return f"http://{host}:{port}/graphql"

    def respond(self, status: int, body: dict) -> None:
        self.httpd.next_response = (status, body)

    def stop(self) -> None:
        self.httpd.shutdown()
        self.thread.join(timeout=5)


class TestHttpTransport(unittest.TestCase):
    def setUp(self):
        self.server = _FakeServer()

    def tearDown(self):
        self.server.stop()

    def test_success_returns_data(self):
        self.server.respond(200, {"data": {"machines": [{"id": "abc"}]}})
        data = http_request(self.server.url, "tok", "{ machines { id } }")
        self.assertEqual(data, {"machines": [{"id": "abc"}]})

    def test_sends_expected_headers_and_body(self):
        self.server.respond(200, {"data": {}})
        http_request(self.server.url, "s3cr3t", "{ info { os } }", {"x": 1})
        self.assertEqual(self.server.httpd.last_headers["Authorization"], "Bearer s3cr3t")
        self.assertEqual(self.server.httpd.last_headers["Content-Type"], "application/json")
        self.assertEqual(
            self.server.httpd.last_request, {"query": "{ info { os } }", "variables": {"x": 1}}
        )

    def test_401_raises_auth_error(self):
        self.server.respond(401, {"errors": [{"message": "nope"}]})
        with self.assertRaises(AuthError):
            http_request(self.server.url, "bad", "{ info { os } }")

    def test_unauthenticated_extension_raises_auth_error(self):
        self.server.respond(
            200,
            {"errors": [{"message": "bad token", "extensions": {"code": "UNAUTHENTICATED"}}]},
        )
        with self.assertRaises(AuthError) as ctx:
            http_request(self.server.url, "bad", "{ info { os } }")
        self.assertIn("bad token", str(ctx.exception))

    def test_other_graphql_error_raises_graphql_error_with_code(self):
        self.server.respond(
            200,
            {
                "errors": [
                    {"message": "no such machine", "extensions": {"code": "INVALID_ARGUMENT"}}
                ]
            },
        )
        with self.assertRaises(GraphQLError) as ctx:
            http_request(self.server.url, "tok", "{ info { os } }")
        self.assertEqual(ctx.exception.code, "INVALID_ARGUMENT")
        self.assertIn("no such machine", str(ctx.exception))

    def test_first_error_wins_when_several_are_returned(self):
        self.server.respond(
            200,
            {"errors": [{"message": "first"}, {"message": "second"}]},
        )
        with self.assertRaises(GraphQLError) as ctx:
            http_request(self.server.url, "tok", "{ info { os } }")
        self.assertIn("first", str(ctx.exception))

    def test_unreachable_daemon_raises_graphql_error(self):
        # Nothing is listening on this port (connect straight to a closed one).
        with self.assertRaises(GraphQLError) as ctx:
            http_request("http://127.0.0.1:1/graphql", "tok", "{ info { os } }")
        self.assertIn("cannot reach the bsdkrun daemon", str(ctx.exception))


if __name__ == "__main__":
    unittest.main()
