//! Fuzz target `git_pre_push`: pre-push stdin parsing.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Drives `Git::pre_push` (02 §16.3, ADR-022) end to end with an in-process backend: the first input byte scripts
//! git's answers (`cat-file -e` exit status, what `rev-list` prints, whether a call fails), the rest is the hook's
//! stdin. Checked: no panic; the only errors are the unpack `ValueError` (exit 3) and the scripted
//! `CalledProcessError`; a namespace never carries a null (all-zero) `to_ref`.
//!
//! Owner: WP-18.
#![no_main]

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::Path;
use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use snp_core::env::Env;
use snp_core::failure::Failure;
use snp_git::{Git, GitBackend, GitCaps, GitExe, GitVersion};
use snp_proc::spawn::{Captured, Group, PyStatus, SpawnError, Stdin};

#[derive(Debug)]
struct Scripted {
    knobs: u8,
}

const ROOT: &[u8] = b"1111111111111111111111111111111111111111";
const OTHER: &[u8] = b"2222222222222222222222222222222222222222";

impl Scripted {
    fn reply(&self, argv: &[OsString]) -> (i64, Vec<u8>) {
        let has = |s: &str| argv.iter().any(|a| a == s);
        if has("rev-parse") && has("--show-cdup") {
            return (0, b"\nfalse\n/r/.git\n/r/.git\n/r/.git/index\n".to_vec());
        }
        if has("--max-parents=0") {
            return if self.knobs & 0x10 != 0 { (128, Vec::new()) } else { (0, [ROOT, b"\n"].concat()) };
        }
        if has("rev-list") {
            return match self.knobs & 0x0c {
                0x00 => (0, Vec::new()),
                0x04 => (0, [ROOT, b"\n", OTHER, b"\n"].concat()),
                0x08 => (0, [OTHER, b"\n", ROOT, b"\n"].concat()),
                _ => (128, Vec::new()),
            };
        }
        if has("rev-parse") {
            return if self.knobs & 0x20 != 0 { (128, Vec::new()) } else { (0, [ROOT, b"\n"].concat()) };
        }
        (1, Vec::new())
    }
}

impl GitBackend for Scripted {
    fn capture(&self, argv: &[OsString], _: &Env, _: &Path, _: Stdin, _: Group) -> Result<Captured, SpawnError> {
        let (code, stdout) = self.reply(argv);
        Ok(Captured { status: PyStatus(code), stdout, stderr: b"fatal: bad object\n".to_vec(), stopped_by_tty: false })
    }
    fn stream(&self, argv: &[OsString], _: &Env, _: &Path, sink: &mut dyn io::Write) -> Result<(PyStatus, Vec<u8>), SpawnError> {
        let (code, stdout) = self.reply(argv);
        sink.write_all(&stdout).map_err(|io| SpawnError { program: OsString::from("git"), io })?;
        Ok((PyStatus(code), Vec::new()))
    }
    fn inherit(&self, _argv: &[OsString], _: &Env, _: &Path, _: Stdin) -> Result<PyStatus, SpawnError> {
        // `cat-file -e <remote>^{commit}`
        Ok(PyStatus(i64::from(self.knobs & 0x01)))
    }
}

fuzz_target!(|data: &[u8]| {
    let Some((&knobs, stdin)) = data.split_first() else { return };
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
    let (git, _) = Git::discover(exe, &env, Path::new("/r"), Arc::new(Scripted { knobs }))
        .expect("canned discovery");
    match git.pre_push(stdin, OsStr::new("origin")) {
        Ok(Some(ns)) => {
            if let Some(to) = &ns.to_ref {
                let b = to.as_encoded_bytes();
                assert!(!(matches!(b.len(), 40 | 64) && b.iter().all(|&c| c == b'0')), "null to_ref");
            }
            assert_eq!(ns.all_files, ns.from_ref.is_none());
        }
        Ok(None) => {}
        Err(Failure::Exc { exc, .. }) => {
            assert!(exc.ty == "ValueError" || exc.ty == "CalledProcessError", "{}", exc.ty);
        }
        Err(other) => panic!("unexpected failure {other:?}"),
    }
});
