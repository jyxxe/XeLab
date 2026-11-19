"""Validate documented micro artifact provenance without running its source."""
import hashlib
from pathlib import Path

import rfc8785


MAX_SOURCE_BYTES = 256 * 1024


def _fixture_file(folder, name):
    if not isinstance(name, str) or not name or Path(name).is_absolute():
        raise ValueError("artifact file must be a nonempty relative path")
    path = (folder / name).resolve()
    if not path.is_relative_to(folder) or not path.is_file():
        raise ValueError("artifact file must be a file inside its fixture directory")
    return path


def _sha256(data):
    return "sha256:" + hashlib.sha256(data).hexdigest()


def read_micro_artifact(folder, artifact_file, expected_manifest_file, read):
    folder = folder.resolve()
    artifact = read(_fixture_file(folder, artifact_file))
    if not isinstance(artifact, dict) or not {
        "manifest_file", "source_file", "source_digest", "package_digest"
    }.issubset(artifact):
        raise ValueError("artifact fixture is missing provenance fields")

    manifest_path = _fixture_file(folder, artifact["manifest_file"])
    if manifest_path != _fixture_file(folder, expected_manifest_file):
        raise ValueError("artifact manifest file differs from the expected manifest")
    manifest = read(manifest_path)

    with _fixture_file(folder, artifact["source_file"]).open("rb") as source_file:
        source = source_file.read(MAX_SOURCE_BYTES + 1)
    if not 1 <= len(source) <= MAX_SOURCE_BYTES or source.startswith(b"\xef\xbb\xbf") or b"\0" in source:
        raise ValueError("artifact source must be 1..256 KiB UTF-8 without BOM or NUL")
    source.decode("utf-8", errors="strict")
    source_digest = _sha256(source)
    if source_digest != artifact["source_digest"]:
        raise ValueError("artifact source digest mismatch")

    package_digest = _sha256(rfc8785.dumps({"manifest": manifest, "source_digest": source_digest}))
    if package_digest != artifact["package_digest"]:
        raise ValueError("artifact package digest mismatch")
    return manifest, source_digest, package_digest
