import json
import copy
import subprocess
import os
from pathlib import Path
import stat
import tempfile
import unittest
import uuid
from unittest.mock import patch, Mock

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


    def stress_fixture(self):
        host = {'resources': self.resources(), 'effective_cpu_millis': 8000,
                'effective_memory_bytes': 32 * run03.GIB,
                'plugin_cpu_millis': 2000, 'plugin_memory_bytes': 12 * run03.GIB}
        kernel = {'memory.min': str(16 * run03.GIB)}
        parent = {'cpu.max': '200000 100000', 'memory.max': str(12 * run03.GIB),
                  'memory.swap.max': '0', 'pids.max': '1024'}
        target = {'cpu.max': '50000 100000', 'memory.max': str(run03.GIB),
                  'memory.swap.max': '0', 'pids.max': '64'}
        limits = {'cpu_millis': 500, 'memory_bytes': run03.GIB, 'pids': 64}
        book = {'host_cpu_millis': 8000, 'host_memory_bytes': 32 * run03.GIB,
                'resources': self.resources(), 'reserved_cpu_millis': 500,
                'instance_reservation_cpu': 500, 'reserved_memory_bytes': run03.GIB,
                'instance_reservation_memory': run03.GIB, 'quota_mismatches': 0,
                'instance': {'phase': 'ready', 'enabled': True, 'archived': False,
                   'limits': limits, 'deployment_limits': copy.deepcopy(limits),
                   'reservation_count': 1, 'reservation_cpu': 500,
                   'reservation_memory': run03.GIB, 'generation': 1,
                   'deployment_id': str(uuid.uuid4())}}
        return host, kernel, parent, target, book

    def test_stress_rejects_finite_but_oversized_and_mismatched_budgets(self):
        values = self.stress_fixture()
        self.assertEqual(run03.validate_stress_limits(*values), values[-1]['instance'])
        for index, key, value in [(3, 'memory.max', str(1024 * run03.GIB)),
                (3, 'cpu.max', '300000 100000'), (3, 'pids.max', '999999'),
                (3, 'memory.swap.max', 'max'), (2, 'memory.max', str(1024 * run03.GIB)),
                (2, 'cpu.max', '300000 100000'), (2, 'pids.max', 'max')]:
            with self.subTest(key=key, index=index):
                fixture = copy.deepcopy(values)
                fixture[index][key] = value
                with self.assertRaises(ValueError): run03.validate_stress_limits(*fixture)
        for key, value in [('reservation_count', 0), ('reservation_cpu', 100),
                           ('reservation_memory', 1), ('enabled', False), ('archived', True)]:
            fixture = copy.deepcopy(values)
            fixture[-1]['instance'][key] = value
            with self.assertRaises(ValueError): run03.validate_stress_limits(*fixture)

    def test_stress_rejects_task_in_another_cgroup_before_any_exec(self):
        instance = str(uuid.uuid4())
        with patch('run03.task_pid', return_value=123), patch('run03.process_identity', return_value=(123, 1)), patch('run03.read_text', return_value='0::/host'), patch('run03.subprocess.Popen') as execute:
            with self.assertRaises(ValueError):
                run03.verify_stress_target({}, instance, {}, Mock())
            execute.assert_not_called()

    def test_stress_rejects_container_label_and_generation_mismatch(self):
        instance = str(uuid.uuid4())
        host, kernel, parent, target, book = self.stress_fixture()
        labels = {'org.xelab.instance': instance, 'org.xelab.deployment': book['instance']['deployment_id'],
                  'org.xelab.generation': '2'}
        spec = {'annotations': labels, 'linux': {'cgroupsPath': f'/xelab/plugins/{instance}'}}
        with patch('run03.task_pid', return_value=123), patch('run03.process_identity', return_value=(123, 1)), patch('run03.read_text', return_value=f'0::/xelab/plugins/{instance}'), patch('run03.ledger', return_value=book), patch('run03.cgroup_snapshot', side_effect=[kernel, parent, target]), patch('run03.ctr', side_effect=[json.dumps({'ID': instance, 'Labels': labels}), json.dumps(spec)]):
            with self.assertRaises(ValueError): run03.verify_stress_target({}, instance, host, Mock())

    def test_timeout_explicitly_kills_remote_and_fences_all_descendants(self):
        group = Mock()
        group.removed.return_value = False
        group.empty.side_effect = [False, True]
        with patch('run03.ctr', side_effect=subprocess.TimeoutExpired('ctr', 5)), patch('run03.time.sleep'):
            result = run03.cleanup_stress({}, 'instance', 'run03-saved-id', group)
        self.assertTrue(result['confirmed'])
        self.assertEqual(result['exec_id'], 'run03-saved-id')
        self.assertEqual(group.write.call_args_list[0].args, ('cgroup.freeze', '1'))
        self.assertEqual(group.write.call_args_list[1].args, ('cgroup.kill', '1'))
        self.assertEqual(group.write.call_count, 2)  # never unfreeze an uncertain exec

    def test_failed_remote_cleanup_is_a_safety_blocker(self):
        group = Mock()
        group.removed.return_value = False
        group.empty.return_value = False
        with patch('run03.ctr'), patch('run03.time.monotonic', side_effect=[0, 6]):
            result = run03.cleanup_stress({}, 'instance', 'run03-saved-id', group)
        self.assertFalse(result['confirmed'])
        self.assertTrue(result['safety_blocked'])

    def test_client_timeout_or_kill_failure_still_cleans_remote(self):
        for kill_failure in (False, True):
            group, process = Mock(), Mock()
            process.poll.return_value = None
            process.wait.side_effect = subprocess.TimeoutExpired('ctr', 1)
            if kill_failure:
                process.kill.side_effect = OSError('client already disappeared')
            report = {}
            with patch('run03.PinnedCgroup', return_value=group), patch('run03.verify_stress_target', return_value={}), patch('run03.release_stress_gate'), patch('run03.subprocess.Popen', return_value=process), patch('run03.cgroup_snapshot', return_value={}), patch('run03.cleanup_stress', return_value={'confirmed': False}) as cleanup:
                self.assertEqual(run03.run_stress({'runtime': {'socket': 's', 'namespace': 'n'}}, str(uuid.uuid4()), {}, 'cpu', 1, 1, report), 1)
            cleanup.assert_called_once()
            self.assertTrue(report['safety_blocked'])
            self.assertEqual(cleanup.call_args.args[2], report['fault']['exec_id'])
            group.close.assert_called_once()

    def test_replaced_cgroup_is_rejected_without_starting_workload(self):
        group = Mock()
        group.same_path.return_value = False
        with patch('run03.subprocess.Popen') as execute:
            with self.assertRaises(ValueError): run03.verify_stress_target({}, str(uuid.uuid4()), {}, group)
            execute.assert_not_called()

    def test_workload_stops_before_any_pressure_and_has_an_alarm(self):
        code = run03.stress_command({'runtime': {'socket': 's', 'namespace': 'n'}}, str(uuid.uuid4()), 'cpu', 10, 2)[-1]
        self.assertLess(code.index('signal.SIGSTOP'), code.index('multiprocessing.Process'))
        self.assertIn('signal.alarm(25)', code)

    def test_gate_releases_pinned_pid_only_after_fresh_target_validation(self):
        instance = str(uuid.uuid4())
        group = Mock()
        group.read.return_value = '123'
        for cgroup, target, allowed in [(f'0::/xelab/plugins/{instance}', {'generation': 1}, True),
                ('0::/host', {'generation': 1}, False),
                (f'0::/xelab/plugins/{instance}', {'generation': 2}, False)]:
            def read(path):
                return cgroup if path.endswith('/cgroup') else '123 (python3) T'
            with patch('run03.Path.read_bytes', return_value=b'/usr/bin/python3\0-c\0nonce\0'), patch('run03.read_text', side_effect=read), patch('run03.process_identity', return_value=(123, 1)), patch('run03.os.pidfd_open', return_value=99), patch('run03.os.close') as close, patch('run03.signal.pidfd_send_signal') as send, patch('run03.verify_stress_target', return_value=target) as verify:
                if allowed:
                    run03.release_stress_gate({}, instance, {}, group, {'generation': 1}, ['nonce'], Mock())
                    verify.assert_called_once()
                    send.assert_called_once_with(99, run03.signal.SIGCONT)
                else:
                    with self.assertRaises(ValueError):
                        run03.release_stress_gate({}, instance, {}, group, {'generation': 1}, ['nonce'], Mock())
                    send.assert_not_called()
                close.assert_called_once_with(99)

    def test_stopped_exec_rejects_reused_pid_and_ambiguous_wrapper(self):
        group = Mock()
        group.read.return_value = '123'
        with patch('run03.Path.read_bytes', return_value=b'/usr/bin/python3\0-c\0nonce\0'), patch('run03.read_text', return_value='123 (python3) T'), patch('run03.process_identity', side_effect=[(123, 1), (123, 2)]):
            self.assertIsNone(run03.stopped_exec(group, 'nonce'))
        group.read.return_value = '123 456'
        with patch('run03.Path.read_bytes', return_value=b'/usr/bin/python3\0-c\0nonce\0'), patch('run03.read_text', return_value='123 (python3) T'), patch('run03.process_identity', side_effect=[(123, 1), (123, 1), (456, 1), (456, 1)]):
            with self.assertRaises(ValueError): run03.stopped_exec(group, 'nonce')

    def test_instance_sql_accepts_only_canonical_uuid_and_remains_readonly(self):
        with patch('run03.private_bytes', return_value=b'postgresql://test'), patch('run03.subprocess.run') as command:
            command.return_value.returncode = 0
            command.return_value.stdout = b'{}'
            instance = str(uuid.uuid4())
            run03.ledger({'database': {'dsn_file': '/unused'}}, instance)
            query = command.call_args.args[0][-1]
            self.assertIn(instance, query)
            self.assertIn('READ ONLY', query)
            self.assertNotIn('__INSTANCE_SQL__', query)
            with self.assertRaises(ValueError): run03.ledger({'database': {'dsn_file': '/unused'}}, "';DROP TABLE instances;")

    def test_unconfirmed_cleanup_forces_cli_safety_exit(self):
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / 'config.toml'
            config.write_text('')
            output = Path(directory) / 'report.json'
            def blocked(*args):
                args[-1]['safety_blocked'] = True
                return 0
            with patch('sys.argv', ['run03', 'stress', '--isolated-test-host', '--daemon-pid', '123', '--instance', str(uuid.uuid4()), '--config', str(config), '--output', str(output)]), patch('run03.preflight', return_value={'status': 'eligible'}), patch('run03.daemon_evidence', return_value={}), patch('run03.cgroup_snapshot', return_value={}), patch('run03.run_stress', side_effect=blocked):
                self.assertEqual(run03.main(), 3)
            report = json.loads(output.read_text())
            self.assertTrue(report['safety_blocked'])
            self.assertEqual(report['acceptance'], 'pending')


if __name__ == '__main__':
    unittest.main()
