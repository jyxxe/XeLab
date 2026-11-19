#!/usr/bin/env python3
"""Compare a pinned ZIP source archive with a local Git commit without extracting it."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import zipfile
import zlib


def git(repo, *args):
    result = subprocess.run(["git", "--no-replace-objects", "-C", str(repo), *args],
                            env={**os.environ, "GIT_ALLOW_PROTOCOL": "", "GIT_NO_LAZY_FETCH": "1"},
                            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    if result.returncode:
        raise ValueError(f"git {args[0]} failed; check the local repository and commit objects")
    return result.stdout


def update_digest(digest, stream):
    for chunk in iter(lambda: stream.read(1024 * 1024), b""):
        digest.update(chunk)
    return digest.hexdigest()


def archive_mode(entry):
    mode = entry.external_attr >> 16
    if entry.create_system == 0 and not mode:
        return b"100644"
    if entry.create_system == 3:
        if stat.S_ISLNK(mode):
            return b"120000"
        if stat.S_IFMT(mode) in {0, stat.S_IFREG}:
            return b"100755" if mode & stat.S_IXUSR else b"100644"
    raise ValueError(f"unsupported ZIP file mode: {entry.filename!r}")


def verify(repo, archive, commit, expected_sha256):
    if not re.fullmatch(r"(?:[0-9a-f]{40}|[0-9a-f]{64})", commit):
        raise ValueError("expected a full lowercase Git commit ID, not a branch or option")
    if not re.fullmatch(r"[0-9a-f]{64}", expected_sha256):
        raise ValueError("expected a full lowercase archive SHA-256")
    if git(repo, "rev-parse", "--verify", commit + "^{commit}").decode().strip() != commit:
        raise ValueError("input must identify a commit object directly")
    tree = git(repo, "rev-parse", "--verify", commit + "^{tree}").decode().strip()
    expected = {}
    for record in git(repo, "ls-tree", "--full-tree", "-rlz", commit).split(b"\0"):
        if not record:
            continue
        metadata, raw_path = record.split(b"\t", 1)
        mode, kind, oid, size = metadata.split()
        if kind != b"blob" or mode not in {b"100644", b"100755", b"120000"}:
            raise ValueError(f"unsupported Git tree entry: {raw_path!r}")
        expected[raw_path.decode("utf-8")] = (mode, oid.decode("ascii"), int(size))

    count, root = len(expected), None
    with archive.open("rb") as source:
        if update_digest(hashlib.sha256(), source) != expected_sha256:
            raise ValueError("archive SHA-256 mismatch")
        source.seek(0)
        with zipfile.ZipFile(source) as zipped:
            for entry in zipped.infolist():
                name = entry.orig_filename
                parts = name.removesuffix("/").split("/")
                if "\0" in name or "\\" in name or any(part in {"", ".", ".."} for part in parts):
                    raise ValueError(f"invalid ZIP path: {name!r}")
                root = parts[0] if root is None else root
                if parts[0] != root:
                    raise ValueError("expected exactly one archive root")
                if entry.is_dir():
                    continue
                path = "/".join(parts[1:])
                if not path or path not in expected:
                    raise ValueError(f"extra or duplicate ZIP file: {name!r}")
                mode, oid, size = expected.pop(path)
                if archive_mode(entry) != mode or entry.file_size != size:
                    raise ValueError(f"file mode or size mismatch: {path!r}")
                algorithm = "sha1" if len(oid) == 40 else "sha256"
                digest = hashlib.new(algorithm, f"blob {size}\0".encode("ascii"))
                with zipped.open(entry) as content:
                    if update_digest(digest, content) != oid:
                        raise ValueError(f"blob content mismatch: {path!r}")
            if root is None:
                raise ValueError("expected one archive root, found an empty ZIP")
            original = zipped.comment.decode("ascii")
            if not re.fullmatch(r"(?:[0-9a-f]{40}|[0-9a-f]{64})", original):
                raise ValueError("archive comment does not contain a full source commit ID")
    if expected:
        raise ValueError(f"missing {len(expected)} Git file(s) in ZIP; first: {min(expected)!r}")
    return {"archive_sha256": expected_sha256, "archive_comment_commit": original, "checked_commit": commit,
            "same_commit_id": original == commit, "verified_tree": tree, "verified_files": count}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True, help="local clone or bare repository")
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--commit", required=True, help="full commit ID in the local repository")
    parser.add_argument("--sha256", required=True, help="archive digest from an independent record")
    parser.add_argument("--require-original", action="store_true", help="also require the checked commit to match the ZIP comment")
    args = parser.parse_args(argv)
    try:
        result = verify(args.repo, args.archive, args.commit, args.sha256)
        if args.require_original and not result["same_commit_id"]:
            raise ValueError("source tree matches, but the checked commit is not the original commit named by the ZIP")
    except (ValueError, OSError, RuntimeError, zipfile.BadZipFile, zlib.error) as error:
        print(f"snapshot verification failed: {error}", file=sys.stderr)
        return 1
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
