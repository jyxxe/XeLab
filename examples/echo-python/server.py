#!/usr/bin/python3
"""Independent ordinary plugin: generated 2.8 protobufs, no kernel/SDK imports."""
import concurrent.futures
import hashlib
import json
import os
import pathlib
import sys
import threading
import time

import grpc
from xelab.v1 import common_pb2 as common
from xelab.v1 import lifecycle_pb2 as life
from xelab.v1 import lifecycle_pb2_grpc as lifecycle
from xelab.v1 import capability_pb2 as cap
from xelab.v1 import capability_pb2_grpc as capability
from xelab.v1 import host_pb2 as host
from xelab.v1 import host_pb2_grpc as host_service
from xelab.v1 import migration_pb2 as migration
from xelab.v1 import migration_pb2_grpc as migration_service
from migration_store import Store, atomic, read


def digest(raw):
    return "sha256:" + hashlib.sha256(raw).hexdigest()


def require_kernel(context):
    identities = context.auth_context().get("x509_subject_alternative_name", [])
    if b"spiffe://xelab.local/kernel" not in identities:
        context.abort(grpc.StatusCode.PERMISSION_DENIED, "Kernel identity required")


class Echo(lifecycle.PluginLifecycleServicer, capability.CapabilityServiceServicer, migration_service.DataMigrationServicer):
    def __init__(self, manifest_path="/xelab/plugin.json", data_path="/data", scratch_path="/xelab-migration"):
        self.raw = pathlib.Path(manifest_path).read_bytes()
        self.manifest = json.loads(self.raw)
        self.contract = common.ContractRef(**self.manifest["capabilities"][0]["contract"])
        self.generation = int(os.environ["XELAB_GENERATION"])
        self.initialized = False
        self.draining = False
        self.results = {}
        self.lock = threading.Lock()
        schema = self.manifest["data_schema_version"]
        self.store = None if schema == "none" else Store(data_path, scratch_path, schema)

    def Describe(self, request, context):
        require_kernel(context)
        return life.DescribeResponse(manifest_json=self.raw, manifest_digest=digest(self.raw))

    def initialization_probe(self, phase):
        # Only the explicitly signed crash-test artifact opts into this probe.
        # Production activation has no scratch mount, so it cannot retain this
        # private journal. The kernel never consumes it as format/health proof.
        if "--migration-pause-after-initialize" not in sys.argv:
            return
        mounts = [line.split() for line in pathlib.Path("/proc/self/mountinfo").read_text().splitlines()]
        scratch = next((mount for mount in mounts if mount[4] == str(self.store.scratch)), None)
        if scratch is None:
            # The image's empty directory survives detachment; its existence
            # does not prove the private project is still mounted there.
            if phase == "initialize_known":
                return
            raise ValueError("Initialization probe requires the actual private scratch mount")
        if "rw" not in scratch[5].split(","):
            raise ValueError("Initialization probe scratch must be writable")
        path = self.store.scratch / "initialization-probe.json"
        events = read(path)[1] if path.exists() else []
        if not isinstance(events, list) or len(events) >= 16:
            raise ValueError("Initialization probe exceeded its bound")
        mount = next(mount for mount in mounts if mount[4] == str(self.store.data))
        events.append({"phase": phase, "business_read_only": "ro" in mount[5].split(",")})
        atomic(path, events)

    def Initialize(self, request, context):
        require_kernel(context)
        if (request.instance_id != os.environ["XELAB_INSTANCE_ID"]
                or request.deployment_id != os.environ["XELAB_DEPLOYMENT_ID"]
                or request.generation != self.generation
                or digest(request.config_json) != request.config_digest
                or json.loads(request.config_json) != {}):
            context.abort(grpc.StatusCode.FAILED_PRECONDITION, "Identity or config differs")
        with self.lock:
            if self.store is not None:
                try:
                    fresh = not self.store.path.exists()
                    self.store.initialize()
                    self.initialization_probe("initialize_fresh" if fresh else "initialize_known")
                    if fresh and "--migration-pause-after-initialize" in sys.argv:
                        while context.is_active():
                            time.sleep(0.05)
                        context.abort(grpc.StatusCode.UNAVAILABLE, "Crash probe initialization response withheld")
                except (OSError, ValueError):
                    context.abort(grpc.StatusCode.FAILED_PRECONDITION, "Existing business schema differs; Initialize cannot migrate data")
            self.initialized, self.draining = True, False
        return life.InitializeResponse(observed_generation=self.generation, initialized=True)

    def GetHealth(self, request, context):
        require_kernel(context)
        with self.lock:
            ready = self.initialized and not self.draining
            if ready and self.store is not None:
                try:
                    self.store.check()
                except (OSError, ValueError):
                    ready = False
        return life.HealthResponse(alive=True, observed_generation=self.generation,
                                   capabilities=[life.CapabilityHealth(contract=self.contract, ready=ready)])

    def Drain(self, request, context):
        require_kernel(context)
        with self.lock:
            self.draining = True
        return life.DrainResponse(accepting_requests=False, active_calls=0, drained=True)

    def Shutdown(self, request, context):
        require_kernel(context)
        with self.lock:
            self.draining = True
        return common.Ack(request_id=request.context.request_id, accepted=True)

    def Invoke(self, request, context):
        require_kernel(context)
        with self.lock:
            if not self.initialized or self.draining:
                context.abort(grpc.StatusCode.UNAVAILABLE, "Not initialized")
            if (request.contract != self.contract or len(request.input_json) > 65536
                    or digest(request.input_json) != request.input_digest
                    or request.context.deadline_unix_ms <= int(time.time() * 1000)
                    or not request.context.delegation_token):
                context.abort(grpc.StatusCode.INVALID_ARGUMENT, "Invalid call envelope")
            try:
                def unique(pairs):
                    result = {}
                    for key, value in pairs:
                        if key in result:
                            raise ValueError("duplicate member")
                        result[key] = value
                    return result
                value = json.loads(request.input_json, object_pairs_hook=unique)
                blob_contract = self.contract.name == "org.xelab.echo-blobs"
                allowed = {"message", "blob_id", "export"} if blob_contract else {"message"}
                if (not isinstance(value, dict) or not set(value).issubset(allowed)
                        or "message" not in value or not isinstance(value["message"], str) or len(value["message"]) > 1024):
                    raise ValueError("invalid echo input")
            except (ValueError, UnicodeDecodeError):
                context.abort(grpc.StatusCode.INVALID_ARGUMENT, "Input does not match contract")
            result = {"message": value["message"]}
            if self.store is not None:
                try:
                    self.store.append(request.call_id, value["message"])
                except (OSError, ValueError):
                    context.abort(grpc.StatusCode.FAILED_PRECONDITION, "Business write cannot be committed in the current schema")
            if blob_contract and ("blob_id" in value or value.get("export")):
                identity = pathlib.Path("/run/xelab/identity")
                credentials = grpc.ssl_channel_credentials((identity / "ca.crt").read_bytes(), (identity / "tls.key").read_bytes(), (identity / "tls.crt").read_bytes())
                endpoint = os.environ["XELAB_HOST_RPC_ENDPOINT"].removeprefix("https://")
                with grpc.secure_channel(endpoint, credentials, options=[("grpc.ssl_target_name_override", "kernel.xelab.local")]) as channel:
                    proxy = host_service.HostServiceStub(channel)
                    if "blob_id" in value:
                        chunks = proxy.ReadBlob(host.ReadBlobRequest(context=request.context, blob_id=value["blob_id"]), timeout=5)
                        sha = hashlib.sha256()
                        offset = 0
                        for chunk in chunks:
                            if chunk.offset != offset or len(chunk.data) > 262144:
                                context.abort(grpc.StatusCode.DATA_LOSS, "Invalid Blob stream")
                            sha.update(chunk.data)
                            offset += len(chunk.data)
                        result["source_digest"] = "sha256:" + sha.hexdigest()
                    if value.get("export"):
                        data = value["message"].encode()
                        frames = [host.WriteBlobRequest(header=host.WriteBlobHeader(context=request.context, content_type="text/plain", size_bytes=len(data), digest=digest(data)))]
                        if data:
                            frames.append(host.WriteBlobRequest(data=host.BlobChunk(offset=0, data=data)))
                        reference = proxy.WriteBlob(iter(frames), timeout=5)
                        result["blob"] = {"blob_id": reference.blob_id, "size_bytes": reference.size_bytes, "digest": reference.digest}
            output = json.dumps(result, ensure_ascii=False, separators=(",", ":")).encode()
            response = cap.InvokeResponse(call_id=request.call_id, output_json=output,
                                          output_digest=digest(output))
            if len(self.results) >= 1024:
                self.results.clear()
            self.results[request.call_id] = response
            return response

    def InvokeStream(self, request, context):
        require_kernel(context)
        context.abort(grpc.StatusCode.UNIMPLEMENTED, "No streaming contract")

    def Cancel(self, request, context):
        require_kernel(context)
        return common.Ack(request_id=request.context.request_id, accepted=True)

    def QueryResult(self, request, context):
        require_kernel(context)
        with self.lock:
            value = self.results.get(request.call_id)
        if value is None:
            return cap.QueryResultResponse(state=common.EXECUTION_STATE_UNKNOWN)
        return cap.QueryResultResponse(state=common.EXECUTION_STATE_SUCCEEDED, completed=value)

    def migration_key(self, request, context, phase):
        require_kernel(context)
        if self.store is None:
            context.abort(grpc.StatusCode.UNIMPLEMENTED, "This release has no business schema")
        key = request.context.idempotency_key
        if request.context.deadline_unix_ms <= int(time.time() * 1000) or not key.endswith(":" + phase):
            context.abort(grpc.StatusCode.INVALID_ARGUMENT, "Migration deadline or operation key differs")
        import uuid
        try:
            operation = str(uuid.UUID(key[:-(len(phase) + 1)]))
            if operation != key[:-(len(phase) + 1)] or uuid.UUID(operation).int == 0:
                raise ValueError("Noncanonical operation")
        except ValueError:
            context.abort(grpc.StatusCode.INVALID_ARGUMENT, "Canonical migration operation required")
        return operation + ":plan"

    def Plan(self, request, context):
        key = self.migration_key(request, context, "plan")
        if request.deployment_id != os.environ["XELAB_DEPLOYMENT_ID"]:
            context.abort(grpc.StatusCode.FAILED_PRECONDITION, "Migration deployment differs")
        with self.lock:
            try:
                plan_id, plan_digest, value = self.store.plan(key, request.deployment_id, request.source_schema, request.target_schema)
                self.initialization_probe("plan")
                return migration.MigrationPlan(plan_id=plan_id, plan_digest=plan_digest, requires_exclusive_write=True,
                                               backward_read_compatible=False, reversible=False,
                                               temporary_bytes_required=value["temporary_bytes_required"])
            except (OSError, ValueError, KeyError, TypeError):
                context.abort(grpc.StatusCode.FAILED_PRECONDITION, "Cannot plan the requested business format transition")

    def Apply(self, request, context):
        key = self.migration_key(request, context, "apply")
        with self.lock:
            try:
                self.store.bind(request.plan_id, key, os.environ["XELAB_DEPLOYMENT_ID"])
                def after_write():
                    if "--migration-fail-after-write" in sys.argv:
                        context.abort(grpc.StatusCode.UNAVAILABLE, "Acceptance fault after the actual business write committed")
                observed = self.store.apply(request.plan_id, request.plan_digest, request.backup_id, request.volume_fence, after_write=after_write)
                return migration.MigrationResult(plan_id=request.plan_id, succeeded=True, observed_schema=observed)
            except (OSError, ValueError, KeyError, TypeError):
                context.abort(grpc.StatusCode.FAILED_PRECONDITION, "Migration outcome requires disk verification")

    def Verify(self, request, context):
        key = self.migration_key(request, context, "verify")
        with self.lock:
            try:
                self.store.bind(request.plan_id, key, os.environ["XELAB_DEPLOYMENT_ID"])
                observed = self.store.verify(request.plan_id, request.volume_fence)
                self.initialization_probe("verify")
                return migration.MigrationResult(plan_id=request.plan_id, succeeded=True, observed_schema=observed)
            except (OSError, ValueError, KeyError, TypeError):
                context.abort(grpc.StatusCode.FAILED_PRECONDITION, "Actual business format does not match the saved plan")


def main():
    echo = Echo()
    cert_path = pathlib.Path(os.environ["XELAB_TLS_CERT_PATH"])
    key = pathlib.Path(os.environ["XELAB_TLS_KEY_PATH"]).read_bytes()
    ca = pathlib.Path(os.environ["XELAB_TLS_CA_PATH"]).read_bytes()
    last = [None]

    def certificates():
        cert = cert_path.read_bytes()  # kernel atomically rotates cert with the same generation key
        if cert == last[0]:
            return None
        config = grpc.ssl_server_certificate_configuration(((key, cert),), root_certificates=ca)
        last[0] = cert
        return config

    credentials = grpc.dynamic_ssl_server_credentials(certificates(), certificates,
                                                       require_client_authentication=True)
    server = grpc.server(concurrent.futures.ThreadPoolExecutor(max_workers=4),
                         options=[("grpc.max_receive_message_length", 1048576)])
    lifecycle.add_PluginLifecycleServicer_to_server(echo, server)
    capability.add_CapabilityServiceServicer_to_server(echo, server)
    migration_service.add_DataMigrationServicer_to_server(echo, server)
    if not server.add_secure_port("0.0.0.0:7000", credentials):
        raise RuntimeError("Cannot bind plugin listener")
    server.start()
    server.wait_for_termination()


if __name__ == "__main__":
    main()
