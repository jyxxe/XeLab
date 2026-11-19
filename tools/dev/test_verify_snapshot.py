"""End-to-end local Git/ZIP migration evidence checks."""
import contextlib
import hashlib
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
import warnings
import zipfile

from verify_snapshot import main, verify


class SnapshotTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.repo = self.root / 'repo'
        self.archive = self.root / 'fixture.zip'
        subprocess.run(['git', 'init', '-q', str(self.repo)], check=True)
        (self.repo / 'regular').write_bytes(b'abc')
        executable = self.repo / 'executable'
        executable.write_bytes(b'run')
        executable.chmod(0o755)
        (self.repo / 'link').symlink_to('regular')
        with_mode = ['git', '-C', str(self.repo), '-c', 'core.filemode=true', '-c', 'core.autocrlf=false']
        subprocess.run([*with_mode, 'add', '--', 'regular', 'executable', 'link'], check=True)
        subprocess.run([*with_mode, '-c', 'core.hooksPath=/dev/null', '-c', 'user.name=Fixture',
                        '-c', 'user.email=fixture@example.invalid', '-c', 'commit.gpgsign=false', 'commit', '-qm', 'fixture'], check=True)
        self.commit = subprocess.check_output(['git', '-C', str(self.repo), 'rev-parse', 'HEAD'], text=True).strip()
        self.entries = [('export/regular', b'abc', 0, 0), ('export/executable', b'run', 3, 0o100755),
                        ('export/link', b'regular', 3, 0o120777)]

    def pack(self, entries=None, comment=None):
        with warnings.catch_warnings(), zipfile.ZipFile(self.archive, 'w') as archive:
            warnings.simplefilter('ignore', UserWarning)
            archive.comment = self.commit.encode() if comment is None else comment
            for name, contents, system, mode in self.entries if entries is None else entries:
                info = zipfile.ZipInfo(name)
                info.create_system = system
                info.external_attr = (mode << 16) if mode else 0x20
                archive.writestr(info, contents)
        return hashlib.sha256(self.archive.read_bytes()).hexdigest()

    def test_regular_executable_and_symlink_without_extracting(self):
        result = verify(self.repo, self.archive, self.commit, self.pack())
        self.assertTrue(result['same_commit_id'])
        self.assertEqual(result['verified_files'], 3)
        self.assertFalse((self.root / 'export').exists())

    def test_identical_tree_is_not_the_original_commit(self):
        digest = self.pack(comment=b'1' * 40)
        args = ['--repo', str(self.repo), '--archive', str(self.archive), '--commit', self.commit, '--sha256', digest]
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            self.assertEqual(main(args), 0)
        self.assertFalse(json.loads(output.getvalue())['same_commit_id'])
        with contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(main([*args, '--require-original']), 1)

    def test_explicit_directories_match_git_and_have_no_payload(self):
        root = ('export/', b'', 3, 0o40755)
        self.assertEqual(verify(self.repo, self.archive, self.commit, self.pack([root, *self.entries]))['verified_files'], 3)
        for entry in [('export/regular/', b'', 3, 0o40755), ('export/extra/', b'', 3, 0o40755),
                      ('export/', b'data', 3, 0o40755), ('export/', b'', 3, 0o120777)]:
            for entries in ([entry, *self.entries], [*self.entries, entry]):
                with self.subTest(entry=entry), self.assertRaisesRegex(ValueError, 'ZIP directory'):
                    verify(self.repo, self.archive, self.commit, self.pack(entries))
        with self.assertRaisesRegex(ValueError, 'ZIP directory'):
            verify(self.repo, self.archive, self.commit, self.pack([root, *self.entries, root]))

    def test_mismatches_and_invalid_archive_paths(self):
        cases = [([], 'empty ZIP'), (self.entries[:-1], 'missing'), (self.entries + self.entries[:1], 'duplicate'),
                 (self.entries + [('outside/extra', b'', 3, 0o100644)], 'one archive root'),
                 (self.entries + [('export/../outside', b'', 3, 0o100644)], 'invalid ZIP path'),
                 ([('export/regular', b'xyz', 0, 0), *self.entries[1:]], 'blob content mismatch'),
                 ([('export/regular', b'abcd', 0, 0), *self.entries[1:]], 'mode or size'),
                 ([self.entries[0], ('export/executable', b'run', 0, 0), self.entries[2]], 'mode or size'),
                 ([*self.entries[:-1], ('export/link', b'regular', 3, 0o010644)], 'unsupported ZIP file mode')]
        for entries, error in cases:
            with self.subTest(error=error), self.assertRaisesRegex(ValueError, error):
                verify(self.repo, self.archive, self.commit, self.pack(entries))

    def test_digest_comment_and_ref_are_checked(self):
        digest = self.pack(comment=b'not-a-commit')
        for commit, checksum, expected in [(self.commit, '0' * 64, 'SHA-256 mismatch'),
                                            (self.commit, digest, 'archive comment'), ('--all', digest, 'full lowercase')]:
            with self.subTest(expected=expected), self.assertRaisesRegex(ValueError, expected):
                verify(self.repo, self.archive, commit, checksum)


if __name__ == '__main__':
    unittest.main()
