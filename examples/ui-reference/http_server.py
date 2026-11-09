"""Real literal HTTP echo/static application. User authority stays in the kernel."""
import base64
import contextlib
import hashlib
import http.server
import json
import math
import pathlib
import socket
import ssl
import threading
import time
import uuid

PLUGIN_ID = "org.xelab.ui-reference"
BUSINESS_PATH = f"/plugins/{PLUGIN_ID}/echo"
STATIC_PREFIX = f"/ui/{PLUGIN_ID}/"
MAXIMUM_BODY_BYTES = 8192
STATIC_FILES = {"window.html": "text/html; charset=utf-8", "widget.html": "text/html; charset=utf-8",
                "bridge.js": "text/javascript; charset=utf-8", "app.js": "text/javascript; charset=utf-8", "styles.css": "text/css; charset=utf-8"}


def encoded(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"), allow_nan=False).encode()


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("Duplicate JSON member")
        result[key] = value
    return result


def invalid_constant(_):
    raise ValueError("Non-finite JSON number")


def bounded_integer(value, minimum, maximum):
    if (isinstance(value, bool) or not isinstance(value, (int, float))
            or (isinstance(value, float) and (not math.isfinite(value) or not value.is_integer()))
            or not minimum <= value <= maximum):
        raise ValueError("Invalid integer")
    return int(value)


class State:
    def __init__(self):
        self.lock = threading.Lock()
        self.initialized = False
        self.draining = False
        self.active = 0
        self.slots = threading.BoundedSemaphore(4)

    def ready(self):
        with self.lock:
            return self.initialized and not self.draining

    def initialize(self):
        with self.lock:
            self.initialized, self.draining = True, False

    def drain(self):
        with self.lock:
            self.draining = True
            return self.active

    @contextlib.contextmanager
    def business(self):
        acquired = self.slots.acquire(blocking=False)
        if acquired:
            with self.lock:
                if not self.initialized or self.draining:
                    self.slots.release()
                    acquired = False
                else:
                    self.active += 1
        try:
            yield acquired
        finally:
            if acquired:
                with self.lock:
                    self.active -= 1
                self.slots.release()


class KernelHTTPServer(http.server.ThreadingHTTPServer):
    daemon_threads = True
    request_queue_size = 16

    def __init__(self, address, state, identity, public=None):
        self.state, self.identity = state, pathlib.Path(identity)
        self.connections = threading.BoundedSemaphore(16)
        root = pathlib.Path(public or pathlib.Path(__file__).with_name("public"))
        # The immutable image supplies this finite map. Requests never select
        # filesystem paths and cannot fall through to a generic file server.
        self.files = {STATIC_PREFIX + name: (kind, (root / name).read_bytes()) for name, kind in STATIC_FILES.items()}
        super().__init__(address, Handler)

    def process_request(self, request, client_address):
        if not self.connections.acquire(blocking=False):
            self.shutdown_request(request)
            return
        try:
            super().process_request(request, client_address)
        except BaseException:
            try:
                self.shutdown_request(request)
            finally:
                self.connections.release()
            raise

    def process_request_thread(self, request, client_address):
        connection = request
        try:
            try:
                # Bound TCP, TLS and HTTP together; a slow handshake must not
                # serialize acceptance ahead of the other sixteen workers.
                connection.settimeout(2)
                tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
                tls.minimum_version = ssl.TLSVersion.TLSv1_2
                tls.verify_mode = ssl.CERT_REQUIRED
                tls.load_verify_locations(cafile=self.identity / "ca.crt")
                tls.load_cert_chain(self.identity / "tls.crt", self.identity / "tls.key")
                tls.set_alpn_protocols(["http/1.1"])
                connection = tls.wrap_socket(connection, server_side=True)
                if ("URI", "spiffe://xelab.local/kernel") not in connection.getpeercert().get("subjectAltName", ()):
                    raise ssl.SSLError("Kernel identity required")
                connection.settimeout(5)
            except (OSError, ValueError):
                return
            super().process_request_thread(connection, client_address)
        finally:
            try:
                self.shutdown_request(connection)
            finally:
                self.connections.release()


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server_version, sys_version = "XeLabUIReference/1", ""

    def log_message(self, *_):
        pass  # Never log paths, bodies, credentials or channel metadata.

    def reply(self, status, body=b"", content_type="application/json", extra=()):
        self.send_response(status)
        for key, value in [("Content-Type", content_type), ("Content-Length", str(len(body))),
                           ("Cache-Control", "no-store"), ("X-Content-Type-Options", "nosniff"),
                           ("Connection", "close"), *extra]:
            self.send_header(key, value)
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)
        self.close_connection = True

    def error(self, status, code):
        self.reply(status, encoded({"error": code}))

    def safe_request(self):
        target = self.requestline.split(" ", 2)[1]
        if target != self.path or self.path not in (*self.server.files, BUSINESS_PATH):
            self.error(404, "NOT_FOUND")
            return False
        if (any(self.headers.get(name) is not None for name in ("Upgrade", "Transfer-Encoding", "Authorization", "Cookie", "Range", "Forwarded",
                "X-User-ID", "X-Plugin-ID", "X-Account-ID", "Sec-WebSocket-Key", "Sec-WebSocket-Version"))
                or any(name.lower().startswith("x-forwarded-") for name in self.headers)
                or any(len(self.headers.get_all(name, [])) > 1 for name in ("Content-Length", "Content-Type", "If-None-Match"))):
            self.error(400, "INVALID_REQUEST")
            return False
        return True

    def do_POST(self):
        if not self.safe_request():
            return
        if self.path != BUSINESS_PATH:
            self.error(405, "METHOD_NOT_ALLOWED")
            return
        try:
            def only(name):
                values = self.headers.get_all(name, [])
                if len(values) != 1:
                    raise ValueError("Missing admitted context")
                return values[0]
            if not 1 <= len(only("X-Xelab-Delegation")) <= 4096:
                raise ValueError("Missing delegation")
            for name in ("X-Xelab-Subject", "X-Xelab-Call-ID"):
                value = only(name)
                if str(uuid.UUID(value)) != value:
                    raise ValueError("Invalid UUID")
            deadline = only("X-Xelab-Deadline-Unix-Ms")
            if not deadline.isascii() or not deadline.isdecimal() or int(deadline) <= int(time.time() * 1000):
                raise ValueError("Expired call")
            key = only("X-Xelab-Idempotency-Key")
            if len(key) > 684:
                raise ValueError("Oversized key")
            decoded = base64.b64decode(key + "=" * (-len(key) % 4), altchars=b"-_", validate=True)
            if base64.urlsafe_b64encode(decoded).rstrip(b"=").decode() != key or not 1 <= len(decoded.decode()) <= 128:
                raise ValueError("Invalid business key")
        except (ValueError, UnicodeError):
            self.error(403, "DELEGATION_REQUIRED")
            return
        if self.headers.get_content_type() != "application/json":
            self.error(415, "UNSUPPORTED_MEDIA_TYPE")
            return
        try:
            size = int(self.headers.get("Content-Length", "-1"))
            if size < 0:
                raise ValueError("Missing body size")
        except ValueError:
            self.error(400, "INVALID_REQUEST")
            return
        if size > MAXIMUM_BODY_BYTES:
            self.error(413, "BODY_TOO_LARGE")
            return
        with self.server.state.business() as acquired:
            if not acquired:
                self.error(429 if self.server.state.ready() else 503, "UNAVAILABLE")
                return
            try:
                raw = self.rfile.read(size)
                if len(raw) != size:
                    raise ValueError("Truncated input")
                value = json.loads(raw.decode(), object_pairs_hook=unique_object, parse_constant=invalid_constant)
                if not isinstance(value, dict) or set(value) != {"message"} or not isinstance(value["message"], str) or len(value["message"]) > 1024:
                    raise ValueError("Input differs from signed schema")
                output = encoded({"message": value["message"]})
            except (ValueError, UnicodeError, RecursionError, socket.timeout):
                self.error(400, "INVALID_REQUEST")
                return
            self.reply(200, output)

    def do_GET(self):
        if not self.safe_request():
            return
        if self.path == BUSINESS_PATH:
            self.error(405, "METHOD_NOT_ALLOWED")
            return
        if self.headers.get("Content-Length", "0") != "0":
            self.error(400, "INVALID_REQUEST")
            return
        with self.server.state.business() as acquired:
            if not acquired:
                self.error(429 if self.server.state.ready() else 503, "UNAVAILABLE")
                return
            media, body = self.server.files[self.path]
            etag = '"sha256:' + hashlib.sha256(body).hexdigest() + '"'
            if self.headers.get("If-None-Match") == etag:
                self.reply(304, extra=[("ETag", etag)])
            else:
                # Embedding origins are assigned by the trusted kernel gateway.
                # The plugin never chooses a parent-origin response header.
                self.reply(200, body, media, [("ETag", etag), ("Referrer-Policy", "no-referrer")])

    do_HEAD = do_GET

    def do_PUT(self):
        self.error(405, "METHOD_NOT_ALLOWED")

    do_DELETE = do_OPTIONS = do_PATCH = do_PUT
