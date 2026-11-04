#!/usr/bin/python3
"""Run the real packaged lifecycle and HTTP servers in an isolated test image.

Only test identities may be supplied. Requires generated bindings and gRPC from
the image, plus identity/tls.*, ca.crt, kernel.crt/key and foreign.crt/key files.
This is a component probe; it does not exercise the kernel or its OCI runtime.
"""
import argparse
import hashlib
import http.client
import json
import os
import pathlib
import ssl
import subprocess
import sys
import time

import grpc
from xelab.v1 import lifecycle_pb2 as life
from xelab.v1 import lifecycle_pb2_grpc as lifecycle

from http_server import BUSINESS_PATH, STATIC_PATH


def main():
    assert os.geteuid() == 65532, "The packaged probe must run with the plugin UID"
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--identity", required=True, type=pathlib.Path)
    args = parser.parse_args()
    root = args.identity
    instance = "00000000-0000-4000-8000-000000000001"
    deployment = "00000000-0000-4000-8000-000000000002"
    child_env = dict(os.environ, XELAB_GENERATION="1", XELAB_INSTANCE_ID=instance,
                     XELAB_DEPLOYMENT_ID=deployment, XELAB_TLS_CERT_PATH=str(root / "tls.crt"),
                     XELAB_TLS_KEY_PATH=str(root / "tls.key"), XELAB_TLS_CA_PATH=str(root / "ca.crt"))
    process = subprocess.Popen([sys.executable, str(pathlib.Path(__file__).with_name("server.py"))], env=child_env)
    ca = (root / "ca.crt").read_bytes()
    credentials = grpc.ssl_channel_credentials(ca, (root / "kernel.key").read_bytes(), (root / "kernel.crt").read_bytes())
    context = ssl.create_default_context(cafile=root / "ca.crt")
    context.load_cert_chain(root / "kernel.crt", root / "kernel.key")

    def request(method, path, body=None):
        connection = http.client.HTTPSConnection("localhost", 8080, context=context, timeout=2)
        try:
            connection.request(method, path, body=body, headers={"Content-Type": "application/json",
                "X-Xelab-Delegation": "component-probe-opaque-handle", "X-Xelab-Subject": instance})
            response = connection.getresponse()
            return response.status, response.read()
        finally:
            connection.close()

    try:
        with grpc.secure_channel("localhost:7000", credentials) as channel:
            grpc.channel_ready_future(channel).result(timeout=10)
            client = lifecycle.PluginLifecycleStub(channel)
            described = client.Describe(life.DescribeRequest(), timeout=2)
            assert described.manifest_digest == "sha256:" + hashlib.sha256(described.manifest_json).hexdigest()
            assert json.loads(described.manifest_json)["plugin_id"] == "org.xelab.http-echo"
            for _ in range(50):
                try:
                    assert request("POST", BUSINESS_PATH, b'{"message":"before"}')[0] == 503
                    break
                except ConnectionRefusedError:
                    time.sleep(0.1)
            else:
                raise AssertionError("HTTP listener did not start")
            initialize = life.InitializeRequest(instance_id=instance, deployment_id=deployment, generation=1,
                config_json=b"{}", config_digest="sha256:" + hashlib.sha256(b"{}").hexdigest())
            assert client.Initialize(initialize, timeout=2).initialized
            assert client.GetHealth(life.HealthRequest(), timeout=2).observed_generation == 1
            status, body = request("POST", BUSINESS_PATH, b'{"message":"real plugin"}')
            assert status == 200 and json.loads(body) == {"message": "real plugin"}
            assert request("GET", STATIC_PATH)[0] == 200
            drained = client.Drain(life.DrainRequest(grace_ms=1000), timeout=2)
            assert drained.drained and not drained.accepting_requests and drained.active_calls == 0
            assert request("POST", BUSINESS_PATH, b'{"message":"after drain"}')[0] == 503
            assert client.Initialize(initialize, timeout=2).initialized
            assert request("POST", BUSINESS_PATH, b'{"message":"restored"}')[0] == 200
        foreign = grpc.ssl_channel_credentials(ca, (root / "foreign.key").read_bytes(), (root / "foreign.crt").read_bytes())
        with grpc.secure_channel("localhost:7000", foreign) as channel:
            try:
                lifecycle.PluginLifecycleStub(channel).Describe(life.DescribeRequest(), timeout=2)
                raise AssertionError("Foreign workload reached lifecycle")
            except grpc.RpcError as error:
                assert error.code() == grpc.StatusCode.PERMISSION_DENIED
        print(json.dumps({"component": "http-reference", "lifecycle": "passed", "https": "passed", "kernel_oci_integration": "not_executed"}))
    finally:
        process.terminate()
        process.wait(timeout=5)


if __name__ == "__main__":
    main()
