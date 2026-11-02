#!/usr/bin/env python3
"""Sign an already imported, platform-specific OCI manifest with an Ed25519 key."""
import argparse
import base64
import hashlib
import json
import pathlib
import re
import subprocess
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--manifest", type=pathlib.Path, required=True)
parser.add_argument("--image", required=True)
parser.add_argument("--key", type=pathlib.Path, required=True)
args = parser.parse_args()
if not re.fullmatch(r"[a-zA-Z0-9/._:@-]+@sha256:[a-f0-9]{64}", args.image):
    parser.error("image must be an immutable OCI reference")
raw = args.manifest.read_bytes()
m = json.loads(raw)
digest = args.image.rsplit("@", 1)[1]
envelope = f"xelab-artifact-v1\n{m['publisher_id']}\n{m['plugin_id']}\n{m['version']}\n{digest}\nsha256:{hashlib.sha256(raw).hexdigest()}\n".encode()
with tempfile.TemporaryDirectory() as directory:
    source = pathlib.Path(directory) / "envelope"
    source.write_bytes(envelope)
    signature = subprocess.check_output(["openssl", "pkeyutl", "-sign", "-rawin", "-inkey", str(args.key), "-in", str(source)])
print(json.dumps({"publisher_id": m["publisher_id"], "image": args.image, "signature": base64.b64encode(signature).decode()}))
