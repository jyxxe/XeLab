#!/usr/bin/python3
"""Independent WebUI OCI workload; only the current kernel is a network peer.

The service owns no user credentials or management proxy. Lifecycle Initialize
supplies public browser configuration, and HTTP serves image-baked assets.
"""
import concurrent.futures
import contextlib
import hashlib
import http.server
import ipaddress
import json
import os
import pathlib
import re
import ssl
import threading
from urllib.parse import urlsplit


BASE_PATH = "/ui/org.xelab.webui/"
KERNEL_IDENTITY = "spiffe://xelab.local/kernel"
DEFAULTS = {
    "management_api_major": 1,
    "default_page": "desktop",
    "default_theme": "system",
    "max_open_windows": 16,
    "max_active_widgets": 8,
    "widget_min_refresh_seconds": 30,
    "ui_protocol_major": 1,
}


def encoded(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode()


def digest(raw):
    return "sha256:" + hashlib.sha256(raw).hexdigest()


def unique_object(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError("Duplicate JSON member")
        value[key] = item
    return value


def invalid_constant(_):
    raise ValueError("Non-finite JSON number")


def https_origin(value):
    if not isinstance(value, str) or len(value) > 2048:
        raise ValueError("HTTPS origin required")
    if any(ord(char) <= 32 or ord(char) >= 127 for char in value) or any(char in value for char in "\\%*"):
        raise ValueError("Origin must be a literal ASCII origin")
    parts = urlsplit(value)
    if (parts.scheme != "https" or not parts.netloc or parts.path or parts.query or parts.fragment
            or parts.username is not None or parts.password is not None or not parts.hostname):
        raise ValueError("Exact HTTPS origin required")
    host = parts.hostname
    try:
        address = ipaddress.ip_address(host)
        canonical_host = "[" + str(address) + "]" if address.version == 6 else str(address)
    except ValueError:
        if (len(host) > 253 or re.fullmatch(r"(?:[0-9]+|0x[0-9a-f]+)", host.rsplit(".", 1)[-1])
                or any(not re.fullmatch(r"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?", label) for label in host.split("."))):
            raise ValueError("Invalid origin host")
        canonical_host = host
    port = parts.port
    if port is not None and (not 1 <= port <= 65535 or port == 443):
        raise ValueError("Noncanonical origin port")
    canonical = "https://" + canonical_host + (":" + str(port) if port is not None else "")
    if canonical != value:
        raise ValueError("Origin must be canonical")
    return value


def validate_config(value, shell_origin=None):
    if not isinstance(value, dict) or set(value) != set(DEFAULTS) | {"core_origin", "client_id"}:
        raise ValueError("Public runtime config fields differ")
    result = dict(value)
    result["core_origin"] = https_origin(value["core_origin"])
    if shell_origin is not None and result["core_origin"] == shell_origin:
        raise ValueError("Trusted core and WebUI origins must differ")
    if not isinstance(value["client_id"], str) or not re.fullmatch(r"[A-Za-z0-9._~-]{1,128}", value["client_id"]):
        raise ValueError("Public client identifier required")
    for field, expected in (("management_api_major", 1), ("ui_protocol_major", 1)):
        if type(value[field]) not in (int, float) or value[field] != expected:
            raise ValueError("Unsupported protocol version")
        result[field] = expected
    if value["default_page"] != "desktop" or value["default_theme"] not in ("system", "light", "dark"):
        raise ValueError("Unsupported shell default")
    for field, low, high in (("max_open_windows", 1, 32), ("max_active_widgets", 0, 32),
                             ("widget_min_refresh_seconds", 30, 3600)):
        number = value[field]
        if type(number) not in (int, float) or not low <= number <= high or int(number) != number:
            raise ValueError("Shell bound differs")
        result[field] = int(number)
    return result


class State:
    def __init__(self):
        self.lock = threading.Lock()
        self.config = None
        self.draining = False
        self.active = 0

    def initialize(self, config):
        with self.lock:
            if self.draining or (self.config is not None and self.config != config):
                raise ValueError("Cannot change initialized generation")
            self.config = dict(config)

    def ready(self):
        with self.lock:
            return self.config is not None and not self.draining

    def drain(self):
        with self.lock:
            self.draining = True
            return self.active

    @contextlib.contextmanager
    def request(self):
        with self.lock:
            accepted = self.config is not None and not self.draining
            if accepted:
                self.active += 1
        try:
            yield accepted
        finally:
            if accepted:
                with self.lock:
                    self.active -= 1

    def public_config(self):
        with self.lock:
            return dict(self.config)


class WebUIServer(http.server.ThreadingHTTPServer):
    daemon_threads = True
    request_queue_size = 32

    def __init__(self, address, state, identity, public_path):
        self.state = state
        self.identity = pathlib.Path(identity)
        self.connections = threading.BoundedSemaphore(32)
        self.assets = {}
        directory = pathlib.Path(public_path)
        if directory.is_symlink() or not directory.is_dir():
            raise ValueError("Static inventory requires an image directory")
        total = 0
        mime = {".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8",
                ".css": "text/css; charset=utf-8", ".svg": "image/svg+xml", ".png": "image/png",
                ".ico": "image/x-icon", ".json": "application/json"}
        for path in sorted(directory.rglob("*")):
            if path.is_symlink():
                raise ValueError("Static inventory cannot contain symlinks")
            if not path.is_file():
                continue
            relative = path.relative_to(directory).as_posix()
            if path.suffix not in mime or not re.fullmatch(r"[A-Za-z0-9_./-]+", relative):
                raise ValueError("Unsupported static asset")
            if path.stat().st_size > 4 * 1024 * 1024:
                raise ValueError("Static asset exceeds image resource bounds")
            raw = path.read_bytes()
            total += len(raw)
            if len(raw) > 4 * 1024 * 1024 or total > 8 * 1024 * 1024 or len(self.assets) >= 128:
                raise ValueError("Static inventory exceeds image resource bounds")
            self.assets[BASE_PATH + relative] = (raw, mime[path.suffix])
        if BASE_PATH + "index.html" not in self.assets:
            raise ValueError("WebUI entry asset is missing")
        self.assets[BASE_PATH] = self.assets[BASE_PATH + "index.html"]
        self.assets[BASE_PATH + "callback"] = self.assets[BASE_PATH + "index.html"]
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
                # The existing permit covers TCP, TLS and HTTP. Handshake work
                # must not hold the accept loop ahead of other bounded workers.
                connection.settimeout(2)
                # New contexts observe atomically rotated workload certificates.
                tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
                tls.minimum_version = ssl.TLSVersion.TLSv1_2
                tls.verify_mode = ssl.CERT_REQUIRED
                tls.load_verify_locations(cafile=self.identity / "ca.crt")
                tls.load_cert_chain(self.identity / "tls.crt", self.identity / "tls.key")
                tls.set_alpn_protocols(["http/1.1"])
                connection = tls.wrap_socket(connection, server_side=True)
                if ("URI", KERNEL_IDENTITY) not in connection.getpeercert().get("subjectAltName", ()):
                    raise ssl.SSLError("Kernel identity required")
                connection.settimeout(5)
            except (OSError, ValueError):
                return
            super().process_request_thread(connection, client_address)
        finally:
            # Also close pre-handler TLS failures; closing twice after the
            # standard handler cleanup is harmless and keeps this slot bounded.
            try:
                self.shutdown_request(connection)
            finally:
                self.connections.release()


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server_version = "XeLabWebUI/1"
    sys_version = ""

    def log_message(self, *_):
        # Request targets, headers and credentials are never logged.
        pass

    def reply(self, status, raw=b"", content_type="application/json", etag=None):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(raw)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("Referrer-Policy", "no-referrer")
        self.send_header("Connection", "close")
        if etag is not None:
            self.send_header("ETag", etag)
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(raw)
        self.close_connection = True

    def error(self, status, code):
        self.reply(status, encoded({"error": code}))

    def do_GET(self):
        # Lookup is literal in an image-baked inventory, never a request-derived
        # filesystem path. OAuth callback queries must be removed by the core's
        # exact registered callback handling before this workload receives it.
        target = self.requestline.split(" ", 2)[1]
        if target != self.path or self.path not in self.server.assets and self.path != BASE_PATH + "runtime-config.js":
            self.error(404, "NOT_FOUND")
            return
        if (any(name in self.headers for name in ("Upgrade", "Transfer-Encoding", "Range", "Authorization", "Cookie"))
                or any(name.lower().startswith(("x-xelab-", "sec-websocket-", "x-forwarded-"))
                       or name.lower() in ("forwarded", "x-user-id", "x-account-id", "x-plugin-id") for name in self.headers)
                or len(self.headers.get_all("Content-Length", [])) > 1
                or self.headers.get("Content-Length", "0") != "0"):
            self.error(400, "INVALID_REQUEST")
            return
        with self.server.state.request() as accepted:
            if not accepted:
                self.error(503, "UNAVAILABLE")
                return
            if self.path == BASE_PATH + "runtime-config.js":
                raw = b"export const runtimeConfig = Object.freeze(" + encoded(self.server.state.public_config()) + b");\n"
                content_type = "text/javascript; charset=utf-8"
            else:
                raw, content_type = self.server.assets[self.path]
            etag = '"' + digest(raw) + '"'
            if self.headers.get("If-None-Match") == etag:
                self.reply(304, etag=etag)
            else:
                self.reply(200, raw, content_type, etag)

    do_HEAD = do_GET

    def do_POST(self):
        self.error(405, "METHOD_NOT_ALLOWED")

    do_PUT = do_POST
    do_PATCH = do_POST
    do_DELETE = do_POST
    do_OPTIONS = do_POST
    do_TRACE = do_POST
    do_CONNECT = do_POST


def lifecycle_service(state, manifest_path="/xelab/plugin.json"):
    # Import generated bindings only in the OCI RPC service. HTTP/component
    # tests have no dependency on Python grpc packages or generated source.
    import grpc
    from xelab.v1 import common_pb2 as common
    from xelab.v1 import lifecycle_pb2 as life
    from xelab.v1 import lifecycle_pb2_grpc as lifecycle

    class Lifecycle(lifecycle.PluginLifecycleServicer):
        def __init__(self):
            self.raw = pathlib.Path(manifest_path).read_bytes()
            self.manifest = json.loads(self.raw)
            self.generation = int(os.environ["XELAB_GENERATION"])

        def kernel(self, context):
            if KERNEL_IDENTITY.encode() not in context.auth_context().get("x509_subject_alternative_name", ()):
                context.abort(grpc.StatusCode.PERMISSION_DENIED, "Kernel identity required")

        def Describe(self, request, context):
            self.kernel(context)
            return life.DescribeResponse(manifest_json=self.raw, manifest_digest=digest(self.raw))

        def Initialize(self, request, context):
            self.kernel(context)
            try:
                if (request.instance_id != os.environ["XELAB_INSTANCE_ID"]
                        or request.deployment_id != os.environ["XELAB_DEPLOYMENT_ID"]
                        or request.generation != self.generation or len(request.config_json) > 8192
                        or digest(request.config_json) != request.config_digest):
                    raise ValueError("Identity or config digest differs")
                config = json.loads(request.config_json.decode("utf-8"), object_pairs_hook=unique_object,
                                    parse_constant=invalid_constant)
                state.initialize(validate_config(config, self.manifest["webui"]["origin"]))
            except (ValueError, UnicodeError):
                context.abort(grpc.StatusCode.FAILED_PRECONDITION, "Identity or public config differs")
            return life.InitializeResponse(observed_generation=self.generation, initialized=True)

        def GetHealth(self, request, context):
            self.kernel(context)
            return life.HealthResponse(alive=True, observed_generation=self.generation, capabilities=[])

        def Drain(self, request, context):
            self.kernel(context)
            active = state.drain()
            return life.DrainResponse(accepting_requests=False, active_calls=active, drained=active == 0)

        def Shutdown(self, request, context):
            self.kernel(context)
            state.drain()
            return common.Ack(request_id=request.context.request_id, accepted=True)

    return Lifecycle()


def main():
    import grpc
    from xelab.v1 import lifecycle_pb2_grpc as lifecycle

    if os.environ.get("XELAB_HTTP_LISTEN", "0.0.0.0:8080") != "0.0.0.0:8080":
        raise ValueError("Fixed HTTP bootstrap listener required")
    if os.environ.get("XELAB_PLUGIN_RPC_LISTEN", "0.0.0.0:7000") != "0.0.0.0:7000":
        raise ValueError("Fixed RPC bootstrap listener required")
    cert_path = pathlib.Path(os.environ["XELAB_TLS_CERT_PATH"])
    identity = cert_path.parent
    if (cert_path != identity / "tls.crt" or pathlib.Path(os.environ["XELAB_TLS_KEY_PATH"]) != identity / "tls.key"
            or pathlib.Path(os.environ["XELAB_TLS_CA_PATH"]) != identity / "ca.crt"):
        raise ValueError("Workload identity paths differ")
    state = State()
    http = WebUIServer(("0.0.0.0", 8080), state, identity, pathlib.Path(__file__).with_name("public"))
    threading.Thread(target=http.serve_forever, daemon=True).start()
    previous = [None]

    def certificates():
        cert = cert_path.read_bytes()
        if cert == previous[0]:
            return None
        config = grpc.ssl_server_certificate_configuration(
            (((identity / "tls.key").read_bytes(), cert),), root_certificates=(identity / "ca.crt").read_bytes())
        previous[0] = cert
        return config

    credentials = grpc.dynamic_ssl_server_credentials(certificates(), certificates, require_client_authentication=True)
    rpc = grpc.server(concurrent.futures.ThreadPoolExecutor(max_workers=4),
                      options=[("grpc.max_receive_message_length", 1048576)])
    lifecycle.add_PluginLifecycleServicer_to_server(lifecycle_service(state), rpc)
    if not rpc.add_secure_port("0.0.0.0:7000", credentials):
        raise RuntimeError("Cannot bind lifecycle listener")
    rpc.start()
    try:
        rpc.wait_for_termination()
    finally:
        state.drain()
        http.shutdown()
        http.server_close()


if __name__ == "__main__":
    main()
