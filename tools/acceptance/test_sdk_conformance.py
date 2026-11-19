import csv
import importlib.util
from pathlib import Path
import unittest


SPEC = importlib.util.spec_from_file_location(
    "sdk_conformance", Path(__file__).with_name("sdk-conformance.py"))
SDK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SDK)


class SDKConformanceDockerTests(unittest.TestCase):
    def test_bind_mounts_keep_each_source_in_one_csv_field(self):
        mounts = [
            (Path("/work/echo"), "/test/echo"),
            (Path('/work/SDK outputs, "nightly"/client'), "/test/client"),
        ]
        command = SDK.docker("sdk-test:local", mounts, {}, ("/usr/bin/true",))
        options = [command[index + 1] for index, arg in enumerate(command) if arg == "--mount"]
        self.assertEqual(list(csv.reader(options, strict=True)), [
            ["type=bind", f"source={source}", f"target={target}", "readonly"]
            for source, target in mounts
        ])


if __name__ == "__main__":
    unittest.main()
