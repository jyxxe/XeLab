#!/usr/bin/env python3
"""Build the immutable org.xelab.webui manifest; sign its actual OCI digest separately."""
import argparse
import json
import pathlib
import re
import uuid

from server import https_origin


def build(root, publisher, webui_origin, platform="linux/amd64", plugin_version="1.0.0"):
    if platform not in ("linux/amd64", "linux/arm64") or not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", plugin_version):
        raise ValueError("An explicit supported platform and release version are required")
    origin = https_origin(webui_origin)
    source = pathlib.Path(__file__).resolve().parents[2] / "docs/preset-plugins/webui/manifest.json"
    manifest = json.loads(source.read_bytes())
    manifest["publisher_id"] = str(uuid.UUID(publisher))
    manifest["platforms"] = [platform]
    manifest["version"] = plugin_version
    manifest["entrypoint"] = ["/usr/bin/python3", "/opt/webui/server.py"]
    manifest["webui"]["origin"] = origin
    schema = manifest["config_schema"]
    schema["properties"].update({
        "core_origin": {"type": "string", "minLength": 1, "maxLength": 2048,
                        "description": "Exact trusted HTTPS core origin; public configuration, never a credential"},
        "client_id": {"type": "string", "minLength": 1, "maxLength": 128,
                      "pattern": "^[A-Za-z0-9._~-]+$", "description": "Registered public OAuth Client identifier"},
    })
    schema["required"] += ["core_origin", "client_id"]
    # Queueing belongs to the existing Broker. This UI workload does not add a
    # second application queue, and its HTTP worker pool is fixed at32.
    manifest["concurrency"] = {"max_active_requests": 32, "max_queue_requests": 0, "queue_timeout_ms": 100}
    directory = pathlib.Path(root) / "xelab"
    directory.mkdir(parents=True, exist_ok=True)
    raw = (json.dumps(manifest, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n").encode()
    (directory / "plugin.json").write_bytes(raw)
    return manifest


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--publisher-id", required=True)
    parser.add_argument("--webui-origin", required=True, help="Exact registered isolated HTTPS WebUI origin")
    parser.add_argument("--output", type=pathlib.Path, default=pathlib.Path("examples/webui/generated"))
    parser.add_argument("--platform", choices=["linux/amd64", "linux/arm64"], default="linux/amd64")
    parser.add_argument("--plugin-version", default="1.0.0")
    args = parser.parse_args()
    build(args.output, args.publisher_id, args.webui_origin, args.platform, args.plugin_version)
