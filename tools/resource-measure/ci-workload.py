#!/usr/bin/env python3
"""Bounded, authenticated HTTPS getNode workload for a real XeLab daemon.

Uses the repository's RUN-03 API (CA validation, no redirects/proxies). Each call
creates a fresh HTTPS connection, so observed daemon CPU includes TLS handshakes.
Does not start/stop a daemon or modify accounts/deployments/host state. Run a
separate measure_kernel.py process concurrently for each uniquely named phase.
"""
from __future__ import annotations

import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
import importlib.util
import json
import math
import os
from pathlib import Path
import re
import threading
import time
import tomllib


def arguments():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--repo', required=True, type=Path)
    p.add_argument('--config', required=True, type=Path)
    p.add_argument('--token-file', required=True, type=Path)
    p.add_argument('--ca-file', required=True, type=Path)
    p.add_argument('--output', required=True, type=Path)
    p.add_argument('--phase', required=True)
    p.add_argument('--seconds', type=float, default=20)
    p.add_argument('--workers', type=int, default=1)
    p.add_argument('--requests', type=int, default=10000,
                   help='Global request cap, maximum 10000')
    p.add_argument('--rps-per-worker', type=float, default=20,
                   help='Per worker request launch limit, maximum 100')
    a = p.parse_args()
    if not 0.2 <= a.seconds <= 90:
        p.error('--seconds must be 0.2..90')
    if not 1 <= a.workers <= 32:
        p.error('--workers must be 1..32')
    if not 1 <= a.requests <= 10000:
        p.error('--requests must be 1..10000')
    if not 0.1 <= a.rps_per_worker <= 100:
        p.error('--rps-per-worker must be 0.1..100')
    if re.fullmatch(r'[A-Za-z0-9_-]{1,80}', a.phase) is None:
        p.error('--phase must be 1..80 letters, digits, underscores or hyphens')
    return a


def load_api(repo):
    path = repo.resolve(strict=True) / 'tools/acceptance/run03.py'
    spec = importlib.util.spec_from_file_location('xelab_resource_run03', path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def percentile(values, percent):
    if not values:
        return None
    ordered = sorted(values)
    return round(ordered[max(0, math.ceil(percent * len(ordered)) - 1)], 3)


def safe_code(value):
    if value is None:
        return None
    if isinstance(value, str) and re.fullmatch(r'[A-Za-z0-9_]{1,80}', value):
        return value
    return 'unrecognized_error_code'


def run_workload(api, args, report):
    started_ns = time.monotonic_ns()
    started = started_ns / 1_000_000_000
    report['workload_started_monotonic_ns'] = started_ns
    report['workload_started_unix'] = time.time()
    deadline = started + args.seconds
    lock = threading.Lock()
    claimed = 0
    inflight = 0
    peak_inflight = 0
    rows = []
    gap = 1.0 / args.rps_per_worker

    def worker(index):
        nonlocal claimed, inflight, peak_inflight
        # Spread initial requests across the rate interval, reducing startup bursts.
        next_launch = started + index * gap / args.workers
        local = []
        while True:
            remaining = next_launch - time.monotonic()
            if remaining > 0:
                time.sleep(min(remaining, max(0.0, deadline - time.monotonic())))
            now = time.monotonic()
            if now >= deadline:
                break
            with lock:
                if claimed >= args.requests:
                    break
                sequence = claimed
                claimed += 1
                inflight += 1
                peak_inflight = max(peak_inflight, inflight)
            request_started = time.monotonic()
            row = {'sequence': sequence, 'worker': index,
                   'started_after_seconds': round(request_started - started, 6)}
            try:
                result = api.call('getNode')
                row.update(status=result['status'],
                           error_code=safe_code(result.get('error_code')))
            except Exception as error:
                # Never store URL/HTTP exception text, response bodies or credentials.
                row['status'] = 'transport_error'
                row['error_type'] = type(error).__name__
            row['latency_ms'] = round((time.monotonic() - request_started) * 1000, 3)
            with lock:
                inflight -= 1
            local.append(row)
            # No catch-up bursts if one request exceeded its intended interval.
            next_launch = max(next_launch + gap, time.monotonic())
        return local

    with ThreadPoolExecutor(max_workers=args.workers) as executor:
        for local in executor.map(worker, range(args.workers)):
            rows.extend(local)
    work_finished = time.monotonic()
    # Keep the requested phase duration even when the explicit request cap is hit.
    remaining = deadline - work_finished
    if remaining > 0:
        time.sleep(remaining)
    finished = time.monotonic()
    rows.sort(key=lambda item: item['sequence'])
    statuses = Counter(str(row['status']) for row in rows)
    codes = Counter(row['error_code'] for row in rows if row.get('error_code'))
    errors = Counter(row['error_type'] for row in rows if row.get('error_type'))
    latencies = [row['latency_ms'] for row in rows]
    success = statuses.get('200', 0)
    report.update(
        status='completed' if len(rows) > 0 and success == len(rows) else 'failed',
        requests_started=len(rows), requests_succeeded=success,
        peak_inflight_requests=peak_inflight,
        requests_failed=len(rows) - success, request_cap_reached=len(rows) >= args.requests,
        workload_finished_monotonic_ns=time.monotonic_ns(),
        elapsed_seconds=round(finished - started, 6),
        request_activity_seconds=round(work_finished - started, 6),
        achieved_requests_per_second=round(len(rows) / (finished - started), 4),
        status_counts=dict(statuses), error_code_counts=dict(codes),
        transport_error_counts=dict(errors),
        latency_ms={'p50': percentile(latencies, 0.50),
                    'p95': percentile(latencies, 0.95),
                    'p99': percentile(latencies, 0.99),
                    'max': max(latencies) if latencies else None},
        requests=rows)
    return 0 if report['status'] == 'completed' else 1


def main():
    args = arguments()
    # Reserve output before doing network work. No overwrite or symlink following.
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC | os.O_NOFOLLOW
    fd = os.open(args.output, flags, 0o600)
    report = {'kind': 'XeLab actual daemon management workload',
              'measurement_only': True, 'production_acceptance': 'pending',
              'phase': args.phase, 'operation': 'getNode',
              'transport': 'HTTPS with CA verification; fresh connection per request; no redirects or proxies',
              'started_unix': time.time(), 'requested_seconds': args.seconds,
              'workers': args.workers, 'request_cap': args.requests,
              'launch_limit_per_worker_per_second': args.rps_per_worker,
              'status': 'initializing'}
    code = 1
    with os.fdopen(fd, 'w') as output:
        json.dump(report, output, indent=2)
        output.flush()
        try:
            run03 = load_api(args.repo)
            config = tomllib.loads(args.config.read_text())
            api = run03.API(config, args.token_file, args.ca_file)
            code = run_workload(api, args, report)
        except Exception as error:
            report.update(status='failed', error_type=type(error).__name__)
        finally:
            report['finished_unix'] = time.time()
            report['exit_code'] = code
            output.seek(0)
            output.truncate()
            json.dump(report, output, indent=2)
            output.write('\n')
    print(json.dumps({'output': str(args.output), 'status': report['status'], 'exit_code': code}))
    return code


if __name__ == '__main__':
    raise SystemExit(main())
