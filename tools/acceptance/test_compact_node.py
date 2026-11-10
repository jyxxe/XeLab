import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import compact_node as compact


class CompactEvidenceTests(unittest.TestCase):
    def fixture(self):
        resources = {'profile': 'compact', 'kernel_cpu_millis': 500,
                     'kernel_memory_bytes': 192 * compact.MIB, 'system_cpu_millis': 250,
                     'system_memory_bytes': 256 * compact.MIB}
        sample = {'identity': [42, 1234], 'boot_id': 'boot-a', 'host_oom_kill': 0,
                  'online_cpus': 1, 'swap_devices': 0,
                  'meminfo': {'MemTotal': 500 * compact.MIB, 'SwapTotal': 0},
                  'daemon_memory': {'VmRSS': 96 * compact.MIB,
                                    'VmHWM': 128 * compact.MIB, 'VmSwap': 0},
                  'kernel': {'memory.min': str(192 * compact.MIB),
                             'memory.max': str(192 * compact.MIB),
                             'memory.swap.max': '0', 'cpu.max': '50000 100000',
                             'memory.events': 'oom 0\noom_kill 0'},
                  'plugins': {'cpu.max': '25000 100000',
                              'memory.max': str(52 * compact.MIB), 'memory.swap.max': '0',
                              'memory.events': 'oom 0\noom_kill 0'}}
        return resources, sample

    def test_nominal_memory_is_never_added_back_to_plugin_pool(self):
        resources, sample = self.fixture()
        with patch('compact_node.Path.exists', return_value=False):
            self.assertEqual(compact.baseline_errors(sample, resources), [])
            sample['plugins']['memory.max'] = str(64 * compact.MIB)
            self.assertTrue(compact.baseline_errors(sample, resources))

    def test_larger_host_container_or_swap_cannot_prove_minimum_vm(self):
        memory = {'MemTotal': 512 * compact.MIB, 'SwapTotal': 0}
        self.assertEqual(compact.shape_errors(memory, 1, 0, False), [])
        self.assertTrue(compact.shape_errors(memory, 2, 0, False))
        self.assertTrue(compact.shape_errors(memory, 1, 0, True))
        self.assertTrue(compact.shape_errors(memory, 1, 1, False))
        memory['MemTotal'] = 1024 * compact.MIB
        self.assertTrue(compact.shape_errors(memory, 1, 0, False))

    def test_pid_reuse_host_oom_plugin_oom_and_limit_drift_fail(self):
        resources, before = self.fixture()
        for change in ('pid', 'host', 'kernel', 'plugin', 'limit', 'swap', 'balloon'):
            after = copy.deepcopy(before)
            if change == 'pid': after['identity'][1] += 1
            if change == 'host': after['host_oom_kill'] += 1
            if change == 'kernel': after['kernel']['memory.events'] = 'oom 1\noom_kill 0'
            if change == 'plugin': after['plugins']['memory.events'] = 'oom 1\noom_kill 1'
            if change == 'limit': after['plugins']['memory.max'] = 'max'
            if change == 'swap': after['swap_devices'] = 1
            if change == 'balloon': after['meminfo']['MemTotal'] += 1
            self.assertTrue(compact.sample_errors(after, before, resources), change)

    def test_counter_reset_does_not_hide_recreated_group(self):
        resources, before = self.fixture()
        before['kernel']['memory.events'] = 'oom 1\noom_kill 1'
        after = copy.deepcopy(before)
        after['kernel']['memory.events'] = 'oom 0\noom_kill 0'
        self.assertTrue(compact.sample_errors(after, before, resources))

    def test_cgroup_undercount_cannot_hide_rss_hwm_or_swapped_pages(self):
        resources, before = self.fixture()
        for name, excessive in (('VmRSS', 192 * compact.MIB + 1),
                                ('VmHWM', 192 * compact.MIB + 1), ('VmSwap', 1)):
            with self.subTest(metric=name):
                after = copy.deepcopy(before)
                after['kernel']['memory.current'] = str(8 * compact.MIB)
                after['daemon_memory'][name] = excessive
                self.assertTrue(compact.sample_errors(after, before, resources))
        exact = copy.deepcopy(before)
        exact['daemon_memory'].update(VmRSS=192 * compact.MIB, VmHWM=192 * compact.MIB)
        self.assertEqual(compact.sample_errors(exact, before, resources), [])

    def test_proc_status_memory_fields_convert_kib_to_bytes(self):
        parsed = compact.meminfo('Name:\txelabd\nVmRSS:\t98304 kB\nVmHWM:\t131072 kB\nVmSwap:\t0 kB\nThreads:\t4\n')
        self.assertEqual(parsed, {'VmRSS': 96 * compact.MIB,
                                  'VmHWM': 128 * compact.MIB, 'VmSwap': 0})

    def test_startup_peak_failure_retains_peak_in_summary(self):
        resources, sample = self.fixture()
        sample['daemon_memory']['VmHWM'] = 193 * compact.MIB
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / 'config.toml'
            config.write_text('[resources]\nprofile="compact"\n' + '\n'.join(
                f'{key}={value}' for key, value in resources.items() if key != 'profile'))
            output = Path(directory) / 'evidence.jsonl'
            argv = ['compact', '--config', str(config), '--daemon-pid', '42',
                    '--whole-node-vm', '--seconds', '1', '--output', str(output)]
            with patch('sys.argv', argv), patch('run03.daemon_evidence', return_value={'pid': 42}), \
                    patch('compact_node.snapshot', return_value=sample), \
                    patch('compact_node.Path.exists', return_value=False):
                self.assertEqual(compact.main(), 1)
            records = [json.loads(line) for line in output.read_text().splitlines()]
            self.assertIn('daemon VmHWM exceeds kernel memory reservation', records[0]['errors'])
            self.assertEqual(records[-1]['peak_daemon_hwm_bytes'], 193 * compact.MIB)
            self.assertEqual(records[-1]['exit_code'], 1)
            self.assertTrue(all(row['acceptance'] == 'pending' for row in records))

    def test_failed_collection_retains_pending_and_hides_config_details(self):
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / 'config.toml'
            config.write_text('[resources]\nprofile="compact"\n')
            output = Path(directory) / 'evidence.jsonl'
            argv = ['compact', '--config', str(config), '--daemon-pid', '42',
                    '--whole-node-vm', '--seconds', '1', '--output', str(output)]
            with patch('sys.argv', argv), patch('run03.daemon_evidence', side_effect=ValueError('secret')):
                self.assertEqual(compact.main(), 1)
            records = [json.loads(line) for line in output.read_text().splitlines()]
            self.assertTrue(all(row['acceptance'] == 'pending' for row in records))
            self.assertEqual(records[-1]['exit_code'], 1)
            self.assertNotIn('secret', output.read_text())
            with patch('sys.argv', argv):
                with self.assertRaises(FileExistsError): compact.main()


if __name__ == '__main__':
    unittest.main()
