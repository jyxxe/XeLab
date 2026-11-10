#!/usr/bin/env python3
"""Read-only resource sampling of an already running, eligible XeLab daemon.

Python 3.11+ on the actual authorized acceptance host. This tool does not start
the daemon, issue HTTP requests, create load, write cgroups, or signal processes.
Successful collection is NOT production/RUN-03 acceptance.
"""
import argparse
from contextlib import ExitStack
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import re
import select
import stat
import subprocess
import sys
import time
import tomllib

# Importing the repository's acceptance module must not create __pycache__ there.
sys.dont_write_bytecode = True


class Blocked(Exception):
    """Only fixed, non-secret reason codes may be passed to this exception."""


def require(condition, code):
    if not condition:
        raise Blocked(code)


def fingerprint(info):
    return (info.st_dev, info.st_ino, info.st_size,
            info.st_mtime_ns, info.st_ctime_ns)


def read_fd_at(directory, name, limit=4 * 1024 * 1024):
    fd = os.open(name, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW,
                 dir_fd=directory)
    with os.fdopen(fd, "rb") as stream:
        data = stream.read(limit + 1)
    require(len(data) <= limit, "observation_too_large")
    return data.decode("utf-8")


def integers(text):
    values = {}
    for line in text.splitlines():
        fields = line.replace(":", "", 1).split()
        require(len(fields) == 2 and fields[0] not in values,
                "invalid_numeric_observation")
        value = int(fields[1])
        require(value >= 0, "invalid_numeric_observation")
        values[fields[0]] = value
    return values


def required_keys(values, keys):
    require(set(keys) <= values.keys(), "required_field_missing")
    return values


def proc_stat(text, pid):
    end = text.rfind(")")
    require(end >= 0 and int(text.split("(", 1)[0]) == pid,
            "invalid_process_stat")
    fields = text[end + 1:].split()
    require(len(fields) >= 22, "invalid_process_stat")
    return {"state": fields[0], "user_ticks": int(fields[11]),
            "system_ticks": int(fields[12]), "threads": int(fields[17]),
            "starttime_ticks": int(fields[19]),
            "virtual_bytes": int(fields[20]), "rss_pages": int(fields[21])}


def kilobytes(text, requested, required):
    result = {}
    for line in text.splitlines():
        key, separator, rest = line.partition(":")
        if not separator or key not in requested:
            continue
        fields = rest.split()
        require(len(fields) == 2 and fields[1] == "kB",
                "invalid_memory_observation")
        value = int(fields[0])
        require(value >= 0, "invalid_memory_observation")
        result[key] = value * 1024
    return required_keys(result, required)


def status_snapshot(text):
    memory = kilobytes(text,
        {"VmRSS", "VmHWM", "VmSize", "RssAnon", "RssFile", "RssShmem"},
        {"VmRSS", "VmHWM", "VmSize"})
    threads = [line.split()[1] for line in text.splitlines()
               if line.startswith("Threads:")]
    require(len(threads) == 1 and int(threads[0]) > 0,
            "invalid_thread_observation")
    return {"memory_bytes": memory, "threads": int(threads[0])}


def io_stat(text):
    # Empty io.stat is a valid kernel report with no reported block devices.
    devices = {}
    for line in text.splitlines():
        fields = line.split()
        require(bool(re.fullmatch(r"[0-9]+:[0-9]+", fields[0]))
                and fields[0] not in devices, "invalid_cgroup_io")
        values = {}
        for item in fields[1:]:
            key, separator, value = item.partition("=")
            require(bool(separator) and key not in values,
                    "invalid_cgroup_io")
            values[key] = int(value)
            require(values[key] >= 0, "invalid_cgroup_io")
        require(bool(values), "invalid_cgroup_io")
        devices[fields[0]] = values
    return devices


def safe_preflight(result):
    # Do not copy configuration, socket paths, DSNs, or arbitrary resource keys.
    return {key: result[key] for key in (
        "status", "errors", "effective_cpu_millis", "effective_memory_bytes",
        "plugin_cpu_millis", "plugin_memory_bytes", "acceptance")}


def measurement_preflight(run03, config, experiment):
    """Keep upstream evidence intact; only the explicit floor experiment differs."""
    result = safe_preflight(run03.preflight(config))
    floor = "kernel reservation below 4 CPU / 16 GiB"
    waived = [error for error in result["errors"]
              if experiment and error == floor]
    remaining = [error for error in result["errors"]
                 if not (experiment and error == floor)]
    measurement_errors = []
    if experiment:
        resources = config["resources"]
        numeric = [resources[key] for key in (
            "kernel_cpu_millis", "kernel_memory_bytes",
            "system_cpu_millis", "system_memory_bytes")]
        numeric += [result["effective_cpu_millis"], result["effective_memory_bytes"]]
        if not all(type(value) is int and value > 0 for value in numeric):
            measurement_errors.append("experiment_requires_positive_integer_resources")
        else:
            actual_cpu = (result["effective_cpu_millis"]
                          - resources["kernel_cpu_millis"] - resources["system_cpu_millis"])
            actual_memory = (result["effective_memory_bytes"]
                             - resources["kernel_memory_bytes"] - resources["system_memory_bytes"])
            if not (actual_cpu > 0 and actual_memory >= 256 * 1024 ** 2
                    and actual_cpu == result["plugin_cpu_millis"]
                    and actual_memory == result["plugin_memory_bytes"]):
                measurement_errors.append("experiment_requires_actual_positive_plugin_pool")
    result["measurement_errors"] = measurement_errors
    result["measurement_gate_status"] = (
        "eligible" if not remaining and not measurement_errors
        and (result["status"] == "eligible" or bool(waived)) else "blocked")
    result["waived_errors"] = waived
    result["reservation_floor_bypassed"] = bool(waived)
    return result


class PinnedDaemon:
    def __init__(self, pid, config_path, run03, stack):
        self.pid = pid
        require(hasattr(os, "pidfd_open"), "pidfd_unavailable")
        self.pidfd = os.pidfd_open(pid, 0)
        stack.callback(os.close, self.pidfd)
        self.poller = select.poll()
        self.poller.register(self.pidfd, select.POLLIN | select.POLLHUP | select.POLLERR)
        self.alive()
        self.procfd = os.open(f"/proc/{pid}", os.O_RDONLY | os.O_DIRECTORY
                              | os.O_NOFOLLOW | os.O_CLOEXEC)
        stack.callback(os.close, self.procfd)
        self.starttime = self.stat()["starttime_ticks"]
        self.exefd = os.open("exe", os.O_RDONLY | os.O_CLOEXEC, dir_fd=self.procfd)
        stack.callback(os.close, self.exefd)
        self.executable_identity = fingerprint(os.fstat(self.exefd))
        self.cmdline = read_fd_at(self.procfd, "cmdline", 1024 * 1024)
        self.cgroup_path = run03.CGROUP / "xelab" / "kernel"
        self.cgroupfd = os.open(self.cgroup_path, os.O_RDONLY | os.O_DIRECTORY
                                | os.O_NOFOLLOW | os.O_CLOEXEC)
        stack.callback(os.close, self.cgroupfd)
        info = os.fstat(self.cgroupfd)
        self.cgroup_identity = (info.st_dev, info.st_ino)
        self.verify()
        # This is the unchanged upstream gate, including explicit config path.
        self.evidence = run03.daemon_evidence(pid, config_path)
        self.verify()
        require(self.hash_binary() == self.evidence["binary_sha256"],
                "daemon_executable_hash_changed")
        self.verify()

    def alive(self):
        require(not self.poller.poll(0), "daemon_exited")

    def stat(self):
        return proc_stat(read_fd_at(self.procfd, "stat"), self.pid)

    def hash_binary(self):
        digest = hashlib.sha256()
        offset = 0
        while True:
            chunk = os.pread(self.exefd, 1024 * 1024, offset)
            if not chunk:
                return digest.hexdigest()
            digest.update(chunk)
            offset += len(chunk)

    def verify(self):
        self.alive()
        observed = self.stat()
        require(observed["starttime_ticks"] == self.starttime
                and observed["state"] not in {"Z", "X", "x"},
                "daemon_identity_changed")
        current = os.stat("exe", dir_fd=self.procfd)
        require(fingerprint(current) == self.executable_identity
                == fingerprint(os.fstat(self.exefd)),
                "daemon_executable_changed")
        require(read_fd_at(self.procfd, "cmdline", 1024 * 1024) == self.cmdline,
                "daemon_arguments_changed")
        require(read_fd_at(self.procfd, "cgroup").strip()
                == "0::/xelab/kernel", "daemon_cgroup_changed")
        info = os.stat(self.cgroup_path, follow_symlinks=False)
        require(stat.S_ISDIR(info.st_mode)
                and (info.st_dev, info.st_ino) == self.cgroup_identity,
                "kernel_cgroup_replaced")
        self.alive()

    def sample(self):
        self.verify()
        process = self.stat()
        process["counter_monotonic_ns"] = time.monotonic_ns()
        process["status"] = status_snapshot(read_fd_at(self.procfd, "status"))
        rollup = kilobytes(read_fd_at(self.procfd, "smaps_rollup"),
            {"Rss", "Pss", "Private_Clean", "Private_Dirty", "Private_Hugetlb"},
            {"Rss", "Pss", "Private_Clean", "Private_Dirty", "Private_Hugetlb"})
        rollup["Uss"] = (rollup["Private_Clean"] + rollup["Private_Dirty"]
                         + rollup["Private_Hugetlb"])
        process["smaps_bytes"] = rollup
        process["io"] = required_keys(integers(read_fd_at(self.procfd, "io")),
                                       {"read_bytes", "write_bytes"})
        fd = os.open("fd", os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
                     | os.O_CLOEXEC, dir_fd=self.procfd)
        try:
            process["fd_count"] = len(os.listdir(fd))
        finally:
            os.close(fd)
        cgroup = {"cpu_stat": required_keys(
            integers(read_fd_at(self.cgroupfd, "cpu.stat")), {"usage_usec"})}
        cgroup["counter_monotonic_ns"] = time.monotonic_ns()
        for name in ("memory.current", "memory.peak", "memory.min"):
            value = int(read_fd_at(self.cgroupfd, name).strip())
            require(value >= 0, "invalid_cgroup_memory")
            cgroup[name] = value
        cgroup["memory_counter_monotonic_ns"] = time.monotonic_ns()
        cgroup["memory.stat"] = integers(read_fd_at(self.cgroupfd, "memory.stat"))
        cgroup["io.stat"] = io_stat(read_fd_at(self.cgroupfd, "io.stat"))
        cgroup["memory.events"] = required_keys(
            integers(read_fd_at(self.cgroupfd, "memory.events")),
            {"low", "high", "max", "oom", "oom_kill"})
        require(bool(cgroup["memory.stat"]), "required_field_missing")
        self.verify()
        return {"unix": time.time(), "monotonic_ns": time.monotonic_ns(),
                "process": process, "kernel_cgroup": cgroup}


def span(values):
    return {"first": values[0], "last": values[-1],
            "min": min(values), "max": max(values)}


def cpu_summary(samples, group, counter, scale, timestamp):
    values = [sample[group][counter] for sample in samples]
    times = [sample[group][timestamp] / 1e9 for sample in samples]
    intervals = []
    for index in range(1, len(values)):
        delta = values[index] - values[index - 1]
        elapsed = times[index] - times[index - 1]
        require(delta >= 0 and elapsed > 0, "cpu_counter_regressed")
        intervals.append(delta / scale / elapsed)
    elapsed = times[-1] - times[0]
    require(elapsed > 0 and bool(intervals), "insufficient_cpu_samples")
    return {"average_cores": (values[-1] - values[0]) / scale / elapsed,
            "peak_interval_average_cores": max(intervals),
            "counter_elapsed_seconds": elapsed,
            "cpu_seconds": (values[-1] - values[0]) / scale,
            "peak_is_instantaneous": False}


def summarize(samples, ticks_per_second):
    require(len(samples) >= 2, "insufficient_samples")
    # Keep only derived counters here; never mutate the original snapshots.
    derived = [{"process": {"ticks": s["process"]["user_ticks"]
                                          + s["process"]["system_ticks"],
                              "time": s["process"]["counter_monotonic_ns"]},
                "cgroup": {"usec": s["kernel_cgroup"]["cpu_stat"]["usage_usec"],
                           "time": s["kernel_cgroup"]["counter_monotonic_ns"]}}
               for s in samples]
    memories = [s["kernel_cgroup"]["memory.current"] for s in samples]
    times = [s["kernel_cgroup"]["memory_counter_monotonic_ns"] / 1e9
             for s in samples]
    duration = times[-1] - times[0]
    require(duration > 0, "invalid_memory_sampling_duration")
    integral = sum((memories[i - 1] + memories[i]) / 2
                   * (times[i] - times[i - 1]) for i in range(1, len(times)))
    io_delta = {}
    for key in ("read_bytes", "write_bytes"):
        values = [s["process"]["io"][key] for s in samples]
        require(all(a <= b for a, b in zip(values, values[1:])),
                "process_io_counter_regressed")
        io_delta[key] = values[-1] - values[0]
    return {"process_cpu": cpu_summary(derived, "process", "ticks",
                                       ticks_per_second, "time"),
            "kernel_cgroup_cpu": cpu_summary(derived, "cgroup", "usec",
                                             1e6, "time"),
            "process_smaps_bytes": {
                key: span([s["process"]["smaps_bytes"][key] for s in samples])
                for key in ("Rss", "Pss", "Uss")},
            "process_vm_hwm_lifetime_bytes": span([
                s["process"]["status"]["memory_bytes"]["VmHWM"] for s in samples]),
            "process_fd_count": span([s["process"]["fd_count"] for s in samples]),
            "process_thread_count": span([
                s["process"]["status"]["threads"] for s in samples]),
            "process_io_delta_bytes": io_delta,
            "kernel_cgroup_memory_current_bytes": {
                **span(memories), "time_weighted_average_estimate": integral / duration,
                "sampled_phase_peak": max(memories)},
            "kernel_cgroup_memory_peak_lifetime_bytes": span([
                s["kernel_cgroup"]["memory.peak"] for s in samples])}


def arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--daemon-pid", type=int, required=True)
    parser.add_argument("--seconds", type=int, default=60, help="duration, 1..900")
    parser.add_argument("--interval", type=float, default=1.0,
                        help="sample interval in seconds, 0.2..5")
    parser.add_argument("--output", type=Path, required=True,
                        help="new mode-0600 report; overwrite/symlinks refused")
    parser.add_argument("--phase", required=True,
                        help="non-secret label: 1..64 letters, numbers, . _ -")
    parser.add_argument("--measurement-reservation-experiment", action="store_true",
                        help="measurement only: waive only the upstream 4 CPU/16 GiB reservation floor; all other host/identity/cgroup gates remain enforced")
    args = parser.parse_args()
    if not 1 <= args.seconds <= 900:
        parser.error("seconds must be 1..900")
    if not math.isfinite(args.interval) or not 0.2 <= args.interval <= 5:
        parser.error("interval must be finite and 0.2..5")
    if args.daemon_pid <= 0:
        parser.error("daemon PID must be positive")
    if not re.fullmatch(r"[A-Za-z0-9_.-]{1,64}", args.phase):
        parser.error("phase must be a non-secret 1..64 character label")
    return args


def save(stream, report):
    stream.seek(0)
    stream.truncate()
    json.dump(report, stream, indent=2, allow_nan=False)
    stream.write("\n")
    stream.flush()


def main():
    args = arguments()
    try:
        fd = os.open(args.output, os.O_RDWR | os.O_CREAT | os.O_EXCL
                     | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
        os.fchmod(fd, 0o600)
        output = os.fdopen(fd, "w+", encoding="utf-8")
    except Exception:
        print('{"status":"blocked","reason":"output_unavailable"}', file=sys.stderr)
        return 2
    report = {"kind": "actual XeLab daemon read-only resource measurements",
              "status": "blocked", "acceptance": "pending issue #50; AWS DEFERRED",
              "measurement_only": True,
              "measurement_reservation_experiment": args.measurement_reservation_experiment,
              "reservation_floor_bypassed": False,
              "build_provenance": "repository HEAD and daemon binary SHA256 are separate observations; the operator must independently verify which source built the binary",
              "phase": args.phase, "requested_seconds": args.seconds,
              "requested_interval_seconds": args.interval,
              "started_unix": time.time(), "samples": [],
              "semantics": {
                  "cpu": "cores; delta CPU seconds / actual monotonic seconds; not normalized by visible CPUs",
                  "cpu_peak": "maximum sampled interval-average cores, not instantaneous",
                  "memory": "bytes; /proc and cgroup files are sequential, not an atomic snapshot",
                  "uss": "smaps Private_Clean + Private_Dirty + Private_Hugetlb",
                  "memory_average": "trapezoidal time-weighted estimate from sampled memory.current",
                  "phase_peak": "maximum sampled memory.current, not a continuous peak",
                  "memory_peak_and_vm_hwm": "lifetime high-water values, not this phase's peaks",
                  "kernel_cgroup": "all processes in actual /xelab/kernel; not plugin cgroups",
                  "load": "none generated; optional run03 calls must be started separately"}}
    stage = "initialize"
    exit_code = 1
    try:
        save(output, report)
        with ExitStack() as stack:
            stage = "import_run03"
            repo = args.repo.resolve(strict=True)
            module_path = repo / "tools" / "acceptance" / "run03.py"
            require(module_path.resolve(strict=True).is_relative_to(repo),
                    "run03_outside_repository")
            spec = importlib.util.spec_from_file_location("xelab_run03_readonly", module_path)
            require(spec is not None and spec.loader is not None, "run03_import_unavailable")
            run03 = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(run03)
            report["run03_sha256"] = hashlib.sha256(module_path.read_bytes()).hexdigest()
            revision = subprocess.run(["git", "-c", "safe.directory=" + str(repo), "-C", str(repo), "rev-parse", "HEAD"],
                                      capture_output=True, text=True, timeout=5, check=False)
            require(revision.returncode == 0
                    and re.fullmatch(r"[0-9a-f]{40}\n?", revision.stdout) is not None,
                    "repository_revision_unavailable")
            report["repository_head"] = revision.stdout.strip()
            stage = "config"
            # Preserve the supplied last component: refuse symlink configuration.
            config_path = args.config.absolute()
            configfd = os.open(config_path, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW)
            stack.callback(os.close, configfd)
            info = os.fstat(configfd)
            require(stat.S_ISREG(info.st_mode) and info.st_size <= 1024 * 1024,
                    "invalid_config_file")
            config_identity = fingerprint(info)
            raw_config = os.pread(configfd, 1024 * 1024 + 1, 0)
            require(len(raw_config) <= 1024 * 1024, "config_too_large")
            config = tomllib.loads(raw_config.decode("utf-8"))
            del raw_config

            def config_unchanged():
                require(fingerprint(os.fstat(configfd)) == config_identity
                        == fingerprint(os.stat(config_path, follow_symlinks=False)),
                        "config_changed")

            config_unchanged()
            stage = "preflight"
            report["preflight"] = measurement_preflight(
                run03, config, args.measurement_reservation_experiment)
            report["reservation_floor_bypassed"] = report["preflight"]["reservation_floor_bypassed"]
            require(report["preflight"]["measurement_gate_status"] == "eligible",
                    "host_preflight_blocked")
            stage = "daemon_identity"
            pinned = PinnedDaemon(args.daemon_pid, config_path, run03, stack)
            report["daemon"] = {**pinned.evidence,
                                "starttime_ticks": pinned.starttime,
                                "pidfd_pinned": True, "executable_inode_pinned": True,
                                "kernel_cgroup_inode_pinned": True}
            ticks = os.sysconf("SC_CLK_TCK")
            require(ticks > 0, "invalid_clock_tick_rate")
            report["clock_ticks_per_second"] = ticks
            stage = "sampling"
            start = time.monotonic()
            end = start + args.seconds
            next_sample = start
            while True:
                config_unchanged()
                current_host = measurement_preflight(
                    run03, config, args.measurement_reservation_experiment)
                report["last_preflight"] = current_host
                report["reservation_floor_bypassed"] |= current_host["reservation_floor_bypassed"]
                require(current_host["measurement_gate_status"] == "eligible",
                        "host_preflight_blocked")
                sample = pinned.sample()
                require(sample["kernel_cgroup"]["memory.min"]
                        >= config["resources"]["kernel_memory_bytes"],
                        "kernel_memory_min_below_configured_reservation")
                report["samples"].append(sample)
                if len(report["samples"]) == 1:
                    print(json.dumps({"event": "sampling_started", "phase": args.phase,
                                      "monotonic_ns": sample["monotonic_ns"]}), flush=True)
                now = time.monotonic()
                if now >= end:
                    break
                next_sample = min(end, max(next_sample + args.interval, now))
                time.sleep(max(0, next_sample - time.monotonic()))
            stage = "final_identity"
            config_unchanged()
            pinned.verify()
            require(pinned.hash_binary() == pinned.evidence["binary_sha256"],
                    "daemon_executable_hash_changed")
            pinned.verify()
            stage = "summary"
            report["summary"] = summarize(report["samples"], ticks)
            report["sample_count"] = len(report["samples"])
            report["actual_sample_elapsed_seconds"] = (
                report["samples"][-1]["monotonic_ns"]
                - report["samples"][0]["monotonic_ns"]) / 1e9
            report["status"] = "complete"
            exit_code = 0
    except KeyboardInterrupt:
        report["error"] = {"stage": stage, "reason": "interrupted"}
        exit_code = 130
    except Exception as error:
        # No exception messages, tracebacks, config paths/content or child stderr.
        report["error"] = {"stage": stage,
                           "reason": str(error) if isinstance(error, Blocked)
                                     else "required_observation_failed",
                           "type": type(error).__name__}
    finally:
        report["finished_unix"] = time.time()
        report["exit_code"] = exit_code
        try:
            save(output, report)
        except Exception:
            exit_code = 1
            print('{"status":"blocked","reason":"report_write_failed"}', file=sys.stderr)
        output.close()
    print(json.dumps({"status": report["status"] if exit_code == 0 else "blocked",
                      "exit_code": exit_code,
                      "sample_count": len(report["samples"])}))
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
