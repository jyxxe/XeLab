#!/usr/bin/env python3
"""RUN-03 host preflight, bounded in-container faults and read-only evidence.

This tool NEVER declares RUN-03 passed. Review the complete exercise record.
Python 3.11+, root on a disposable, explicitly authorized Linux VM for stress.
"""
import argparse
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import shutil
import ssl
import stat
import subprocess
import time
import tomllib
import urllib.error
import urllib.request
import uuid

GIB = 1024 ** 3
CGROUP = Path('/sys/fs/cgroup')
REPO = Path(__file__).resolve().parents[2]


def private_bytes(path):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    with os.fdopen(fd, 'rb') as stream:
        info = os.fstat(stream.fileno())
        if (not stat.S_ISREG(info.st_mode) or stat.S_IMODE(info.st_mode) != 0o600
                or info.st_uid != os.geteuid() or info.st_nlink != 1):
            raise ValueError('private input requires current owner, mode 0600 and one link')
        result = stream.read(16385)
    if len(result) > 16384:
        raise ValueError('private input too large')
    return result


def new_report(path):
    # Keep this descriptor open throughout the run; never reopen a replaced path.
    fd = os.open(path, os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    return os.fdopen(fd, 'w+')


def capacity_errors(resources, cpu_millis, memory_bytes):
    errors = []
    if resources['kernel_cpu_millis'] < 4000 or resources['kernel_memory_bytes'] < 16 * GIB:
        errors.append('kernel reservation below 4 CPU / 16 GiB')
    if resources['system_cpu_millis'] <= 0 or resources['system_memory_bytes'] < 1024 ** 2:
        errors.append('system reservation missing')
    pc = cpu_millis - resources['kernel_cpu_millis'] - resources['system_cpu_millis']
    pm = memory_bytes - resources['kernel_memory_bytes'] - resources['system_memory_bytes']
    if pc <= 0 or pm < 256 * 1024 ** 2:
        errors.append('no usable plugin pool beyond kernel and system reservations')
    return errors, pc, pm


def read_text(path):
    return Path(path).read_text().strip()


def preflight(config):
    errors = []
    cpu = len(os.sched_getaffinity(0)) * 1000
    memory = int(next(x.split()[1] for x in Path('/proc/meminfo').read_text().splitlines()
                      if x.startswith('MemTotal:'))) * 1024
    controllers = read_text(CGROUP / 'cgroup.controllers').split()
    if not {'cpu', 'memory', 'pids'} <= set(controllers):
        errors.append('cgroups v2 cpu/memory/pids unavailable')
    # Ancestor caps matter: MemTotal alone can describe the host from a container.
    relative = next(x[3:] for x in Path('/proc/self/cgroup').read_text().splitlines()
                    if x.startswith('0::'))
    current = (CGROUP / relative.lstrip('/')).resolve()
    if not current.is_relative_to(CGROUP):
        raise ValueError('invalid cgroup path')
    for ancestor in [current, *current.parents]:
        if not ancestor.is_relative_to(CGROUP):
            break
        if (ancestor / 'cpu.max').exists():
            quota, period = read_text(ancestor / 'cpu.max').split()
            if quota != 'max':
                cpu = min(cpu, int(quota) * 1000 // int(period))
        if (ancestor / 'memory.max').exists():
            limit = read_text(ancestor / 'memory.max')
            if limit != 'max':
                memory = min(memory, int(limit))
    if Path('/.dockerenv').exists() or Path('/run/.containerenv').exists():
        errors.append('container environment is not an independent acceptance host')
    if os.geteuid() != 0:
        errors.append('host administration requires root')
    for binary in ('ctr', 'runc', 'ip', 'nft', 'psql'):
        if shutil.which(binary) is None:
            errors.append(f'missing host binary: {binary}')
    if not Path(config['runtime']['socket']).is_socket():
        errors.append('containerd socket unavailable')
    extra, pool_cpu, pool_memory = capacity_errors(config['resources'], cpu, memory)
    return {'status': 'blocked' if errors or extra else 'eligible', 'errors': errors + extra,
            'effective_cpu_millis': cpu, 'effective_memory_bytes': memory,
            'plugin_cpu_millis': pool_cpu, 'plugin_memory_bytes': pool_memory,
            'resources': config['resources'], 'acceptance': 'pending'}


def cgroup_snapshot(path):
    files = ('cpu.max', 'cpu.stat', 'memory.max', 'memory.min', 'memory.current',
             'memory.events', 'memory.swap.max', 'pids.max', 'pids.current', 'pids.events',
             'cgroup.events')
    return {name: read_text(path / name) for name in files if (path / name).is_file()}


class API:
    def __init__(self, config, token_file, ca_file):
        self.origin = config['server']['public_origin'].rstrip('/')
        self.token = private_bytes(token_file).decode().strip()
        if not self.origin.startswith('https://'):
            raise ValueError('HTTPS required')
        self.context = ssl.create_default_context(cafile=str(ca_file))
        self.paths = json.loads((REPO / 'docs/contracts/openapi.json').read_text())['paths']

    def call(self, operation, body=None):
        route, method = next((path, method) for path, methods in self.paths.items()
                             for method, spec in methods.items() if spec.get('operationId') == operation)
        headers = {'Authorization': f'Bearer {self.token}', 'Idempotency-Key': str(uuid.uuid4())}
        data = None
        if body is not None:
            headers['Content-Type'] = 'application/json'
            data = json.dumps(body).encode()
        request = urllib.request.Request(self.origin + route, data=data, headers=headers, method=method.upper())
        started = time.monotonic()
        try:
            # Do not forward bearer credentials through redirects or system proxies.
            opener = urllib.request.build_opener(urllib.request.ProxyHandler({}),
                       urllib.request.HTTPSHandler(context=self.context), NoRedirect())
            with opener.open(request, timeout=5) as response:
                status, raw = response.status, response.read(1048577)
        except urllib.error.HTTPError as error:
            status, raw = error.code, error.read(1048577)
        if len(raw) > 1048576:
            raise ValueError('response too large')
        value = json.loads(raw)
        # Evidence contains no bodies, credentials or user data.
        return {'operation': operation, 'status': status,
                'error_code': value.get('error', {}).get('code'),
                'latency_ms': round((time.monotonic() - started) * 1000, 2)}


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


LEDGER_SQL = """
BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;
SET LOCAL statement_timeout='5s';
SELECT json_build_object(
 'schema_versions',(SELECT json_agg(version ORDER BY version) FROM schema_migrations),
 'host_cpu_millis',(SELECT host_cpu_millis FROM node_budget),
 'host_memory_bytes',(SELECT host_memory_bytes FROM node_budget),
 'resources',(SELECT config FROM node_budget),
 'reserved_cpu_millis',(SELECT reserved_cpu_millis FROM node_budget),
 'reserved_memory_bytes',(SELECT reserved_memory_bytes FROM node_budget),
 'instance_reservation_cpu',(SELECT coalesce(sum(cpu_millis),0) FROM reservations WHERE kind='instance' AND status<>'released'),
 'instance_reservation_memory',(SELECT coalesce(sum(memory_bytes),0) FROM reservations WHERE kind='instance' AND status<>'released'),
 'live_instances',(SELECT count(*) FROM instances WHERE phase<>'stopped'),
 'unreleased_calls',(SELECT count(*) FROM reservations WHERE kind='call' AND status<>'released'),
 'quota_mismatches',(SELECT count(*) FROM quotas q WHERE q.concurrent_calls_used<>(SELECT count(*) FROM reservations r JOIN account_closure a ON a.descendant_id=r.account_id WHERE r.kind='call' AND r.status<>'released' AND a.ancestor_id=q.account_id))
);
COMMIT;
"""


def ledger(config):
    env = dict(os.environ)
    env['PGDATABASE'] = private_bytes(config['database']['dsn_file']).decode().strip()
    env['PGCONNECT_TIMEOUT'] = '5'
    result = subprocess.run(['psql', '-X', '-qAt', '-v', 'ON_ERROR_STOP=1', '-c', LEDGER_SQL],
                            env=env, capture_output=True, timeout=10, check=False)
    if result.returncode:
        raise ValueError('read-only ledger snapshot failed (database details withheld)')
    return json.loads(result.stdout)


def evidence_errors(host, kernel, plugins, book):
    errors = []
    if int(kernel.get('memory.min', '0')) < host['resources']['kernel_memory_bytes']:
        errors.append('kernel memory.min below reservation')
    quota, period = plugins.get('cpu.max', 'max 100000').split()
    if quota == 'max' or int(quota) * 1000 > host['plugin_cpu_millis'] * int(period):
        errors.append('plugin CPU parent cap exceeds pool')
    memory = plugins.get('memory.max', 'max')
    if memory == 'max' or int(memory) > host['plugin_memory_bytes']:
        errors.append('plugin memory parent cap exceeds pool')
    if plugins.get('memory.swap.max') != '0':
        errors.append('plugin swap must be disabled')
    if book['reserved_cpu_millis'] != book['instance_reservation_cpu'] or book['reserved_memory_bytes'] != book['instance_reservation_memory'] or book['quota_mismatches']:
        errors.append('resource ledger mismatch')
    if (book['host_cpu_millis'] != host['effective_cpu_millis'] or book['host_memory_bytes'] != host['effective_memory_bytes'] or book['resources'] != host['resources']):
        errors.append('database budget differs from measured host/config')
    return errors


def daemon_evidence(pid, config):
    executable = Path(f'/proc/{pid}/exe').resolve(strict=True)
    if executable.name != 'xelabd':
        raise ValueError('PID is not xelabd')
    if read_text(f'/proc/{pid}/cgroup') != '0::/xelab/kernel':
        raise ValueError('xelabd is not in the real kernel cgroup')
    arguments = Path(f'/proc/{pid}/cmdline').read_bytes().split(b'\0')
    # Require explicit configuration to prevent measuring a different daemon.
    if b'--config' not in arguments:
        raise ValueError('xelabd must use an explicit --config path')
    i = arguments.index(b'--config')
    if Path(os.fsdecode(arguments[i + 1])).resolve() != config.resolve():
        raise ValueError('daemon config differs from collector config')
    return {'pid': pid, 'binary_sha256': hashlib.sha256(executable.read_bytes()).hexdigest()}


def stress_command(config, instance, mode, seconds, workers):
    instance = str(uuid.UUID(instance))
    if not 1 <= seconds <= 900 or not 1 <= workers <= 256:
        raise ValueError('bounded seconds/workers required')
    if mode == 'cpu':
        code = ("import multiprocessing,time\n"
                "def burn(end):\n while time.monotonic()<end: pass\n"
                f"end=time.monotonic()+{seconds}\n"
                f"ps=[multiprocessing.Process(target=burn,args=(end,)) for _ in range({workers})]\n"
                "for p in ps:p.start()\nfor p in ps:p.join()\n")
    elif mode == 'oom':
        code = f"import time\na=[]\nend=time.monotonic()+{seconds}\nwhile time.monotonic()<end:\n a.append(bytearray(8*1024*1024))\n time.sleep(.01)\n"
    elif mode == 'pids':
        code = ("import subprocess,time\nps=[]\ntry:\n"
                f" for _ in range(512):ps.append(subprocess.Popen(['/usr/bin/python3','-c','import time;time.sleep({seconds})']))\n"
                "except OSError:pass\nfinally:\n"
                " for p in ps:p.terminate()\n for p in ps:p.wait()\n")
    else:
        raise ValueError('unknown fault mode')
    return ['ctr', '--address', config['runtime']['socket'], '--namespace', config['runtime']['namespace'],
            'tasks', 'exec', '--exec-id', 'run03-' + uuid.uuid4().hex, instance, '/usr/bin/python3', '-c', code]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=['preflight', 'collect', 'stress', 'calls'])
    parser.add_argument('--config', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--token-file', type=Path)
    parser.add_argument('--ca-file', type=Path)
    parser.add_argument('--daemon-pid', type=int)
    parser.add_argument('--instance', type=uuid.UUID, action='append', default=[])
    parser.add_argument('--seconds', type=int, default=60)
    parser.add_argument('--workers', type=int, default=4)
    parser.add_argument('--fault', choices=['cpu', 'oom', 'pids'], default='cpu')
    parser.add_argument('--request-file', type=Path)
    parser.add_argument('--requests', type=int, default=100)
    parser.add_argument('--isolated-test-host', action='store_true')
    args = parser.parse_args()
    if not 1 <= args.seconds <= 900 or not 1 <= args.workers <= 256 or not 1 <= args.requests <= 10000:
        parser.error('seconds 1..900, workers 1..256, requests 1..10000 required')
    config = tomllib.loads(args.config.read_text())
    report = {'kind': 'RUN-03 evidence', 'acceptance': 'pending', 'mode': args.mode,
              'started_unix': time.time(), 'checks': {}}
    # Reserve the path before any mutation; refuse overwrite/symlink immediately.
    output = new_report(args.output)
    json.dump(report, output)
    output.flush()
    exit_code = 1
    try:
        report['preflight'] = preflight(config)
        if args.mode == 'preflight':
            exit_code = int(report['preflight']['status'] != 'eligible')
            return exit_code
        if not args.isolated_test_host or report['preflight']['status'] != 'eligible':
            raise ValueError('requires eligible, explicitly authorized isolated test host')
        if not args.daemon_pid:
            raise ValueError('--daemon-pid required')
        report['daemon'] = daemon_evidence(args.daemon_pid, args.config)
        root = CGROUP / 'xelab'
        report['before'] = {str(x): cgroup_snapshot(root / 'plugins' / str(x)) for x in args.instance}
        if args.mode == 'stress':
            if len(args.instance) != 1:
                raise ValueError('stress requires exactly one instance UUID')
            cg = root / 'plugins' / str(args.instance[0])
            # Faults only run inside an existing capped plugin, never on host.
            for field in ('memory.max', 'pids.max'):
                cap = read_text(cg / field)
                if cap == 'max' or int(cap) <= 0:
                    raise ValueError('stress target lacks hard memory/PID caps')
            if read_text(cg / 'cpu.max').split()[0] == 'max':
                raise ValueError('stress target lacks CPU cap')
            command = stress_command(config, str(args.instance[0]), args.fault, args.seconds, args.workers)
            result = subprocess.run(command, capture_output=True, timeout=args.seconds + 30, check=False)
            report['fault'] = {'mode': args.fault, 'exit_code': result.returncode,
                               'seconds': args.seconds, 'workers': args.workers}
            report['after'] = {str(args.instance[0]): cgroup_snapshot(cg)}
            # Nonzero may mean expected OOM, or infrastructure failure. A human
            # must inspect counter deltas; never convert it to a pass.
            exit_code = 0 if result.returncode == 0 else 2
        else:
            if not args.token_file or not args.ca_file:
                raise ValueError('token and server CA files required')
            api = API(config, args.token_file, args.ca_file)
            if args.mode == 'calls':
                if not args.request_file:
                    raise ValueError('--request-file required')
                body = json.loads(args.request_file.read_text())
                deadline = time.monotonic() + args.seconds
                def invoke(_):
                    if time.monotonic() >= deadline:
                        return {'status': 'not_attempted', 'reason': 'time_budget'}
                    return api.call('invokeCapability', body)
                with concurrent.futures.ThreadPoolExecutor(max_workers=args.workers) as executor:
                    report['calls'] = list(executor.map(invoke, range(args.requests)))
                report['ledger_after'] = ledger(config)
            else:
                samples = report['samples'] = []
                end = time.monotonic() + args.seconds
                while time.monotonic() < end:
                    sample = {'unix': time.time(), 'api': api.call('getNode'),
                              'kernel': cgroup_snapshot(root / 'kernel'),
                              'plugins': cgroup_snapshot(root / 'plugins'), 'ledger': ledger(config)}
                    sample['errors'] = evidence_errors(report['preflight'], sample['kernel'], sample['plugins'], sample['ledger'])
                    if sample['api']['status'] != 200:
                        sample['errors'].append('management API unavailable')
                    samples.append(sample)
                    time.sleep(min(1, max(0, end - time.monotonic())))
            exit_code = int(any(x['errors'] for x in report.get('samples', [])))
    except Exception as error:
        # Never include exception text from a URL, HTTP response, DSN or child.
        report['error_type'] = type(error).__name__
        report['collection_failed'] = True
    finally:
        report['finished_unix'] = time.time()
        report['exit_code'] = exit_code
        # Same open descriptor as the initial placeholder, including on failure.
        output.seek(0)
        output.truncate()
        json.dump(report, output, indent=2)
        output.write('\n')
        output.close()
        print(json.dumps({'output': str(args.output), 'acceptance': 'pending', 'exit_code': exit_code}))
    return exit_code


if __name__ == '__main__':
    raise SystemExit(main())
