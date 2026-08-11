"""A uv-managed Python service, to prove a locked dependency reaches the guest.

Bottle is a single-file WSGI framework that serves on the standard library's
own wsgiref server, so what this demonstrates is uv resolving and installing
a third-party package — not a second web server in the rootfs.
"""

import sys

from bottle import Bottle, response

app = Bottle()
PORT = 8080


@app.route("/")
def index():
    response.content_type = "application/json"
    return {
        "message": "Hello from Python + uv on Unikraft!",
        "python": sys.version.split()[0],
        "bottle": __import__("bottle").__version__,
    }


if __name__ == "__main__":
    print(f"Python {sys.version.split()[0]} listening on :{PORT}", flush=True)
    app.run(host="0.0.0.0", port=PORT, quiet=True)
