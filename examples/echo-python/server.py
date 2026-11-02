#!/usr/bin/python3
"""Independent ordinary plugin: generated 2.8 protobufs, no kernel/SDK imports."""
import concurrent.futures
import hashlib
import json
import os
import pathlib
import threading
import time

import grpc
from xelab.v1 import common_pb2 as common
from xelab.v1 import lifecycle_pb2 as life
from xelab.v1 import lifecycle_pb2_grpc as lifecycle
from xelab.v1 import capability_pb2 as cap
from xelab.v1 import capability_pb2_grpc as capability


def digest(raw):
    return "sha256:" + hashlib.sha256(raw).hexdigest()


def require_kernel(context):
    identities = context.auth_context().get("x509_subject_alternative_name", [])
    if b"spiffe://xelab.local/kernel" not in identities:
        context.abort(grpc.StatusCode.PERMISSION_DENIED, "Kernel identity required")


class Echo(lifecycle.PluginLifecycleServicer, capability.CapabilityServiceServicer):
    def __init__(self):
        self.raw = pathlib.Path("/xelab/plugin.json").read_bytes()
        self.manifest = json.loads(self.raw)
        self.contract = common.ContractRef(**self.manifest["capabilities"][0]["contract"])
        self.generation = int(os.environ["XELAB_GENERATION"])
        self.initialized = False
        self.draining = False
        self.results = {}
        self.lock = threading.Lock()

    def Describe(self, request, context):
        require_kernel(context)
        return life.DescribeResponse(manifest_json=self.raw, manifest_digest=digest(self.raw))

    def Initialize(self, request, context):
        require_kernel(context)
        if (request.instance_id != os.environ["XELAB_INSTANCE_ID"]
                or request.deployment_id != os.environ["XELAB_DEPLOYMENT_ID"]
                or request.generation != self.generation
                or digest(request.config_json) != request.config_digest
                or json.loads(request.config_json) != {}):
            context.abort(grpc.StatusCode.FAILED_PRECONDITION, "Identity or config differs")
        with self.lock:
            self.initialized, self.draining = True, False
        return life.InitializeResponse(observed_generation=self.generation, initialized=True)

    def GetHealth(self, request, context):
        require_kernel(context)
        with self.lock:
            ready = self.initialized and not self.draining
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
                if (not isinstance(value, dict) or set(value) != {"message"}
                        or not isinstance(value["message"], str) or len(value["message"]) > 1024):
                    raise ValueError("invalid echo input")
            except (ValueError, UnicodeDecodeError):
                context.abort(grpc.StatusCode.INVALID_ARGUMENT, "Input does not match contract")
            output = json.dumps({"message": value["message"]}, ensure_ascii=False,
                                separators=(",", ":")).encode()
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
    if not server.add_secure_port("0.0.0.0:7000", credentials):
        raise RuntimeError("Cannot bind plugin listener")
    server.start()
    server.wait_for_termination()


if __name__ == "__main__":
    main()
