# Actual daemon resource experiment

This branch is a user-authorized measurement experiment based on main
`44aac50407fb420252023121afde4a2146f6918c`. **Do not merge its lowered reservation
floor into production.** Only the two `NodeConfig` minimum kernel CPU/memory
reservation values are lowered. The configured experiment reserves 200m CPU /
256 MiB for the kernel and 1000m CPU / 2 GiB for the system; reservations are not
measured consumption or hard limits on the kernel.

The real optimized `xelabd` entry point runs as root, holds the actual host and
database leases, reconciles Schema19, sets up the real kernel/plugin cgroups and
HostService network, and serves verified HTTPS. All existing authorization, TLS,
resource accounting, plugin isolation and host identity checks remain enabled.
The measurement sampler's explicit experiment switch exempts only the old fixed
4 CPU/16 GiB reservation floor from the existing RUN-03 preflight. It does not
exempt host permissions, actual capacity, plugin pool, cgroup or daemon identity.

The dedicated workflow builds release binaries and measures three idle repeats,
authenticated management reads with 1 and 8 workers, and idle recovery. Request
generators are bounded to 20 requests/second/worker and 10000 requests/phase;
reported throughput is actual completed throughput. CPU values are cores
(`CPU seconds / wall seconds`); sampled peaks are interval averages, and
`memory.peak`/VmHWM are lifetime high-water values. RSS/PSS/USS are measured from
the actual daemon's `smaps_rollup`; cgroup memory includes charged cache and
helper processes. Measurements exclude database/plugin consumption unless
explicitly recorded separately.

Numeric artifacts never contain tokens, passwords, private keys or API bodies.
This is an engineering resource measurement, not production capacity or RUN-03
acceptance. AWS remains DEFERRED and production acceptance remains in #50.
