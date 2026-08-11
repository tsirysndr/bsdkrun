"""Started by the Procfile, not by the provider's own inference.

The Python provider would look for main.py, app.py or server.py and find
none of them. The Procfile is what says to run this.
"""

import json
import os
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

PORT = int(os.environ.get("PORT", "8080"))


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        body = json.dumps(
            {
                "message": "Hello from a Procfile on Unikraft!",
                "python": sys.version.split()[0],
                # Set by railpack.json's deploy.variables, compiled into the
                # image as kconfig — there is no shell in a unikernel to
                # export anything.
                "greeting": os.environ.get("GREETING", "(unset)"),
                "port": PORT,
            }
        ).encode()

        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


if __name__ == "__main__":
    print(f"Procfile app listening on :{PORT}", flush=True)
    HTTPServer(("0.0.0.0", PORT), Handler).serve_forever()
