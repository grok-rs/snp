//! Fuzz target `git_ls_files_tagged`: the incremental parser of `git ls-files -s -v -z` (ADR-106).
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! The first input byte is the chunk size git's stdout arrives in (0: one write), the rest is that stdout, served by
//! an in-process backend (no git) to `Git::all_files_tagged`. Checked: the parser never panics; the result does not
//! depend on how the stream is chunked; on success there is one mode per path, the paths are the record paths with
//! adjacent duplicates folded (what `ls-files -z --deduplicate` prints), each mode is the record's octal mode or 0
//! for the empty blob, and `plain` holds exactly when every record is tagged `H` at stage 0.
//!
//! Owner: WP-18.
#![no_main]

use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use snp_core::env::Env;
use snp_git::{Git, GitBackend, GitCaps, GitExe, GitVersion, TaggedIndex};
use snp_proc::spawn::{Captured, Group, PyStatus, SpawnError, Stdin};

/// Discovery succeeds; every streamed call writes `out` in chunks of `chunk` bytes (0: at once).
#[derive(Debug)]
struct Canned {
    out: Vec<u8>,
    chunk: usize,
}

impl GitBackend for Canned {
    fn capture(&self, _: &[OsString], _: &Env, _: &Path, _: Stdin, _: Group) -> Result<Captured, SpawnError> {
        Ok(Captured {
            status: PyStatus(0),
            stdout: b"\nfalse\n/r/.git\n/r/.git\n/r/.git/index\n".to_vec(),
            stderr: Vec::new(),
            stopped_by_tty: false,
        })
    }
    fn stream(&self, _: &[OsString], _: &Env, _: &Path, sink: &mut dyn io::Write) -> Result<(PyStatus, Vec<u8>), SpawnError> {
        let size = if self.chunk == 0 { self.out.len().max(1) } else { self.chunk };
        for part in self.out.chunks(size) {
            sink.write_all(part).map_err(|io| SpawnError { program: OsString::from("git"), io })?;
        }
        Ok((PyStatus(0), Vec::new()))
    }
    fn inherit(&self, _: &[OsString], _: &Env, _: &Path, _: Stdin) -> Result<PyStatus, SpawnError> {
        Ok(PyStatus(0))
    }
}

fn git(out: &[u8], chunk: usize) -> Git {
    let exe = Arc::new(GitExe {
        argv0: vec![OsString::from("git")],
        caps: GitCaps {
            version: Some(GitVersion { major: 2, minor: 43, patch: 0 }),
            ls_files_dedup: true,
            path_format: true,
            config_env: true,
            init_object_format: true,
            config_hooks: false,
            pathspec_from_file: true,
            diff_no_relative: true,
        },
    });
    let backend = Arc::new(Canned { out: out.to_vec(), chunk });
    let env = Env::from_pairs([("PATH", "")]);
    match Git::discover(exe, &env, Path::new("/r"), backend) {
        Ok((g, _)) => g,
        Err(e) => panic!("discovery with a canned rev-parse must succeed: {e:?}"),
    }
}

fn same(a: &TaggedIndex, b: &TaggedIndex) -> bool {
    a.paths == b.paths && a.modes == b.modes && a.plain == b.plain
}

/// An octal mode field as git prints it (at most 7 digits).
fn octal(f: &[u8]) -> Option<u32> {
    if f.is_empty() || f.len() > 7 {
        return None;
    }
    f.iter().try_fold(0_u32, |acc, &b| {
        let d = b.checked_sub(b'0').filter(|d| *d < 8)?;
        acc.checked_mul(8)?.checked_add(u32::from(d))
    })
}

fuzz_target!(|data: &[u8]| {
    let Some((&chunk, out)) = data.split_first() else {
        return;
    };
    let whole = git(out, 0).all_files_tagged();
    let chunked = git(out, usize::from(chunk)).all_files_tagged();
    match (&whole, &chunked) {
        (Ok(a), Ok(b)) => assert!(same(a, b), "chunking changed the result"),
        (Err(_), Err(_)) => {}
        _ => panic!("chunking changed success: {} vs {}", whole.is_ok(), chunked.is_ok()),
    }
    let Ok(t) = whole else {
        return;
    };
    assert_eq!(t.paths.len(), t.modes.len());
    // the records: a success means no empty record, and a final NUL ends the last one
    let body = out.strip_suffix(b"\0").unwrap_or(out);
    let records: Vec<&[u8]> = if body.is_empty() { Vec::new() } else { body.split(|&b| b == 0).collect() };
    let mut want_paths: Vec<&[u8]> = Vec::new();
    let mut want_modes: Vec<u32> = Vec::new();
    let mut plain = true;
    for r in &records {
        let tab = r.iter().position(|&b| b == b'\t').expect("a parsed record has a TAB");
        let (head, path) = (&r[..tab], &r[tab + 1..]);
        let fields: Vec<&[u8]> = head.split(|&b| b == b' ').collect();
        assert_eq!(fields.len(), 4, "tag, mode, object, stage");
        plain &= fields[0] == b"H" && fields[3] == b"0";
        if want_paths.last() == Some(&path) {
            continue;
        }
        let empty = fields[2].eq_ignore_ascii_case(b"e69de29bb2d1d6434b8b29ae775ad8c2e48c5391")
            || fields[2].eq_ignore_ascii_case(b"473a0f4c3be8a93681a267e3b1e9a7dcda1185436fe141f7749120a303721813");
        want_paths.push(path);
        want_modes.push(if empty { 0 } else { octal(fields[1]).expect("a parsed mode is octal") });
    }
    assert_eq!(t.paths.iter().collect::<Vec<_>>(), want_paths);
    assert_eq!(&*t.modes, &want_modes[..]);
    assert_eq!(t.plain, plain);
});
