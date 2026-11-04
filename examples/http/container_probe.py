#!/usr/bin/env python3
"""Exercise the packaged non-root plugin using temporary test-only identities."""
import argparse
import os
import subprocess

from test_http import HTTPTests


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", default="xelab-http-reference:m3")
    args = parser.parse_args()
    HTTPTests.setUpClass()
    try:
        # These are a disposable CA and test leaves, never production identity.
        # The image runs as UID 65532 and receives this test bundle read-only.
        os.chmod(HTTPTests.root, 0o755)
        for file in HTTPTests.root.iterdir():
            if file.suffix in (".key", ".crt"):
                os.chmod(file, 0o444)
        subprocess.run(["docker", "run", "--rm", "--network", "none",
            "--mount", f"type=bind,src={HTTPTests.root},dst=/identity,readonly",
            "--entrypoint", "/usr/bin/python3", args.image,
            "/opt/plugin/lifecycle_probe.py", "--identity", "/identity"], check=True, timeout=30)
    finally:
        HTTPTests.tearDownClass()


if __name__ == "__main__":
    main()
