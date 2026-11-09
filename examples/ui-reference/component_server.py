#!/usr/bin/env python3
"""Disposable test-process adapter, never the signed OCI entrypoint.

The real OCI plugin is initialized over authenticated PluginLifecycle RPC.
This adapter initializes only a caller-owned loopback mTLS component fixture.
"""
import argparse
import json
import os
import pathlib

from http_server import KernelHTTPServer, State


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--identity", type=pathlib.Path, required=True)
    parser.add_argument("--port-file", type=pathlib.Path, required=True)
    args = parser.parse_args()
    state = State()
    state.initialize()
    server = KernelHTTPServer(("127.0.0.1", 0), state, args.identity)
    fd = os.open(args.port_file, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as output:
        json.dump({"port": server.server_address[1]}, output)
    try:
        server.serve_forever()
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
