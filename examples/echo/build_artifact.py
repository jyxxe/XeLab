#!/usr/bin/env python3
"""Build the signed manifest/contract inputs; image signing happens after import."""
import argparse
import hashlib
import json
import pathlib
import uuid


def encoded(value):
    return (json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n").encode()


def build(root, publisher, platform="linux/amd64"):
    publisher = str(uuid.UUID(publisher))
    schema = {"type": "object", "properties": {"message": {"type": "string", "maxLength": 1024}}, "required": ["message"], "additionalProperties": False}
    name, version = "org.xelab.echo", "1.0.0"
    directory = root / "xelab" / "contracts" / name / version
    directory.mkdir(parents=True, exist_ok=True)
    checks = []
    for file, value in [("input.schema.json", schema), ("output.schema.json", schema), ("stream.schema.json", None), ("semantics.json", {"side_effect": "none"})]:
        raw = encoded(value)
        (directory / file).write_bytes(raw)
        checks.append(f"{file} {hashlib.sha256(raw).hexdigest()}\n")
    contract = {"name": name, "version": version, "schema_digest": "sha256:" + hashlib.sha256("".join(checks).encode()).hexdigest()}
    manifest = {
        "manifest_version": "1", "plugin_id": name, "version": version,
        "publisher_id": publisher, "host_api_major": 1, "platforms": [platform],
        "entrypoint": ["/usr/local/bin/xelab-echo"],
        "capabilities": [{"contract": contract, "input_schema": schema, "output_schema": schema, "stream_item_schema": None, "side_effect": "none", "maximum_input_bytes": 65536, "maximum_output_bytes": 65536}],
        "dependencies": [], "requested_rules": [], "volumes": [], "routes": [],
        "minimum_resources": {"cpu_millis": 100, "memory_bytes": 67108864, "ephemeral_bytes": 1048576, "pids": 64},
        "health": {"startup_timeout_ms": 60000, "interval_ms": 5000, "timeout_ms": 2000, "failure_threshold": 3},
        "config_schema": {"type": "object", "additionalProperties": False},
        "data_schema_version": "none", "upgrade_strategy": "recreate", "business_entry": "direct",
        "concurrency": {"max_active_requests": 4, "max_queue_requests": 0, "queue_timeout_ms": 100},
    }
    (root / "xelab" / "plugin.json").write_bytes(encoded(manifest))
    return manifest


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--publisher-id", required=True)
    parser.add_argument("--output", type=pathlib.Path, default=pathlib.Path("examples/generated"))
    parser.add_argument("--platform", choices=["linux/amd64", "linux/arm64"], default="linux/amd64")
    args = parser.parse_args()
    build(args.output, args.publisher_id, args.platform)
