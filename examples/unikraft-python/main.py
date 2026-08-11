"""A tiny HTTP service, to prove CPython runs as a Unikraft unikernel."""

import json
import platform
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

PORT = 8080


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        body = json.dumps(
            {
                "message": "Hello from Python on Unikraft!",
                "python": sys.version.split()[0],
                "machine": platform.machine(),
                "path": self.path,
            }
        ).encode()

        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    # The default handler logs every request to stderr, which on a unikernel
    # is the serial console — one line per request, in front of whoever is
    # reading the boot log.
    def log_message(self, *args):
        pass


if __name__ == "__main__":
    print(f"Python {sys.version.split()[0]} listening on :{PORT}", flush=True)
    HTTPServer(("0.0.0.0", PORT), Handler).serve_forever()
