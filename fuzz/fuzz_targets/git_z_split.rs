//! Fuzz target `git_z_split`: `-z` output splitting.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! The input is git's stdout for every `-z` list snp-git parses (`ls-files -z [--deduplicate]`, the staged and
//! intent-to-add lists, `ls-files -s -z`, `diff --staged --raw -z`, `config -z --get-regexp`, `config --list
//! --show-origin -z`), served by an in-process backend (no git). Checked: the lists equal `git.zsplit` (pycompat), the
//! below-2.31 fallback equals adjacent dedup of the same records, and the parsers fail cleanly instead of panicking.
//!
//! Owner: WP-18.
#![no_main]

use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use snp_core::env::Env;
use snp_git::{Git, GitBackend, GitCaps, GitExe, GitVersion};
use snp_proc::spawn::{Captured, Group, PyStatus, SpawnError, Stdin};

/// Discovery succeeds; every other call prints `out`.
#[derive(Debug)]
struct Canned {
    out: Vec<u8>,
}

impl Canned {
    fn reply(&self, argv: &[OsString]) -> Vec<u8> {
        if argv.iter().any(|a| a == "rev-parse") {
            b"\nfalse\n/r/.git\n/r/.git\n/r/.git/index\n".to_vec()
        } else {
            self.out.clone()
        }
    }
}

impl GitBackend for Canned {
    fn capture(&self, argv: &[OsString], _: &Env, _: &Path, _: Stdin, _: Group) -> Result<Captured, SpawnError> {
        Ok(Captured { status: PyStatus(0), stdout: self.reply(argv), stderr: Vec::new(), stopped_by_tty: false })
    }
    fn stream(&self, argv: &[OsString], _: &Env, _: &Path, sink: &mut dyn io::Write) -> Result<(PyStatus, Vec<u8>), SpawnError> {
        sink.write_all(&self.reply(argv)).map_err(|io| SpawnError { program: OsString::from("git"), io })?;
        Ok((PyStatus(0), Vec::new()))
    }
    fn inherit(&self, _: &[OsString], _: &Env, _: &Path, _: Stdin) -> Result<PyStatus, SpawnError> {
        Ok(PyStatus(0))
    }
}

fn exe(version: GitVersion) -> Arc<GitExe> {
    let modern = version >= GitVersion { major: 2, minor: 31, patch: 0 };
    Arc::new(GitExe {
        argv0: vec![OsString::from("git")],
        caps: GitCaps {
            version: Some(version),
            ls_files_dedup: modern,
            path_format: modern,
            config_env: modern,
            init_object_format: true,
            config_hooks: false,
            pathspec_from_file: modern,
            diff_no_relative: modern,
        },
    })
}

fn git(data: &[u8], version: GitVersion) -> Option<Git> {
    let env = Env::from_pairs([("PATH", "")]);
    let backend = Arc::new(Canned { out: data.to_vec() });
    // below 2.31 discovery asks separately; the canned reply is then not line-per-call, which may fail: fine
    Git::discover(exe(version), &env, Path::new("/r"), backend).ok().map(|(g, _)| g)
}

fuzz_target!(|data: &[u8]| {
    let expected: Vec<&[u8]> = snp_pycompat::precommit::zsplit(data).collect();
    let Some(new) = git(data, GitVersion { major: 2, minor: 43, patch: 0 }) else {
        panic!("discovery with a canned rev-parse must succeed");
    };
    for list in [new.all_files(), new.staged_files(), new.intent_to_add()] {
        let list = list.expect("a -z list always parses");
        let got: Vec<&[u8]> = list.iter().collect();
        assert_eq!(got, expected);
        assert_eq!(list.len(), expected.len());
        let (buf, spans) = list.clone().into_parts();
        assert_eq!(buf, data);
        assert_eq!(spans.len(), expected.len());
    }
    if let Some(old) = git(data, GitVersion { major: 2, minor: 30, patch: 2 }) {
        if let Ok(list) = old.all_files() {
            let mut dedup: Vec<&[u8]> = expected.clone();
            dedup.dedup();
            assert_eq!(list.iter().collect::<Vec<_>>(), dedup);
        }
    }
    if let Ok(entries) = new.tracked_with_modes() {
        assert_eq!(entries.paths.len(), entries.modes.len());
    }
    if let Ok(entries) = new.staged_with_modes() {
        assert_eq!(entries.paths.len(), entries.modes.len());
    }
    if let Ok(pairs) = new.config_get_regexp("^x") {
        assert!(pairs.len() <= expected.len());
    }
    if let Ok(files) = new.config_origins() {
        assert!(files.len() <= expected.len());
    }
});
