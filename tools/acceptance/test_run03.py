import json
import os
from pathlib import Path
import stat
import tempfile
import unittest
import uuid
from unittest.mock import patch

import run03


class Run03Tests(unittest.TestCase):
    def resources(self):
        return {'kernel_cpu_millis': 4000, 'kernel_memory_bytes': 16 * run03.GIB,
                'system_cpu_millis': 2000, 'system_memory_bytes': 4 * run03.GIB,
                'shutdown_grace_ms': 30000}

    def test_real_pool_requires_capacity_beyond_reservations(self):
        errors, cpu, memory = run03.capacity_errors(self.resources(), 8000, 32 * run03.GIB)
        self.assertEqual((errors, cpu, memory), ([], 2000, 12 * run03.GIB))
        self.assertTrue(run03.capacity_errors(self.resources(), 6000, 20 * run03.GIB)[0])
        self.assertTrue(run03.capacity_errors(self.resources(), 2000, 7 * run03.GIB)[0])

    def test_lowered_reservation_cannot_validate_synthetic_host(self):
        resources = self.resources()
        resources['kernel_memory_bytes'] = run03.GIB
        resources['kernel_cpu_millis'] = 1000
        self.assertTrue(run03.capacity_errors(resources, 8000, 32 * run03.GIB)[0])

    def test_private_inputs_reject_permissions_symlinks_and_hardlinks(self):
        with tempfile.TemporaryDirectory() as directory:
            key = Path(directory) / 'key'
            key.write_bytes(b'test-only-secret')
            key.chmod(0o644)
            with self.assertRaises(ValueError): run03.private_bytes(key)
            key.chmod(0o600)
            self.assertEqual(run03.private_bytes(key), b'test-only-secret')
            link = key.with_name('symlink')
            link.symlink_to(key)
            with self.assertRaises(OSError): run03.private_bytes(link)
            os.link(key, key.with_name('hardlink'))
            with self.assertRaises(ValueError): run03.private_bytes(key)

    def test_report_never_overwrites_and_keeps_reserved_inode(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'result.json'
            with run03.new_report(path) as report:
                self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
                with self.assertRaises(FileExistsError): run03.new_report(path)
                path.rename(path.with_name('original'))
                path.write_text('do not truncate')
                json.dump({'acceptance': 'pending'}, report)
            self.assertEqual(path.read_text(), 'do not truncate')
            self.assertEqual(json.loads(path.with_name('original').read_text())['acceptance'], 'pending')

    def test_stress_executes_only_inside_selected_container_and_is_bounded(self):
        config = {'runtime': {'socket': '/run/containerd/containerd.sock', 'namespace': 'run03'}}
        identity = str(uuid.uuid4())
        for mode in ['cpu', 'oom', 'pids']:
            command = run03.stress_command(config, identity, mode, 15, 4)
            self.assertEqual(command[0], 'ctr')
            self.assertIn(identity, command)
            self.assertEqual(command[-3:-1], ['/usr/bin/python3', '-c'])
            compile(command[-1], '<container-workload>', 'exec')
            self.assertIn('15', command[-1])
        for seconds, workers in [(0, 1), (901, 1), (1, 257)]:
            with self.assertRaises(ValueError): run03.stress_command(config, identity, 'cpu', seconds, workers)
        with self.assertRaises(ValueError): run03.stress_command(config, '../../host', 'cpu', 1, 1)

    def test_mismatched_ledger_and_unlimited_parent_are_reported(self):
        host = {'resources': self.resources(), 'effective_cpu_millis': 8000,
                'effective_memory_bytes': 32 * run03.GIB,
                'plugin_cpu_millis': 2000, 'plugin_memory_bytes': 12 * run03.GIB}
        kernel = {'memory.min': str(16 * run03.GIB)}
        plugins = {'cpu.max': '200000 100000', 'memory.max': str(12 * run03.GIB), 'memory.swap.max': '0'}
        book = {'host_cpu_millis': 8000, 'host_memory_bytes': 32 * run03.GIB,
                'resources': self.resources(), 'reserved_cpu_millis': 500,
                'instance_reservation_cpu': 500, 'reserved_memory_bytes': 1,
                'instance_reservation_memory': 1, 'quota_mismatches': 0}
        self.assertEqual(run03.evidence_errors(host, kernel, plugins, book), [])
        plugins['cpu.max'] = 'max 100000'
        book['quota_mismatches'] = 1
        book['host_memory_bytes'] = 64 * run03.GIB
        self.assertEqual(len(run03.evidence_errors(host, kernel, plugins, book)), 3)

    def test_database_failures_do_not_disclose_dsn(self):
        with patch('run03.private_bytes', return_value=b'host=127.0.0.1 password=secret'), patch('run03.subprocess.run') as command:
            command.return_value.returncode = 1
            with self.assertRaises(ValueError) as error:
                run03.ledger({'database': {'dsn_file': '/unused'}})
            self.assertNotIn('secret', str(error.exception))
            self.assertNotIn('secret', str(command.call_args.args))
            self.assertIn('READ ONLY', run03.LEDGER_SQL)

    def test_redirects_cannot_forward_bearer(self):
        self.assertIsNone(run03.NoRedirect().redirect_request(None, None, 302, '', {}, 'https://other.test'))

    def test_failed_collection_remains_pending_and_writes_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / 'config.toml'
            config.write_text('')
            output = Path(directory) / 'report.json'
            with patch('sys.argv', ['run03', 'collect', '--config', str(config), '--output', str(output)]), patch('run03.preflight', side_effect=RuntimeError('secret must not appear')):
                self.assertEqual(run03.main(), 1)
            result = json.loads(output.read_text())
            self.assertEqual(result['acceptance'], 'pending')
            self.assertTrue(result['collection_failed'])
            self.assertNotIn('secret', output.read_text())


if __name__ == '__main__':
    unittest.main()
