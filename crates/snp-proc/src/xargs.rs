//! `pre_commit.xargs`: concurrency, command-length budget, partitioning and status aggregation (02 §11.4).
//!
//! W0 working baseline (ADR-071): exact ports of `xargs.py` and `lang_base.target_concurrency`; WP-15 owns and
//! completes the module without changing a signature.
//!
//! Owner: WP-15.

use std::ffi::{OsStr, OsString};
use std::num::NonZeroUsize;
use std::ops::Range;
use std::path::Path;

use snp_core::env::Env;

use crate::spawn::PyStatus;

/// `py_cpu_count()`: `sched_getaffinity` on Linux (14 A.4.3).
///
/// `xargs.cpu_count`: `len(os.sched_getaffinity(0))` where it exists (Linux; cgroup CPU quotas are ignored, as in
/// Python), else `multiprocessing.cpu_count()` (`os.cpu_count()`), else 1.
pub fn cpu_count() -> NonZeroUsize {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    if let Some(n) = rustix::thread::sched_getaffinity(None)
        .ok()
        .and_then(|set| NonZeroUsize::new(usize::try_from(set.count()).unwrap_or(0)))
    {
        return n;
    }
    // os.cpu_count(): sysconf(_SC_NPROCESSORS_ONLN) on Unix, the active processor count on Windows.
    std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN)
}

/// `PRE_COMMIT_NO_CONCURRENCY` present → 1; `TRAVIS` present → 2; else `cpu`; 1 when `require_serial`.
///
/// `lang_base.target_concurrency()` (presence semantics: an empty value counts) plus `run_xargs`'s
/// `jobs = 1 if require_serial else target_concurrency()`.
pub fn target_concurrency(env: &Env, require_serial: bool, cpu: NonZeroUsize) -> NonZeroUsize {
    if require_serial || env.is_present("PRE_COMMIT_NO_CONCURRENCY") {
        NonZeroUsize::MIN
    } else if env.is_present("TRAVIS") {
        NonZeroUsize::MIN.saturating_add(1)
    } else {
        cpu
    }
}

/// `2 ** 17`, `2 ** 12`: the clamp of `_get_platform_max_length` on POSIX.
const POSIX_MAX: usize = 131_072;
const POSIX_MIN: usize = 4_096;

/// POSIX clamp(`ARG_MAX` − 2048 − env, 4096, 131072); Windows 30720.
///
/// `xargs._get_platform_max_length()`; `startup_env_size` is `_environ_size()` of the startup environment
/// ([`Env::python_environ_size`]).
pub fn platform_max_length(startup_env_size: usize) -> usize {
    if cfg!(windows) {
        // 2 ** 15 - 2048: UNICODE_STRING max - headroom.
        30_720
    } else {
        let maximum = sysconf_arg_max()
            .saturating_sub(2048)
            .saturating_sub(startup_env_size);
        maximum.clamp(POSIX_MIN, POSIX_MAX)
    }
}

/// `os.sysconf('SC_ARG_MAX')`: glibc and musl compute `max(131072, RLIMIT_STACK / 4)` on Linux; macOS reports
/// `kern.argmax` (1 MiB). Other Unix: the POSIX legacy value (the clamp makes the result 131072 for every value
/// ≥ 135168 + env anyway).
fn sysconf_arg_max() -> usize {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let stack = rustix::process::getrlimit(rustix::process::Resource::Stack).current;
        match stack.map(|s| usize::try_from(s / 4)) {
            Some(Ok(quarter)) => quarter.max(POSIX_MAX),
            Some(Err(_)) | None => usize::MAX,
        }
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        1_048_576
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    )))]
    {
        POSIX_MAX
    }
}

/// Windows .bat/.cmd budget (8192 − len − 4 − 1024).
///
/// `xargs.xargs`: `8192 - len(cmd_exe) - len(' /c ') - 1024`, where `len` counts Python characters.
pub fn batch_max_length(cmd_exe: &Path) -> usize {
    let chars = cmd_exe.as_os_str().to_string_lossy().chars().count();
    8192_usize
        .saturating_sub(chars)
        .saturating_sub(4)
        .saturating_sub(1024)
}

/// `_command_length` unit: bytes (Unix) / UTF-16 units (Windows).
///
/// `len(s.encode(sys.getfilesystemencoding()))` on POSIX (the raw bytes: snp keeps file names as bytes, D-CRASH-12),
/// `len(s.encode('utf-16le')) // 2` on Windows.
pub fn cmdlen(s: &OsStr) -> usize {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        s.encode_wide().count()
    }
    #[cfg(not(windows))]
    {
        s.len()
    }
}

/// `_command_length(*cmd)` = the length of `' '.join(cmd)`.
fn command_length(cmd: &[OsString]) -> usize {
    let seps = cmd.len().saturating_sub(1);
    cmd.iter()
        .fold(seps, |acc, a| acc.saturating_add(cmdlen(a)))
}

/// `xargs.ArgumentTooLongError` with the offending argument.
#[derive(Debug)]
pub struct ArgumentTooLong(pub OsString);

/// Exact port of `xargs.partition`: index ranges into `files`; ≥ 1 partition (zero files → [0..0]).
///
/// `max_len` is `_max_length`; like Python's `_max_length or _get_platform_max_length()`, 0 selects
/// [`platform_max_length`] (computed here with an empty environment: callers pass the real budget).
pub fn partition(
    cmd: &[OsString],
    files: &[&OsStr],
    jobs: NonZeroUsize,
    max_len: usize,
) -> Result<Vec<Range<usize>>, ArgumentTooLong> {
    let max_len = if max_len == 0 {
        platform_max_length(0)
    } else {
        max_len
    };
    // max(4, math.ceil(len(varargs) / target_concurrency))
    let max_args = files.len().div_ceil(jobs.get()).max(4);
    let base = command_length(cmd).saturating_add(1);

    let mut ret = Vec::new();
    let mut start = 0_usize;
    let mut count = 0_usize;
    let mut total = base;
    let mut i = 0_usize;
    while let Some(arg) = files.get(i) {
        let arg_len = cmdlen(arg).saturating_add(1);
        if total.saturating_add(arg_len) <= max_len && count < max_args {
            count = count.saturating_add(1);
            total = total.saturating_add(arg_len);
            i = i.saturating_add(1);
        } else if count == 0 {
            return Err(ArgumentTooLong(arg.to_os_string()));
        } else {
            // Exceeded the budget: emit a command and retry this argument in a fresh one.
            ret.push(start..i);
            start = i;
            count = 0;
            total = base;
        }
    }
    ret.push(start..i);
    Ok(ret)
}

/// `max(abs(rc))` over the partitions, first wins on ties.
///
/// `xargs.xargs`: `if abs(proc_retcode) > abs(retcode): retcode = proc_retcode`, starting from 0.
pub fn aggregate(codes: impl IntoIterator<Item = PyStatus>) -> PyStatus {
    codes.into_iter().fold(PyStatus(0), |acc, rc| {
        if rc.0.unsigned_abs() > acc.0.unsigned_abs() {
            rc
        } else {
            acc
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    /// The partitions as Python would return them: `cmd + files[range]`.
    fn parts(cmd: &[&str], files: &[&str], jobs: usize, max: usize) -> Vec<Vec<String>> {
        let files_os: Vec<&OsStr> = files.iter().map(OsStr::new).collect();
        let ranges = partition(&os(cmd), &files_os, NonZeroUsize::new(jobs).unwrap(), max).unwrap();
        ranges
            .into_iter()
            .map(|r| {
                cmd.iter()
                    .chain(&files[r])
                    .map(|s| (*s).to_owned())
                    .collect()
            })
            .collect()
    }

    fn n(v: usize) -> NonZeroUsize {
        NonZeroUsize::new(v).unwrap()
    }

    #[test]
    fn t10_xrg_03_partition_trivial_and_simple() {
        assert_eq!(parts(&["cmd"], &[], 1, POSIX_MAX), [vec!["cmd"]]);
        let ranges = partition(&os(&["cmd"]), &[], n(1), POSIX_MAX).unwrap();
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges.first(), Some(&(0..0)));
        assert_eq!(
            parts(&["cmd"], &["foo"], 1, POSIX_MAX),
            [vec!["cmd", "foo"]]
        );
    }

    #[test]
    fn t10_xrg_04_partition_limits() {
        let got = parts(
            &["ninechars"],
            &[".....", "....", "..........", ".....", "......"],
            1,
            21,
        );
        assert_eq!(
            got,
            [
                vec!["ninechars", ".....", "...."],
                vec!["ninechars", ".........."],
                vec!["ninechars", "....."],
                vec!["ninechars", "......"],
            ]
        );
    }

    #[test]
    fn t10_xrg_05_partition_unicode_lengths() {
        let emoji5: String = std::iter::repeat_n('\u{1f611}', 5).collect();
        let emoji10: String = std::iter::repeat_n('\u{1f611}', 10).collect();
        if cfg!(windows) {
            // UTF-16: 2 units per emoji → 9 + 1 + 10 + 1 = 21.
            assert_eq!(parts(&["ninechars"], &[&emoji5], 1, 21).len(), 1);
        } else {
            // UTF-8: 4 bytes per emoji → 9 + 1 + 20 + 1 = 31.
            assert_eq!(parts(&["ninechars"], &[&emoji5], 1, 31).len(), 1);
            let e = partition(&os(&["ninechars"]), &[OsStr::new(&emoji10)], n(1), 20).unwrap_err();
            assert_eq!(e.0, OsString::from(&emoji10));
        }
    }

    #[test]
    fn t10_xrg_06_partition_target_concurrency() {
        let a22 = ["A"; 22];
        let lens: Vec<usize> = parts(&["foo"], &a22, 4, 50)
            .iter()
            .map(|p| p.len() - 1)
            .collect();
        assert_eq!(lens, [6, 6, 6, 4]);
        // Won't make tiny partitions: max_args = max(4, ceil(10 / 4)) = 4.
        let a10 = ["A"; 10];
        let lens: Vec<usize> = parts(&["foo"], &a10, 4, 50)
            .iter()
            .map(|p| p.len() - 1)
            .collect();
        assert_eq!(lens, [4, 4, 2]);
    }

    #[test]
    fn t10_xrg_07_argument_too_long() {
        let e = partition(&os(&["aaaaa"]), &[OsStr::new("aaaaa")], n(1), 10).unwrap_err();
        assert_eq!(e.0, OsString::from("aaaaa"));
    }

    #[test]
    fn t10_xrg_15_target_concurrency() {
        let cpu = n(4);
        let env = |pairs: &[(&str, &str)]| Env::from_pairs(pairs.iter().copied());
        assert_eq!(
            target_concurrency(&env(&[("PRE_COMMIT_NO_CONCURRENCY", "1")]), false, cpu),
            n(1)
        );
        assert_eq!(
            target_concurrency(&env(&[("TRAVIS", "1")]), false, cpu),
            n(2)
        );
        assert_eq!(target_concurrency(&env(&[]), false, cpu), n(4));
        // Presence semantics: an empty value counts; require_serial wins.
        assert_eq!(
            target_concurrency(&env(&[("PRE_COMMIT_NO_CONCURRENCY", "")]), false, cpu),
            n(1)
        );
        assert_eq!(target_concurrency(&env(&[]), true, cpu), n(1));
    }

    #[test]
    fn platform_max_length_clamps() {
        if cfg!(windows) {
            assert_eq!(platform_max_length(0), 30_720);
        } else {
            assert_eq!(platform_max_length(0), POSIX_MAX);
            assert_eq!(platform_max_length(usize::MAX), POSIX_MIN);
        }
    }

    #[test]
    fn batch_budget_and_cmdlen() {
        assert_eq!(
            batch_max_length(Path::new(r"C:\Windows\system32\cmd.exe")),
            8192 - 27 - 4 - 1024
        );
        assert_eq!(cmdlen(OsStr::new("abc")), 3);
        assert_eq!(command_length(&[]), 0);
        assert_eq!(command_length(&os(&["a", "bc"])), 4);
    }

    #[test]
    fn aggregate_max_abs_first_wins() {
        let st = |v: &[i64]| aggregate(v.iter().copied().map(PyStatus));
        assert_eq!(st(&[]), PyStatus(0));
        assert_eq!(st(&[0, 1]), PyStatus(1));
        assert_eq!(st(&[0, 5, 1]), PyStatus(5));
        assert_eq!(st(&[1, -9]), PyStatus(-9));
        assert_eq!(st(&[-3, 3]), PyStatus(-3));
        assert_eq!(st(&[i64::MIN, 1]), PyStatus(i64::MIN));
    }

    #[test]
    fn cpu_count_is_positive() {
        assert!(cpu_count().get() >= 1);
    }
}
