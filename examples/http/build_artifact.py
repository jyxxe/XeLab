#!/usr/bin/env python3
"""Build immutable HTTP contract and signed-manifest inputs for the OCI image."""
import argparse
import hashlib
import json
import pathlib
import uuid


PLUGIN_ID = "org.xelab.http-echo"


def encoded(value):
    return (json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n").encode()


def build(root, publisher, platform="linux/amd64"):
    publisher = str(uuid.UUID(publisher))
    source = pathlib.Path(__file__).with_name("openapi.json")
    raw = source.read_bytes()
    contract = json.loads(raw)
    digest = "sha256:" + hashlib.sha256(raw).hexdigest()
    path = "/plugins/" + PLUGIN_ID + "/echo"
    operation = contract["paths"][path]["post"]
    manifest = {
        "manifest_version": "1", "plugin_id": PLUGIN_ID, "version": "1.0.0",
        "publisher_id": publisher, "host_api_major": 1, "platforms": [platform],
        "entrypoint": ["/usr/bin/python3", "/opt/plugin/server.py"],
        "capabilities": [], "dependencies": [], "requested_rules": [], "volumes": [],
        "minimum_resources": {"cpu_millis": 100, "memory_bytes": 67108864, "ephemeral_bytes": 1048576, "pids": 64},
        "health": {"startup_timeout_ms": 60000, "interval_ms": 5000, "timeout_ms": 2000, "failure_threshold": 3},
        "routes": [
            {"path_prefix": path, "methods": ["POST"], "kind": "http", "required_action": "plugin.http.invoke",
             "maximum_body_bytes": 8192, "timeout_ms": 10000, "http_contract_digest": digest, "authentication": "user"},
            {"path_prefix": "/ui/" + PLUGIN_ID + "/", "methods": ["GET", "HEAD"], "kind": "static",
             "required_action": "plugin.http.invoke", "maximum_body_bytes": 0, "timeout_ms": 10000, "authentication": "public"},
        ],
        "config_schema": {"type": "object", "additionalProperties": False},
        "data_schema_version": "none", "upgrade_strategy": "recreate", "business_entry": "direct",
        "concurrency": {"max_active_requests": 4, "max_queue_requests": 0, "queue_timeout_ms": 100},
        "skills": [{
            "name": PLUGIN_ID + ".echo", "version": "1.0.0", "description": "Echo a message through the fixed signed HTTP operation",
            "input_schema": operation["requestBody"]["content"]["application/json"]["schema"],
            "output_schema": operation["responses"]["200"]["content"]["application/json"]["schema"], "stream_item_schema": None,
            "target": {"kind": "http", "plugin_id": PLUGIN_ID, "http_operation_id": operation["operationId"], "http_contract_digest": digest},
            "mapping": "identity", "maximum_timeout_ms": 10000, "input_blobs": False, "tags": ["http", "echo"],
        }],
    }
    directory = root / "xelab" / "http"
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "openapi.json").write_bytes(raw)
    (root / "xelab" / "plugin.json").write_bytes(encoded(manifest))
    return manifest


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--publisher-id", required=True)
    parser.add_argument("--output", type=pathlib.Path, default=pathlib.Path("examples/http/generated"))
    parser.add_argument("--platform", choices=["linux/amd64", "linux/arm64"], default="linux/amd64")
    args = parser.parse_args()
    build(args.output, args.publisher_id, args.platform)
