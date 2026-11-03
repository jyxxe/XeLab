"""Explicit OCI acceptance probe. Never print or persist leased plaintext."""
import hashlib
import os
import sys
import time
import grpc
from xelab.v1 import common_pb2, host_pb2, host_pb2_grpc

identity = "/run/xelab/identity/"
with open(identity + "ca.crt", "rb") as f:
    ca = f.read()
with open(identity + "tls.crt", "rb") as f:
    cert = f.read()
with open(identity + "tls.key", "rb") as f:
    key = f.read()
credentials = grpc.ssl_channel_credentials(ca, key, cert)
endpoint = os.environ["XELAB_HOST_RPC_ENDPOINT"].removeprefix("https://")
channel = grpc.secure_channel(endpoint, credentials, options=[("grpc.ssl_target_name_override", "kernel.xelab.local")])
request = host_pb2.LeaseSecretRequest(
    context=common_pb2.RpcContext(deadline_unix_ms=int(time.time() * 1000) + 1900),
    binding_id=sys.argv[1], requested_seconds=30,
)
try:
    lease = host_pb2_grpc.HostServiceStub(channel).LeaseSecret(request, timeout=2)
    if sys.argv[2] == "denied":
        raise RuntimeError("Revoked Secret unexpectedly leased")
    assert hashlib.sha256(lease.value).hexdigest() == sys.argv[2]
    assert lease.secret_version == int(sys.argv[3])
    assert 0 < lease.expires_unix_ms - int(time.time() * 1000) <= 30000
    print("lease-verified")
except grpc.RpcError as error:
    if sys.argv[2] != "denied" or error.code() not in (grpc.StatusCode.PERMISSION_DENIED, grpc.StatusCode.UNAUTHENTICATED):
        raise RuntimeError("Secret lease probe failed") from None
    print("lease-denied")
