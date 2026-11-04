#!/usr/bin/env python3
"""Produce the immutable manifest/OpenAPI inputs for a standalone UI OCI plugin."""
import argparse
import hashlib
import json
import pathlib
import re
import uuid

PLUGIN_ID = "org.xelab.ui-reference"
VERSION = re.compile(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?\Z")


def encoded(value):
    return (json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n").encode()


def build(root, publisher, platform="linux/amd64", plugin_version="1.0.0"):
    root = pathlib.Path(root)
    publisher = str(uuid.UUID(publisher))
    if platform not in ("linux/amd64", "linux/arm64") or not VERSION.fullmatch(plugin_version):
        raise ValueError("An explicit supported platform and SemVer are required")
    raw = pathlib.Path(__file__).with_name("openapi.json").read_bytes()
    operation = json.loads(raw)["paths"][f"/plugins/{PLUGIN_ID}/echo"]["post"]
    digest = "sha256:" + hashlib.sha256(raw).hexdigest()
    binding = {"alias": "echo", "name": PLUGIN_ID + ".echo", "version": plugin_version, "modes": ["call"]}
    manifest = {
        "manifest_version": "1", "plugin_id": PLUGIN_ID, "version": plugin_version,
        "publisher_id": publisher, "host_api_major": 1, "platforms": [platform],
        "entrypoint": ["/usr/bin/python3", "/opt/plugin/server.py"],
        "capabilities": [], "dependencies": [], "requested_rules": [], "volumes": [],
        "minimum_resources": {"cpu_millis": 100, "memory_bytes": 134217728, "ephemeral_bytes": 8388608, "pids": 64},
        "health": {"startup_timeout_ms": 60000, "interval_ms": 5000, "timeout_ms": 2000, "failure_threshold": 3},
        "routes": [
            {"path_prefix": f"/plugins/{PLUGIN_ID}/echo", "methods": ["POST"], "kind": "http", "required_action": "plugin.http.invoke",
             "maximum_body_bytes": 8192, "timeout_ms": 10000, "http_contract_digest": digest, "authentication": "user"},
            {"path_prefix": f"/ui/{PLUGIN_ID}/", "methods": ["GET", "HEAD"], "kind": "static", "required_action": "plugin.http.invoke",
             "maximum_body_bytes": 0, "timeout_ms": 10000, "authentication": "public"},
        ],
        "config_schema": {"type": "object", "additionalProperties": False},
        "data_schema_version": "none", "upgrade_strategy": "recreate", "business_entry": "direct",
        "concurrency": {"max_active_requests": 4, "max_queue_requests": 0, "queue_timeout_ms": 100},
        "skills": [{
            "name": PLUGIN_ID + ".echo", "version": plugin_version, "description": "Return submitted text through this fixed signed HTTP operation",
            "input_schema": operation["requestBody"]["content"]["application/json"]["schema"],
            "output_schema": operation["responses"]["200"]["content"]["application/json"]["schema"], "stream_item_schema": None,
            "target": {"kind": "http", "plugin_id": PLUGIN_ID, "http_operation_id": operation["operationId"], "http_contract_digest": digest},
            "mapping": "identity", "maximum_timeout_ms": 10000, "input_blobs": False, "tags": ["ui", "reference"],
        }],
        "ui": {"protocol_version": "1", "base_path": f"/ui/{PLUGIN_ID}/", "contributions": [
            {"id": "echo_window", "kind": "window", "title": "Reference echo", "description": "Real echo through an approved fixed Skill binding",
             "entry_path": "/window.html", "skill_bindings": [binding], "refresh_interval_seconds": 0,
             "window": {"width": 640, "height": 420, "min_width": 320, "min_height": 200, "max_width": 1280, "max_height": 900, "resizable": True}},
            {"id": "echo_widget", "kind": "widget", "title": "Echo widget", "description": "Call the same binding or open this deployment's echo window",
             "entry_path": "/widget.html", "skill_bindings": [binding], "refresh_interval_seconds": 0,
             "widget": {"columns": 3, "rows": 2, "min_columns": 2, "min_rows": 2, "max_columns": 6, "max_rows": 4}},
        ]},
    }
    directory = root / "xelab" / "http"
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "openapi.json").write_bytes(raw)
    (root / "xelab" / "plugin.json").write_bytes(encoded(manifest))
    return manifest


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--publisher-id", required=True)
    parser.add_argument("--output", type=pathlib.Path, default=pathlib.Path("examples/ui-reference/generated"))
    parser.add_argument("--platform", choices=["linux/amd64", "linux/arm64"], default="linux/amd64")
    parser.add_argument("--plugin-version", default="1.0.0")
    args = parser.parse_args()
    build(args.output, args.publisher_id, args.platform, args.plugin_version)
