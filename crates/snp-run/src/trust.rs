//! Step 15 of a `--all-files` run whose filters allow it: trusted index modes instead of an `lstat` per file
//! (ARCHITECTURE §2.3, ADR-106, amending ADR-041).
//!
//! Three read-only git queries run at once: `git ls-files -s -v -f -z` (the `all_files` list with each entry's mode
//! and flags), pre-commit's `prior_diff` (`git diff`, taken here instead of lazily before the first hook) and the
//! configuration gate, resolved by git. When the diff is empty and clean (exit 0, nothing on stderr), no entry is
//! flagged or unmerged, and the configuration lets git compare kinds, `git diff` has `lstat`ed every unflagged entry
//! and found it with its index kind: the file table answers `os.path.lexists` and the stat tier from the modes
//! (never the executable bit), and the diff oracle clears that trust after every hook that may have changed the
//! worktree. Any failure falls back to today's path at today's position: the listing to `ls-files -z
//! --deduplicate`, the diff to the lazy one before the first hook.
//!
//! Owner: WP-32.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use snp_files::FileTable;
use snp_git::{DiffFingerprint, Git};

use crate::common;
use crate::files::{self, Selection};

/// What the trust-capable step 15 produced.
#[derive(Debug)]
pub(crate) struct AllFiles {
    /// The file table of the tagged listing; `None` when that listing failed (the caller then asks `git ls-files -z
    /// --deduplicate` at the same position, so a failure reports pre-commit's own error).
    pub(crate) table: Option<FileTable>,
    /// The before-diff's fingerprint, the oracle's baseline; `None` when it failed (the lazy before-diff then reports
    /// it where it does today).
    pub(crate) baseline: Option<DiffFingerprint>,
    /// The flag the table's trust depends on, for the diff oracle to guard; `None` when nothing is trusted.
    pub(crate) trust: Option<Arc<AtomicBool>>,
}

/// Whether `files::table` takes its `--all-files` branch (first match wins: a no-file stage, a commit-msg stage,
/// `--from-ref`/`--to-ref` and `--files` come first, `run.py:_all_filenames`).
pub(crate) fn all_files_branch(sel: &Selection<'_>) -> bool {
    sel.all_files
        && sel.files.is_empty()
        && sel.refs.is_none()
        && !files::no_file_stage(sel.stage)
        && !files::commit_msg_stage(sel.stage)
}

/// The tagged listing, the before-diff and the configuration gate, in parallel (each a read-only git child; `git
/// diff` may refresh the index's stat data, which no listing field depends on). `trust` sets the table's trusted
/// modes (`FileTable::trust_index_modes` with the run's referenced tags) and is called only when the gate holds:
/// the listing is plain, the diff is empty and clean, and git's configuration allows it.
pub(crate) fn all_files(
    git: &Git,
    trust: impl FnOnce(&mut FileTable, Box<[u32]>, Arc<AtomicBool>) -> bool,
) -> AllFiles {
    let (tagged, diff, config) = std::thread::scope(|s| {
        let tagged = common::fork(s, || git.all_files_tagged().ok());
        let config = common::fork(s, || git.trusts_index_modes());
        let diff = git.diff_fingerprint_clean().ok();
        // a thread that panicked costs the trust (or the listing: `ls-files -z --deduplicate` is asked instead)
        (tagged.join(|_| None), diff, config.join(|_| false))
    });
    let baseline = diff.map(|(fp, _)| fp);
    let Some(tagged) = tagged else {
        return AllFiles {
            table: None,
            baseline,
            trust: None,
        };
    };
    let proven = tagged.plain
        && config
        && diff.is_some_and(|(fp, clean)| clean && fp == DiffFingerprint::EMPTY);
    let (buf, spans) = tagged.paths.into_parts();
    let mut table = FileTable::from_zbuf(buf, spans, git.root());
    let flag = Arc::new(AtomicBool::new(true));
    let trusted = proven && trust(&mut table, tagged.modes, Arc::clone(&flag));
    AllFiles {
        table: Some(table),
        baseline,
        trust: trusted.then_some(flag),
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use snp_core::env::Env;
    use snp_files::{FileId, Needs, TagOutcome};
    use snp_git::{GitExe, ProcBackend};
    use snp_proc::spawn::Role;

    use super::*;

    /// xorshift64*: deterministic randomness.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }

        fn below(&mut self, n: usize) -> usize {
            usize::try_from(self.next() % u64::try_from(n).unwrap()).unwrap()
        }
    }

    fn env(home: &Path) -> Env {
        Env::from_pairs([
            ("PATH", "/usr/local/bin:/usr/bin:/bin"),
            ("HOME", home.to_str().unwrap()),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_AUTHOR_NAME", "a"),
            ("GIT_AUTHOR_EMAIL", "a@b"),
            ("GIT_COMMITTER_NAME", "a"),
            ("GIT_COMMITTER_EMAIL", "a@b"),
            ("LANG", "C.UTF-8"),
        ])
    }

    fn run(root: &Path, env: &Env, args: &[&str]) -> bool {
        Command::new("git")
            .args(args)
            .current_dir(root)
            .env_clear()
            .envs(env.iter())
            .output()
            .unwrap()
            .status
            .success()
    }

    /// The template's tracked files: names with and without name tags, nested directories, an executable, a
    /// shebang without an extension, binary content and an empty file (the empty blob); plus a symlink and a gitlink.
    const FILES: [(&str, &[u8]); 9] = [
        ("a.py", b"print(1)\n"),
        ("b.txt", b"text\n"),
        ("run.sh", b"#!/bin/sh\necho\n"),
        ("tool", b"#!/usr/bin/env python3\nprint(2)\n"),
        ("data.unknownext", b"\x00\x01binary"),
        ("empty", b""),
        ("d1/x.js", b"x;\n"),
        ("d1/d2/y.md", b"# y\n"),
        ("d1/z", b"zzz\n"),
    ];

    const TRACKED: [&str; 11] = [
        "a.py",
        "b.txt",
        "run.sh",
        "tool",
        "data.unknownext",
        "empty",
        "d1/x.js",
        "d1/d2/y.md",
        "d1/z",
        "link",
        "sub",
    ];

    fn template(root: &Path, env: &Env) {
        std::fs::create_dir_all(root).unwrap();
        assert!(run(root, env, &["init", "-q", "-b", "main"]));
        for (name, content) in FILES {
            let p = root.join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, content).unwrap();
        }
        std::fs::set_permissions(root.join("run.sh"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        std::os::unix::fs::symlink("b.txt", root.join("link")).unwrap();
        assert!(run(root, env, &["add", "."]));
        // an unpopulated submodule: an empty directory at the gitlink's path
        std::fs::create_dir_all(root.join("sub")).unwrap();
        assert!(run(
            root,
            env,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                "160000,78981922613b2afb6025042ff6bd878ac1994e85,sub",
            ]
        ));
        assert!(run(
            root,
            env,
            &["commit", "-q", "--no-verify", "-m", "init"]
        ));
    }

    fn remove(p: &Path) {
        drop(std::fs::remove_file(p));
        drop(std::fs::remove_dir_all(p));
    }

    /// One random worktree or index change; returns its name.
    fn mutate(rng: &mut Rng, root: &Path, env: &Env, as_root: bool) -> &'static str {
        let name = TRACKED[rng.below(TRACKED.len())];
        let p = root.join(name);
        match rng.below(17) {
            0 => {
                let mode = [0o644, 0o755, 0o654, 0o600, 0o000, 0o711, 0o744][rng.below(7)];
                drop(std::fs::set_permissions(
                    &p,
                    std::fs::Permissions::from_mode(mode),
                ));
                "chmod"
            }
            1 => {
                remove(&p);
                "rm"
            }
            2 => {
                remove(&p);
                assert!(Command::new("mkfifo").arg(&p).status().unwrap().success());
                "mkfifo"
            }
            3 => {
                // a socket with a regular file's permission bits: over the empty file `git diff` stays clean, which
                // is why an empty blob is never trusted
                let target = if rng.below(2) == 0 {
                    root.join("empty")
                } else {
                    p
                };
                remove(&target);
                drop(std::os::unix::net::UnixListener::bind(&target).unwrap());
                let mode = [0o644, 0o600, 0o755][rng.below(3)];
                std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode)).unwrap();
                "socket"
            }
            4 => {
                remove(&p);
                let to = ["b.txt", "missing", "d1", "a.py"][rng.below(4)];
                std::os::unix::fs::symlink(to, &p).unwrap();
                "symlink swap"
            }
            5 => {
                remove(&p);
                std::fs::create_dir_all(&p).unwrap();
                std::fs::write(p.join("inner"), b"i").unwrap();
                "file to directory"
            }
            6 if !as_root => {
                let d = root.join(["d1", "d1/d2"][rng.below(2)]);
                drop(std::fs::set_permissions(
                    &d,
                    std::fs::Permissions::from_mode(0o000),
                ));
                "unsearchable directory"
            }
            7 => {
                run(root, env, &["update-index", "--skip-worktree", name]);
                if rng.below(2) == 0 {
                    remove(&p);
                }
                "skip-worktree"
            }
            8 => {
                run(root, env, &["update-index", "--assume-unchanged", name]);
                if rng.below(2) == 0 {
                    remove(&p);
                }
                "assume-unchanged"
            }
            9 => {
                drop(std::fs::write(&p, b"changed\n"));
                "content"
            }
            10 => {
                std::fs::write(root.join("new.py"), b"n\n").unwrap();
                run(root, env, &["add", "-N", "new.py"]);
                "intent-to-add"
            }
            11 => {
                run(root, env, &["rm", "-q", "--cached", "-r", name]);
                "rm --cached"
            }
            12 => {
                // regular files only: opening a FIFO for writing would wait for a reader
                if std::fs::symlink_metadata(&p).is_ok_and(|m| m.is_file())
                    && let Ok(f) = std::fs::File::options().append(true).open(&p)
                {
                    drop(f.set_modified(std::time::SystemTime::now()));
                }
                "touch"
            }
            13 => {
                // the leading directory replaced by a symlink to a copy holding the same names
                drop(std::fs::rename(root.join("d1"), root.join("d1copy")));
                drop(std::os::unix::fs::symlink("d1copy", root.join("d1")));
                "leading symlink"
            }
            14 => {
                std::fs::write(root.join("untracked.py"), b"u\n").unwrap();
                "untracked file"
            }
            15 => {
                // `core.symlinks=false`: git checks a symlink out as a file holding its target and keeps comparing
                // it as a symlink, so `git diff` stays clean while `lstat` sees a regular file
                run(root, env, &["config", "core.symlinks", "false"]);
                remove(&root.join("link"));
                std::fs::write(root.join("link"), b"b.txt").unwrap();
                "core.symlinks=false"
            }
            _ => "none",
        }
    }

    /// Restores directory permissions so the tree can be removed.
    fn unlock(p: &Path) {
        if std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()) {
            drop(std::fs::set_permissions(
                p,
                std::fs::Permissions::from_mode(0o755),
            ));
            for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
                unlock(&e.path());
            }
        }
    }

    /// An outcome as text, without the executable-bit tags (never trusted, so never compared).
    fn shown(o: TagOutcome<'_>) -> String {
        match o {
            TagOutcome::Tags(t) => t
                .names()
                .filter(|n| !matches!(*n, "executable" | "non-executable"))
                .collect::<Vec<_>>()
                .join(","),
            TagOutcome::Vanished => "<vanished>".to_owned(),
            TagOutcome::Unreadable(e) => {
                format!("<unreadable {}>", String::from_utf8_lossy(&e.text))
            }
            _ => "<other>".to_owned(),
        }
    }

    /// ADR-106's randomized soundness test: whenever the production gate ([`all_files`]: a plain listing, an empty
    /// and clean `git diff`, git's configuration) sets the trust, existence and every stat-, encoding- and
    /// shebang-tier outcome equal what `lstat` gives (the executable-bit tags excepted: they are never taken from a
    /// mode), over chmod, rm, mkfifo, sockets, an unsearchable directory, symlink swaps, file to directory,
    /// skip-worktree, assume-unchanged, intent-to-add, `rm --cached`, touch, a leading symlink and untracked files.
    /// The gate must both hold and fail often enough for the test to mean something.
    #[test]
    fn trusted_modes_equal_lstat_whenever_the_gate_passes() {
        let tmp = tempfile::tempdir().unwrap();
        let base = snp_pycompat::ospath::realpath(tmp.path().as_os_str(), Path::new("/")).unwrap();
        let home = base.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let env = env(&home);
        let as_root = std::fs::metadata(&home).is_ok_and(|m| m.uid() == 0);
        let iterations: u64 = Env::capture_process()
            .get_nonempty("SNP_TRUST_ITERATIONS")
            .and_then(|v| v.to_str()?.parse().ok())
            .unwrap_or(300);
        let needs = [
            Needs {
                stat: true,
                ..Needs::default()
            },
            Needs {
                stat: true,
                encoding: true,
                shebang: false,
            },
            Needs {
                stat: true,
                encoding: true,
                shebang: true,
            },
        ];
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let (mut trusted, mut refused) = (0_u64, 0_u64);
        let mut refused_by: std::collections::BTreeMap<&str, u64> =
            std::collections::BTreeMap::new();
        for it in 0..iterations {
            let root: PathBuf = base.join(format!("r{it}"));
            template(&root, &env);
            let mut applied = Vec::new();
            for _ in 0..rng.below(3) {
                applied.push(mutate(&mut rng, &root, &env, as_root));
            }
            let exe = Arc::new(GitExe::resolve(&env, &root, None).unwrap());
            let backend = Arc::new(ProcBackend {
                cancel: snp_core::cancel::CancelToken::new(),
                role: Role::Tool,
            });
            let (git, _) = Git::discover(exe, &env, &root, backend).unwrap();
            // the untrusted reference: the same list, an `lstat` for every file
            let (buf, spans) = git.all_files().unwrap().into_parts();
            let reference = FileTable::from_zbuf(buf, spans, &root);
            // the run's referenced tags: `python` and `text` (a shebang filter) or none (the default `[file]`
            // references no name tag); a name-tier answer is the one way to hold a tag set here
            let python = (0..reference.len())
                .map(|i| FileId(u32::try_from(i).unwrap()))
                .find(|&id| reference.bytes(id) == b"a.py")
                .map(|id| reference.tags(id, Needs::default()));
            let shebang_filter = rng.below(2) == 0;
            let early = all_files(&git, |table, modes, flag| match python {
                Some(TagOutcome::Tags(py)) => {
                    let referenced = if shebang_filter {
                        py
                    } else {
                        py.difference(py)
                    };
                    table.trust_index_modes(modes, flag, referenced)
                }
                _ => false,
            });
            let table = early.table.expect("the tagged listing parses");
            if early.trust.is_none() {
                refused = refused.saturating_add(1);
                for m in &applied {
                    *refused_by.entry(m).or_default() += 1;
                }
                unlock(&root);
                continue;
            }
            trusted = trusted.saturating_add(1);
            assert_eq!(table.len(), reference.len(), "{applied:?}");
            for i in 0..reference.len() {
                let id = FileId(u32::try_from(i).unwrap());
                assert_eq!(table.bytes(id), reference.bytes(id));
                let name = String::from_utf8_lossy(reference.bytes(id)).into_owned();
                for n in needs {
                    assert_eq!(
                        shown(table.tags(id, n)),
                        shown(reference.tags(id, n)),
                        "iteration {it}, {applied:?}: {name} with {n:?}"
                    );
                }
            }
            unlock(&root);
        }
        let total = trusted.saturating_add(refused);
        assert!(
            trusted.saturating_mul(5) >= total && refused.saturating_mul(5) >= total,
            "trusted {trusted}, refused {refused} of {total}: {refused_by:?}"
        );
    }
}
