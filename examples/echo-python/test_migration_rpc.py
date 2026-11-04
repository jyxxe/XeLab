"""Real loopback mTLS/Protobuf/files; not kernel/OCI/XFS/off-host acceptance."""
import concurrent.futures
import hashlib
import importlib.util
import os
import pathlib
import shutil
import subprocess
import tempfile
import time
import unittest
import uuid
from unittest.mock import patch

import grpc
from xelab.v1 import common_pb2 as common
from xelab.v1 import lifecycle_pb2 as life
from xelab.v1 import lifecycle_pb2_grpc as lifecycle
from xelab.v1 import migration_pb2 as migration
from xelab.v1 import migration_pb2_grpc as migration_service
from migration_store import Store, read
from server import Echo


class MigrationRpc(unittest.TestCase):
    def command(self, *arguments):
        subprocess.run(["openssl", *arguments], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)

    def identity(self, name, alternative, usage):
        key, cert, csr = [self.root / (name + suffix) for suffix in (".key", ".crt", ".csr")]
        self.command("req", "-new", "-nodes", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-keyout", str(key), "-out", str(csr), "-subj", "/CN=" + name)
        extensions = self.root / (name + ".extensions")
        extensions.write_text("basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=" + usage + "\nsubjectAltName=" + alternative + "\n")
        self.command("x509", "-req", "-in", str(csr), "-CA", str(self.root / "ca.crt"), "-CAkey", str(self.root / "ca.key"), "-set_serial", str(uuid.uuid4().int), "-days", "1", "-extfile", str(extensions), "-out", str(cert))
        return key.read_bytes(), cert.read_bytes()

    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = pathlib.Path(temporary.name)
        self.command("req", "-new", "-x509", "-nodes", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-keyout", str(self.root / "ca.key"), "-out", str(self.root / "ca.crt"), "-days", "1", "-subj", "/CN=reference-migration-ca", "-addext", "basicConstraints=critical,CA:TRUE", "-addext", "keyUsage=critical,keyCertSign,cRLSign")
        self.ca = (self.root / "ca.crt").read_bytes()
        self.server_identity = self.identity("reference-server", "DNS:reference-migration.xelab.local", "serverAuth")
        self.kernel_identity = self.identity("kernel-client", "URI:spiffe://xelab.local/kernel", "clientAuth")
        self.other_identity = self.identity("other-client", "URI:spiffe://xelab.local/workload/untrusted", "clientAuth")
        self.deployment, self.operation, self.subject, self.backup = [str(uuid.uuid4()) for _ in range(4)]
        environment = patch.dict(os.environ, {"XELAB_DEPLOYMENT_ID": self.deployment, "XELAB_GENERATION": "7"})
        environment.start()
        self.addCleanup(environment.stop)
        self.original, self.data, self.scratch = [self.root / name for name in ("original", "data", "scratch")]
        self.original.mkdir()
        self.scratch.mkdir()
        source = Store(self.original, self.scratch, "1")
        source.initialize()
        source.append("existing", "保留业务数据")
        shutil.copytree(self.original, self.data)
        self.before = (self.original / "store.json").read_bytes()
        builder_path = pathlib.Path(__file__).resolve().parents[1] / "echo" / "build_artifact.py"
        spec = importlib.util.spec_from_file_location("migration_test_builder", builder_path)
        builder = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(builder)
        builder.build(self.root / "artifact", str(uuid.uuid4()), implementation="python", volume_access="rw", plugin_version="2.0.0", data_schema="2")
        self.manifest = self.root / "artifact" / "xelab" / "plugin.json"
        self.start()

    def channel(self, identity):
        key, cert = identity
        credentials = grpc.ssl_channel_credentials(self.ca, key, cert)
        channel = grpc.secure_channel(self.address, credentials, options=[("grpc.ssl_target_name_override", "reference-migration.xelab.local")])
        self.addCleanup(channel.close)
        grpc.channel_ready_future(channel).result(timeout=5)
        return channel

    def start(self):
        executor = concurrent.futures.ThreadPoolExecutor(max_workers=4)
        self.addCleanup(lambda: executor.shutdown(wait=True))
        server = grpc.server(executor)
        self.addCleanup(lambda: server.stop(0).wait())
        echo = Echo(self.manifest, self.data, self.scratch)
        lifecycle.add_PluginLifecycleServicer_to_server(echo, server)
        migration_service.add_DataMigrationServicer_to_server(echo, server)
        key, cert = self.server_identity
        credentials = grpc.ssl_server_credentials(((key, cert),), root_certificates=self.ca, require_client_auth=True)
        port = server.add_secure_port("127.0.0.1:0", credentials)
        self.assertNotEqual(port, 0)
        server.start()
        self.address = "127.0.0.1:" + str(port)
        self.server = server
        self.client = migration_service.DataMigrationStub(self.channel(self.kernel_identity))

    def context(self, phase, operation=None):
        return common.RpcContext(request_id=str(uuid.uuid4()), subject_id=self.subject,
                                 idempotency_key=(operation or self.operation) + ":" + phase,
                                 deadline_unix_ms=int(time.time() * 1000) + 10000)

    def refused(self, call, request, code):
        with self.assertRaises(grpc.RpcError) as failure:
            call(request, timeout=5)
        self.assertEqual(failure.exception.code(), code)

    def test_known_initialize_with_image_scratch_directory_but_no_scratch_mount(self):
        self.server.stop(0).wait()
        builder_path = pathlib.Path(__file__).resolve().parents[1] / "echo" / "build_artifact.py"
        spec = importlib.util.spec_from_file_location("initialization_probe_builder", builder_path)
        builder = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(builder)
        builder.build(self.root / "artifact", str(uuid.uuid4()), implementation="python",
                      volume_access="rw", data_schema="1", migration_pause_after_initialize=True)
        self.start()
        client = lifecycle.PluginLifecycleStub(self.channel(self.kernel_identity))
        instance = str(uuid.uuid4())
        request = life.InitializeRequest(context=self.context("initialize"), deployment_id=self.deployment,
                                         instance_id=instance, generation=7, config_json=b"{}",
                                         config_digest="sha256:" + hashlib.sha256(b"{}").hexdigest(), approval_revision=1)
        # The image retains a plain scratch directory after the actual private
        # mount is detached. Known activation must not try writing into it.
        os.chmod(self.scratch, 0o500)
        try:
            with patch("sys.argv", ["server.py", "--migration-pause-after-initialize"]), patch.dict(os.environ, {"XELAB_INSTANCE_ID": instance}):
                response = client.Initialize(request, timeout=5)
        finally:
            os.chmod(self.scratch, 0o700)
        self.assertTrue(response.initialized)
        self.assertEqual(response.observed_generation, 7)
        self.assertEqual((self.data / "store.json").read_bytes(), self.before)
        self.assertEqual(list(self.scratch.iterdir()), [])

    def test_real_tls_plan_ambiguous_apply_restart_and_actual_disk_verification(self):
        plan_request = migration.PlanMigrationRequest(context=self.context("plan"), deployment_id=self.deployment, source_schema="1", target_schema="2")
        other = migration_service.DataMigrationStub(self.channel(self.other_identity))
        self.refused(other.Plan, plan_request, grpc.StatusCode.PERMISSION_DENIED)
        self.assertEqual(list(self.scratch.iterdir()), [])
        life_client = lifecycle.PluginLifecycleStub(self.channel(self.kernel_identity))
        described = life_client.Describe(life.DescribeRequest(context=self.context("describe")), timeout=5)
        self.assertEqual(described.manifest_json, self.manifest.read_bytes())
        self.assertEqual(described.manifest_digest, "sha256:" + hashlib.sha256(described.manifest_json).hexdigest())
        os.chmod(self.data, 0o500)
        os.chmod(self.data / "store.json", 0o400)
        try:
            plan = self.client.Plan(plan_request, timeout=5)
            self.assertTrue(plan.requires_exclusive_write)
            self.assertFalse(plan.backward_read_compatible)
            self.assertEqual(self.client.Plan(plan_request, timeout=5), plan)
        finally:
            os.chmod(self.data, 0o700)
            os.chmod(self.data / "store.json", 0o600)
        self.assertEqual((self.data / "store.json").read_bytes(), self.before)
        initialize = life.InitializeRequest(context=self.context("initialize"), deployment_id=self.deployment,
                                            instance_id=str(uuid.uuid4()), generation=7, config_json=b"{}",
                                            config_digest="sha256:" + hashlib.sha256(b"{}").hexdigest(), approval_revision=1)
        # The normal lifecycle identity check runs before its schema guard.
        with patch.dict(os.environ, {"XELAB_INSTANCE_ID": initialize.instance_id}):
            self.refused(life_client.Initialize, initialize, grpc.StatusCode.FAILED_PRECONDITION)
        self.assertEqual((self.data / "store.json").read_bytes(), self.before)
        apply = migration.ApplyMigrationRequest(context=self.context("apply"), plan_id=plan.plan_id, plan_digest=plan.plan_digest, backup_id=self.backup, volume_fence=13)
        wrong = migration.ApplyMigrationRequest()
        wrong.CopyFrom(apply)
        wrong.plan_digest = "sha256:" + "0" * 64
        self.refused(self.client.Apply, wrong, grpc.StatusCode.FAILED_PRECONDITION)
        with patch("sys.argv", ["server.py", "--migration-fail-after-write"]):
            self.refused(self.client.Apply, apply, grpc.StatusCode.UNAVAILABLE)
        self.assertEqual(read(self.data / "store.json")[1]["schema"], "2")
        self.server.stop(0).wait()
        self.start()  # Fresh Python service state, same persisted private files.
        verify = migration.VerifyMigrationRequest(context=self.context("verify"), plan_id=plan.plan_id, volume_fence=13)
        result = self.client.Verify(verify, timeout=5)
        self.assertTrue(result.succeeded)
        self.assertFalse(result.HasField("error"))
        self.assertEqual(result.observed_schema, "2")
        self.assertEqual(Store(self.data, self.scratch, "2").check(), Store(self.original, self.scratch, "1").check())
        self.assertEqual((self.original / "store.json").read_bytes(), self.before)
        verify.context.CopyFrom(self.context("verify", str(uuid.uuid4())))
        self.refused(self.client.Verify, verify, grpc.StatusCode.FAILED_PRECONDITION)
        verify.context.CopyFrom(self.context("verify"))
        Store(self.data, self.scratch, "2").append("unexpected", "unpublished unexpected write")
        self.refused(self.client.Verify, verify, grpc.StatusCode.FAILED_PRECONDITION)


if __name__ == "__main__":
    unittest.main()
