"""Collect mode must not attribute samples to a replaced or missing daemon."""
import contextlib
import io
import json
import os
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import run03


class CollectionDaemonIdentityTests(unittest.TestCase):
    def collect(self, identities):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            config, output = root / 'config.toml', root / 'report.json'
            config.write_text('')
            argv = ['run03', 'collect', '--isolated-test-host', '--config', str(config),
                    '--output', str(output), '--daemon-pid', '42', '--seconds', '1',
                    '--token-file', str(root / 'token'), '--ca-file', str(root / 'ca')]
            api = SimpleNamespace(call=lambda operation: {'operation': operation, 'status': 200})
            times = iter([100, 100, 100, 102])
            with patch('sys.argv', argv), patch('run03.preflight', return_value={'status': 'eligible'}), \
                    patch('run03.daemon_evidence', return_value={'pid': 42, 'binary_sha256': 'fixture'}), \
                    patch('run03.process_identity', side_effect=identities), \
                    patch('run03.API', return_value=api), patch('run03.cgroup_snapshot', return_value={}), \
                    patch('run03.ledger', return_value={}), patch('run03.evidence_errors', return_value=[]), \
                    patch('run03.time.monotonic', side_effect=lambda: next(times, 102)), \
                    patch('run03.time.sleep'), contextlib.redirect_stdout(io.StringIO()):
                code = run03.main()
            return code, json.loads(output.read_text())

    def test_pid_reuse_during_sampling_is_reported_as_incomplete(self):
        code, report = self.collect([(42, 100), (42, 100), (42, 100), (42, 200)])
        self.assertEqual(code, 1)
        self.assertEqual(report['acceptance'], 'pending')
        self.assertIn('daemon identity changed or unavailable', report['samples'][0]['errors'])

    def test_unchanged_daemon_can_complete_a_collection(self):
        code, report = self.collect([(42, 100)] * 4)
        self.assertEqual(code, 0)
        self.assertEqual(report['daemon']['starttime_ticks'], 100)
        self.assertEqual(report['samples'][0]['errors'], [])

    def test_missing_or_replaced_daemon_before_sampling_stops_collection(self):
        for changed in [(42, 200), FileNotFoundError('do not disclose')]:
            with self.subTest(changed=type(changed).__name__):
                code, report = self.collect([(42, 100), (42, 100), changed])
                self.assertEqual(code, 1)
                self.assertEqual(report['samples'][0]['errors'], ['daemon identity changed or unavailable'])
                self.assertNotIn('api', report['samples'][0])
                self.assertNotIn('do not disclose', json.dumps(report))

    def test_daemon_replacement_during_initial_verification_stops_collection(self):
        code, report = self.collect([(42, 100), (42, 200)])
        self.assertEqual(code, 1)
        self.assertTrue(report['collection_failed'])
        self.assertNotIn('samples', report)

    def test_real_proc_identity_rejects_an_exited_process(self):
        process = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'])
        try:
            identity = run03.process_identity(process.pid)
            self.assertTrue(run03.same_process_identity(process.pid, identity))
            process.terminate()
            deadline = time.monotonic() + 5
            exited = None
            while exited is None and time.monotonic() < deadline:
                exited = os.waitid(os.P_PID, process.pid, os.WEXITED | os.WNOWAIT | os.WNOHANG)
                if exited is None:
                    time.sleep(0.01)
            self.assertIsNotNone(exited, 'child did not exit')
            # The PID and start time still exist while the child is a zombie.
            self.assertEqual(run03.process_identity(process.pid), identity)
            self.assertFalse(run03.same_process_identity(process.pid, identity))
            process.wait(timeout=5)
            self.assertFalse(run03.same_process_identity(process.pid, identity))
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)


if __name__ == '__main__':
    unittest.main()
