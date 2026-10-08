//! Shared plumbing of the test kit: the harness failure path, running git with the hermetic env and fixed dates,
//! path canonicalisation and file copies.
//!
//! Owner: WP-18.

use std::ffi::OsStr;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use snp_core::env::Env;

/// A broken fixture step aborts the test that asked for it (the frozen builder API returns plain values).
#[expect(
    clippy::panic,
    reason = "test harness: a failed fixture step must abort the calling test with its context"
)]
pub(crate) fn fail(msg: &str) -> ! {
    panic!("snp-testkit: {msg}")
}

/// `.or_fail(context)`: unwrap with a test-harness failure.
pub(crate) trait OrFail<T> {
    fn or_fail(self, context: &str) -> T;
}

impl<T, E: Display> OrFail<T> for Result<T, E> {
    fn or_fail(self, context: &str) -> T {
        self.unwrap_or_else(|e| fail(&format!("{context}: {e}")))
    }
}

impl<T> OrFail<T> for Option<T> {
    fn or_fail(self, context: &str) -> T {
        self.unwrap_or_else(|| fail(context))
    }
}

/// The first commit date of every repository (2020-01-01T00:00:00Z, 10 §7.3.2); each commit adds one second.
pub(crate) const BASE_DATE: u64 = 1_577_836_800;

/// The git date of tick `n` (`<unix seconds> +0000`, git's raw format).
pub(crate) fn date(n: u64) -> String {
    format!("{} +0000", BASE_DATE.saturating_add(n))
}

/// git resolved on the hermetic `PATH` (never the parent process's lookup).
pub(crate) fn git_exe(env: &Env, cwd: &Path) -> PathBuf {
    snp_proc::resolve::find_executable(OsStr::new("git"), env, cwd)
        .or_fail("git is not on the hermetic PATH")
}

/// Runs git in `cwd` with `env` and the current date tick (advanced by `commit`, `merge`, `tag`, `cherry-pick`).
/// Returns stdout; fails the test on a
/// non-zero exit unless `allow_failure`.
pub(crate) fn git_full<S: AsRef<OsStr>>(
    env: &Env,
    cwd: &Path,
    args: &[S],
    dates: &AtomicU64,
    allow_failure: bool,
) -> (bool, Vec<u8>) {
    let exe = git_exe(env, cwd);
    let mut cmd = Command::new(exe);
    env.apply_to(&mut cmd);
    // +1 s per object-creating command (10 §7.3.2): the dates, and so the SHAs, depend only on the script.
    let creates_objects = args.first().is_some_and(|a| {
        ["commit", "merge", "tag", "cherry-pick"]
            .iter()
            .any(|c| a.as_ref() == OsStr::new(c))
    });
    let d = if creates_objects {
        date(dates.fetch_add(1, Ordering::SeqCst))
    } else {
        date(dates.load(Ordering::SeqCst))
    };
    cmd.env("GIT_AUTHOR_DATE", &d)
        .env("GIT_COMMITTER_DATE", &d)
        .args(args)
        .current_dir(cwd);
    let out = cmd.output().or_fail("spawning git");
    if !out.status.success() && !allow_failure {
        let shown: Vec<String> = args
            .iter()
            .map(|a| a.as_ref().to_string_lossy().into_owned())
            .collect();
        fail(&format!(
            "git {shown:?} in {} failed: {}",
            cwd.display(),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    (out.status.success(), out.stdout)
}

/// [`git_full`] that must succeed.
pub(crate) fn git<S: AsRef<OsStr>>(
    env: &Env,
    cwd: &Path,
    args: &[S],
    dates: &AtomicU64,
) -> Vec<u8> {
    git_full(env, cwd, args, dates, false).1
}

/// `os.path.realpath` of an existing directory without Windows `\\?\` verbatim prefixes (macOS `/var` →
/// `/private/var`, Windows 8.3 short names expanded; 10 §7.3.2).
pub(crate) fn canonical(p: &Path) -> PathBuf {
    #[expect(
        clippy::disallowed_methods,
        reason = "dev-only test kit; snp_os::realpath is not an edge of snp-testkit (ARCHITECTURE §1.2)"
    )]
    let c = std::fs::canonicalize(p).or_fail("canonicalising the test root");
    strip_verbatim(c)
}

#[cfg(windows)]
fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.as_os_str().to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match s.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => p,
    }
}

#[cfg(not(windows))]
fn strip_verbatim(p: PathBuf) -> PathBuf {
    p
}

/// Creates `p` and its parents.
pub(crate) fn mkdirs(p: &Path) {
    std::fs::create_dir_all(p).or_fail(&format!("creating {}", p.display()));
}

/// Writes `bytes` to `p`, creating parents.
pub(crate) fn write(p: &Path, bytes: &[u8]) {
    if let Some(parent) = p.parent() {
        mkdirs(parent);
    }
    std::fs::write(p, bytes).or_fail(&format!("writing {}", p.display()));
}

/// Sets the executable bits (Unix); a no-op elsewhere (the index mode is set with `update-index --chmod=+x`).
pub(crate) fn make_executable(p: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755))
            .or_fail(&format!("chmod +x {}", p.display()));
    }
    #[cfg(not(unix))]
    let _: &Path = p;
}

/// `copy_tree_to_path` (`testing/fixtures.py:21-36`): copies the contents of `src` into the existing `dest`
/// (permissions kept). Returns the copied files relative to `dest`, sorted.
pub(crate) fn copy_tree(src: &Path, dest: &Path) -> Vec<PathBuf> {
    let mut copied = Vec::new();
    copy_into(src, dest, Path::new(""), &mut copied);
    copied.sort();
    copied
}

fn copy_into(src: &Path, dest: &Path, rel: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(src).or_fail(&format!("reading {}", src.display()));
    for entry in entries {
        let entry = entry.or_fail("reading a directory entry");
        let name = entry.file_name();
        let from = entry.path();
        let to = dest.join(&name);
        let rel_name = rel.join(&name);
        if from.is_dir() {
            mkdirs(&to);
            copy_into(&from, &to, &rel_name, out);
        } else {
            std::fs::copy(&from, &to).or_fail(&format!("copying {}", from.display()));
            out.push(rel_name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_fixed_ticks() {
        assert_eq!(date(0), "1577836800 +0000");
        assert_eq!(date(3), "1577836803 +0000");
    }

    #[test]
    fn or_fail_passes_values_and_panics_with_context() {
        assert_eq!(Ok::<_, String>(3).or_fail("x"), 3);
        assert_eq!(Some(4).or_fail("y"), 4);
        let r = std::panic::catch_unwind(|| Err::<u8, _>("boom").or_fail("ctx"));
        let msg = *r.unwrap_err().downcast::<String>().unwrap();
        assert_eq!(msg, "snp-testkit: ctx: boom");
    }

    #[test]
    fn copies_trees() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        write(&src.path().join("bin/hook.sh"), b"#!/bin/sh\n");
        make_executable(&src.path().join("bin/hook.sh"));
        write(&src.path().join("a"), b"a");
        let files = copy_tree(src.path(), dst.path());
        assert_eq!(
            files,
            vec![PathBuf::from("a"), PathBuf::from("bin/hook.sh")]
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dst.path().join("bin/hook.sh"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111);
        }
        assert_eq!(canonical(dst.path()), canonical(&canonical(dst.path())));
    }
}
