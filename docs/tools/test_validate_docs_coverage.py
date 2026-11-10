#!/usr/bin/env python3
"""Exercise the real validator against isolated copies of the repository."""
import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


def strict_dependencies_available():
    try:
        return all(importlib.util.find_spec(name) is not None for name in (
            'openapi_spec_validator', 'jsonschema', 'referencing', 'google.protobuf', 'pglast',
        ))
    except ModuleNotFoundError:
        return False


class MarkdownCoverageIntegration(unittest.TestCase):
    SOURCES = (
        'CONTRIBUTING.md', 'SECURITY.md', 'crates/kernel/README.md',
        'examples/echo-python/README.md', 'examples/http/README.md',
        'examples/m4-consumer/README.md', 'examples/ui-reference/README.md',
        'examples/webui/README.md', 'examples/webui/DEPLOYMENT.md',
    )
    VALID_FRAGMENT = 'controller-稳定化pr-4-遗留项'
    INVALID_FRAGMENT = 'issue-85-nonexistent-anchor'

    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory(prefix='xelab-doc-coverage-')
        cls.root = Path(cls.temp.name) / 'repo'
        source = Path(__file__).resolve().parents[2]
        shutil.copytree(source, cls.root, symlinks=True,
                        ignore=shutil.ignore_patterns('.git', '__pycache__'))
        cls.original = {name: (cls.root / name).read_text(encoding='utf-8') for name in cls.SOURCES}

    @classmethod
    def tearDownClass(cls):
        cls.temp.cleanup()

    def tearDown(self):
        for name, content in self.original.items():
            (self.root / name).write_text(content, encoding='utf-8')
        (self.root / 'docs/implementation/issue85-unreachable-fixture.md').unlink(missing_ok=True)

    def run_validator(self, strict=False):
        args = [sys.executable, 'docs/tools/validate_docs.py']
        if strict:
            args.append('--strict')
        env = os.environ.copy()
        env.pop('GITHUB_TOKEN', None)
        return subprocess.run(args, cwd=self.root, env=env, text=True,
                              capture_output=True, timeout=120)

    def check_mode(self, strict):
        target = self.root / 'docs/implementation/m1-acceptance.md'
        for name, content in self.original.items():
            source = self.root / name
            relative = os.path.relpath(target, source.parent).replace(os.sep, '/')
            # The newly covered files must not acquire the old 220-line limit.
            filler = '\ncoverage fixture\n' * 230 if name == 'CONTRIBUTING.md' else ''
            source.write_text(content + filler + f'\n[valid fixture]({relative}#{self.VALID_FRAGMENT})\n',
                              encoding='utf-8')
        valid = self.run_validator(strict)
        self.assertEqual(valid.returncode, 0, valid.stdout + valid.stderr)

        for name in self.SOURCES:
            source = self.root / name
            relative = os.path.relpath(target, source.parent).replace(os.sep, '/')
            with source.open('a', encoding='utf-8') as output:
                output.write(f'\n[invalid fixture]({relative}#{self.INVALID_FRAGMENT})\n')
        invalid = self.run_validator(strict)
        self.assertNotEqual(invalid.returncode, 0, invalid.stdout + invalid.stderr)
        for name in self.SOURCES:
            relative = os.path.relpath(target, (self.root / name).parent).replace(os.sep, '/')
            self.assertIn(f'broken Markdown anchor {name} -> {relative}#{self.INVALID_FRAGMENT}',
                          invalid.stdout)
        self.assertEqual(invalid.stdout.count(self.INVALID_FRAGMENT), len(self.SOURCES))

    def test_extended_links_in_normal_mode(self):
        self.check_mode(False)

    @unittest.skipUnless(strict_dependencies_available(), 'strict documentation dependencies missing')
    def test_extended_links_in_strict_mode(self):
        self.check_mode(True)

    def test_extra_pages_do_not_make_docs_reachable(self):
        orphan = self.root / 'docs/implementation/issue85-unreachable-fixture.md'
        orphan.write_text('# Orphan fixture\n', encoding='utf-8')
        source = self.root / 'CONTRIBUTING.md'
        source.write_text(self.original['CONTRIBUTING.md'] +
                          '\n[orphan](docs/implementation/issue85-unreachable-fixture.md)\n',
                          encoding='utf-8')
        result = self.run_validator()
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('unreachable Markdown page: implementation/issue85-unreachable-fixture.md',
                      result.stdout)


if __name__ == '__main__':
    unittest.main()
