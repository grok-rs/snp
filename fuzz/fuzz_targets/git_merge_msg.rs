//! Fuzz target `git_merge_msg`: `MERGE_MSG` conflict parsing.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! The input is a `MERGE_MSG` (bytes, never decoded: D-CRASH-12, X13-GIT-07); `Git::conflicted_files` reads it from a
//! scratch git dir and unions it with a canned merge diff. Checked: no panic, the union is duplicate-free, holds every
//! `merge_msg_conflicts` line in first-seen order, then the diff's records.
//!
//! Owner: WP-18.
#![no_main]

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use libfuzzer_sys::fuzz_target;
use snp_core::env::Env;
use snp_git::{Git, GitBackend, GitCaps, GitExe, GitVersion, RepoInfo};
use snp_proc::spawn::{Captured, Group, PyStatus, SpawnError, Stdin};

const DIFF: &[u8] = b"conflict_file\0other file\0";

#[derive(Debug)]
struct Canned;

impl Canned {
    fn reply(argv: &[OsString]) -> Vec<u8> {
        let has = |s: &str| argv.iter().any(|a| a == s);
        if has("rev-parse") {
            b"\nfalse\n/r/.git\n/r/.git\n/r/.git/index\n".to_vec()
        } else if has("write-tree") {
            b"4b825dc642cb6eb9a060e54bf8d69288fbee4904\n".to_vec()
        } else {
            DIFF.to_vec()
        }
    }
}

impl GitBackend for Canned {
    fn capture(&self, argv: &[OsString], _: &Env, _: &Path, _: Stdin, _: Group) -> Result<Captured, SpawnError> {
        Ok(Captured { status: PyStatus(0), stdout: Self::reply(argv), stderr: Vec::new(), stopped_by_tty: false })
    }
    fn stream(&self, argv: &[OsString], _: &Env, _: &Path, sink: &mut dyn io::Write) -> Result<(PyStatus, Vec<u8>), SpawnError> {
        sink.write_all(&Self::reply(argv)).map_err(|io| SpawnError { program: OsString::from("git"), io })?;
        Ok((PyStatus(0), Vec::new()))
    }
    fn inherit(&self, _: &[OsString], _: &Env, _: &Path, _: Stdin) -> Result<PyStatus, SpawnError> {
        Ok(PyStatus(0))
    }
}

/// One scratch git dir per fuzz process.
fn scratch() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let d = std::env::temp_dir().join(format!("snp-fuzz-git-merge-msg-{}", std::process::id()));
        std::fs::create_dir_all(&d).expect("scratch dir");
        d
    })
}

fuzz_target!(|data: &[u8]| {
    let git_dir = scratch();
    std::fs::write(git_dir.join("MERGE_MSG"), data).expect("write MERGE_MSG");
    std::fs::write(git_dir.join("MERGE_HEAD"), b"x\n").expect("write MERGE_HEAD");
    let info = RepoInfo {
        root: PathBuf::from("/r"),
        git_dir: git_dir.clone(),
        common_dir: git_dir.clone(),
        index_file: git_dir.join("index"),
        is_worktree: false,
    };
    let exe = Arc::new(GitExe {
        argv0: vec![OsString::from("git")],
        caps: GitCaps {
            version: Some(GitVersion { major: 2, minor: 43, patch: 0 }),
            ls_files_dedup: true,
            path_format: true,
            config_env: true,
            init_object_format: true,
            config_hooks: false,
        },
    });
    let env = Env::from_pairs([("PATH", "")]);
    let (git, _) = Git::discover(exe, &env, Path::new("/r"), Arc::new(Canned)).expect("canned discovery");
    let got = git.conflicted_files(&info).expect("a readable MERGE_MSG always parses");
    let got: Vec<&[u8]> = got.iter().collect();
    let mut want: Vec<&[u8]> = Vec::new();
    for r in snp_pycompat::precommit::merge_msg_conflicts(data)
        .into_iter()
        .chain(snp_pycompat::precommit::zsplit(DIFF))
    {
        if !want.contains(&r) {
            want.push(r);
        }
    }
    assert_eq!(got, want);
});
