"""Small real HTTP application behind the kernel's admitted mTLS proxy.

It does not authenticate users or interpret opaque delegation handles. The
kernel's authenticated workload identity is the only accepted HTTP peer.
"""
import contextlib
import hashlib
import http.server
import json
import pathlib
import socket
import ssl
import threading
import uuid


PLUGIN_ID = "org.xelab.http-echo"
BUSINESS_PATH = "/plugins/" + PLUGIN_ID + "/echo"
STATIC_PATH = "/ui/" + PLUGIN_ID + "/index.html"
KERNEL_IDENTITY = "spiffe://xelab.local/kernel"
MAXIMUM_BODY_BYTES = 8192


def encoded(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode()


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("Duplicate JSON member")
        result[key] = value
    return result


def invalid_constant(_):
    raise ValueError("Non-finite number")


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
            self.initialized = True
            self.draining = False

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

    def __init__(self, address, state, identity, static_path=None):
        self.state = state
        self.identity = pathlib.Path(identity)
        self.static = pathlib.Path(static_path or pathlib.Path(__file__).with_name("index.html")).read_bytes()
        self.etag = '"sha256:' + hashlib.sha256(self.static).hexdigest() + '"'
        super().__init__(address, Handler)

    def get_request(self):
        connection, address = self.socket.accept()
        connection.settimeout(2)
        try:
            # A fresh context per handshake reloads atomically rotated leaves.
            tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            tls.minimum_version = ssl.TLSVersion.TLSv1_2
            tls.verify_mode = ssl.CERT_REQUIRED
            tls.load_verify_locations(cafile=self.identity / "ca.crt")
            tls.load_cert_chain(self.identity / "tls.crt", self.identity / "tls.key")
            tls.set_alpn_protocols(["http/1.1"])
            connection = tls.wrap_socket(connection, server_side=True)
            identities = connection.getpeercert().get("subjectAltName", ())
            if ("URI", KERNEL_IDENTITY) not in identities:
                raise ssl.SSLError("Kernel identity required")
            connection.settimeout(5)
            return connection, address
        except (OSError, ValueError):
            connection.close()
            raise


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server_version = "XeLabHTTPReference/1"
    sys_version = ""

    def log_message(self, *_):
        # Raw request paths, tokens, cookies and bodies never enter logs.
        pass

    def reply(self, status, body=b"", content_type="application/json", extra=()):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("Connection", "close")
        for key, value in extra:
            self.send_header(key, value)
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)
        self.close_connection = True

    def error(self, status, code):
        self.reply(status, encoded({"error": code}))

    def safe_request(self):
        # The application is deliberately literal; it never resolves a user path
        # against the filesystem or accepts an absolute target URL.
        # BaseHTTPRequestHandler itself collapses a leading //; compare against
        # the request line so that normalization cannot turn an undeclared
        # external spelling into a declared route.
        target = self.requestline.split(" ", 2)[1]
        if target != self.path or self.path not in (BUSINESS_PATH, STATIC_PATH):
            self.error(404, "NOT_FOUND")
            return False
        if (self.headers.get("Upgrade") or self.headers.get("Transfer-Encoding")
                or self.headers.get("Authorization") or self.headers.get("Cookie")
                or len(self.headers.get_all("Content-Length", [])) > 1
                or len(self.headers.get_all("Content-Type", [])) > 1):
            self.error(400, "INVALID_REQUEST")
            return False
        return True

    def do_POST(self):
        if not self.safe_request():
            return
        if self.path != BUSINESS_PATH:
            self.error(405, "METHOD_NOT_ALLOWED")
            return
        if not self.server.state.ready():
            self.error(503, "UNAVAILABLE")
            return
        delegates = self.headers.get_all("X-Xelab-Delegation", [])
        subjects = self.headers.get_all("X-Xelab-Subject", [])
        try:
            if len(delegates) != 1 or not 1 <= len(delegates[0]) <= 4096 or len(subjects) != 1:
                raise ValueError("Missing fixed delegation")
            if str(uuid.UUID(subjects[0])) != subjects[0]:
                raise ValueError("Invalid subject")
        except (ValueError, AttributeError):
            self.error(403, "DELEGATION_REQUIRED")
            return
        if self.headers.get_content_type() != "application/json":
            self.error(415, "UNSUPPORTED_MEDIA_TYPE")
            return
        try:
            size = int(self.headers.get("Content-Length", "-1"))
        except ValueError:
            self.error(400, "INVALID_REQUEST")
            return
        if size < 0:
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
                    raise ValueError("Truncated request")
                value = json.loads(raw.decode("utf-8"), object_pairs_hook=unique_object,
                                   parse_constant=invalid_constant)
                if (not isinstance(value, dict) or set(value) != {"message"}
                        or not isinstance(value["message"], str) or len(value["message"]) > 1024):
                    raise ValueError("Input differs from schema")
                result = encoded({"message": value["message"]})
            except (ValueError, UnicodeError, socket.timeout):
                self.error(400, "INVALID_REQUEST")
                return
            # Actual business work: parse and return exactly the submitted text.
            # Delegation and subject are never reflected in the response body.
            self.reply(200, result)

    def do_GET(self):
        if not self.safe_request():
            return
        if self.path != STATIC_PATH:
            self.error(405, "METHOD_NOT_ALLOWED")
            return
        if self.headers.get("Range"):
            self.error(400, "INVALID_REQUEST")
            return
        if self.headers.get("Content-Length", "0") != "0":
            self.error(400, "INVALID_REQUEST")
            return
        if self.headers.get("If-None-Match") == self.server.etag:
            self.reply(304)
            return
        self.reply(200, self.server.static, "text/html; charset=utf-8",
                   (("ETag", self.server.etag),
                    ("Content-Security-Policy", "default-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'")))

    do_HEAD = do_GET

    def do_PUT(self):
        self.error(405, "METHOD_NOT_ALLOWED")

    do_PATCH = do_PUT
    do_DELETE = do_PUT
    do_OPTIONS = do_PUT
    do_CONNECT = do_PUT
    do_TRACE = do_PUT
