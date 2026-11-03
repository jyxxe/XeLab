#!/usr/bin/env python3
"""Build the signed manifest/contract inputs; image signing happens after import."""
import argparse
import hashlib
import json
import pathlib
import uuid


def encoded(value):
    return (json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n").encode()


def build(root, publisher, platform="linux/amd64", implementation="rust", request_secret=False, volume_access=None, request_blob=False):
    publisher = str(uuid.UUID(publisher))
    schema = {"type": "object", "properties": {"message": {"type": "string", "maxLength": 1024}}, "required": ["message"], "additionalProperties": False}
    input_schema = json.loads(json.dumps(schema))
    output_schema = json.loads(json.dumps(schema))
    name, version = ("org.xelab.echo-blobs" if request_blob else "org.xelab.echo"), "1.0.0"
    if request_blob:
        input_schema["properties"].update({"blob_id": {"type": "string", "format": "uuid", "x-xelab-resource-kind": "blob"}, "export": {"type": "boolean"}})
        output_schema["properties"].update({"source_digest": {"type": "string", "maxLength": 71}, "blob": {"type": "object", "properties": {"blob_id": {"type": "string", "format": "uuid"}, "size_bytes": {"type": "integer", "minimum": 0, "maximum": 67108864}, "digest": {"type": "string", "maxLength": 71}}, "required": ["blob_id", "size_bytes", "digest"], "additionalProperties": False}})
    directory = root / "xelab" / "contracts" / name / version
    directory.mkdir(parents=True, exist_ok=True)
    checks = []
    for file, value in [("input.schema.json", input_schema), ("output.schema.json", output_schema), ("stream.schema.json", None), ("semantics.json", {"side_effect": "none"})]:
        raw = encoded(value)
        (directory / file).write_bytes(raw)
        checks.append(f"{file} {hashlib.sha256(raw).hexdigest()}\n")
    contract = {"name": name, "version": version, "schema_digest": "sha256:" + hashlib.sha256("".join(checks).encode()).hexdigest()}
    manifest = {
        "manifest_version": "1", "plugin_id": name if implementation == "rust" else "org.xelab.echo-python", "version": version,
        "publisher_id": publisher, "host_api_major": 1, "platforms": [platform],
        "entrypoint": ["/usr/local/bin/xelab-echo"] if implementation == "rust" else ["/usr/bin/python3", "/opt/plugin/server.py"],
        "capabilities": [{"contract": contract, "input_schema": input_schema, "output_schema": output_schema, "stream_item_schema": None, "side_effect": "none", "maximum_input_bytes": 65536, "maximum_output_bytes": 65536}],
        "dependencies": [], "requested_rules": [], "volumes": [], "routes": [],
        "minimum_resources": {"cpu_millis": 100, "memory_bytes": 67108864, "ephemeral_bytes": 1048576, "pids": 64},
        "health": {"startup_timeout_ms": 60000, "interval_ms": 5000, "timeout_ms": 2000, "failure_threshold": 3},
        "config_schema": {"type": "object", "additionalProperties": False},
        "data_schema_version": "none", "upgrade_strategy": "recreate", "business_entry": "direct",
        "concurrency": {"max_active_requests": 4, "max_queue_requests": 0, "queue_timeout_ms": 100},
    }
    if request_secret:
        manifest["requested_rules"] = [{"rule": {"action": "secret.use", "resource": {"kind": "secret", "match": "all"}}, "required": False}]
    if request_blob:
        manifest["plugin_id"] = "org.xelab.echo-blobs"
        manifest["requested_rules"].extend({"rule": {"action": action, "resource": {"kind": "blob", "match": "all"}}, "required": False} for action in ["blob.read", "blob.create"])
    if volume_access:
        manifest["volumes"] = [{"name": "data", "mount_path": "/data", "access": volume_access, "scope": "deployment", "minimum_bytes": 1048576}]
    (root / "xelab" / "plugin.json").write_bytes(encoded(manifest))
    return manifest


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--publisher-id", required=True)
    parser.add_argument("--output", type=pathlib.Path, default=pathlib.Path("examples/generated"))
    parser.add_argument("--platform", choices=["linux/amd64", "linux/arm64"], default="linux/amd64")
    parser.add_argument("--implementation", choices=["rust", "python"], default="rust")
    parser.add_argument("--request-secret", action="store_true", help="Enable the explicit Secret lease acceptance probe")
    parser.add_argument("--volume-access", choices=["ro", "rw"], help="Enable explicit persistent-volume acceptance")
    parser.add_argument("--request-blob", action="store_true", help="Enable explicit Blob proxy acceptance with a separate contract")
    args = parser.parse_args()
    build(args.output, args.publisher_id, args.platform, args.implementation, args.request_secret, args.volume_access, args.request_blob)

