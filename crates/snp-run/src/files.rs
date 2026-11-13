//! File selection per mode (`run._all_filenames`, `run.py:253-271`; ARCHITECTURE §3.2) and `--stdin-files`.
//!
//! Owner: WP-32.

use std::ffi::OsString;
use std::io::Read;
use std::path::Path;

use snp_cli::StdinFormat;
use snp_core::failure::Failure;
use snp_core::output::{Level, Printer};
use snp_core::stage::{HookType, Stage};
use snp_files::FileTable;
use snp_git::{Git, MergeState, RepoInfo, ZList};

use crate::messages;

/// The 64 MiB cap of every stdin snp reads (ADR-076).
pub(crate) const STDIN_CAP: u64 = 64 * 1024 * 1024;

/// The stages whose hooks get no files (`run.py:255-259`).
pub(crate) const fn no_file_stage(s: Stage) -> bool {
    matches!(
        s,
        Stage::Hook(
            HookType::PostCheckout
                | HookType::PostCommit
                | HookType::PostMerge
                | HookType::PostRewrite
                | HookType::PreRebase
        )
    )
}

/// The stages that need `--commit-msg-filename`.
pub(crate) const fn commit_msg_stage(s: Stage) -> bool {
    matches!(
        s,
        Stage::Hook(HookType::PrepareCommitMsg | HookType::CommitMsg)
    )
}

/// The file source of a run, after `_adjust_args_and_chdir` (paths are root-relative).
#[derive(Debug)]
pub(crate) struct Selection<'a> {
    /// `--hook-stage`.
    pub(crate) stage: Stage,
    /// `--commit-msg-filename` (root-relative).
    pub(crate) commit_msg_filename: Option<&'a OsString>,
    /// `--from-ref` / `--to-ref` (both truthy, else `None`).
    pub(crate) refs: Option<(&'a OsString, &'a OsString)>,
    /// `--files` or the `--stdin-files` list (root-relative, CLI order, duplicates kept).
    pub(crate) files: &'a [OsString],
    /// `--all-files`.
    pub(crate) all_files: bool,
}

/// `_all_filenames(args)` as a [`FileTable`] (first match wins; git order kept, never sorted).
///
/// The merge-conflict branch logs `[INFO] Checking merge-conflict files only.` (`git.py:115`) before reading
/// `MERGE_MSG`.
pub(crate) fn table(
    sel: &Selection<'_>,
    git: &Git,
    info: &RepoInfo,
    printer: &Printer,
    staged: Option<ZList>,
) -> Result<FileTable, Failure> {
    let root = git.root();
    if no_file_stage(sel.stage) {
        return Ok(FileTable::from_args(Vec::new(), root));
    }
    if commit_msg_stage(sel.stage) {
        let f: Vec<OsString> = sel.commit_msg_filename.into_iter().cloned().collect();
        return Ok(FileTable::from_args(f, root));
    }
    if let Some((from, to)) = sel.refs {
        return Ok(from_z(git.changed_files(from, to)?, root));
    }
    if !sel.files.is_empty() {
        return Ok(FileTable::from_args(sel.files.to_vec(), root));
    }
    if sel.all_files {
        return Ok(from_z(git.all_files()?, root));
    }
    if git.merge_state(info) == MergeState::Merging {
        printer.log(Level::Info, messages::MERGE_CONFLICT_ONLY);
        return Ok(from_z(git.conflicted_files(info)?, root));
    }
    let staged = match staged {
        Some(z) => z,
        None => git.staged_files()?,
    };
    Ok(from_z(staged, root))
}

/// Whether the default (staged-files) branch of [`table`] is the one taken (the fan-out pre-fetches it).
pub(crate) fn wants_staged(sel: &Selection<'_>, git: &Git, info: &RepoInfo) -> bool {
    !no_file_stage(sel.stage)
        && !commit_msg_stage(sel.stage)
        && sel.refs.is_none()
        && sel.files.is_empty()
        && !sel.all_files
        && git.merge_state(info) == MergeState::None
}

/// Whether [`table`] takes the merge-conflict branch (its `[INFO]` line).
pub(crate) fn merge_branch(sel: &Selection<'_>, git: &Git, info: &RepoInfo) -> bool {
    !no_file_stage(sel.stage)
        && !commit_msg_stage(sel.stage)
        && sel.refs.is_none()
        && sel.files.is_empty()
        && !sel.all_files
        && git.merge_state(info) == MergeState::Merging
}

fn from_z(z: ZList, root: &Path) -> FileTable {
    let (buf, spans) = z.into_parts();
    FileTable::from_zbuf(buf, spans, root)
}

/// `--stdin-files[=nul|lines]` (snp brand, ARCHITECTURE §3.2): the list read from stdin, as if given to `--files`.
///
/// NUL-separated records, or lines (`\n`, an optional `\r` before it is dropped); empty records are skipped. More
/// than 64 MiB fails closed (ADR-076), never truncated.
pub(crate) fn read_stdin_files(
    fmt: StdinFormat,
    stdin: &mut dyn Read,
) -> Result<Vec<OsString>, Failure> {
    let mut buf = Vec::new();
    let mut limited = stdin.take(STDIN_CAP.saturating_add(1));
    limited
        .read_to_end(&mut buf)
        .map_err(|e| Failure::from_io(&e, None, None))?;
    if u64::try_from(buf.len()).unwrap_or(u64::MAX) > STDIN_CAP {
        return Err(Failure::fatal(
            "FatalError",
            messages::STDIN_FILES_TOO_LARGE,
        ));
    }
    Ok(split_records(&buf, fmt))
}

/// The records of a `--stdin-files` buffer.
pub(crate) fn split_records(buf: &[u8], fmt: StdinFormat) -> Vec<OsString> {
    let sep = match fmt {
        StdinFormat::Nul => b'\0',
        _ => b'\n',
    };
    buf.split(|b| *b == sep)
        .map(|r| match fmt {
            StdinFormat::Nul => r,
            _ => r.trim_suffix(b"\r"),
        })
        .filter(|r| !r.is_empty())
        .map(os_from_bytes)
        .collect()
}

/// Raw bytes as an `OsString` (Unix: exact; elsewhere: lossy UTF-8).
pub(crate) fn os_from_bytes(b: &[u8]) -> OsString {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt as _;
        OsString::from_vec(b.to_vec())
    }
    #[cfg(not(unix))]
    {
        OsString::from(String::from_utf8_lossy(b).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdin_records() {
        assert_eq!(
            split_records(b"a\0b c\0\0", StdinFormat::Nul),
            vec![OsString::from("a"), OsString::from("b c")]
        );
        assert_eq!(
            split_records(b"a\r\n\nb\n", StdinFormat::Lines),
            vec![OsString::from("a"), OsString::from("b")]
        );
        assert_eq!(
            split_records(b"", StdinFormat::Lines),
            Vec::<OsString>::new()
        );
    }

    #[test]
    fn stdin_cap_fails_closed() {
        struct Endless;
        impl Read for Endless {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                buf.fill(b'a');
                Ok(buf.len())
            }
        }
        let e = read_stdin_files(StdinFormat::Lines, &mut Endless).unwrap_err();
        assert_eq!(e.exit_code(), 1);
    }

    #[test]
    fn stage_classes() {
        assert!(no_file_stage(Stage::Hook(HookType::PostCheckout)));
        assert!(!no_file_stage(Stage::Hook(HookType::PrePush)));
        assert!(!no_file_stage(Stage::Manual));
        assert!(commit_msg_stage(Stage::Hook(HookType::CommitMsg)));
        assert!(!commit_msg_stage(Stage::Hook(HookType::PreCommit)));
    }
}
