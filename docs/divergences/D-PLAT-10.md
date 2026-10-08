# D-PLAT-10

Windows: the CPU count is `available_parallelism` (`GetSystemInfo`), not `GetActiveProcessorCount(ALL_PROCESSOR_GROUPS)`.

* **Class:** PLAT (COMPATIBILITY §5.5)
* **Owner:** WP-15 (`snp_proc::xargs::cpu_count`); the first package of the register row owns this file and
  `tests/differential/divergences/D-PLAT-10.yaml` (WBS "Divergence files", ADR-071).
* **Tests:** `t10_xrg_01_cpu_count_fallbacks` (`crates/snp-proc/src/xargs.rs`),
  `t10_xrg_01_cpu_count_is_the_affinity_count` (`crates/snp-proc/tests/catalog.rs`).

## Register entry (COMPATIBILITY §5.5)

| pre-commit 4.6.2 | snp |
|---|---|
| Windows CPU count = `GetActiveProcessorCount(ALL_PROCESSOR_GROUPS)` | `available_parallelism` (differs only above 64 logical CPUs; no `unsafe`) |

## Rationale

`xargs.cpu_count()` falls back to `multiprocessing.cpu_count()` on Windows (no `sched_getaffinity`), which CPython
implements with `GetActiveProcessorCount(ALL_PROCESSOR_GROUPS)`: every logical CPU of every processor group
(`posixmodule.c:14448-14451`, 14 A.1 #14). The count sets `target_concurrency`, which decides `max_args` and thus the
partition boundaries of every non-serial hook (`xargs.py:76-117`).

Calling `GetActiveProcessorCount` needs FFI, and snp forbids `unsafe` in all of its crates (FD-5, ARCHITECTURE §11.6;
the `snp-sys` crate proposed in 14 A.7.1 was rejected). Rust's `std::thread::available_parallelism` reports the
processors of the current processor group, and Windows puts at most 64 logical CPUs in a group, so the two counts are
identical on every machine with at most 64 logical CPUs — every desktop, laptop and standard CI runner. Above 64 the
count may be lower in snp, giving fewer, larger partitions: the same files are checked, in the same shuffled order,
with the same exit status; only the number of hook invocations (visible to hooks that print per invocation) differs.
`PRE_COMMIT_NO_CONCURRENCY`, `TRAVIS` and `require_serial` behave identically on both sides. On Linux both tools count
the affinity mask (`sched_getaffinity`), on macOS both use `sysconf(_SC_NPROCESSORS_ONLN)`.

## Examples

Windows with 128 logical CPUs in two processor groups, a hook `entry: snp-probe count` over 2 000 files.

pre-commit 4.6.2: `target_concurrency()` = 128, `max_args` = max(4, ⌈2000/128⌉) = 16 → 125 invocations, 125 output
lines `16`.

snp: `cpu_count()` = 64 (the current group), `max_args` = ⌈2000/64⌉ = 32 → 63 invocations (62 × `32`, 1 × `16`).

Both exit 0 and check all 2 000 files.
