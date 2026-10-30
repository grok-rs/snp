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
    let affinity = rustix::thread::sched_getaffinity(None)
        .ok()
        .map(|set| usize::try_from(set.count()).unwrap_or(0));
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let affinity: Option<usize> = None;
    // os.cpu_count(): sysconf(_SC_NPROCESSORS_ONLN) on Unix; Windows `GetSystemInfo` (D-PLAT-10: CPython uses
    // `GetActiveProcessorCount(ALL_PROCESSOR_GROUPS)`, which differs only above 64 logical CPUs).
    cpu_count_from(affinity, || {
        std::thread::available_parallelism()
            .ok()
            .map(NonZeroUsize::get)
    })
}

/// `xargs.cpu_count`'s fallback chain: the affinity count when available (and non-zero), else `cpu_count()`
/// (`None` = `NotImplementedError`), else 1.
fn cpu_count_from(
    affinity: Option<usize>,
    os_count: impl FnOnce() -> Option<usize>,
) -> NonZeroUsize {
    affinity
        .and_then(NonZeroUsize::new)
        .or_else(|| os_count().and_then(NonZeroUsize::new))
        .unwrap_or(NonZeroUsize::MIN)
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
    #[cfg(windows)]
    if let Some(script) = cmd.first().filter(|c| is_batch(c)) {
        return Ok(resplit_batch(
            script,
            cmd.get(1..).unwrap_or_default(),
            files,
            ret,
        ));
    }
    Ok(ret)
}

/// cmd.exe's command-line limit (D-PLAT-09).
#[cfg(any(test, windows))]
const BATCH_LINE_MAX: usize = 8191;

/// `cmd[0].lower().endswith(('.bat', '.cmd'))` (`xargs.py:155`).
#[cfg(any(test, windows))]
fn is_batch(program: &OsStr) -> bool {
    let lower = program.as_encoded_bytes().to_ascii_lowercase();
    lower.ends_with(b".bat") || lower.ends_with(b".cmd")
}

/// UTF-16 length of `s` (lossy for unpaired surrogates on Unix, exact on Windows).
#[cfg(any(test, windows))]
fn utf16_len(s: &OsStr) -> usize {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        s.encode_wide().count()
    }
    #[cfg(not(windows))]
    {
        s.to_string_lossy().encode_utf16().count()
    }
}

/// The UTF-16 length of one argument as Rust's `make_bat_command_line` renders it (14 P-26): quoted when empty,
/// ending with `\` or containing an ASCII character outside `[A-Za-z0-9#$*+-./:?@\_]`; inside quotes `"` becomes
/// `""` and the backslashes before it are doubled, as are those before the closing quote; every `%` becomes
/// `%%cd:~,%` (+7).
#[cfg(any(test, windows))]
fn bat_arg_len(arg: &OsStr) -> usize {
    let text = arg.to_string_lossy();
    let safe = |c: char| c.is_ascii_alphanumeric() || "#$*+-./:?@\\_".contains(c);
    let quote = text.is_empty()
        || text.ends_with('\\')
        || text
            .chars()
            .any(|c| c.is_ascii() && (!safe(c) || c.is_ascii_control()));
    let mut len: usize = 0;
    let mut backslashes: usize = 0;
    for c in text.chars() {
        if c == '\\' {
            backslashes = backslashes.saturating_add(1);
        } else {
            if c == '"' {
                // n backslashes become 2n before an internal quote, and the quote is doubled.
                len = len.saturating_add(backslashes).saturating_add(1);
            }
            backslashes = 0;
        }
        len = len.saturating_add(if c == '%' { 8 } else { c.len_utf16() });
    }
    if quote {
        len = len.saturating_add(backslashes).saturating_add(2);
    }
    len
}

/// The length of `cmd.exe /e:ON /v:OFF /d /c ""<script>" <args…>"` (Rust's batch command line).
#[cfg(any(test, windows))]
fn bat_line_len<'a>(script: &OsStr, args: impl Iterator<Item = &'a OsStr>) -> usize {
    let prefix = "cmd.exe /e:ON /v:OFF /d /c \"".len();
    args.fold(
        prefix.saturating_add(utf16_len(script)).saturating_add(3),
        |acc, a| acc.saturating_add(1).saturating_add(bat_arg_len(a)),
    )
}

/// D-PLAT-09: halves every partition whose real batch command line would exceed 8191 characters (pre-commit's
/// budget counts unquoted lengths, so names needing quotes or containing `%` could overflow and crash).
#[cfg(any(test, windows))]
fn resplit_batch(
    script: &OsStr,
    cmd_args: &[OsString],
    files: &[&OsStr],
    ranges: Vec<Range<usize>>,
) -> Vec<Range<usize>> {
    let fits = |r: &Range<usize>| {
        let part = files.get(r.clone()).unwrap_or_default();
        let args = cmd_args
            .iter()
            .map(OsString::as_os_str)
            .chain(part.iter().copied());
        bat_line_len(script, args) <= BATCH_LINE_MAX
    };
    let mut out = Vec::new();
    let mut todo: Vec<Range<usize>> = ranges.into_iter().rev().collect();
    while let Some(r) = todo.pop() {
        let n = r.end.saturating_sub(r.start);
        if n <= 1 || fits(&r) {
            out.push(r);
        } else {
            let mid = r.start.saturating_add(n / 2);
            todo.push(mid..r.end);
            todo.push(r.start..mid);
        }
    }
    out
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

    /// `xargs_test.py:16-47`: affinity 345 → 345; no affinity and `cpu_count()` 123 → 123; neither → 1.
    #[test]
    fn t10_xrg_01_cpu_count_fallbacks() {
        assert_eq!(cpu_count_from(Some(345), || Some(7)), n(345));
        assert_eq!(cpu_count_from(None, || Some(123)), n(123));
        assert_eq!(cpu_count_from(None, || None), n(1));
        // An empty affinity set behaves like a missing one.
        assert_eq!(cpu_count_from(Some(0), || Some(5)), n(5));
    }

    #[test]
    fn batch_argument_lengths_follow_rusts_quoting() {
        let l = |s: &str| bat_arg_len(OsStr::new(s));
        assert_eq!(l("plain"), 5);
        assert_eq!(l(""), 2);
        assert_eq!(l("a b"), 5); // "a b"
        assert_eq!(l("a\"b"), 6); // "a""b"
        assert_eq!(l("a&b"), 5); // "a&b"
        assert_eq!(l("a\\b\\"), 7); // "a\b\\"
        assert_eq!(l("100%"), 13); // "100%%cd:~,%" quoted? '%' is unsafe → quoted
        assert_eq!(l("C:/x/y.py"), 9);
        assert_eq!(l("é"), 1);
        let script = OsStr::new("C:\\t.bat");
        // cmd.exe /e:ON /v:OFF /d /c ""C:\t.bat" x"
        assert_eq!(
            bat_line_len(script, [OsStr::new("x")].into_iter()),
            "cmd.exe /e:ON /v:OFF /d /c \"\"C:\\t.bat\" x\"".len()
        );
        assert!(is_batch(OsStr::new("x.CMD")) && is_batch(OsStr::new("y.bat")));
        assert!(!is_batch(OsStr::new("x.exe")));
    }

    /// D-PLAT-09: partitions whose quoted batch line exceeds 8191 are halved until they fit.
    #[test]
    fn d_plat_09_batch_partitions_are_resplit() {
        let names: Vec<String> = (0..200).map(|i| format!("a%%%%%-{i:04}.txt")).collect();
        let files: Vec<&OsStr> = names.iter().map(OsStr::new).collect();
        let cmd = os(&["C:\\x\\run.cmd", "--fix"]);
        // pre-commit's unquoted budget accepts all 200 in one partition…
        let ranges: Vec<Range<usize>> = std::iter::once(0..200).collect();
        let total: usize = files.iter().map(|f| f.len() + 1).sum();
        assert!(total < batch_max_length(Path::new("C:\\Windows\\system32\\cmd.exe")));
        let out = resplit_batch(OsStr::new("C:\\x\\run.cmd"), &cmd[1..], &files, ranges);
        // …but every resulting partition fits the real limit, in order, without loss.
        let mut next = 0;
        for r in &out {
            assert_eq!(r.start, next);
            next = r.end;
            let line = bat_line_len(
                OsStr::new("C:\\x\\run.cmd"),
                cmd[1..]
                    .iter()
                    .map(OsString::as_os_str)
                    .chain(files[r.clone()].iter().copied()),
            );
            assert!(line <= BATCH_LINE_MAX, "{line}");
        }
        assert_eq!(next, 200);
        assert!(out.len() > 1);
        // Fitting partitions are unchanged.
        assert_eq!(
            resplit_batch(OsStr::new("t.bat"), &[], &files[..3], vec![0..1, 1..3]),
            [0..1, 1..3]
        );
    }

    /// A line-by-line transliteration of `xargs.partition` (`xargs.py:76-117`): a reversed list popped from the end,
    /// an overflowing argument pushed back.
    fn reference_partition(
        cmd: &[OsString],
        files: &[&OsStr],
        jobs: usize,
        max_len: usize,
    ) -> Result<Vec<Vec<OsString>>, OsString> {
        let max_args = files.len().div_ceil(jobs).max(4);
        let mut ret = Vec::new();
        let mut ret_cmd: Vec<OsString> = Vec::new();
        let mut varargs: Vec<&OsStr> = files.iter().rev().copied().collect();
        let base = command_length(cmd) + 1;
        let mut total = base;
        while let Some(arg) = varargs.pop() {
            let arg_len = cmdlen(arg) + 1;
            if total + arg_len <= max_len && ret_cmd.len() < max_args {
                ret_cmd.push(arg.to_os_string());
                total += arg_len;
            } else if ret_cmd.is_empty() {
                return Err(arg.to_os_string());
            } else {
                ret.push([cmd, &ret_cmd].concat());
                ret_cmd = Vec::new();
                total = base;
                varargs.push(arg);
            }
        }
        ret.push([cmd, &ret_cmd].concat());
        Ok(ret)
    }

    proptest::proptest! {
        /// 10 §7.5.1 model test: `partition` equals the Python transliteration on random inputs.
        #[test]
        fn partition_matches_the_python_model(
            cmd in proptest::collection::vec("[a-z]{1,6}", 1..4),
            files in proptest::collection::vec("[a-zé😑 ]{0,20}", 0..120),
            jobs in 1usize..32,
            max_len in 10usize..600,
        ) {
            let cmd_os: Vec<OsString> = cmd.iter().map(OsString::from).collect();
            let files_os: Vec<&OsStr> = files.iter().map(OsStr::new).collect();
            let got = partition(&cmd_os, &files_os, n(jobs), max_len)
                .map(|ranges| {
                    ranges
                        .into_iter()
                        .map(|r| {
                            let mut v = cmd_os.clone();
                            v.extend(files_os[r].iter().map(|f| f.to_os_string()));
                            v
                        })
                        .collect::<Vec<_>>()
                })
                .map_err(|e| e.0);
            proptest::prop_assert_eq!(got, reference_partition(&cmd_os, &files_os, jobs, max_len));
        }

        /// 10 §7.5.1: never drops or duplicates a file, keeps the order, respects the budget and max_args, and
        /// raises ArgumentTooLong exactly when a single argument cannot fit.
        #[test]
        fn partition_properties(
            cmd in proptest::collection::vec("[a-z]{1,8}", 1..4),
            files in proptest::collection::vec("[a-zé😑 ]{0,30}", 0..200),
            jobs in 1usize..64,
            max_len in 10usize..2000,
        ) {
            let cmd_os: Vec<OsString> = cmd.iter().map(OsString::from).collect();
            let files_os: Vec<&OsStr> = files.iter().map(OsStr::new).collect();
            let base = command_length(&cmd_os) + 1;
            match partition(&cmd_os, &files_os, n(jobs), max_len) {
                Ok(ranges) => {
                    proptest::prop_assert!(!ranges.is_empty());
                    let max_args = files.len().div_ceil(jobs).max(4);
                    let mut next = 0;
                    for r in &ranges {
                        proptest::prop_assert_eq!(r.start, next);
                        next = r.end;
                        proptest::prop_assert!(r.len() <= max_args);
                        let used: usize = base + files_os[r.clone()].iter().map(|f| cmdlen(f) + 1).sum::<usize>();
                        proptest::prop_assert!(used <= max_len || r.is_empty());
                        proptest::prop_assert!(!r.is_empty() || files.is_empty());
                    }
                    proptest::prop_assert_eq!(next, files.len());
                }
                Err(ArgumentTooLong(arg)) => {
                    let alen = cmdlen(&arg) + 1;
                    proptest::prop_assert!(base + alen > max_len);
                    proptest::prop_assert!(files_os.contains(&arg.as_os_str()));
                }
            }
        }
    }
}
