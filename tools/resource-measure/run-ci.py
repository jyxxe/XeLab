#!/usr/bin/env python3
"""Supervise the actual daemon on a disposable, user-authorized CI host."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import signal
import subprocess
import sys
import time
import tomllib


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


def write(path, value):
    with path.open('x') as stream:
        json.dump(value, stream, indent=2)


def main():
    os.umask(0o077)
    repo, work, rustc = Path(sys.argv[1]).resolve(), Path(sys.argv[2]), sys.argv[3]
    if os.geteuid() != 0 or work.exists():
        raise RuntimeError('fresh root-owned disposable host required')
    tools, out = repo/'tools/resource-measure', repo/'resource-evidence'
    out.mkdir(mode=0o700)
    sampler = module('kernel_sampler', tools/'measure_kernel.py')
    run03 = module('run03_gate', repo/'tools/acceptance/run03.py')
    daemon = pidfd = None
    result = 1
    status = {'measurement_only':True, 'production_acceptance':'pending #50; AWS DEFERRED'}
    try:
        config = tomllib.loads((repo/'docs/contracts/examples/kernel.toml').read_text())
        config['resources'].update(kernel_cpu_millis=200, kernel_memory_bytes=256*1024**2,
                                   system_cpu_millis=1000, system_memory_bytes=2*1024**3)
        preflight = sampler.measurement_preflight(run03,config,True)
        write(out/'preflight.json',preflight)
        if preflight['measurement_gate_status'] != 'eligible':
            raise RuntimeError('actual host preflight blocked')
        subprocess.run([sys.executable,str(tools/'ci-daemon-setup.py'),'prepare',
                        '--repo',str(repo),'--root',str(work)],check=True,timeout=180)
        with (work/'daemon.log').open('x') as log:
            daemon = subprocess.Popen([str(repo/'target/release/xelabd'),'--config',str(work/'kernel.toml')],
                stdout=log,stderr=subprocess.STDOUT,env=dict(os.environ,RUST_LOG='warn'))
            # Child remains unreaped until after pidfd acquisition, preventing PID reuse.
            pidfd = os.pidfd_open(daemon.pid,0)
            subprocess.run([sys.executable,str(tools/'ci-daemon-setup.py'),'login',
                            '--repo',str(repo),'--root',str(work)],check=True,timeout=120)
            binary = repo/'target/release/xelabd'
            write(out/'build-and-host.json',{
                'source_sha':subprocess.check_output(['git','-C',str(repo),'rev-parse','HEAD'],text=True).strip(),
                'baseline_main_sha':'44aac50407fb420252023121afde4a2146f6918c',
                'experiment':'Only two NodeConfig minimum kernel reservations lowered; main unchanged',
                'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),
                'binary_bytes':binary.stat().st_size,'pid':daemon.pid,
                'build_profile':'release, opt-level=3, thin LTO, stripped','rustc':rustc,
                'platform':platform.platform(),'cpu_affinity_count':len(os.sched_getaffinity(0)),
                'host_meminfo':Path('/proc/meminfo').read_text(),'observed_unix':time.time(),
                'setup':json.loads((work/'setup.json').read_text())})

            def phase(name,seconds,workers,rate=20):
                duration=seconds+8 if workers else seconds
                with (out/f'{name}-sampler.log').open('x+') as sample_log:
                    child=subprocess.Popen([sys.executable,str(tools/'measure_kernel.py'),
                        '--repo',str(repo),'--config',str(work/'kernel.toml'),
                        '--daemon-pid',str(daemon.pid),'--seconds',str(duration),'--interval','0.5',
                        '--phase',name,'--measurement-reservation-experiment',
                        '--output',str(out/f'{name}-resources.json')],stdout=sample_log,stderr=subprocess.STDOUT)
                    try:
                        deadline=time.monotonic()+10
                        while True:
                            sample_log.seek(0)
                            if '"event": "sampling_started"' in sample_log.read(): break
                            if child.poll() is not None or time.monotonic()>deadline:
                                raise RuntimeError('sampler did not become ready')
                            time.sleep(0.05)
                        if workers:
                            subprocess.run([sys.executable,str(tools/'ci-workload.py'),
                                '--repo',str(repo),'--config',str(work/'kernel.toml'),
                                '--token-file',str(work/'read.token'),'--ca-file',str(work/'server.crt'),
                                '--output',str(out/f'{name}-requests.json'),'--phase',name,
                                '--seconds',str(seconds),'--workers',str(workers),
                                '--requests','10000','--rps-per-worker',str(rate)],check=True,timeout=seconds+15)
                        if child.wait(timeout=duration+15)!=0: raise RuntimeError('sampler failed')
                    finally:
                        if child.poll() is None:
                            child.terminate()
                            try: child.wait(timeout=5)
                            except subprocess.TimeoutExpired:
                                child.kill()
                                child.wait()
                if daemon.poll() is not None: raise RuntimeError('daemon exited during measurement')
                if workers:
                    resources=json.loads((out/f'{name}-resources.json').read_text())
                    load=json.loads((out/f'{name}-requests.json').read_text())
                    start,end=load['workload_started_monotonic_ns'],load['workload_finished_monotonic_ns']
                    samples=resources['samples']
                    if not (samples[0]['monotonic_ns']<=start<end<=samples[-1]['monotonic_ns']):
                        raise RuntimeError('sampler did not cover entire workload')
                    selected=[s for s in samples if start<=s['monotonic_ns']<=end]
                    write(out/f'{name}-active-window.json',{
                        'phase':name,'status':'complete','sampling_covers_all_workload':True,
                        'workload_started_monotonic_ns':start,'workload_finished_monotonic_ns':end,
                        'first_selected_monotonic_ns':selected[0]['monotonic_ns'],
                        'last_selected_monotonic_ns':selected[-1]['monotonic_ns'],
                        'semantics':'Whole samples inside workload window; edge intervals excluded',
                        'sample_count':len(selected),
                        'summary':sampler.summarize(selected,resources['clock_ticks_per_second'])})

            phase('warmup',10,1)
            for index in range(1,4): phase(f'idle-{index}',30,0)
            phase('management-single',30,1)
            phase('management-concurrent',30,8)
            phase('management-burst',20,8,50)
            phase('recovery-idle',30,0)
            write(out/'disk.json',{
                'state_directory_bytes':int(subprocess.check_output(['du','-sb',str(work/'state')],text=True).split()[0]),
                'scope':'Kernel state/identity only; excludes database, binaries, images, logs and builds'})
            result=0
    except Exception as error:
        status.update(error_type=type(error).__name__,failed=True)
    finally:
        if daemon is not None:
            if daemon.poll() is None:
                signal.pidfd_send_signal(pidfd,signal.SIGTERM)
                try: exit_code=daemon.wait(timeout=35)
                except subprocess.TimeoutExpired:
                    signal.pidfd_send_signal(pidfd,signal.SIGKILL)
                    exit_code=daemon.wait(timeout=10)
                    result=1
                    status['forced_shutdown']=True
                if exit_code!=0: result=1
            else:
                exit_code=daemon.returncode
                result=1
                status['unexpected_daemon_exit']=True
            status['daemon_exit_code']=exit_code
        if pidfd is not None: os.close(pidfd)
        log=work/'daemon.log'
        if log.is_file():
            raw=log.read_bytes()
            reasons={}
            for line in raw.decode(errors='replace').splitlines():
                try: code=json.loads(line).get('fields',{}).get('reason_code')
                except (ValueError,AttributeError):
                    found=re.match(r'^([A-Z][A-Z0-9_]+):',line)
                    code=found.group(1) if found else None
                if isinstance(code,str) and re.fullmatch(r'[A-Z][A-Z0-9_]{0,79}',code):
                    reasons[code]=reasons.get(code,0)+1
            write(out/'daemon-log-summary.json',{'bytes':len(raw),'sha256':hashlib.sha256(raw).hexdigest(),
                'reason_code_counts':reasons,'raw_log_uploaded':False})
        status['driver_exit_code']=result
        write(out/'driver-status.json',status)
        uid,gid=int(os.environ['SUDO_UID']),int(os.environ['SUDO_GID'])
        os.chown(out,uid,gid)
        for path in out.iterdir():
            if path.is_file():
                path.chmod(0o600)
                os.chown(path,uid,gid)
    return result


if __name__=='__main__':
    raise SystemExit(main())
