import hashlib
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest

from validate_docs import no_duplicate_pairs
from validate_micro_artifact import MAX_SOURCE_BYTES, read_micro_artifact


def digest(data):
    return "sha256:" + hashlib.sha256(data).hexdigest()


class MicroArtifactTests(unittest.TestCase):
    def setUp(self):
        temp = TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.folder = self.root / "fixture"
        (self.folder / "guest").mkdir(parents=True)
        self.source = "// 中文\r\n".encode()
        self.descriptor = {
            "manifest_file": "micro-manifest.json",
            "source_file": "guest/main.js",
            "source_digest": digest(self.source),
        }
        # RFC 8785 compares UTF-16 code units and formats numbers as ECMAScript.
        self.manifest = {"input_schema": {"properties": {
            "\ue000": {"maximum": 1e-7, "minimum": -9007199254740991},
            "\U00010000": {"minimum": -0.0, "maximum": 1e-6},
        }}}
        self.canonical_manifest = (
            '{"input_schema":{"properties":{"\U00010000":{"maximum":0.000001,"minimum":0},'
            '"\ue000":{"maximum":1e-7,"minimum":-9007199254740991}}}}'
        )
        (self.folder / "guest/main.js").write_bytes(self.source)
        self.write_json("micro-manifest.json", self.manifest)
        self.write_artifact()

    def write_json(self, filename, value):
        (self.folder / filename).write_text(json.dumps(value), encoding="utf-8")

    def write_artifact(self, **changes):
        artifact = self.descriptor | changes
        if "package_digest" not in changes:
            data = (
                '{"manifest":' + self.canonical_manifest +
                ',"source_digest":' + json.dumps(artifact["source_digest"]) + '}'
            )
            artifact["package_digest"] = digest(data.encode("utf-8"))
        self.write_json("artifact.json", artifact)
        return artifact

    def validate(self):
        def read(path):
            return json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=no_duplicate_pairs)

        return read_micro_artifact(self.folder, "artifact.json", "micro-manifest.json", read)

    def test_uses_utf16_order_ecmascript_numbers_and_original_source_bytes(self):
        artifact = self.write_artifact()
        self.assertEqual(
            self.validate(),
            (self.manifest, artifact["source_digest"], artifact["package_digest"]),
        )
        python_sorted = json.dumps(
            {"manifest": self.manifest, "source_digest": artifact["source_digest"]},
            sort_keys=True, ensure_ascii=False, separators=(",", ":"),
        )
        self.write_artifact(package_digest=digest(python_sorted.encode()))
        with self.assertRaisesRegex(ValueError, "package digest mismatch"):
            self.validate()

    def test_manifest_reference_must_match_the_catalog_or_echo(self):
        self.write_json("other-manifest.json", self.manifest)
        self.write_artifact(manifest_file="other-manifest.json")
        with self.assertRaisesRegex(ValueError, "manifest file differs"):
            self.validate()

    def test_source_digest_detects_changed_bytes(self):
        (self.folder / "guest/main.js").write_bytes(self.source.replace(b"\r\n", b"\n"))
        with self.assertRaisesRegex(ValueError, "source digest mismatch"):
            self.validate()

    def test_source_requires_utf8_and_contract_size_even_with_matching_digests(self):
        limit = self.source.ljust(MAX_SOURCE_BYTES, b" ")
        (self.folder / "guest/main.js").write_bytes(limit)
        self.write_artifact(source_digest=digest(limit))
        self.assertEqual(self.validate()[1], digest(limit))

        for source in (b"", b"\xef\xbb\xbfvalid", b"nul\0byte", b"\xff", b"x" * (MAX_SOURCE_BYTES + 1)):
            with self.subTest(length=len(source), prefix=source[:8]):
                (self.folder / "guest/main.js").write_bytes(source)
                self.write_artifact(source_digest=digest(source))
                with self.assertRaisesRegex(ValueError, "UTF-8|utf-8"):
                    self.validate()

    def test_source_path_stays_in_the_fixture_directory(self):
        outside = self.root / "outside.js"
        outside.write_bytes(self.source)
        (self.folder / "guest/alias.js").symlink_to(outside)
        for path in (str(outside), "../outside.js", "guest/alias.js"):
            with self.subTest(path=path):
                self.write_artifact(source_file=path)
                with self.assertRaisesRegex(ValueError, "relative path|inside its fixture"):
                    self.validate()
