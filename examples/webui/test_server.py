"""Real loopback mTLS static workload tests, without a kernel or OCI daemon."""
import http.client
import hashlib
import json
import os
import pathlib
import ssl
import subprocess
import sys
import tempfile
import threading
import unittest

from server import BASE_PATH, DEFAULTS, State, WebUIServer, https_origin, validate_config


def public_config():
    return {**DEFAULTS, "core_origin": "https://core.example.test", "client_id": "xelab-client-123"}


def create_test_identities(root):
    def openssl(*args):
        subprocess.run(["openssl", *map(str, args)], check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

    openssl("req", "-new", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
            "-keyout", root / "ca.key", "-out", root / "ca.crt", "-subj", "/CN=WebUI Component CA",
            "-addext", "basicConstraints=critical,CA:TRUE")
    for name, san, purpose in [("server", "DNS:localhost,IP:127.0.0.1", "serverAuth"),
                               ("kernel", "URI:spiffe://xelab.local/kernel", "clientAuth"),
                               ("foreign", "URI:spiffe://xelab.local/instance/foreign", "clientAuth")]:
        openssl("req", "-new", "-newkey", "rsa:2048", "-nodes", "-keyout", root / (name + ".key"),
                "-out", root / (name + ".csr"), "-subj", "/CN=" + name)
        extension = root / (name + ".ext")
        extension.write_text("subjectAltName=" + san + "\nextendedKeyUsage=" + purpose + "\n")
        openssl("x509", "-req", "-in", root / (name + ".csr"), "-CA", root / "ca.crt",
                "-CAkey", root / "ca.key", "-CAcreateserial", "-out", root / (name + ".crt"),
                "-days", "1", "-extfile", extension)
    (root / "tls.crt").write_bytes((root / "server.crt").read_bytes())
    (root / "tls.key").write_bytes((root / "server.key").read_bytes())


class ConfigTests(unittest.TestCase):
    def test_exact_https_origin_and_config(self):
        for origin in ["https://core.example.test", "https://localhost:8443", "https://127.0.0.1:8443", "https://[::1]:8443"]:
            with self.subTest(origin=origin):
                self.assertEqual(https_origin(origin), origin)
        for origin in ["http://localhost:8443", "https://core.example.test/", "https://core.example.test/path",
                       "https://core.example.test?query=1", "https://core.example.test#fragment", "https://user@core.example.test",
                       "https://core.example.test:443", "https://CORE.example.test", "https://*.example.test",
                       "https://127.1", "https://0x7f000001", "https://127.00.0.1",
                       "https://core.example.test\\@evil.test", "https://core.example.test%2f", "https://core.example.test\n"]:
            with self.subTest(origin=origin):
                with self.assertRaises(ValueError):
                    https_origin(origin)
        self.assertEqual(validate_config(public_config()), public_config())
        integer_numbers = {**public_config(), "max_open_windows": 16.0}
        self.assertEqual(validate_config(integer_numbers)["max_open_windows"], 16)
        for update in [{"token": "never-allowed"}, {"client_id": "bad client"}, {"core_origin": "http://core.test"},
                       {"max_active_widgets": True}, {"max_open_windows": 1.5}, {"management_api_major": True},
                       {"widget_min_refresh_seconds": 29}, {"default_theme": "external-css"}]:
            with self.subTest(update=update):
                with self.assertRaises(ValueError):
                    validate_config({**public_config(), **update})
        with self.assertRaises(ValueError):
            validate_config(public_config(), "https://core.example.test")

    def test_manifest_is_explicit_public_nonprivileged_workload(self):
        import build_artifact

        with tempfile.TemporaryDirectory() as directory:
            manifest = build_artifact.build(pathlib.Path(directory), "00000000-0000-4000-8000-000000000001", "https://desktop.example.test")
            persisted = json.loads((pathlib.Path(directory) / "xelab/plugin.json").read_bytes())
            self.assertEqual(manifest, persisted)
            self.assertEqual(manifest["plugin_id"], "org.xelab.webui")
            self.assertEqual(manifest["capabilities"], [])
            self.assertEqual(manifest["requested_rules"], [])
            self.assertEqual(manifest["volumes"], [])
            self.assertEqual(manifest["webui"]["origin"], "https://desktop.example.test")
            self.assertEqual(manifest["routes"][0]["methods"], ["GET", "HEAD"])
            self.assertEqual(manifest["routes"][0]["maximum_body_bytes"], 0)
            self.assertEqual(manifest["minimum_resources"]["memory_bytes"], 536870912)
            self.assertEqual(set(manifest["config_schema"]["required"]), set(public_config()))

    def test_generation_lifecycle_and_active_drain(self):
        state = State()
        self.assertFalse(state.ready())
        with state.request() as accepted:
            self.assertFalse(accepted)
        state.initialize(public_config())
        state.initialize(public_config())
        self.assertTrue(state.ready())
        copied = state.public_config()
        copied["client_id"] = "changed"
        self.assertNotEqual(state.public_config(), copied)
        with state.request() as accepted:
            self.assertTrue(accepted)
            self.assertEqual(state.drain(), 1)
            self.assertFalse(state.ready())
            with state.request() as new:
                self.assertFalse(new)
        self.assertEqual(state.drain(), 0)
        with self.assertRaises(ValueError):
            state.initialize(public_config())


class HTTPTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.directory = tempfile.TemporaryDirectory(prefix="xelab-webui-component-")
        cls.root = pathlib.Path(cls.directory.name)

        create_test_identities(cls.root)
        cls.public = cls.root / "public"
        cls.public.mkdir()
        (cls.public / "index.html").write_text('<!doctype html><title>WebUI component</title><script type="module" src="./app.js"></script>')
        (cls.public / "app.js").write_text('document.title="WebUI";')
        cls.server = WebUIServer(("127.0.0.1", 0), State(), cls.root, cls.public)
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()
        cls.thread.join(timeout=2)
        cls.directory.cleanup()

    def setUp(self):
        self.server.state = State()
        self.server.state.initialize(public_config())

    def connection(self, identity="kernel"):
        context = ssl.create_default_context(cafile=self.root / "ca.crt")
        if identity is not None:
            context.load_cert_chain(self.root / (identity + ".crt"), self.root / (identity + ".key"))
        return http.client.HTTPSConnection("localhost", self.server.server_address[1], context=context, timeout=2)

    def request(self, method="GET", path=BASE_PATH, body=b"", headers=None, identity="kernel"):
        connection = self.connection(identity)
        try:
            connection.request(method, path, body=body, headers=headers or {})
            response = connection.getresponse()
            return response.status, dict(response.getheaders()), response.read()
        finally:
            connection.close()

    def test_literal_assets_callback_head_and_conditional(self):
        status, headers, raw = self.request()
        self.assertEqual(status, 200)
        self.assertEqual(raw, (self.public / "index.html").read_bytes())
        self.assertEqual(self.request(path=BASE_PATH + "callback")[2], raw)
        self.assertNotIn("Set-Cookie", headers)
        self.assertNotIn("Location", headers)
        status, head, empty = self.request("HEAD")
        self.assertEqual(status, 200)
        self.assertEqual(head["Content-Length"], str(len(raw)))
        self.assertEqual(empty, b"")
        self.assertEqual(self.request(headers={"If-None-Match": headers["ETag"]})[0], 304)
        self.assertEqual(self.request(path=BASE_PATH + "app.js")[1]["Content-Type"], "text/javascript; charset=utf-8")

    def test_only_validated_public_runtime_configuration(self):
        status, headers, raw = self.request(path=BASE_PATH + "runtime-config.js")
        self.assertEqual(status, 200)
        self.assertEqual(headers["Cache-Control"], "no-store")
        self.assertTrue(raw.startswith(b"export const runtimeConfig = Object.freeze("))
        value = json.loads(raw.decode().split("Object.freeze(", 1)[1].removesuffix(");\n"))
        self.assertEqual(value, public_config())
        self.assertNotIn("secret", value)

    def test_no_catchall_query_or_encoded_normalization(self):
        for path in [BASE_PATH + "missing", "/oauth/authorize", "https://core.example.test/", "/xelab/plugin.json",
                     BASE_PATH + "../server.py", BASE_PATH + "%2e%2e/server.py", BASE_PATH + "%61pp.js",
                     BASE_PATH + "app.js?anything=1", BASE_PATH + "callback?code=abc&state=xyz",
                     "//ui/org.xelab.webui/", BASE_PATH + "foo//app.js", BASE_PATH + "foo\\app.js"]:
            with self.subTest(path=path):
                self.assertEqual(self.request(path=path)[0], 404)

    def test_credentials_identity_and_protocol_headers_are_refused(self):
        for headers in [{"Authorization": "Bearer user-token"}, {"Cookie": "__Host-xelab_session=forbidden"},
                        {"Authorization": ""}, {"Cookie": ""},
                        {"X-Xelab-Subject": "user"}, {"X-User-ID": "user"}, {"Forwarded": "host=evil.test"},
                        {"X-Forwarded-Host": "evil.test"}, {"Upgrade": "websocket"}, {"Sec-WebSocket-Key": "x"},
                        {"Range": "bytes=0-1"}, {"Transfer-Encoding": "chunked"}]:
            with self.subTest(header=next(iter(headers))):
                self.assertEqual(self.request(headers=headers)[0], 400)
        self.assertEqual(self.request(body=b"unexpected")[0], 400)
        for method in ["POST", "PUT", "PATCH", "DELETE", "OPTIONS", "TRACE", "CONNECT"]:
            self.assertEqual(self.request(method=method)[0], 405)

    def test_real_mtls_refuses_missing_or_foreign_identity(self):
        for identity in [None, "foreign"]:
            with self.subTest(identity=identity):
                with self.assertRaises((ssl.SSLError, http.client.RemoteDisconnected, ConnectionResetError, BrokenPipeError)):
                    self.request(identity=identity)

    def test_disabled_or_uninitialized_lifecycle_refuses_assets(self):
        self.server.state = State()
        self.assertEqual(self.request()[0], 503)
        self.server.state.initialize(public_config())
        self.assertEqual(self.request()[0], 200)
        self.assertEqual(self.server.state.drain(), 0)
        self.assertEqual(self.request()[0], 503)
        self.assertEqual(self.server.state.active, 0)


def rpc_probe():
    """Execute the actual packaged entry point and generated mTLS RPC clients."""
    import grpc
    from xelab.v1 import lifecycle_pb2 as life
    from xelab.v1 import lifecycle_pb2_grpc as lifecycle

    if os.geteuid() != 65532:
        raise AssertionError("The packaged component must run as plugin UID65532")
    instance = "00000000-0000-4000-8000-000000000001"
    deployment = "00000000-0000-4000-8000-000000000002"
    with tempfile.TemporaryDirectory(prefix="xelab-webui-rpc-component-") as directory:
        root = pathlib.Path(directory)
        create_test_identities(root)
        child_env = {
            "PATH": "/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE": "1",
            "XELAB_GENERATION": "1", "XELAB_INSTANCE_ID": instance, "XELAB_DEPLOYMENT_ID": deployment,
            "XELAB_TLS_CERT_PATH": str(root / "tls.crt"), "XELAB_TLS_KEY_PATH": str(root / "tls.key"),
            "XELAB_TLS_CA_PATH": str(root / "ca.crt"),
        }
        process = subprocess.Popen([sys.executable, str(pathlib.Path(__file__).with_name("server.py"))],
                                   env=child_env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        ca = (root / "ca.crt").read_bytes()
        credentials = grpc.ssl_channel_credentials(ca, (root / "kernel.key").read_bytes(), (root / "kernel.crt").read_bytes())
        context = ssl.create_default_context(cafile=root / "ca.crt")
        context.load_cert_chain(root / "kernel.crt", root / "kernel.key")

        def request(path=BASE_PATH):
            connection = http.client.HTTPSConnection("localhost", 8080, context=context, timeout=2)
            try:
                connection.request("GET", path)
                response = connection.getresponse()
                return response.status, response.read()
            finally:
                connection.close()

        def refused(call, code):
            try:
                call()
            except grpc.RpcError as error:
                if error.code() != code:
                    raise AssertionError("RPC failed with an unexpected status") from error
            else:
                raise AssertionError("Invalid RPC was accepted")

        try:
            with grpc.secure_channel("localhost:7000", credentials) as channel:
                grpc.channel_ready_future(channel).result(timeout=10)
                client = lifecycle.PluginLifecycleStub(channel)
                described = client.Describe(life.DescribeRequest(), timeout=2)
                assert described.manifest_json == pathlib.Path("/xelab/plugin.json").read_bytes()
                assert described.manifest_digest == "sha256:" + hashlib.sha256(described.manifest_json).hexdigest()
                assert json.loads(described.manifest_json)["plugin_id"] == "org.xelab.webui"
                assert request()[0] == 503
                value = public_config()
                if value["core_origin"] == json.loads(described.manifest_json)["webui"]["origin"]:
                    value["core_origin"] = "https://different-core.example.test"
                raw = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
                initialize = life.InitializeRequest(instance_id=instance, deployment_id=deployment, generation=1,
                    config_json=raw, config_digest="sha256:" + hashlib.sha256(raw).hexdigest())
                invalid = life.InitializeRequest()
                invalid.CopyFrom(initialize)
                invalid.config_digest = "sha256:" + "0" * 64
                refused(lambda: client.Initialize(invalid, timeout=2), grpc.StatusCode.FAILED_PRECONDITION)
                assert request()[0] == 503
                invalid.CopyFrom(initialize)
                invalid.generation = 2
                refused(lambda: client.Initialize(invalid, timeout=2), grpc.StatusCode.FAILED_PRECONDITION)
                assert client.Initialize(initialize, timeout=2).initialized
                assert client.Initialize(initialize, timeout=2).initialized
                health = client.GetHealth(life.HealthRequest(), timeout=2)
                assert health.alive and health.observed_generation == 1 and not health.capabilities
                assert request()[0] == 200
                assert request(BASE_PATH + "callback")[0] == 200
                assert request(BASE_PATH + "callback?code=must-not-reach-workload")[0] == 404
                config_status, config_raw = request(BASE_PATH + "runtime-config.js")
                assert config_status == 200
                actual_config = json.loads(config_raw.decode().split("Object.freeze(", 1)[1].removesuffix(");\n"))
                assert actual_config == value
                drained = client.Drain(life.DrainRequest(grace_ms=1000), timeout=2)
                assert drained.drained and not drained.accepting_requests and drained.active_calls == 0
                assert request()[0] == 503
                refused(lambda: client.Initialize(initialize, timeout=2), grpc.StatusCode.FAILED_PRECONDITION)
                assert client.Shutdown(life.ShutdownRequest(), timeout=2).accepted
            foreign = grpc.ssl_channel_credentials(ca, (root / "foreign.key").read_bytes(), (root / "foreign.crt").read_bytes())
            with grpc.secure_channel("localhost:7000", foreign) as channel:
                client = lifecycle.PluginLifecycleStub(channel)
                refused(lambda: client.Describe(life.DescribeRequest(), timeout=2), grpc.StatusCode.PERMISSION_DENIED)
            without_certificate = grpc.ssl_channel_credentials(ca)
            with grpc.secure_channel("localhost:7000", without_certificate) as channel:
                client = lifecycle.PluginLifecycleStub(channel)
                refused(lambda: client.Describe(life.DescribeRequest(), timeout=2), grpc.StatusCode.UNAVAILABLE)
            print(json.dumps({"component": "webui-packaged", "uid": 65532, "lifecycle_mtls": "passed",
                              "https_static": "passed", "kernel_oci_integration": "not_executed"}))
        finally:
            process.terminate()
            try:
                _, stderr = process.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                _, stderr = process.communicate(timeout=5)
            if process.returncode not in (0, -15):
                sys.stderr.write(stderr[-4096:].decode("utf-8", errors="replace"))


if __name__ == "__main__":
    if sys.argv[1:] == ["--rpc-probe"]:
        rpc_probe()
    else:
        unittest.main()
