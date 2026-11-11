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
import signal
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
    # Open without waiting for a FIFO writer so fstat can reject special files.
    # O_NONBLOCK does not change reads from ordinary local regular files.
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW | os.O_CLOEXEC)
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
 'instance',__INSTANCE_SQL__,
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


INSTANCE_SQL = """(SELECT json_build_object(
 'deployment_id',i.deployment_id,'generation',i.generation,'phase',i.phase,'limits',i.limits,
 'deployment_limits',d.spec->'limits','enabled',d.spec->'enabled','archived',d.archived_at IS NOT NULL,
 'reservation_count',(SELECT count(*) FROM reservations r WHERE r.instance_id=i.id AND r.kind='instance' AND r.status<>'released'),
 'reservation_cpu',(SELECT sum(cpu_millis) FROM reservations r WHERE r.instance_id=i.id AND r.kind='instance' AND r.status<>'released'),
 'reservation_memory',(SELECT sum(memory_bytes) FROM reservations r WHERE r.instance_id=i.id AND r.kind='instance' AND r.status<>'released')
 ) FROM instances i JOIN deployments d ON d.id=i.deployment_id WHERE i.id='__ID__'::uuid)"""


def ledger(config, instance=None):
    env = dict(os.environ)
    env['PGDATABASE'] = private_bytes(config['database']['dsn_file']).decode().strip()
    env['PGCONNECT_TIMEOUT'] = '5'
    target = 'NULL' if instance is None else INSTANCE_SQL.replace('__ID__', str(uuid.UUID(instance)))
    query = LEDGER_SQL.replace('__INSTANCE_SQL__', target)
    result = subprocess.run(['psql', '-X', '-qAt', '-v', 'ON_ERROR_STOP=1', '-c', query],
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
    # Clap accepts both --config PATH and --config=PATH. Require one explicit,
    # nonempty value rather than guessing if command-line evidence is ambiguous.
    configs = []
    for index, argument in enumerate(arguments[1:], 1):
        if argument == b'--config':
            configs.append(arguments[index + 1] if index + 1 < len(arguments) else b'')
        elif argument.startswith(b'--config='):
            configs.append(argument.split(b'=', 1)[1])
    if len(configs) != 1 or not configs[0]:
        raise ValueError('xelabd must use an explicit --config path')
    daemon_config = Path(os.fsdecode(configs[0]))
    if not daemon_config.is_absolute():
        # A collector may run in another directory than the measured daemon.
        daemon_config = Path(f'/proc/{pid}/cwd') / daemon_config
    if daemon_config.resolve() != config.resolve():
        raise ValueError('daemon config differs from collector config')
    return {'pid': pid, 'binary_sha256': hashlib.sha256(executable.read_bytes()).hexdigest()}


def ctr(config, *args, timeout=5):
    command = ['ctr', '--address', config['runtime']['socket'], '--namespace', config['runtime']['namespace'], *args]
    result = subprocess.run(command, capture_output=True, timeout=timeout, check=False)
    if result.returncode:
        raise ValueError('containerd observation failed')
    return result.stdout


def task_pid(config, instance):
    rows = [line.split() for line in ctr(config, 'tasks', 'list').decode().splitlines()]
    matches = [row for row in rows if row and row[0] == instance]
    if len(matches) != 1 or len(matches[0]) != 3 or matches[0][2] != 'RUNNING':
        raise ValueError('target task is not uniquely running')
    pid = int(matches[0][1])
    if pid <= 1:
        raise ValueError('invalid task PID')
    return pid


def process_identity(pid, *, require_live=False):
    fields = read_text(f'/proc/{pid}/stat').rsplit(')', 1)[1].split()
    if require_live and fields[0] in {'Z', 'X', 'x'}:
        raise ValueError('process has exited')
    return pid, int(fields[19])  # starttime prevents PID reuse


def same_process_identity(pid, expected):
    try:
        return process_identity(pid, require_live=True) == expected
    except (OSError, ValueError, IndexError):
        return False


def cpu_cap(value):
    quota, period = value.split()
    if quota == 'max' or int(quota) <= 0 or int(period) <= 0:
        raise ValueError('finite positive CPU limit required')
    return int(quota), int(period)


def finite_cap(value):
    if value == 'max' or int(value) <= 0:
        raise ValueError('finite positive limit required')
    return int(value)


def validate_stress_limits(host, kernel, parent, target, book):
    if evidence_errors(host, kernel, parent, book):
        raise ValueError('host/parent/ledger mismatch')
    record = book.get('instance')
    if not record or record['phase'] != 'ready' or record['archived'] or not record['enabled']:
        raise ValueError('target is not an enabled, admitted ready instance')
    limits = record['limits']
    if (limits != record['deployment_limits'] or record['reservation_count'] != 1
            or record['reservation_cpu'] != limits['cpu_millis']
            or record['reservation_memory'] != limits['memory_bytes']):
        raise ValueError('instance reservation/intent mismatch')
    cq, cp = cpu_cap(target['cpu.max'])
    pq, pp = cpu_cap(parent['cpu.max'])
    memory, pids = finite_cap(target['memory.max']), finite_cap(target['pids.max'])
    pm, pid_parent = finite_cap(parent['memory.max']), finite_cap(parent['pids.max'])
    if (cq * 1000 != limits['cpu_millis'] * cp or memory != limits['memory_bytes']
            or pids != limits['pids'] or target['memory.swap.max'] != '0'):
        raise ValueError('actual target hard limits differ from admitted limits')
    if (pq * 1000 != host['plugin_cpu_millis'] * pp or pm != host['plugin_memory_bytes']
            or cq * pp > pq * cp or memory > pm or pids > pid_parent
            or book['reserved_cpu_millis'] > host['plugin_cpu_millis']
            or book['reserved_memory_bytes'] > host['plugin_memory_bytes']):
        raise ValueError('actual parent or reservation exceeds the isolated pool')
    return record


class PinnedCgroup:
    """Pin the verified kernel object; never kill a replacement at the same path."""
    def __init__(self, path):
        self.path = path
        self.fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
        self.controls = {}
        try:
            for name in ('cgroup.freeze', 'cgroup.kill'):
                self.controls[name] = os.open(name, os.O_WRONLY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=self.fd)
        except BaseException:
            self.close()
            raise

    def same_path(self):
        a, b = os.fstat(self.fd), self.path.stat()
        return (a.st_dev, a.st_ino) == (b.st_dev, b.st_ino)

    def read(self, name):
        fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=self.fd)
        with os.fdopen(fd) as stream:
            return stream.read().strip()

    def write(self, name, value):
        os.lseek(self.controls[name], 0, os.SEEK_SET)
        os.write(self.controls[name], value.encode())

    def removed(self):
        return os.fstat(self.fd).st_nlink == 0

    def empty(self):
        if self.removed():
            return True  # the kernel cannot remove a populated cgroup
        return dict(line.split() for line in self.read('cgroup.events').splitlines())['populated'] == '0'

    def close(self):
        for fd in self.controls.values():
            os.close(fd)
        self.controls.clear()
        os.close(self.fd)


def verify_stress_target(config, instance, host, group):
    if not group.same_path():
        raise ValueError('target cgroup was replaced')
    pid = task_pid(config, instance)
    identity = process_identity(pid)
    if read_text(f'/proc/{pid}/cgroup') != f'0::/xelab/plugins/{instance}':
        raise ValueError('task PID does not belong to target cgroup')
    book = ledger(config, instance)
    root = CGROUP / 'xelab'
    record = validate_stress_limits(host, cgroup_snapshot(root / 'kernel'),
              cgroup_snapshot(root / 'plugins'), cgroup_snapshot(group.path), book)
    container = json.loads(ctr(config, 'containers', 'info', instance))
    spec = json.loads(ctr(config, 'containers', 'info', '--spec', instance))
    labels = {'org.xelab.instance': instance, 'org.xelab.deployment': record['deployment_id'],
              'org.xelab.generation': str(record['generation'])}
    if (container['ID'] != instance or any(container['Labels'].get(k) != v for k, v in labels.items())
            or any(spec['annotations'].get(k) != v for k, v in labels.items())
            or spec['linux']['cgroupsPath'] != f'/xelab/plugins/{instance}'):
        raise ValueError('container identity/generation/cgroup mismatch')
    resources, limits = spec['linux']['resources'], record['limits']
    if (resources['cpu']['quota'] * 1000 != limits['cpu_millis'] * resources['cpu']['period']
            or resources['memory']['limit'] != limits['memory_bytes']
            or resources['memory']['swap'] != limits['memory_bytes']
            or resources['pids']['limit'] != limits['pids']):
        raise ValueError('OCI resource limits differ from admission')
    if (process_identity(pid) != identity or task_pid(config, instance) != pid or not group.same_path()
            or read_text(f'/proc/{pid}/cgroup') != f'0::/xelab/plugins/{instance}'):
        raise ValueError('target changed during observation')
    return {'pid': pid, 'starttime': identity[1], 'generation': record['generation'],
            'deployment_id': record['deployment_id'], 'limits': limits}


def cleanup_stress(config, instance, exec_id, group, exec_completed=False):
    result = {'exec_id': exec_id, 'confirmed': False, 'instance_stopped': False,
              'quarantined': not exec_completed}
    # An RPC timeout may only kill ctr. Explicitly signal the remote exec too.
    try:
        ctr(config, 'tasks', 'kill', '--exec-id', exec_id, '--signal', 'SIGKILL', instance)
        result['remote_signal'] = 'sent'
    except Exception:
        result['remote_signal'] = 'unconfirmed'
    try:
        if not group.removed():
            # A successful ctr wait proves the wrapper has exited: no delayed
            # exec can start. cgroup.kill atomically includes descendants/forks.
            # On uncertainty retain a freeze fence; never automatically thaw it.
            # Freezing even a successful run makes runc report an empty task as
            # PAUSED and prevents the ordinary controller stop/delete path.
            if not exec_completed:
                group.write('cgroup.freeze', '1')
            group.write('cgroup.kill', '1')
        end = time.monotonic() + 5
        while not group.empty():
            if time.monotonic() >= end:
                raise TimeoutError('target cgroup remains populated')
            time.sleep(.05)
        result.update(confirmed=True, instance_stopped=True)
    except Exception:
        result['safety_blocked'] = True
    return result


def stopped_exec(group, code):
    # ctr exec has no --pid-file. Read host PIDs from the pinned cgroup,
    # and match the unique exec nonce embedded in the exact wrapper argv.
    found = set()
    for value in group.read('cgroup.procs').split():
        pid = int(value)
        if pid <= 1:
            raise ValueError('invalid exec PID')
        try:
            identity = process_identity(pid)
            if (Path(f'/proc/{pid}/cmdline').read_bytes().split(b'\0') ==
                    [b'/usr/bin/python3', b'-c', code.encode(), b'']
                    and read_text(f'/proc/{pid}/stat').rsplit(')', 1)[1].split()[0] == 'T'
                    and process_identity(pid) == identity):
                found.add(identity)
        except (FileNotFoundError, ProcessLookupError):
            continue  # a different task exited during enumeration
    if len(found) > 1:
        raise ValueError('ambiguous stopped exec')
    return next(iter(found), None)


def release_stress_gate(config, instance, host, group, expected, command, process):
    # The wrapper SIGSTOPs before any workload. Release only this exact
    # process via pidfd after validating its actual cgroup and fresh budget.
    end = time.monotonic() + 10
    while True:
        identity = stopped_exec(group, command[-1])
        if identity is not None:
            break
        if process.poll() is not None or time.monotonic() >= end:
            raise TimeoutError('remote exec did not reach the validation gate')
        time.sleep(.05)
    pid = identity[0]
    pidfd = os.pidfd_open(pid)
    try:
        if (read_text(f'/proc/{pid}/cgroup') != f'0::/xelab/plugins/{instance}'
                or process_identity(pid) != identity
                or verify_stress_target(config, instance, host, group) != expected):
            raise ValueError('exec or admitted target changed before workload start')
        signal.pidfd_send_signal(pidfd, signal.SIGCONT)
    finally:
        os.close(pidfd)


def run_stress(config, instance, host, mode, seconds, workers, report):
    group = PinnedCgroup(CGROUP / 'xelab' / 'plugins' / instance)
    process = None
    previous_term = signal.getsignal(signal.SIGTERM)
    def interrupted(*_):
        raise InterruptedError('stress interrupted')
    signal.signal(signal.SIGTERM, interrupted)
    exec_id = 'run03-' + uuid.uuid4().hex
    fault = report['fault'] = {'mode': mode, 'exec_id': exec_id, 'seconds': seconds, 'workers': workers}
    try:
        fault['target'] = verify_stress_target(config, instance, host, group)
        command = stress_command(config, instance, mode, seconds, workers, exec_id)
        process = subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        release_stress_gate(config, instance, host, group, fault['target'], command, process)
        fault['exit_code'] = process.wait(timeout=seconds + 5)
        return 0 if fault['exit_code'] == 0 else 2
    except BaseException as error:
        fault['error_type'] = type(error).__name__
        return 1
    finally:
        if process is not None:
            try:
                if process.poll() is None:
                    process.kill()
                    process.wait(timeout=5)
            except Exception:
                report['client_cleanup_failed'] = True
            # Remote cleanup MUST still run if killing/waiting for ctr fails.
            try:
                report['after'] = {instance: cgroup_snapshot(group.path)}
            except Exception:
                report['after_unavailable'] = True
            report['cleanup'] = cleanup_stress(
                config, instance, exec_id, group, exec_completed=fault.get('exit_code') == 0)
            if not report['cleanup']['confirmed']:
                report['safety_blocked'] = True
        group.close()
        signal.signal(signal.SIGTERM, previous_term)


def stress_command(config, instance, mode, seconds, workers, exec_id=None):
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
    exec_id = exec_id or 'run03-' + uuid.uuid4().hex
    # No load executes until the host validates this exact stopped process.
    code = (f"# exec nonce: {exec_id}\nimport os,signal\nsignal.signal(signal.SIGALRM,lambda *_:os._exit(124))\n"
            f"signal.alarm({seconds + 15})\nos.kill(os.getpid(),signal.SIGSTOP)\n" + code)
    return ['ctr', '--address', config['runtime']['socket'], '--namespace', config['runtime']['namespace'],
            'tasks', 'exec', '--exec-id', exec_id,
            instance, '/usr/bin/python3', '-c', code]


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
        # Collection evidence must remain tied to the verified process, not a
        # recycled PID or a new daemon serving the same API endpoint.
        daemon_identity = process_identity(args.daemon_pid, require_live=True) if args.mode == 'collect' else None
        report['daemon'] = daemon_evidence(args.daemon_pid, args.config)
        if daemon_identity is not None:
            if not same_process_identity(args.daemon_pid, daemon_identity):
                raise ValueError('daemon identity changed during verification')
            report['daemon']['starttime_ticks'] = daemon_identity[1]
        root = CGROUP / 'xelab'
        report['before'] = {str(x): cgroup_snapshot(root / 'plugins' / str(x)) for x in args.instance}
        if args.mode == 'stress':
            if len(args.instance) != 1:
                raise ValueError('stress requires exactly one instance UUID')
            exit_code = run_stress(config, str(args.instance[0]), report['preflight'],
                                   args.fault, args.seconds, args.workers, report)
            if report.get('safety_blocked'):
                exit_code = 3
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
                    if not same_process_identity(args.daemon_pid, daemon_identity):
                        samples.append({'unix': time.time(), 'errors': ['daemon identity changed or unavailable']})
                        break
                    sample = {'unix': time.time(), 'api': api.call('getNode'),
                              'kernel': cgroup_snapshot(root / 'kernel'),
                              'plugins': cgroup_snapshot(root / 'plugins'), 'ledger': ledger(config)}
                    sample['errors'] = evidence_errors(report['preflight'], sample['kernel'], sample['plugins'], sample['ledger'])
                    if sample['api']['status'] != 200:
                        sample['errors'].append('management API unavailable')
                    unchanged = same_process_identity(args.daemon_pid, daemon_identity)
                    if not unchanged:
                        sample['errors'].append('daemon identity changed or unavailable')
                    samples.append(sample)
                    if not unchanged:
                        break
                    time.sleep(min(1, max(0, end - time.monotonic())))
                if not samples:
                    report['collection_failed'] = True
                    report['collection_error'] = 'no samples recorded before time budget expired'
            exit_code = int(report.get('collection_failed', False)
                            or any(x['errors'] for x in report.get('samples', [])))
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
