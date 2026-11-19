#!/usr/bin/env python3
"""Select one executable from the current Cargo build's JSON messages."""

import argparse
import json
import os
from pathlib import Path


def find_executable(messages, kind, source):
    source_path = source.absolute()
    test_profile = kind != "bin"
    candidates = set()
    for line in messages:
        artifact = json.loads(line)
        if artifact.get("reason") != "compiler-artifact":
            continue
        target = artifact["target"]
        executable = artifact["executable"]
        if (target["kind"] == [kind] and artifact["profile"]["test"] is test_profile
                and isinstance(executable, str) and executable):
            candidates.add((Path(target["src_path"]), executable))

    # Distinct Cargo targets can declare different paths to the same source file.
    matches = {executable for path, executable in candidates if path == source_path}
    if not matches:
        resolved_source = source_path.resolve()
        matches = {executable for path, executable in candidates if path.resolve() == resolved_source}
    if len(matches) != 1:
        raise ValueError(f"expected one {kind} executable for {source}, found {len(matches)}")
    binary = Path(matches.pop()).resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise ValueError(f"Cargo artifact is not executable: {binary}")
    return binary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("artifacts", type=Path)
    parser.add_argument("kind", choices=("test", "lib", "bin"))
    parser.add_argument("source", type=Path)
    args = parser.parse_args()
    try:
        with args.artifacts.open(encoding="utf-8") as messages:
            print(find_executable(messages, args.kind, args.source))
    except (OSError, ValueError) as error:
        parser.exit(1, f"{parser.prog}: {error}\n")


if __name__ == "__main__":
    main()
