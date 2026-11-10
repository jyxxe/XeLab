#!/usr/bin/env python3
"""Read-only whole-node compact evidence. Never declares stability accepted.

JSONL streams to a newly created private file. Python 3.11+, Linux cgroups v2.
The caller separately supplies hypervisor, API/load and persistence evidence.
"""
import argparse
import json
import os
from pathlib import Path
import sys
import time
import tomllib

import run03

MIB = 1024 ** 2


def counters(text):
    return {key: int(value) for key, value in (line.split() for line in text.splitlines())}


def meminfo(text):
    return {parts[0].rstrip(':'): int(parts[1]) * 1024
            for line in text.splitlines() if len(parts := line.split()) == 3 and parts[2] == 'kB'}


def shape_errors(memory, online_cpus, swap_count, in_container):
    errors = []
    if online_cpus != 1:
        errors.append('whole-node test requires one online CPU')
    if not 480 * MIB <= memory['MemTotal'] <= 512 * MIB:
        errors.append('observed MemTotal must be 480..512 MiB; supply nominal VM size separately')
    if memory['SwapTotal'] != 0 or swap_count != 0:
        errors.append('swap/zram must be absent')
    if in_container:
        errors.append('a container limit is not a whole-node VM')
    return errors


def sample_errors(sample, baseline, resources):
    errors = []
    if sample['identity'] != baseline['identity'] or sample['boot_id'] != baseline['boot_id']:
        errors.append('daemon identity or boot changed')
    if sample['host_oom_kill'] != baseline['host_oom_kill']:
        errors.append('host OOM counter changed')
    if sample['meminfo']['SwapTotal'] != 0 or sample['swap_devices']:
        errors.append('swap enabled during collection')
    if sample['meminfo']['MemTotal'] != baseline['meminfo']['MemTotal']:
        errors.append('host memory changed during collection')
    if sample['online_cpus'] != 1:
        errors.append('online CPU count changed')
    # Pages allocated before a cgroup move can remain charged to the old group.
    # RSS/HWM must independently fit; a small memory.current is not sufficient.
    for name in ('VmRSS', 'VmHWM'):
        if sample['daemon_memory'][name] > resources['kernel_memory_bytes']:
            errors.append(f'daemon {name} exceeds kernel memory reservation')
    if sample['daemon_memory']['VmSwap'] != 0:
        errors.append('daemon has swapped pages')
    for name in ('kernel', 'plugins'):
        events = counters(sample[name]['memory.events'])
        before = counters(baseline[name]['memory.events'])
        for key in ('oom', 'oom_kill', 'oom_group_kill'):
            if events.get(key, 0) != before.get(key, 0):
                errors.append(f'{name} {key} counter changed')
    if int(sample['kernel']['memory.min']) < resources['kernel_memory_bytes']:
        errors.append('kernel memory protection below reservation')
    if sample['kernel'].get('memory.max') != str(resources['kernel_memory_bytes']):
        errors.append('kernel memory hard cap differs from reservation')
    if sample['kernel'].get('memory.swap.max') != '0':
        errors.append('kernel swap cap must be zero')
    try:
        quota, period = run03.cpu_cap(sample['kernel']['cpu.max'])
        if quota * 1000 != resources['kernel_cpu_millis'] * period:
            errors.append('kernel CPU hard cap differs from reservation')
    except (KeyError, ValueError):
        errors.append('kernel CPU hard cap must be finite')
    for name in ('cpu.max', 'memory.max', 'memory.swap.max'):
        if sample['plugins'][name] != baseline['plugins'][name]:
            errors.append(f'plugin parent {name} changed')
    return errors


def snapshot(pid):
    identity = list(run03.process_identity(pid))
    if run03.read_text(f'/proc/{pid}/cgroup') != '0::/xelab/kernel':
        raise ValueError('daemon cgroup changed')
    root = run03.CGROUP / 'xelab'
    result = {
        'unix': time.time(), 'monotonic': time.monotonic(), 'identity': identity,
        'boot_id': run03.read_text('/proc/sys/kernel/random/boot_id'),
        'online_cpus': os.cpu_count(),
        'meminfo': meminfo(run03.read_text('/proc/meminfo')),
        'daemon_memory': {key: value for key, value in
                          meminfo(run03.read_text(f'/proc/{pid}/status')).items()
                          if key in ('VmRSS', 'VmHWM', 'VmSwap')},
        'swap_devices': max(0, len(run03.read_text('/proc/swaps').splitlines()) - 1),
        'host_oom_kill': counters(run03.read_text('/proc/vmstat'))['oom_kill'],
        'kernel': run03.cgroup_snapshot(root / 'kernel'),
        'plugins': run03.cgroup_snapshot(root / 'plugins'),
        'memory_pressure': run03.read_text('/proc/pressure/memory'),
        'cpu_pressure': run03.read_text('/proc/pressure/cpu'),
    }
    if list(run03.process_identity(pid)) != identity:
        raise ValueError('daemon identity changed during snapshot')
    return result


def baseline_errors(sample, resources):
    errors = shape_errors(sample['meminfo'], sample['online_cpus'], sample['swap_devices'],
                          Path('/.dockerenv').exists() or Path('/run/.containerenv').exists())
    if resources.get('profile') != 'compact':
        errors.append('explicit compact profile required')
    if resources['kernel_cpu_millis'] < 500 or resources['kernel_memory_bytes'] < 192 * MIB:
        errors.append('compact kernel reservation too small')
    if resources['system_cpu_millis'] < 250 or resources['system_memory_bytes'] < 256 * MIB:
        errors.append('compact system reservation too small')
    # Do not fabricate capacity from the nominal 512 MiB VM size.
    pool_memory = sample['meminfo']['MemTotal'] - resources['kernel_memory_bytes'] - resources['system_memory_bytes']
    pool_cpu = 1000 - resources['kernel_cpu_millis'] - resources['system_cpu_millis']
    try:
        quota, period = run03.cpu_cap(sample['plugins']['cpu.max'])
        if pool_cpu <= 0 or quota * 1000 != pool_cpu * period:
            errors.append('plugin CPU cap differs from measured node budget')
        if pool_memory <= 0 or int(sample['plugins']['memory.max']) != pool_memory:
            errors.append('plugin memory cap differs from measured node budget')
        if sample['plugins']['memory.swap.max'] != '0':
            errors.append('plugin swap cap must be zero')
    except (ValueError, TypeError):
        errors.append('plugin caps must be finite')
    return errors + sample_errors(sample, sample, resources)


def write_record(output, kind, **values):
    output.write(json.dumps({'kind': kind, 'acceptance': 'pending', **values}) + '\n')
    output.flush()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', type=Path, required=True)
    parser.add_argument('--daemon-pid', type=int, required=True)
    parser.add_argument('--whole-node-vm', action='store_true', required=True)
    parser.add_argument('--seconds', type=int, required=True)
    parser.add_argument('--interval', type=int, default=10)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if not 1 <= args.seconds <= 86400 or not 1 <= args.interval <= 60:
        parser.error('seconds must be 1..86400; interval must be 1..60')
    # Refuse overwriting existing evidence and stream to one reserved descriptor.
    with run03.new_report(args.output) as output:
        status = 1
        count, maximum_gap, minimum_available, peak_kernel, peak_plugins = 0, 0, None, 0, 0
        peak_rss, peak_hwm, maximum_swap = 0, 0, 0
        started = time.monotonic()
        try:
            config = tomllib.loads(args.config.read_text())
            daemon = run03.daemon_evidence(args.daemon_pid, args.config)
            baseline = snapshot(args.daemon_pid)
            peak_rss = baseline['daemon_memory']['VmRSS']
            peak_hwm = baseline['daemon_memory']['VmHWM']
            maximum_swap = baseline['daemon_memory']['VmSwap']
            errors = baseline_errors(baseline, config['resources'])
            write_record(output, 'header', daemon=daemon, resources=config['resources'],
                         requested_seconds=args.seconds, interval_seconds=args.interval,
                         whole_node_vm_attested=True, daemon_memory=baseline['daemon_memory'],
                         errors=errors)
            if errors:
                return 1
            previous = baseline['monotonic']
            while True:
                current = baseline if count == 0 else snapshot(args.daemon_pid)
                errors = sample_errors(current, baseline, config['resources'])
                gap = current['monotonic'] - previous
                maximum_gap = max(maximum_gap, gap)
                if gap > args.interval + 2:
                    errors.append('sample gap exceeds interval plus two seconds')
                available = current['meminfo']['MemAvailable']
                minimum_available = available if minimum_available is None else min(minimum_available, available)
                peak_kernel = max(peak_kernel, int(current['kernel']['memory.current']))
                peak_plugins = max(peak_plugins, int(current['plugins']['memory.current']))
                peak_rss = max(peak_rss, current['daemon_memory']['VmRSS'])
                peak_hwm = max(peak_hwm, current['daemon_memory']['VmHWM'])
                maximum_swap = max(maximum_swap, current['daemon_memory']['VmSwap'])
                write_record(output, 'sample', **current, errors=errors)
                count += 1
                if errors:
                    break
                elapsed = current['monotonic'] - baseline['monotonic']
                if elapsed >= args.seconds:
                    status = 0
                    break
                previous = current['monotonic']
                time.sleep(min(args.interval, args.seconds - elapsed))
        except Exception as error:
            # Do not persist config, DSN, process arguments or exception details.
            write_record(output, 'failure', error_type=type(error).__name__)
        finally:
            write_record(output, 'summary', exit_code=status, samples=count,
                         elapsed_seconds=round(time.monotonic() - started, 3),
                         maximum_sample_gap_seconds=round(maximum_gap, 3),
                         minimum_mem_available_bytes=minimum_available,
                         peak_kernel_memory_bytes=peak_kernel, peak_plugin_memory_bytes=peak_plugins,
                         peak_daemon_rss_bytes=peak_rss, peak_daemon_hwm_bytes=peak_hwm,
                         maximum_daemon_swap_bytes=maximum_swap)
    print(json.dumps({'output': str(args.output), 'acceptance': 'pending', 'exit_code': status}))
    return status


if __name__ == '__main__':
    sys.exit(main())
