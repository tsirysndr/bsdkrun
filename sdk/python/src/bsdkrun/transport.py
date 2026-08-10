"""The GraphQL transport for talking to a remote ``bsdkrund``.

Two halves:

* :func:`http_request` — a plain ``POST`` for queries and mutations, using
  only :mod:`urllib.request` and :mod:`json`. Mirrors ``web/src/lib/graphql.ts``'s
  ``gql()`` byte-for-byte (headers, error mapping, transport-failure wrapping).

* :class:`WSTransport` — subscriptions need a persistent socket speaking the
  ``graphql-transport-ws`` protocol, and the standard library has no
  WebSocket client at all. This hand-rolls RFC 6455: the HTTP Upgrade
  handshake (:func:`compute_accept`) and frame codec (:func:`build_frame`,
  :func:`parse_frame`) are pure/stateless so they can be unit tested without a
  socket; :class:`WSTransport` wraps them with a background reader thread.

Only the standard library is used throughout: no ``requests``, no
``websockets``, matching the rest of this dependency-free SDK.
"""

from __future__ import annotations

import base64
import hashlib
import json
import os
import re
import socket
import ssl
import struct
import threading
import urllib.error
import urllib.request
from collections.abc import Callable
from dataclasses import dataclass
from typing import Any
from urllib.parse import urlsplit

from .errors import AuthError, GraphQLError

__all__ = [
    "URL_ENV",
    "TOKEN_ENV",
    "normalize_url",
    "ws_url",
    "http_request",
    "Frame",
    "build_frame",
    "parse_frame",
    "compute_accept",
    "make_ws_key",
    "WSTransport",
    "OP_CONTINUATION",
    "OP_TEXT",
    "OP_BINARY",
    "OP_CLOSE",
    "OP_PING",
    "OP_PONG",
]

URL_ENV = "BSDKRUN_URL"
TOKEN_ENV = "BSDKRUN_TOKEN"

# ---------------------------------------------------------------------------
# URL handling
# ---------------------------------------------------------------------------

_SCHEME_RE = re.compile(r"^https?://", re.IGNORECASE)
_GRAPHQL_SUFFIX_RE = re.compile(r"/graphql$", re.IGNORECASE)


def normalize_url(raw: str) -> str:
    """Turn what a person pastes into a full GraphQL endpoint URL.

    Mirrors ``web/src/lib/connection.ts``'s ``normalizeUrl``: trim, assume
    ``http://`` when no scheme is given (people type ``localhost:50052``),
    strip trailing slashes, and append ``/graphql`` unless the path already
    ends with it.
    """
    s = raw.strip()
    if not s:
        return s
    if not _SCHEME_RE.match(s):
        s = f"http://{s}"
    s = s.rstrip("/")
    if not _GRAPHQL_SUFFIX_RE.search(s):
        s = f"{s}/graphql"
    return s


def ws_url(http_url: str) -> str:
    """Derive the subscriptions URL from a normalized HTTP endpoint URL.

    ``http://`` becomes ``ws://``, ``https://`` becomes ``wss://``; trailing
    slashes on the path are stripped and ``/ws`` is appended — e.g.
    ``http://host:50052/graphql`` -> ``ws://host:50052/graphql/ws``.
    """
    if http_url.startswith("https://"):
        scheme, rest = "wss://", http_url[len("https://") :]
    elif http_url.startswith("http://"):
        scheme, rest = "ws://", http_url[len("http://") :]
    else:
        scheme, rest = "ws://", http_url
    return f"{scheme}{rest.rstrip('/')}/ws"


# ---------------------------------------------------------------------------
# HTTP transport (queries + mutations)
# ---------------------------------------------------------------------------


def http_request(
    url: str,
    token: str,
    query: str,
    variables: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Run a query or mutation over HTTP and return ``data``.

    Raises :class:`~bsdkrun.errors.AuthError` on an HTTP 401 or a GraphQL
    error whose ``extensions.code`` is ``"UNAUTHENTICATED"``, and
    :class:`~bsdkrun.errors.GraphQLError` for any other GraphQL error or for
    a transport-level failure (the daemon could not be reached at all).
    """
    body = json.dumps({"query": query, "variables": variables or {}}).encode("utf-8")
    request = urllib.request.Request(
        url,
        data=body,
        method="POST",
        headers={
            "content-type": "application/json",
            "authorization": f"Bearer {token}",
        },
    )
    try:
        with urllib.request.urlopen(request) as response:
            status = response.status
            raw = response.read()
    except urllib.error.HTTPError as e:
        # A non-2xx response still carries a JSON body worth parsing (the
        # daemon returns structured GraphQL errors even on a 4xx/5xx).
        status = e.code
        raw = e.read()
    except urllib.error.URLError as e:
        raise GraphQLError(f"cannot reach the bsdkrun daemon at {url} — {e.reason}") from e
    except OSError as e:
        raise GraphQLError(f"cannot reach the bsdkrun daemon at {url} — {e}") from e

    if status == 401:
        raise AuthError()

    try:
        parsed = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as e:
        raise GraphQLError(f"the daemon returned a non-JSON response ({status})") from e

    errors = parsed.get("errors") or []
    if errors:
        first = errors[0]
        message = first.get("message", "unknown error")
        code = (first.get("extensions") or {}).get("code")
        if code == "UNAUTHENTICATED":
            raise AuthError(message)
        raise GraphQLError(message, code)

    data = parsed.get("data")
    return data if isinstance(data, dict) else {}


# ---------------------------------------------------------------------------
# WebSocket handshake (RFC 6455 §1.3)
# ---------------------------------------------------------------------------

_WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
WS_SUBPROTOCOL = "graphql-transport-ws"


def make_ws_key() -> str:
    """A fresh ``Sec-WebSocket-Key`` (16 random bytes, base64-encoded)."""
    return base64.b64encode(os.urandom(16)).decode("ascii")


def compute_accept(key: str) -> str:
    """The ``Sec-WebSocket-Accept`` value the server must answer `key` with."""
    digest = hashlib.sha1((key + _WS_GUID).encode("ascii")).digest()  # noqa: S324
    return base64.b64encode(digest).decode("ascii")


# ---------------------------------------------------------------------------
# WebSocket frame codec (RFC 6455 §5.2)
# ---------------------------------------------------------------------------

OP_CONTINUATION = 0x0
OP_TEXT = 0x1
OP_BINARY = 0x2
OP_CLOSE = 0x8
OP_PING = 0x9
OP_PONG = 0xA


@dataclass(frozen=True)
class Frame:
    """One decoded RFC 6455 frame (already unmasked, if it was masked)."""

    fin: bool
    opcode: int
    payload: bytes


def build_frame(
    payload: bytes, opcode: int = OP_TEXT, *, mask: bool = True, fin: bool = True
) -> bytes:
    """Encode one frame.

    Client->server frames MUST be masked (the default); :class:`WSTransport`
    never sends unmasked frames, but tests exercising the codec by itself
    can turn it off to check the unmasked wire shape.
    """
    out = bytearray([(0x80 if fin else 0x00) | (opcode & 0x0F)])

    length = len(payload)
    mask_bit = 0x80 if mask else 0x00
    if length < 126:
        out.append(mask_bit | length)
    elif length < (1 << 16):
        out.append(mask_bit | 126)
        out += struct.pack("!H", length)
    else:
        out.append(mask_bit | 127)
        out += struct.pack("!Q", length)

    if mask:
        key = os.urandom(4)
        out += key
        out += bytes(b ^ key[i % 4] for i, b in enumerate(payload))
    else:
        out += payload
    return bytes(out)


def parse_frame(buf: bytes) -> tuple[Frame | None, int]:
    """Parse one frame from the start of ``buf``.

    Returns ``(None, 0)`` if ``buf`` does not yet contain a complete frame
    (the caller should read more and retry). Otherwise returns the frame and
    the number of bytes it consumed from the front of ``buf``.
    """
    if len(buf) < 2:
        return None, 0

    first, second = buf[0], buf[1]
    fin = bool(first & 0x80)
    opcode = first & 0x0F
    masked = bool(second & 0x80)
    length = second & 0x7F

    offset = 2
    if length == 126:
        if len(buf) < offset + 2:
            return None, 0
        length = struct.unpack_from("!H", buf, offset)[0]
        offset += 2
    elif length == 127:
        if len(buf) < offset + 8:
            return None, 0
        length = struct.unpack_from("!Q", buf, offset)[0]
        offset += 8

    mask_key = b""
    if masked:
        if len(buf) < offset + 4:
            return None, 0
        mask_key = bytes(buf[offset : offset + 4])
        offset += 4

    if len(buf) < offset + length:
        return None, 0

    raw_payload = buf[offset : offset + length]
    if masked:
        payload = bytes(b ^ mask_key[i % 4] for i, b in enumerate(raw_payload))
    else:
        payload = bytes(raw_payload)

    return Frame(fin=fin, opcode=opcode, payload=payload), offset + length


# ---------------------------------------------------------------------------
# socket-level connect + handshake
# ---------------------------------------------------------------------------


def _open_socket(url: str, timeout: float | None = 10.0) -> socket.socket:
    parts = urlsplit(url)
    host = parts.hostname
    if not host:
        raise GraphQLError(f"invalid WebSocket URL: {url!r}")
    port = parts.port or (443 if parts.scheme == "wss" else 80)
    sock = socket.create_connection((host, port), timeout=timeout)
    if parts.scheme == "wss":
        ctx = ssl.create_default_context()
        sock = ctx.wrap_socket(sock, server_hostname=host)
    sock.settimeout(None)  # blocking reads for the lifetime of the reader thread
    return sock


def _do_handshake(sock: socket.socket, url: str) -> bytes:
    """Perform the HTTP Upgrade handshake. Returns any bytes already read

    past the header block (the start of the first WebSocket frame, if the
    server was quick to send one).
    """
    parts = urlsplit(url)
    path = parts.path or "/"
    if parts.query:
        path += "?" + parts.query
    host_header = parts.hostname or ""
    if parts.port:
        host_header = f"{host_header}:{parts.port}"
    key = make_ws_key()

    request = (
        f"GET {path} HTTP/1.1\r\n"
        f"Host: {host_header}\r\n"
        "Upgrade: websocket\r\n"
        "Connection: Upgrade\r\n"
        f"Sec-WebSocket-Key: {key}\r\n"
        "Sec-WebSocket-Version: 13\r\n"
        f"Sec-WebSocket-Protocol: {WS_SUBPROTOCOL}\r\n"
        "\r\n"
    )
    sock.sendall(request.encode("ascii"))

    buf = b""
    while b"\r\n\r\n" not in buf:
        chunk = sock.recv(4096)
        if not chunk:
            raise GraphQLError("the daemon closed the connection during the WebSocket handshake")
        buf += chunk
    head, _, rest = buf.partition(b"\r\n\r\n")

    lines = head.decode("iso-8859-1").split("\r\n")
    status_line = lines[0]
    if not re.match(r"^HTTP/1\.[01]\s+101\b", status_line):
        raise GraphQLError(f"WebSocket handshake failed: {status_line!r}")

    headers: dict[str, str] = {}
    for line in lines[1:]:
        if ":" in line:
            k, _, v = line.partition(":")
            headers[k.strip().lower()] = v.strip()

    accept = headers.get("sec-websocket-accept")
    if accept != compute_accept(key):
        raise GraphQLError("WebSocket handshake failed: Sec-WebSocket-Accept did not match")

    return rest


# ---------------------------------------------------------------------------
# subscription client
# ---------------------------------------------------------------------------


@dataclass
class _Sub:
    on_next: Callable[[Any], None]
    on_error: Callable[[Exception], None]
    on_complete: Callable[[], None]


class WSTransport:
    """One ``graphql-transport-ws`` connection, shared by every subscription
    a :class:`~bsdkrun.client.Client` opens.

    Threading model: a single background daemon thread (started lazily by
    the first :meth:`subscribe`) owns the socket's *read* side — it blocks in
    ``recv()``, decodes frames, and dispatches ``next``/``error``/``complete``
    messages to the matching subscription's callbacks, all from that one
    thread. The public methods (:meth:`subscribe`, :meth:`unsubscribe`,
    :meth:`close`) only ever *write* to the socket and are safe to call from
    any thread; a plain :class:`threading.Lock` (``_state_lock``) guards the
    shared dict of subscriptions and the "are we connected yet" bookkeeping,
    and a second lock (``_write_lock``) serializes the actual
    ``socket.sendall`` calls so two threads writing at once can never
    interleave a frame. This keeps the rest of the SDK synchronous (no
    asyncio) while still letting `exec()` block a calling thread on a
    `queue.Queue` fed by the reader thread, and letting `shell()`'s
    callbacks fire from that same thread for live output.
    """

    def __init__(self, url: str, token: str) -> None:
        self._url = url
        self._token = token
        self._sock: socket.socket | None = None
        self._state_lock = threading.Lock()
        self._write_lock = threading.Lock()
        self._subs: dict[str, _Sub] = {}
        self._pending: list[str] = []
        self._acked = threading.Event()
        self._next_id = 1
        self._reader: threading.Thread | None = None

    # -- connection lifecycle -------------------------------------------------

    def _ensure_connected(self) -> None:
        with self._state_lock:
            if self._sock is not None:
                return
            try:
                sock = _open_socket(self._url)
                leftover = _do_handshake(sock, self._url)
            except GraphQLError:
                raise
            except Exception as e:
                raise GraphQLError(f"cannot reach the bsdkrun daemon at {self._url} — {e}") from e
            self._sock = sock
            self._acked.clear()
            self._pending = []
            reader = threading.Thread(target=self._reader_loop, args=(sock, leftover), daemon=True)
            self._reader = reader
            reader.start()

        # The token travels in connection_init, not a header a real browser
        # could never set on a WS handshake anyway — keeping parity with the
        # other SDKs' behavior rather than relying on a header trick.
        self._send_raw(
            json.dumps(
                {
                    "type": "connection_init",
                    "payload": {"authorization": f"Bearer {self._token}"},
                }
            )
        )

    def _send_raw(self, text: str) -> None:
        with self._state_lock:
            sock = self._sock
        if sock is None:
            raise GraphQLError("the WebSocket is not connected")
        frame = build_frame(text.encode("utf-8"), opcode=OP_TEXT)
        with self._write_lock:
            sock.sendall(frame)

    def close(self) -> None:
        """Close the socket. Idempotent."""
        with self._state_lock:
            sock = self._sock
            self._sock = None
            self._subs.clear()
            self._pending = []
            self._acked.clear()
        if sock is not None:
            try:
                sock.close()
            except OSError:
                pass

    # -- subscriptions ----------------------------------------------------------

    def subscribe(
        self,
        query: str,
        variables: dict[str, Any] | None,
        on_next: Callable[[Any], None],
        on_error: Callable[[Exception], None] | None = None,
        on_complete: Callable[[], None] | None = None,
    ) -> str:
        """Start a subscription. Returns its id (pass to :meth:`unsubscribe`)."""
        self._ensure_connected()
        with self._state_lock:
            sub_id = str(self._next_id)
            self._next_id += 1
            self._subs[sub_id] = _Sub(on_next, on_error or _noop_error, on_complete or _noop)
            message = json.dumps(
                {
                    "id": sub_id,
                    "type": "subscribe",
                    "payload": {"query": query, "variables": variables or {}},
                }
            )
            acked = self._acked.is_set()
            if not acked:
                # Flushed once connection_ack arrives (see _dispatch_text).
                self._pending.append(message)
        if acked:
            self._send_raw(message)
        return sub_id

    def unsubscribe(self, sub_id: str) -> None:
        with self._state_lock:
            if sub_id not in self._subs:
                return
            del self._subs[sub_id]
            remaining = len(self._subs)
            sock = self._sock
        if sock is not None:
            try:
                self._send_raw(json.dumps({"id": sub_id, "type": "complete"}))
            except (OSError, GraphQLError):
                pass
        # Drop the socket once nothing is using it, so a later subscribe()
        # reconnects fresh rather than talking to a stale connection.
        if remaining == 0:
            self.close()

    # -- reader thread ------------------------------------------------------

    def _reader_loop(self, sock: socket.socket, leftover: bytes) -> None:
        buf = bytearray(leftover)
        # Minimal fragmentation support: accumulate continuation frames into
        # one payload and dispatch on the final (FIN) frame. In practice
        # async-graphql's WS implementation (axum/tungstenite underneath)
        # sends each graphql-transport-ws message as a single unfragmented
        # text frame, so this path is exercised mainly by the frame-codec
        # unit tests, not real daemon traffic.
        frag_payload = bytearray()
        try:
            while True:
                frame, consumed = parse_frame(bytes(buf))
                if frame is None:
                    chunk = sock.recv(4096)
                    if not chunk:
                        break
                    buf += chunk
                    continue
                del buf[:consumed]

                if frame.opcode == OP_CONTINUATION:
                    frag_payload += frame.payload
                    if frame.fin:
                        self._dispatch_text(bytes(frag_payload))
                        frag_payload = bytearray()
                    continue

                if frame.opcode in (OP_TEXT, OP_BINARY):
                    if not frame.fin:
                        frag_payload = bytearray(frame.payload)
                        continue
                    self._dispatch_text(frame.payload)
                    continue

                if frame.opcode == OP_PING:
                    try:
                        with self._write_lock:
                            sock.sendall(build_frame(frame.payload, opcode=OP_PONG))
                    except OSError:
                        pass
                    continue

                if frame.opcode == OP_CLOSE:
                    break

                # OP_PONG (and anything unrecognized): nothing to do.
        except OSError:
            pass
        finally:
            self._on_socket_closed()

    def _dispatch_text(self, payload: bytes) -> None:
        try:
            msg = json.loads(payload.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError):
            return
        msg_type = msg.get("type")

        if msg_type == "connection_ack":
            with self._state_lock:
                self._acked.set()
                pending = self._pending
                self._pending = []
            for text in pending:
                try:
                    self._send_raw(text)
                except (OSError, GraphQLError):
                    pass
            return

        if msg_type == "ping":
            try:
                self._send_raw(json.dumps({"type": "pong"}))
            except (OSError, GraphQLError):
                pass
            return

        if msg_type == "pong":
            return

        sub_id = msg.get("id")
        if sub_id is None:
            return

        if msg_type == "next":
            with self._state_lock:
                sub = self._subs.get(sub_id)
            if sub:
                sub.on_next((msg.get("payload") or {}).get("data"))
            return

        if msg_type == "error":
            with self._state_lock:
                sub = self._subs.pop(sub_id, None)
            if sub:
                errs = msg.get("payload") or []
                if isinstance(errs, list):
                    detail = "; ".join(str(e.get("message", e)) for e in errs)
                else:
                    detail = json.dumps(errs)
                sub.on_error(GraphQLError(detail))
            return

        if msg_type == "complete":
            with self._state_lock:
                sub = self._subs.pop(sub_id, None)
            if sub:
                sub.on_complete()
            return

    def _on_socket_closed(self) -> None:
        with self._state_lock:
            was_acked = self._acked.is_set()
            subs = list(self._subs.values())
            self._subs.clear()
            self._pending = []
            self._sock = None
            self._acked.clear()

        # An unacked close means the daemon rejected connection_init (a bad
        # token) and hung up before ever getting to acknowledge it; an acked
        # close is just the connection going away later on.
        err: Exception = (
            AuthError()
            if not was_acked
            else GraphQLError("the connection to the daemon was closed")
        )
        for sub in subs:
            try:
                sub.on_error(err)
            except Exception:
                pass


def _noop() -> None:
    return None


def _noop_error(_err: Exception) -> None:
    return None
