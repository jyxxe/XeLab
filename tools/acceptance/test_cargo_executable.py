import json
from pathlib import Path
import tempfile
import unittest

from cargo_executable import find_executable


def artifact(kind, source, executable, *, test=True):
    return json.dumps({
        "reason": "compiler-artifact",
        "target": {"kind": [kind], "src_path": str(source)},
        "profile": {"test": test},
        "executable": str(executable) if executable is not None else None,
    })


class CargoExecutableTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name).resolve()
        self.source = self.root / "tests/image_archive.rs"
        self.binary = self.root / "custom target/debug/build/xelab-kernel/hash/out/image_archive-hash"
        self.binary.parent.mkdir(parents=True)
        self.binary.touch()
        self.binary.chmod(0o700)

    def link_source(self, name):
        self.source.parent.mkdir(exist_ok=True)
        self.source.touch()
        link = self.source.with_name(name)
        link.symlink_to(self.source.name)
        return link

    def test_selects_current_test_by_kind_and_source(self):
        expected = artifact("test", self.source, self.binary)
        messages = [
            '{"reason":"build-script-executed"}',
            artifact("test", self.root / "other/tests/image_archive.rs", self.root / "other"),
            artifact("bin", self.source, self.root / "bin"),
            expected,
            expected,
        ]
        self.assertEqual(find_executable(messages, "test", self.source), self.binary)

    def test_prefers_the_declared_source_when_targets_share_a_file(self):
        link = self.link_source("linked_archive.rs")
        linked_binary = self.binary.with_name("linked-archive-hash")
        linked_binary.touch()
        linked_binary.chmod(0o700)
        messages = [artifact("test", self.source, self.binary), artifact("test", link, linked_binary)]

        self.assertEqual(find_executable(messages, "test", self.source), self.binary)
        self.assertEqual(find_executable(messages, "test", link), linked_binary)

    def test_falls_back_to_a_unique_resolved_source(self):
        first = self.link_source("first.rs")
        second = self.link_source("second.rs")
        second_binary = self.binary.with_name("second-hash")
        second_binary.touch()
        second_binary.chmod(0o700)
        messages = [artifact("test", first, self.binary)]

        self.assertEqual(find_executable(messages, "test", self.source), self.binary)
        with self.assertRaisesRegex(ValueError, "found 2"):
            find_executable(messages + [artifact("test", second, second_binary)], "test", self.source)

    def test_library_test_and_regular_binary_use_the_expected_profile(self):
        for kind, source, test in (
            ("lib", self.root / "src/lib.rs", True),
            ("bin", self.root / "src/bin/xelabctl.rs", False),
        ):
            with self.subTest(kind=kind):
                messages = [
                    artifact(kind, source, self.root / "different-profile", test=not test),
                    artifact(kind, source, self.binary, test=test),
                ]
                self.assertEqual(find_executable(messages, kind, source), self.binary)

    def test_rejects_missing_and_ambiguous_outputs(self):
        with self.assertRaisesRegex(ValueError, "found 0"):
            find_executable([artifact("test", self.source, None)], "test", self.source)
        with self.assertRaisesRegex(ValueError, "found 2"):
            find_executable([
                artifact("test", self.source, self.binary),
                artifact("test", self.source, self.root / "another-output"),
            ], "test", self.source)

    def test_rejects_nonexecutable_path(self):
        self.binary.chmod(0o600)
        with self.assertRaisesRegex(ValueError, "not executable"):
            find_executable([artifact("test", self.source, self.binary)], "test", self.source)


if __name__ == "__main__":
    unittest.main()
