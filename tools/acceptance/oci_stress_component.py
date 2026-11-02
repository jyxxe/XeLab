#!/usr/bin/env python3
"""CI component test of the actual stopped-exec gate and remote cleanup.

The small hosted runner cannot pass RUN-03's 4C16G host admission. Only the
host/SQL admission observer is substituted here; negative budget/identity tests
exercise the real observer separately. No acceptance report is declared passed.
"""
import json
import os
from pathlib import Path
import sys
from unittest.mock import patch

import run03

assert os.geteuid() == 0 and os.environ.get('XELAB_OCI_TEST') == '1'
instance = str(run03.uuid.UUID(sys.argv[1]))
config = {'runtime': {'socket': '/run/containerd/containerd.sock', 'namespace': 'xelab-ci'}}
path = run03.CGROUP / 'xelab' / 'plugins' / instance
snapshot = run03.cgroup_snapshot(path)
assert 0 < run03.finite_cap(snapshot['memory.max']) <= 256 * 1024 * 1024
assert 0 < run03.finite_cap(snapshot['pids.max']) <= 512
q, p = run03.cpu_cap(snapshot['cpu.max'])
assert q <= p


def observe_ci(*_):
    pid = run03.task_pid(config, instance)
    assert run03.read_text(f'/proc/{pid}/cgroup') == f'0::/xelab/plugins/{instance}'
    return {'identity': run03.process_identity(pid)}


report = {}
with patch.object(run03, 'verify_stress_target', side_effect=observe_ci):
    result = run03.run_stress(config, instance, {}, 'cpu', 1, 1, report)
assert result == 0, report
assert report['cleanup']['confirmed'] and report['cleanup']['instance_stopped'], report
assert not report.get('safety_blocked'), report
# Actual cgroup is empty or was removed by the controller. Do not inspect a
# replacement at the same path as though it were the object just stopped.
print(json.dumps({'component': 'stopped_exec_gate_and_remote_cleanup',
                  'verified': True, 'RUN-03': 'pending'}))
