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


def build(root, publisher, platform="linux/amd64", plugin_version="1.0.0", include_rpc=False):
    publisher = str(uuid.UUID(publisher))
    source = pathlib.Path(__file__).with_name("openapi.json")
    raw = source.read_bytes()
    contract = json.loads(raw)
    digest = "sha256:" + hashlib.sha256(raw).hexdigest()
    path = "/plugins/" + PLUGIN_ID + "/echo"
    operation = contract["paths"][path]["post"]
    manifest = {
        "manifest_version": "1", "plugin_id": PLUGIN_ID, "version": plugin_version,
        "publisher_id": publisher, "host_api_major": 1, "platforms": [platform],
        "entrypoint": ["/usr/bin/python3", "/opt/plugin/server.py"],
        "capabilities": [], "dependencies": [], "requested_rules": [], "volumes": [],
        "minimum_resources": {"cpu_millis": 100, "memory_bytes": 67108864, "ephemeral_bytes": 1048576, "pids": 64},
        "health": {"startup_timeout_ms": 60000, "interval_ms": 5000, "timeout_ms": 2000, "failure_threshold": 3},
        "routes": [
            {"path_prefix": "/plugins/" + PLUGIN_ID + "/", "methods": ["POST"], "kind": "http", "required_action": "plugin.http.invoke",
             "maximum_body_bytes": 8192, "timeout_ms": 10000, "http_contract_digest": digest, "authentication": "user"},
            {"path_prefix": "/ui/" + PLUGIN_ID + "/", "methods": ["GET", "HEAD"], "kind": "static",
             "required_action": "plugin.http.invoke", "maximum_body_bytes": 0, "timeout_ms": 10000, "authentication": "public"},
        ],
        "config_schema": {"type": "object", "additionalProperties": False},
        "data_schema_version": "none", "upgrade_strategy": "recreate", "business_entry": "direct",
        "concurrency": {"max_active_requests": 4, "max_queue_requests": 0, "queue_timeout_ms": 100},
        "skills": [{
            "name": PLUGIN_ID + ".echo", "version": plugin_version, "description": "Echo a message through the fixed signed HTTP operation",
            "input_schema": operation["requestBody"]["content"]["application/json"]["schema"],
            "output_schema": operation["responses"]["200"]["content"]["application/json"]["schema"], "stream_item_schema": None,
            "target": {"kind": "http", "plugin_id": PLUGIN_ID, "http_operation_id": operation["operationId"], "http_contract_digest": digest},
            "mapping": "identity", "maximum_timeout_ms": 10000, "input_blobs": False, "tags": ["http", "echo"],
        }],
    }
    if include_rpc:
        name, version = PLUGIN_ID + ".rpc", "1.0.0"
        input_schema = contract["paths"]["/plugins/" + PLUGIN_ID + "/hold"]["post"]["requestBody"]["content"]["application/json"]["schema"]
        output_schema = operation["responses"]["200"]["content"]["application/json"]["schema"]
        directory = root / "xelab" / "contracts" / name / version
        directory.mkdir(parents=True, exist_ok=True)
        checks = []
        for file, value in [("input.schema.json", input_schema), ("output.schema.json", output_schema),
                            ("stream.schema.json", None), ("semantics.json", {"side_effect": "none"})]:
            value_raw = encoded(value)
            (directory / file).write_bytes(value_raw)
            checks.append(f"{file} {hashlib.sha256(value_raw).hexdigest()}\n")
        reference = {"name": name, "version": version,
                     "schema_digest": "sha256:" + hashlib.sha256("".join(checks).encode()).hexdigest()}
        manifest["capabilities"] = [{"contract": reference, "input_schema": input_schema,
            "output_schema": output_schema, "stream_item_schema": None, "side_effect": "none",
            "maximum_input_bytes": 8192, "maximum_output_bytes": 8192}]
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
    parser.add_argument("--plugin-version", default="1.0.0")
    parser.add_argument("--include-rpc", action="store_true")
    args = parser.parse_args()
    build(args.output, args.publisher_id, args.platform, args.plugin_version, args.include_rpc)
