"""Actual loopback mTLS and signed inputs; not kernel/OCI acceptance."""
import base64
import hashlib
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
from unittest import mock

import build_artifact
from http_server import BUSINESS_PATH, STATIC_PREFIX, KernelHTTPServer, State


class ObservedAcceptSocket:
    """Observe real TCP acceptance without replacing TLS or HTTP handling."""
    def __init__(self, wrapped):
        self.wrapped = wrapped
        self.condition = threading.Condition()
        self.accepted = 0

    def accept(self):
        result = self.wrapped.accept()
        with self.condition:
            self.accepted += 1
            self.condition.notify_all()
        return result

    def wait_accepted(self, count):
        with self.condition:
            return self.condition.wait_for(lambda: self.accepted >= count, timeout=2)

    def __getattr__(self, name):
        return getattr(self.wrapped, name)


class ObservedKernelHTTPServer(KernelHTTPServer):
    def __init__(self, *args):
        self.worker_condition = threading.Condition()
        self.active_workers = 0
        super().__init__(*args)
        self.socket = ObservedAcceptSocket(self.socket)

    def process_request_thread(self, request, client_address):
        with self.worker_condition:
            self.active_workers += 1
            self.worker_condition.notify_all()
        try:
            super().process_request_thread(request, client_address)
        finally:
            with self.worker_condition:
                self.active_workers -= 1
                self.worker_condition.notify_all()

    def wait_workers(self, count, timeout=2):
        with self.worker_condition:
            return self.worker_condition.wait_for(lambda: self.active_workers == count, timeout=timeout)


class HTTPTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.directory = tempfile.TemporaryDirectory(prefix="xelab-ui-reference-")
        cls.root = pathlib.Path(cls.directory.name)

        def openssl(*args):
            subprocess.run(["openssl", *map(str, args)], check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

        openssl("req", "-new", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1", "-keyout", cls.root / "ca.key", "-out", cls.root / "ca.crt", "-subj", "/CN=Disposable UI Reference CA", "-addext", "basicConstraints=critical,CA:TRUE")
        for name, san, purpose in [
            ("server", "DNS:localhost,IP:127.0.0.1,URI:spiffe://xelab.local/instance/00000000-0000-4000-8000-000000000001/generation/1", "serverAuth"),
            ("kernel", "URI:spiffe://xelab.local/kernel", "clientAuth"),
            ("foreign", "URI:spiffe://xelab.local/instance/00000000-0000-4000-8000-000000000002/generation/1", "clientAuth"),
        ]:
            openssl("req", "-new", "-newkey", "rsa:2048", "-nodes", "-keyout", cls.root / (name + ".key"), "-out", cls.root / (name + ".csr"), "-subj", "/CN=" + name)
            extension = cls.root / (name + ".ext")
            extension.write_text(f"subjectAltName={san}\nextendedKeyUsage={purpose}\n")
            openssl("x509", "-req", "-in", cls.root / (name + ".csr"), "-CA", cls.root / "ca.crt", "-CAkey", cls.root / "ca.key", "-CAcreateserial", "-out", cls.root / (name + ".crt"), "-days", "1", "-extfile", extension)
        (cls.root / "tls.crt").write_bytes((cls.root / "server.crt").read_bytes())
        (cls.root / "tls.key").write_bytes((cls.root / "server.key").read_bytes())
        (cls.root / "untrusted.key").write_bytes((cls.root / "foreign.key").read_bytes())
        openssl("req", "-new", "-x509", "-key", cls.root / "untrusted.key", "-days", "1",
                "-out", cls.root / "untrusted.crt", "-subj", "/CN=Untrusted Kernel",
                "-addext", "subjectAltName=URI:spiffe://xelab.local/kernel",
                "-addext", "extendedKeyUsage=clientAuth")
        cls.state = State()
        cls.server = ObservedKernelHTTPServer(("127.0.0.1", 0), cls.state, cls.root)
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
        return http.client.HTTPSConnection("localhost", self.server.server_address[1], context=context, timeout=3)

    def request(self, method="POST", path=BUSINESS_PATH, body=b'{"message":"actual independent backend"}', headers=None, identity="kernel"):
        connection = self.connection(identity)
        prepared = {"Content-Type": "application/json", "X-Xelab-Delegation": "component-opaque-kernel-handle", "X-Xelab-Subject": str(uuid.UUID(int=1)), "X-Xelab-Call-ID": str(uuid.uuid4()), "X-Xelab-Deadline-Unix-Ms": str(int(time.time() * 1000) + 10000), "X-Xelab-Idempotency-Key": base64.urlsafe_b64encode(b"component-intent").rstrip(b"=").decode()}
        prepared.update(headers or {})
        try:
            connection.request(method, path, body, prepared)
            response = connection.getresponse()
            return response.status, dict(response.getheaders()), response.read()
        finally:
            connection.close()

    def test_actual_echo_and_strict_signed_input(self):
        value = {"message": "实际业务 <img src=x onerror=alert(1)>"}
        status, headers, raw = self.request(body=json.dumps(value).encode())
        self.assertEqual(status, 200)
        self.assertEqual(json.loads(raw), value)
        self.assertNotIn("Set-Cookie", headers)
        self.assertNotIn("Location", headers)
        for body in [b'{}', b'{"message":1}', b'{"message":"x","url":"https://other/"}', b'{"message":"a","message":"b"}', b'{"message":NaN}', b'{"message":"\xff"}', br'{"message":"\ud800"}']:
            self.assertEqual(self.request(body=body)[0], 400)
            self.assertEqual(self.state.active, 0)

    def test_exact_static_files_get_head_and_no_path_escape(self):
        for name in ("window.html", "widget.html", "bridge.js", "app.js", "styles.css"):
            path = STATIC_PREFIX + name
            status, headers, raw = self.request("GET", path, b"")
            self.assertEqual(status, 200)
            self.assertEqual(raw, self.server.files[path][1])
            status, head, raw = self.request("HEAD", path, b"")
            self.assertEqual(status, 200)
            self.assertEqual(raw, b"")
            self.assertEqual(head["Content-Length"], headers["Content-Length"])
            self.assertEqual(self.request("GET", path, b"", {"If-None-Match": headers["ETag"]})[0], 304)
        for path in [STATIC_PREFIX, STATIC_PREFIX + "../plugin.json", STATIC_PREFIX + "%2e%2e/plugin.json", STATIC_PREFIX + "window.html?next=bad", STATIC_PREFIX + "unknown", "/" + STATIC_PREFIX + "window.html", "https://kernel.example/oauth/authorize", BUSINESS_PATH + "/other"]:
            self.assertEqual(self.request("GET", path, b"")[0], 404)
        self.assertEqual(self.request("POST", STATIC_PREFIX + "window.html")[0], 405)
        self.assertEqual(self.request("GET", BUSINESS_PATH, b"")[0], 405)

    def test_mtls_refuses_foreign_and_missing_workload_identities(self):
        for identity in (None, "foreign", "untrusted"):
            with self.assertRaises((ssl.SSLError, http.client.RemoteDisconnected, ConnectionResetError, BrokenPipeError)):
                self.request(identity=identity)
            self.assert_all_connection_slots_released()
        self.assertEqual(self.request()[0], 200)

    def held_handshakes(self, count):
        connections = []
        try:
            for _ in range(count):
                connection = socket.create_connection(self.server.server_address, timeout=2)
                connections.append(connection)
                connection.sendall(b"\x16")  # Real TLS record left incomplete.
            return connections
        except BaseException:
            for connection in connections:
                connection.close()
            raise

    def assert_all_connection_slots_released(self):
        self.assertTrue(self.server.wait_workers(0))
        acquired = 0
        try:
            while acquired < 17 and self.server.connections.acquire(blocking=False):
                acquired += 1
            self.assertEqual(acquired, 16)
        finally:
            for _ in range(acquired):
                self.server.connections.release()

    def test_slow_tls_handshakes_do_not_block_a_legal_static_request(self):
        # Before the connection limiter exists, still exercise the original
        # real server to prove that TLS in its accept loop blocks this request.
        self.assertTrue(self.server.wait_workers(0))
        accepted = self.server.socket.accepted
        stalled = self.held_handshakes(1)
        try:
            self.assertTrue(self.server.socket.wait_accepted(accepted + 1))
            stalled.extend(self.held_handshakes(1))
            self.assertEqual(self.request("GET", STATIC_PREFIX + "widget.html", b"")[0], 200)
            self.assertTrue(all(connection.fileno() >= 0 for connection in stalled))
            self.assertGreaterEqual(self.server.active_workers, 2)
            self.assertEqual(self.state.active, 0)
        finally:
            for connection in stalled:
                connection.close()
        self.assert_all_connection_slots_released()

    def test_tls_handshakes_share_the_16_connection_bound(self):
        self.assert_all_connection_slots_released()
        stalled = self.held_handshakes(16)
        try:
            self.assertTrue(self.server.wait_workers(16))
            self.assertFalse(self.server.connections.acquire(blocking=False))
            self.assertEqual(self.state.active, 0)
            with self.assertRaises((OSError, http.client.RemoteDisconnected)):
                self.request("GET", STATIC_PREFIX + "widget.html", b"")
            self.assertEqual(self.server.active_workers, 16)
        finally:
            for connection in stalled:
                connection.close()
        self.assert_all_connection_slots_released()
        self.assertEqual(self.request()[0], 200)
        self.assert_all_connection_slots_released()

    def test_tls_handshake_timeout_releases_its_slot_without_http(self):
        self.assert_all_connection_slots_released()
        stalled = self.held_handshakes(1)
        try:
            self.assertTrue(self.server.wait_workers(1))
            self.assertTrue(self.server.wait_workers(0, timeout=3))
            self.assertEqual(stalled[0].recv(1), b"")
        finally:
            stalled[0].close()
        self.assert_all_connection_slots_released()
        self.assertEqual(self.request()[0], 200)

    def test_worker_start_failure_closes_socket_and_releases_its_permit(self):
        self.assert_all_connection_slots_released()
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            listener.listen(1)
            with socket.create_connection(listener.getsockname(), timeout=2) as peer:
                request, address = listener.accept()
                try:
                    with mock.patch("socketserver.threading.Thread.start", side_effect=RuntimeError("worker start failed")):
                        with self.assertRaises(RuntimeError):
                            self.server.process_request(request, address)
                    self.assertEqual(request.fileno(), -1)
                    self.assertEqual(peer.recv(1), b"")
                finally:
                    request.close()
        self.assert_all_connection_slots_released()
        self.assertEqual(self.request()[0], 200)

    def test_authenticated_handler_failure_releases_its_connection_slot(self):
        self.assert_all_connection_slots_released()
        with mock.patch.object(self.server, "finish_request", side_effect=RuntimeError("handler fixture failure")):
            with mock.patch.object(self.server, "handle_error") as reported:
                with self.assertRaises((OSError, http.client.RemoteDisconnected)):
                    self.request("GET", STATIC_PREFIX + "widget.html", b"")
                self.assert_all_connection_slots_released()
                reported.assert_called_once()
        self.assertEqual(self.request()[0], 200)
        self.assert_all_connection_slots_released()

    def test_new_connections_observe_rotated_workload_certificate(self):
        def peer_certificate():
            connection = self.connection()
            try:
                connection.connect()
                certificate = connection.sock.getpeercert(binary_form=True)
                connection.request("GET", STATIC_PREFIX + "widget.html")
                response = connection.getresponse()
                self.assertEqual(response.status, 200)
                response.read()
                return certificate
            finally:
                connection.close()

        before = peer_certificate()
        replacement = self.root / "renewed.crt"
        subprocess.run(["openssl", "x509", "-req", "-in", str(self.root / "server.csr"),
                        "-CA", str(self.root / "ca.crt"), "-CAkey", str(self.root / "ca.key"),
                        "-set_serial", "123456789", "-out", str(replacement), "-days", "1",
                        "-extfile", str(self.root / "server.ext")],
                       check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        original = (self.root / "tls.crt").read_bytes()
        try:
            replacement.replace(self.root / "tls.crt")
            self.assertNotEqual(peer_certificate(), before)
        finally:
            (self.root / "tls.crt").write_bytes(original)
        self.assertEqual(peer_certificate(), before)
        self.assert_all_connection_slots_released()

    def test_original_credentials_forwarded_identity_and_bad_context_are_rejected(self):
        for headers, expected in [({"Authorization": "Bearer raw-user-token"}, 400), ({"Cookie": "__Host-xelab_auth=user-cookie"}, 400), ({"Forwarded": "host=evil"}, 400), ({"X-Forwarded-Host": "evil"}, 400), ({"Upgrade": "websocket"}, 400), ({"Range": "bytes=0-1"}, 400), ({"Transfer-Encoding": "chunked"}, 400), ({"X-Xelab-Delegation": ""}, 403), ({"X-Xelab-Subject": "forged"}, 403), ({"X-Xelab-Deadline-Unix-Ms": "1"}, 403), ({"X-Xelab-Idempotency-Key": "not+url/safe="}, 403), ({"Content-Type": "text/plain"}, 415)]:
            self.assertEqual(self.request(headers=headers)[0], expected)
        self.assertEqual(self.request(body=b"x" * 8193)[0], 413)

    def test_shared_slots_drain_and_reinitialization(self):
        for _ in range(4):
            self.assertTrue(self.state.slots.acquire(blocking=False))
        try:
            self.assertEqual(self.request()[0], 429)
            self.assertEqual(self.request("GET", STATIC_PREFIX + "widget.html", b"")[0], 429)
        finally:
            for _ in range(4):
                self.state.slots.release()
        self.assertEqual(self.state.drain(), 0)
        self.assertEqual(self.request()[0], 503)
        self.assertEqual(self.request("GET", STATIC_PREFIX + "window.html", b"")[0], 503)
        self.state.initialize()
        self.assertEqual(self.request()[0], 200)
        self.assertEqual(self.state.active, 0)


class ArtifactInputs(unittest.TestCase):
    def test_exact_openapi_digest_manifest_models_and_candidates(self):
        import jsonschema
        source = pathlib.Path(__file__).resolve().parents[2]
        api = json.loads((source / "docs/contracts/openapi.json").read_text())
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            manifest = build_artifact.build(root, str(uuid.uuid4()))
            schema = {"$ref": "#/components/schemas/Manifest", "components": api["components"]}
            jsonschema.Draft202012Validator(schema, format_checker=jsonschema.FormatChecker()).validate(manifest)
            raw = (root / "xelab/http/openapi.json").read_bytes()
            self.assertEqual(raw, pathlib.Path(__file__).with_name("openapi.json").read_bytes())
            digest = "sha256:" + hashlib.sha256(raw).hexdigest()
            self.assertEqual(manifest["routes"][0]["http_contract_digest"], digest)
            self.assertEqual(manifest["skills"][0]["target"]["http_contract_digest"], digest)
            for contribution in manifest["ui"]["contributions"]:
                self.assertEqual(contribution["skill_bindings"][0]["name"], manifest["skills"][0]["name"])
                self.assertEqual(contribution["skill_bindings"][0]["version"], manifest["skills"][0]["version"])
            self.assertEqual(json.loads((root / "xelab/plugin.json").read_bytes()), manifest)

    def test_existing_ed25519_signer_binds_manifest_and_immutable_image_reference(self):
        # This checks the actual signing envelope, not an OCI image admission.
        source = pathlib.Path(__file__).resolve().parents[2]
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            manifest = build_artifact.build(root, str(uuid.uuid4()))
            key = root / "disposable-publisher.key"
            subprocess.run(["openssl", "genpkey", "-algorithm", "Ed25519", "-out", str(key)], check=True, capture_output=True)
            public = root / "publisher.pub"
            subprocess.run(["openssl", "pkey", "-in", str(key), "-pubout", "-out", str(public)], check=True, capture_output=True)
            image = "docker.io/local/xelab-ui-reference@sha256:" + hashlib.sha256(b"component-image-reference-only").hexdigest()
            installed = json.loads(subprocess.check_output(["python3", str(source / "examples/echo/sign_artifact.py"), "--manifest", str(root / "xelab/plugin.json"), "--image", image, "--key", str(key)]))
            raw = (root / "xelab/plugin.json").read_bytes()
            envelope = f"xelab-artifact-v1\n{manifest['publisher_id']}\n{manifest['plugin_id']}\n{manifest['version']}\n{image.rsplit('@', 1)[1]}\nsha256:{hashlib.sha256(raw).hexdigest()}\n"
            (root / "envelope").write_text(envelope)
            (root / "signature").write_bytes(base64.b64decode(installed["signature"]))
            command = ["openssl", "pkeyutl", "-verify", "-pubin", "-inkey", str(public), "-rawin", "-in", str(root / "envelope"), "-sigfile", str(root / "signature")]
            subprocess.run(command, check=True, capture_output=True)
            (root / "envelope").write_text(envelope.replace(manifest["plugin_id"], "org.xelab.other"))
            self.assertNotEqual(subprocess.run(command, capture_output=True).returncode, 0)


if __name__ == "__main__":
    unittest.main()
