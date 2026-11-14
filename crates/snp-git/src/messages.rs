//! Every user-visible text snp-git produces (ARCHITECTURE §1.3 "Messages"): pre-commit's texts verbatim, the
//! brand-dependent ones rendered per [`Brand`], and the snp-only diagnostics of the divergence register.
//!
//! Owner: WP-18.

use std::ffi::OsStr;

use snp_core::brand::Brand;
use snp_core::output::{Color, Printer};

use crate::GitVersion;

/// `git.get_root` (`git.py:63-67`): every failure of the discovery `rev-parse` (no git, not a repository,
/// `safe.directory`, …; 02 G.0 item 10).
pub(crate) const GIT_FAILED: &str =
    "git failed. Is it installed, and are you in a Git repository directory?";

/// `git.get_root` (`git.py:68-72`): `--is-inside-git-dir` was not `false` (inside `.git`, bare repository).
pub(crate) const TOPLEVEL_EMPTY: &str = "git toplevel unexpectedly empty! make sure you are not inside the `.git` directory of your repository.";

/// `staged_files_only.py:101-104`: `FatalError(f'pre-commit failed to diff -- perhaps due to permissions?\n\n{e}')`
/// with `e` the `CalledProcessError` of the `diff-index` call (format A, bare `'git'` in the argv); `{prog}` per brand
/// (COMPATIBILITY §3.3).
pub(crate) fn diff_failed(brand: Brand, called_process: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "{} failed to diff -- perhaps due to permissions?\n\n",
        brand.prog()
    )
    .into_bytes();
    out.extend_from_slice(called_process);
    out
}

/// D-DATA-19 (ADR-093, amended for ADR-095 K7): the stash's path-limited checkout refuses a path that would select
/// more than itself — an empty `:(literal)` pathspec is the whole tree, a NUL byte splits an item. git-made lists
/// never hold one; `repr` is the path as Python's `repr` of bytes.
pub(crate) fn refused_checkout_path(repr: &str) -> String {
    format!(
        "refusing to check out the path {repr}: an empty or NUL-bearing path would select more than one file; \
         nothing was checked out"
    )
}

/// D-DIAG-01 (02 G.4): stderr, once per process, `[WARNING]` coloured like pre-commit's log lines. The text names
/// snp in both brands (an snp-only diagnostic; COMPATIBILITY §5.8).
pub(crate) fn old_git_warning(v: GitVersion, printer: &Printer) -> String {
    format!(
        "{} git {v} is older than 2.31.0, the oldest git version snp is tested with. Set SNP_ALLOW_OLD_GIT=1 to \
         silence this warning.",
        printer.paint("[WARNING]", Color::Yellow)
    )
}

/// X13-GIT-10 (C-err): `git rev-parse --git-dir` echoed its option (git < 2.5, where pre-commit dies with
/// `AssertionError: unreachable: no git dir`). Raised during discovery, before the brand is known, so the text does
/// not name the tool (identical in both brands).
pub(crate) fn git_dir_unsupported(version: &str, exe: &OsStr) -> String {
    format!(
        "git {version} is not supported: `git rev-parse --git-dir` needs git 2.5.0 or newer (2.31.0 or newer \
         recommended; found {}).",
        exe.to_string_lossy()
    )
}

/// D-PLAT-08: a Cygwin/MSYS2 git on Windows (pre-commit warns `pre-commit has detected a mix of cygwin python /
/// git` and then misbehaves).
pub(crate) fn cygwin_git(brand: Brand, exe: &OsStr) -> String {
    format!(
        "{prog} does not support a Cygwin or MSYS2 git on Windows (found {}); install Git for Windows \
         (https://gitforwindows.org/) and put it first on PATH.",
        exe.to_string_lossy(),
        prog = brand.prog()
    )
}

#[cfg(test)]
mod tests {
    use snp_core::cancel::CancelToken;
    use snp_core::output::PrinterOpts;

    use super::*;

    fn printer(brand: Brand, color: bool) -> Printer {
        Printer::new(
            Box::new(std::io::sink()),
            Box::new(std::io::sink()),
            PrinterOpts {
                brand,
                color,
                stdout_is_tty: false,
                windows_text_mode: false,
                cancel: CancelToken::new(),
            },
        )
    }

    #[test]
    fn pre_commit_texts_are_verbatim() {
        assert_eq!(
            GIT_FAILED,
            "git failed. Is it installed, and are you in a Git repository directory?"
        );
        assert_eq!(
            TOPLEVEL_EMPTY,
            "git toplevel unexpectedly empty! make sure you are not inside the `.git` directory of your \
             repository."
        );
    }

    #[test]
    fn diff_failed_both_brands() {
        let cpe = b"command: ('git', 'diff-index')\nreturn code: 2\nstdout: (none)\nstderr: (none)";
        assert_eq!(
            diff_failed(Brand::PreCommit, cpe),
            b"pre-commit failed to diff -- perhaps due to permissions?\n\ncommand: ('git', 'diff-index')\n\
              return code: 2\nstdout: (none)\nstderr: (none)"
                .to_vec()
        );
        assert_eq!(
            diff_failed(Brand::Snp, cpe),
            b"snp failed to diff -- perhaps due to permissions?\n\ncommand: ('git', 'diff-index')\n\
              return code: 2\nstdout: (none)\nstderr: (none)"
                .to_vec()
        );
    }

    #[test]
    fn old_git_warning_both_brands_and_colours() {
        let v = GitVersion::new(2, 30, 2);
        let plain = "[WARNING] git 2.30.2 is older than 2.31.0, the oldest git version snp is tested with. Set \
                     SNP_ALLOW_OLD_GIT=1 to silence this warning.";
        assert_eq!(old_git_warning(v, &printer(Brand::PreCommit, false)), plain);
        assert_eq!(old_git_warning(v, &printer(Brand::Snp, false)), plain);
        let coloured = "\x1b[43;30m[WARNING]\x1b[m git 2.30.2 is older than 2.31.0, the oldest git version snp \
                        is tested with. Set SNP_ALLOW_OLD_GIT=1 to silence this warning.";
        assert_eq!(old_git_warning(v, &printer(Brand::Snp, true)), coloured);
        assert_eq!(
            old_git_warning(v, &printer(Brand::PreCommit, true)),
            coloured
        );
    }

    #[test]
    fn git_dir_unsupported_is_brand_neutral() {
        let exe = OsStr::new("/opt/git-2.4/bin/git");
        assert_eq!(
            git_dir_unsupported("2.4.0", exe),
            "git 2.4.0 is not supported: `git rev-parse --git-dir` needs git 2.5.0 or newer (2.31.0 or newer \
             recommended; found /opt/git-2.4/bin/git)."
        );
        assert_eq!(
            git_dir_unsupported("(unknown version)", exe),
            "git (unknown version) is not supported: `git rev-parse --git-dir` needs git 2.5.0 or newer (2.31.0 \
             or newer recommended; found /opt/git-2.4/bin/git)."
        );
    }

    #[test]
    fn cygwin_both_brands() {
        let exe = OsStr::new(r"C:\cygwin64\bin\git.exe");
        assert_eq!(
            cygwin_git(Brand::PreCommit, exe),
            r"pre-commit does not support a Cygwin or MSYS2 git on Windows (found C:\cygwin64\bin\git.exe); install Git for Windows (https://gitforwindows.org/) and put it first on PATH."
        );
        assert_eq!(
            cygwin_git(Brand::Snp, exe),
            r"snp does not support a Cygwin or MSYS2 git on Windows (found C:\cygwin64\bin\git.exe); install Git for Windows (https://gitforwindows.org/) and put it first on PATH."
        );
    }
}
