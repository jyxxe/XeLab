"""Real loopback mTLS tests; no kernel, OCI or external credentials required."""
import hashlib
import base64
import http.client
import json
import pathlib
import socket
import ssl
import subprocess
import tempfile
import threading
import time
import unittest
import uuid

import build_artifact
from http_server import BUSINESS_PATH, EVENTS_PATH, HOLD_PATH, STATIC_PATH, KernelHTTPServer, State


class HTTPTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.directory = tempfile.TemporaryDirectory(prefix="xelab-http-reference-")
        cls.root = pathlib.Path(cls.directory.name)

        def openssl(*args):
            subprocess.run(["openssl", *map(str, args)], check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

        openssl("req", "-new", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
                "-keyout", cls.root / "ca.key", "-out", cls.root / "ca.crt", "-subj", "/CN=Test Workload CA",
                "-addext", "basicConstraints=critical,CA:TRUE")
        for name, san, purpose in [
            ("server", "DNS:localhost,IP:127.0.0.1,URI:spiffe://xelab.local/instance/00000000-0000-4000-8000-000000000001/generation/1", "serverAuth"),
            ("kernel", "URI:spiffe://xelab.local/kernel", "clientAuth"),
            ("foreign", "URI:spiffe://xelab.local/instance/00000000-0000-4000-8000-000000000002/generation/1", "clientAuth"),
        ]:
            openssl("req", "-new", "-newkey", "rsa:2048", "-nodes", "-keyout", cls.root / (name + ".key"),
                    "-out", cls.root / (name + ".csr"), "-subj", "/CN=" + name)
            extension = cls.root / (name + ".ext")
            extension.write_text("subjectAltName=" + san + "\nextendedKeyUsage=" + purpose + "\n")
            openssl("x509", "-req", "-in", cls.root / (name + ".csr"), "-CA", cls.root / "ca.crt", "-CAkey", cls.root / "ca.key",
                    "-CAcreateserial", "-out", cls.root / (name + ".crt"), "-days", "1", "-extfile", extension)
        (cls.root / "tls.crt").write_bytes((cls.root / "server.crt").read_bytes())
        (cls.root / "tls.key").write_bytes((cls.root / "server.key").read_bytes())
        cls.state = State()
        cls.server = KernelHTTPServer(("127.0.0.1", 0), cls.state, cls.root)
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()
        cls.thread.join(timeout=2)
        cls.directory.cleanup()

    def setUp(self):
        self.state.initialize()

    def connection(self, identity="kernel"):
        context = ssl.create_default_context(cafile=self.root / "ca.crt")
        if identity:
            context.load_cert_chain(self.root / (identity + ".crt"), self.root / (identity + ".key"))
        return http.client.HTTPSConnection("localhost", self.server.server_address[1], context=context, timeout=2)

    def request(self, method="POST", path=BUSINESS_PATH, body=b'{"message":"hello"}', headers=None, identity="kernel"):
        prepared = {"Content-Type": "application/json", "X-Xelab-Delegation": "opaque-kernel-handle",
                    "X-Xelab-Subject": "00000000-0000-4000-8000-000000000001",
                    "X-Xelab-Call-ID": str(uuid.uuid4()), "X-Xelab-Deadline-Unix-Ms": str(int(time.time() * 1000) + 10000),
                    "X-Xelab-Idempotency-Key": base64.urlsafe_b64encode("component-test-business-key".encode()).rstrip(b"=").decode()}
        if headers is not None:
            prepared.update(headers)
        connection = self.connection(identity)
        try:
            connection.request(method, path, body=body, headers=prepared)
            response = connection.getresponse()
            return response.status, dict(response.getheaders()), response.read()
        finally:
            connection.close()

    def test_real_echo_and_strict_shape(self):
        status, headers, raw = self.request(body='{"message":"你好"}'.encode())
        self.assertEqual(status, 200)
        self.assertEqual(json.loads(raw), {"message": "你好"})
        self.assertEqual(headers["Content-Type"], "application/json")
        self.assertNotIn("Set-Cookie", headers)
        self.assertNotIn("Location", headers)
        self.assertEqual(self.state.active, 0)
        for body in [b'{"message":"a","message":"b"}', b'{"message":NaN}', b'{"message":1}', b'{"message":"a","other":true}', b'{}', b'{"message":"\xff"}', br'{"message":"\ud800"}']:
            with self.subTest(body=body):
                self.assertEqual(self.request(body=body)[0], 400)
                self.assertEqual(self.state.active, 0)

    def test_mtls_rejects_no_certificate_and_foreign_workload(self):
        for identity in [None, "foreign"]:
            with self.subTest(identity=identity):
                with self.assertRaises((ssl.SSLError, http.client.RemoteDisconnected, ConnectionResetError, BrokenPipeError)):
                    self.request(identity=identity)

    def test_static_get_head_and_conditional(self):
        status, headers, body = self.request("GET", STATIC_PATH, b"", {"X-Xelab-Delegation": "", "X-Xelab-Subject": ""})
        self.assertEqual(status, 200)
        self.assertEqual(body, self.server.static)
        self.assertIn("default-src 'none'", headers["Content-Security-Policy"])
        status, head, body = self.request("HEAD", STATIC_PATH, b"")
        self.assertEqual(status, 200)
        self.assertEqual(body, b"")
        self.assertEqual(head["Content-Length"], str(len(self.server.static)))
        self.assertEqual(self.request("GET", STATIC_PATH, b"", {"If-None-Match": headers["ETag"]})[0], 304)
        self.assertEqual(self.request("GET", STATIC_PATH, b"", {"Range": "bytes=0-9"})[0], 400)

    def test_delegation_and_credential_leakage_refused(self):
        for headers, status in [
            ({"X-Xelab-Delegation": ""}, 403),
            ({"X-Xelab-Subject": "other-account"}, 403),
            ({"X-Xelab-Call-ID": "forged"}, 403),
            ({"X-Xelab-Deadline-Unix-Ms": "1"}, 403),
            ({"X-Xelab-Idempotency-Key": "not+canonical/base64="}, 403),
            ({"Authorization": "Bearer user-credential"}, 400),
            ({"Cookie": "__Host-xelab_session=user-cookie"}, 400),
            ({"Upgrade": "websocket"}, 400),
            ({"Transfer-Encoding": "chunked"}, 400),
            ({"X-User-ID": "external-identity"}, 400),
            ({"X-Forwarded-Host": "external-origin"}, 400),
            ({"Content-Type": "text/plain"}, 415),
        ]:
            with self.subTest(headers=headers):
                self.assertEqual(self.request(headers=headers)[0], status)

    def test_no_catch_all_or_path_normalization(self):
        for path in ["/plugins/org.xelab.http-echo/other", BUSINESS_PATH + "/other", STATIC_PATH + "/../plugin.json",
                     "/ui/org.xelab.http-echo/%2e%2e/plugin.json", "/ui/org.xelab.http-echo%2findex.html",
                     "//ui/org.xelab.http-echo/index.html", STATIC_PATH + "?path=/xelab/plugin.json",
                     "https://kernel.xelab.local/oauth/callback"]:
            with self.subTest(path=path):
                self.assertEqual(self.request("GET", path, b"")[0], 404)
        self.assertEqual(self.request("POST", STATIC_PATH)[0], 405)
        self.assertEqual(self.request("GET", BUSINESS_PATH, b"")[0], 405)

    def test_limits_and_lifecycle_release_slots(self):
        self.assertEqual(self.request(body=b"x" * 8193)[0], 413)
        for _ in range(4):
            self.assertTrue(self.state.slots.acquire(blocking=False))
        try:
            self.assertEqual(self.request()[0], 429)
        finally:
            for _ in range(4):
                self.state.slots.release()
        self.assertEqual(self.request()[0], 200)
        self.state.drain()
        self.assertEqual(self.request()[0], 503)
        self.assertEqual(self.request("GET", STATIC_PATH, b"")[0], 200)
        self.state.initialize()
        self.assertEqual(self.request()[0], 200)
        self.assertEqual(self.state.active, 0)

    def test_duplicate_framing_and_delegation_headers(self):
        for header in ["Content-Length", "Content-Type", "X-Xelab-Subject", "X-Xelab-Delegation",
                       "X-Xelab-Call-ID", "X-Xelab-Deadline-Unix-Ms", "X-Xelab-Idempotency-Key"]:
            with self.subTest(header=header):
                connection = self.connection()
                try:
                    connection.putrequest("POST", BUSINESS_PATH)
                    for key, value in [("Content-Type", "application/json"), ("Content-Length", "19"),
                                       ("X-Xelab-Subject", "00000000-0000-4000-8000-000000000001"), ("X-Xelab-Delegation", "opaque"),
                                       ("X-Xelab-Call-ID", str(uuid.uuid4())), ("X-Xelab-Deadline-Unix-Ms", str(int(time.time() * 1000) + 10000)),
                                       ("X-Xelab-Idempotency-Key", "YnVzaW5lc3M")]:
                        connection.putheader(key, value)
                        if key == header:
                            connection.putheader(key, value)
                    connection.endheaders(b'{"message":"hello"}')
                    response = connection.getresponse()
                    self.assertIn(response.status, [400, 403])
                    response.read()
                finally:
                    connection.close()

    def test_real_delayed_operation_and_bounded_sse(self):
        status, _, raw = self.request(path=HOLD_PATH, body=b'{"message":"hold","delay_ms":20}')
        self.assertEqual(status, 200)
        self.assertEqual(json.loads(raw), {"message": "hold"})
        call = str(uuid.uuid4())
        status, headers, raw = self.request(path=EVENTS_PATH,
            body=b'{"message":"event","items":2,"interval_ms":10}', headers={"X-Xelab-Call-ID": call})
        self.assertEqual(status, 200)
        self.assertEqual(headers["Content-Type"], "text/event-stream")
        frames = []
        for raw_frame in raw.decode().strip().split("\n\n"):
            fields = dict(line.split(": ", 1) for line in raw_frame.splitlines())
            frame = json.loads(fields["data"])
            self.assertEqual(fields["id"], f"{call}:{frame['sequence']}")
            self.assertEqual(fields["event"], frame["kind"])
            frames.append(frame)
        self.assertEqual([frame["kind"] for frame in frames], ["accepted", "item", "item", "completed"])
        self.assertEqual([frame["sequence"] for frame in frames], [1, 2, 3, 4])
        self.assertEqual(frames[-1]["payload"]["output"], {"message": "event"})
        self.assertEqual(frames[-1]["payload"]["contract"]["name"], "org.xelab.http-echo.http")
        self.assertEqual(sum(frame["terminal"] for frame in frames), 1)
        self.assertEqual(self.state.active, 0)

    def test_sse_deadline_is_a_terminal_frame_and_releases_the_slot(self):
        status, _, raw = self.request(path=EVENTS_PATH,
            body=b'{"message":"deadline","items":2,"interval_ms":1000}',
            headers={"X-Xelab-Deadline-Unix-Ms": str(int(time.time() * 1000) + 1000)})
        self.assertEqual(status, 200)
        self.assertIn(b'"kind":"accepted"', raw)
        self.assertIn(b'"kind":"canceled"', raw)
        self.assertIn(b'"reason":"deadline"', raw)
        self.assertNotIn(b'"kind":"completed"', raw)
        self.assertEqual(self.state.active, 0)

    def test_json_schema_integral_numbers_are_accepted_without_truncation(self):
        self.assertEqual(self.request(path=HOLD_PATH, body=b'{"message":"float","delay_ms":1.0}')[0], 200)
        status, _, raw = self.request(path=EVENTS_PATH, body=b'{"message":"float","items":2.0,"interval_ms":1.0}')
        self.assertEqual(status, 200)
        self.assertEqual(raw.count(b'"kind":"item"'), 2)
        for value in [b'true', b'false', b'1.5', b'-1', b'5001', b'1e999']:
            with self.subTest(value=value):
                self.assertEqual(self.request(path=HOLD_PATH, body=b'{"message":"bad number","delay_ms":' + value + b'}')[0], 400)
        self.assertEqual(self.state.active, 0)


class ArtifactTests(unittest.TestCase):
    def test_raw_contract_binding_and_fixed_http_skill(self):
        with tempfile.TemporaryDirectory(prefix="xelab-http-artifact-") as directory:
            root = pathlib.Path(directory)
            manifest = build_artifact.build(root, "00000000-0000-4000-8000-000000000001")
            raw = (root / "xelab/http/openapi.json").read_bytes()
            self.assertEqual(raw, pathlib.Path(__file__).with_name("openapi.json").read_bytes())
            digest = "sha256:" + hashlib.sha256(raw).hexdigest()
            self.assertEqual(manifest["routes"][0]["http_contract_digest"], digest)
            self.assertEqual(manifest["skills"][0]["target"]["http_contract_digest"], digest)
            operation = json.loads(raw)["paths"][BUSINESS_PATH]["post"]
            self.assertEqual(operation["x-permission"], manifest["routes"][0]["required_action"])
            self.assertEqual(operation["operationId"], manifest["skills"][0]["target"]["http_operation_id"])
            self.assertEqual(manifest["skills"][0]["input_schema"], operation["requestBody"]["content"]["application/json"]["schema"])

    def test_optional_rpc_contract_is_a_real_signed_package(self):
        with tempfile.TemporaryDirectory(prefix="xelab-http-mixed-") as directory:
            root = pathlib.Path(directory)
            manifest = build_artifact.build(root, "00000000-0000-4000-8000-000000000001",
                                            plugin_version="2.0.0", include_rpc=True)
            cap = manifest["capabilities"][0]
            self.assertEqual(cap["contract"]["name"], "org.xelab.http-echo.rpc")
            package = root / "xelab/contracts/org.xelab.http-echo.rpc/1.0.0"
            checks = []
            for name in ["input.schema.json", "output.schema.json", "stream.schema.json", "semantics.json"]:
                checks.append(f"{name} {hashlib.sha256((package / name).read_bytes()).hexdigest()}\n")
            self.assertEqual(cap["contract"]["schema_digest"], "sha256:" + hashlib.sha256("".join(checks).encode()).hexdigest())
            self.assertEqual(manifest["skills"][0]["version"], "2.0.0")


if __name__ == "__main__":
    unittest.main()
