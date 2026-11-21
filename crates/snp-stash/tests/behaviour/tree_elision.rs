//! ADR-153: the stash's `write-tree` and `diff-index` decided in-process. Every case builds the same repository twice
//! and runs the stash once with the elision armed (step 6's configuration answer given) and once without, then
//! compares what pre-commit could observe: the outcome (nothing, or the patch's bytes, and the clean flag), the
//! error, stdout and stderr, the index's entries and cache-tree afterwards, the object store and the absence of
//! `index.lock`. The spy shows which git commands ran. Every case runs with the git on `PATH` and every `bin`
//! directory of `SNP_TEST_GIT_MATRIX` (`:`-separated; locally 2.18.5 and 2.56.0 beside the system git).

#![cfg(unix)]

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use snp_core::env::{Env, EnvPatch};
use snp_git::index_file::IndexFile;
use snp_stash::Stashed;

use crate::common::{Sandbox, Spy};

/// The sandboxes to run in: the git on `PATH`, then each `SNP_TEST_GIT_MATRIX` directory first on `PATH`.
fn sandboxes() -> Vec<(String, Sandbox)> {
    let process = Env::capture_process();
    let mut dirs: Vec<Option<PathBuf>> = vec![None];
    if let Some(m) = process.get("SNP_TEST_GIT_MATRIX") {
        dirs.extend(std::env::split_paths(m).map(Some));
    }
    dirs.into_iter()
        .map(|dir| {
            let mut sb = Sandbox::new();
            if let Some(d) = dir {
                let mut path = OsString::from(d.as_os_str());
                path.push(":");
                path.push(sb.env.get("PATH").unwrap_or_default());
                sb.env = sb.env.patched(&EnvPatch::default().set("PATH", &path));
            }
            let version = sb.git_text(&sb.root, &["--version"]);
            (version, sb)
        })
        .collect()
}

fn safe(repo: &Path) -> bool {
    snp_os::fsinfo::stat_oracle_safe(repo).unwrap_or(false)
}

/// Writes `files` (relative name, bytes) with an mtime 10 s in the past, so no entry is racy.
fn write_old(sb: &Sandbox, repo: &Path, files: &[(&str, &[u8])]) {
    let past = SystemTime::now() - Duration::from_secs(10);
    for (name, bytes) in files {
        sb.write(repo, name, bytes);
        std::fs::File::options()
            .write(true)
            .open(repo.join(name))
            .unwrap()
            .set_modified(past)
            .unwrap();
    }
}

/// A committed repository with a valid cache-tree and every object loose: `a`, `b`, `d/c`, `d/e/f`.
fn committed(sb: &Sandbox, name: &str) -> PathBuf {
    let repo = sb.init(name);
    write_old(
        sb,
        &repo,
        &[
            ("a", b"a\n"),
            ("b", b"b\n"),
            ("d/c", b"c\n"),
            ("d/e/f", b"f\n"),
        ],
    );
    sb.git(&repo, &["add", "."]);
    sb.commit(&repo, "init");
    repo
}

/// What one stash run showed.
#[derive(Debug, PartialEq, Eq)]
struct Seen {
    /// `None`: nothing stashed; `Some(bytes)`: the patch.
    patch: Option<Vec<u8>>,
    clean: bool,
    error: Option<String>,
    stdout: String,
    stderr: String,
    entries: Vec<(Vec<u8>, u32, Vec<u8>)>,
    tree: Option<Vec<u8>>,
    /// Objects the run added, and objects it removed.
    objects: (BTreeSet<String>, BTreeSet<String>),
    lock_left: bool,
    worktree: Vec<u8>,
}

/// The git commands of a run.
struct Run {
    seen: Seen,
    calls: Vec<String>,
}

impl Run {
    fn ran(&self, cmd: &str) -> usize {
        self.calls.iter().filter(|c| c.starts_with(cmd)).count()
    }
}

fn objects(repo: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let dir = repo.join(".git/objects");
    for d in std::fs::read_dir(&dir).unwrap() {
        let d = d.unwrap();
        let n = d.file_name().to_string_lossy().into_owned();
        if n.len() == 2 {
            for f in std::fs::read_dir(d.path()).unwrap() {
                out.insert(format!("{n}{}", f.unwrap().file_name().to_string_lossy()));
            }
        } else if n == "pack" {
            for f in std::fs::read_dir(d.path()).unwrap() {
                out.insert(format!("pack/{}", f.unwrap().file_name().to_string_lossy()));
            }
        }
    }
    out
}

/// A failure as text (an exception's type, text and hint).
fn shown_failure(f: &snp_core::failure::Failure) -> String {
    match f {
        snp_core::failure::Failure::Exc { exc, .. } => format!(
            "{}: {} {:?}",
            exc.ty,
            String::from_utf8_lossy(&exc.text),
            exc.hint
        ),
        other => format!("{other:?}"),
    }
}

/// Runs the stash on `repo`, armed or not, with `extra` in the run environment.
fn run(sb: &Sandbox, repo: &Path, armed: bool, extra: &[(&'static str, &str)]) -> Run {
    let spy = Spy::new();
    let (git, info) = sb.discover_with(repo, Arc::<Spy>::clone(&spy));
    let mut patch = EnvPatch::default();
    for (k, v) in extra {
        patch = patch.set(*k, *v);
    }
    let git = git.with_env(git.env().patched(&patch));
    let listing = armed.then(|| Arc::new(git.config_listing().unwrap()));
    let before = spy.calls().len();
    let objects_before = objects(repo);
    let (mut cx, out, err) = sb.ctx_with(git, info);
    cx.config_listing = listing;
    let mut patch_path = None;
    let (patch, clean, error) = match snp_stash::enter(cx) {
        Ok(g) => {
            let patch = match g.stashed() {
                Stashed::Patch(p) => {
                    patch_path = Some(p.to_string_lossy().into_owned());
                    Some(std::fs::read(p).unwrap())
                }
                _ => None,
            };
            let clean = g.diff_index_clean();
            let restored = g
                .restore()
                .err()
                .map(|f| format!("restore: {}", shown_failure(&f)));
            (patch, clean, restored)
        }
        Err(f) => (None, false, Some(shown_failure(&f))),
    };
    let objects_after = objects(repo);
    let calls: Vec<String> = spy.calls().into_iter().skip(before).collect();
    let ix = IndexFile::read_keep(&repo.join(".git/index"), 20).unwrap();
    let entries = ix
        .entries
        .iter()
        .map(|e| (ix.name(e).to_vec(), e.mode, ix.oid(e).unwrap().to_vec()))
        .collect();
    let shown = repo.to_string_lossy().into_owned();
    let norm = |s: String| {
        let s = s.replace(&shown, "<repo>");
        match &patch_path {
            Some(p) => s.replace(p, "<patch>"),
            None => s,
        }
    };
    let mut worktree = Vec::new();
    for f in ["a", "b", "d/c", "d/e/f"] {
        worktree.extend(std::fs::read(repo.join(f)).unwrap_or_default());
        worktree.push(0);
    }
    Run {
        seen: Seen {
            patch,
            clean,
            error: error.map(norm),
            stdout: norm(out.text()),
            stderr: norm(err.text()),
            entries,
            tree: ix.tree_extension().map(<[u8]>::to_vec),
            objects: (
                objects_after.difference(&objects_before).cloned().collect(),
                objects_before.difference(&objects_after).cloned().collect(),
            ),
            lock_left: repo.join(".git/index.lock").exists(),
            worktree,
        },
        calls,
    }
}

/// Builds the repository twice with `build`, runs the armed and the plain stash, and checks they agree; returns
/// both runs (armed first).
fn differential(sb: &Sandbox, name: &str, build: impl Fn(&Sandbox, &str) -> PathBuf) -> (Run, Run) {
    let armed_repo = build(sb, &format!("{name}-armed"));
    let plain_repo = build(sb, &format!("{name}-plain"));
    let armed = run(sb, &armed_repo, true, &[]);
    let plain = run(sb, &plain_repo, false, &[]);
    assert_eq!(
        armed.seen, plain.seen,
        "{name}: {:?} vs {:?}",
        armed.calls, plain.calls
    );
    assert_eq!(plain.ran("write-tree"), 1, "{name}: {:?}", plain.calls);
    (armed, plain)
}

/// B2's path: a clean, committed repository. Neither `write-tree` nor `diff-index` runs; the index is untouched and
/// no `index.lock` is left.
#[test]
fn a_clean_repository_skips_both_commands() {
    for (v, sb) in sandboxes() {
        if !safe(&sb.root) {
            return;
        }
        let (armed, plain) = differential(&sb, "clean", committed);
        assert!(armed.seen.clean && armed.seen.patch.is_none(), "{v}");
        assert_eq!(armed.ran("write-tree"), 0, "{v}: {:?}", armed.calls);
        assert_eq!(armed.ran("diff-index"), 0, "{v}: {:?}", armed.calls);
        assert_eq!(plain.ran("diff-index"), 1, "{v}");
    }
}

/// B8's path: an unstaged change. `write-tree` is skipped (the cache-tree is valid), `diff-index` runs and the patch
/// is the same.
#[test]
fn an_unstaged_change_skips_only_write_tree() {
    for (v, sb) in sandboxes() {
        if !safe(&sb.root) {
            return;
        }
        let (armed, plain) = differential(&sb, "dirty", |sb, name| {
            let repo = committed(sb, name);
            sb.write(&repo, "d/c", b"changed\n");
            repo
        });
        assert!(armed.seen.patch.is_some(), "{v}");
        assert_eq!(armed.ran("write-tree"), 0, "{v}: {:?}", armed.calls);
        assert_eq!(armed.ran("diff-index"), plain.ran("diff-index"), "{v}");
    }
}

/// A staged change: the cache-tree is invalid, so `write-tree` runs (and writes the new tree objects and the
/// cache-tree); the proof then holds on the index it left, so `diff-index` is skipped.
#[test]
fn a_staged_change_runs_write_tree_then_proves_the_diff() {
    for (v, sb) in sandboxes() {
        if !safe(&sb.root) {
            return;
        }
        let (armed, _) = differential(&sb, "staged", |sb, name| {
            let repo = committed(sb, name);
            write_old(sb, &repo, &[("d/e/f", b"new\n")]);
            sb.git(&repo, &["add", "d/e/f"]);
            repo
        });
        assert!(armed.seen.clean, "{v}");
        assert_eq!(armed.ran("write-tree"), 1, "{v}: {:?}", armed.calls);
        assert_eq!(armed.ran("diff-index"), 0, "{v}: {:?}", armed.calls);
    }
}

/// A touched file (stat data changed, content equal) and a racy index: the stash's intent-to-add query may refresh
/// the index first (then `write-tree` runs), the proof refuses what stays in doubt, and the outcome is the same.
#[test]
fn stat_doubts_keep_the_outcome() {
    for (v, sb) in sandboxes() {
        if !safe(&sb.root) {
            return;
        }
        let (armed, _) = differential(&sb, "touched", |sb, name| {
            let repo = committed(sb, name);
            std::fs::File::options()
                .write(true)
                .open(repo.join("b"))
                .unwrap()
                .set_modified(SystemTime::now() - Duration::from_secs(5))
                .unwrap();
            repo
        });
        assert!(armed.seen.error.is_none(), "{v}");
        let (armed, _) = differential(&sb, "racy", |sb, name| {
            let repo = sb.init(name);
            sb.write(&repo, "a", b"a\n");
            sb.git(&repo, &["add", "a"]);
            sb.commit(&repo, "init");
            repo
        });
        assert!(armed.seen.error.is_none(), "{v}");
    }
}

/// Replaces the `TREE` extension of `repo`'s index with `tree` and fixes the trailing checksum.
fn put_tree_extension(repo: &Path, tree: &[u8]) {
    let path = repo.join(".git/index");
    let img = std::fs::read(&path).unwrap();
    let at = (0..img.len() - 3)
        .find(|&i| &img[i..i + 4] == b"TREE")
        .unwrap();
    let len = usize::try_from(u32::from_be_bytes(img[at + 4..at + 8].try_into().unwrap())).unwrap();
    let mut out = img[..at].to_vec();
    out.extend_from_slice(b"TREE");
    out.extend_from_slice(&u32::try_from(tree.len()).unwrap().to_be_bytes());
    out.extend_from_slice(tree);
    out.extend_from_slice(&img[at + 8 + len..img.len() - 20]);
    let sum = snp_git::testing::sha1(&out);
    out.extend_from_slice(&sum);
    std::fs::write(&path, out).unwrap();
}

/// deep-b2 fact 2: a stale cache-tree that still claims to be valid. `write-tree` prints its (old) root, and
/// `diff-index` against it reports the change: the byte comparison refuses the proof, so the real `diff-index`
/// runs and the stash saves the same patch.
#[test]
fn a_stale_cache_tree_is_refused_by_the_byte_comparison() {
    for (v, sb) in sandboxes() {
        if !safe(&sb.root) {
            return;
        }
        let (armed, plain) = differential(&sb, "stale", |sb, name| {
            let repo = committed(sb, name);
            let ix = IndexFile::read_keep(&repo.join(".git/index"), 20).unwrap();
            let old = ix.tree_extension().unwrap().to_vec();
            // the entry changes (and the cache-tree is invalidated); then the old, valid cache-tree goes back in
            write_old(sb, &repo, &[("a", b"other\n")]);
            sb.git(&repo, &["add", "a"]);
            put_tree_extension(&repo, &old);
            repo
        });
        assert!(
            plain.seen.patch.is_some(),
            "{v}: diff-index sees the stale tree"
        );
        assert_eq!(
            armed.ran("write-tree"),
            0,
            "{v}: write-tree trusts the cache-tree"
        );
        assert_eq!(
            armed.ran("diff-index"),
            plain.ran("diff-index"),
            "{v}: {:?}",
            armed.calls
        );
    }
}

/// A corrupt root tree object: `write-tree` still prints the root (it only checks existence), `diff-index` fails;
/// the strict read refuses the proof, so the run fails as pre-commit's does.
#[test]
fn a_corrupt_tree_object_fails_as_git_does() {
    for (v, sb) in sandboxes() {
        if !safe(&sb.root) {
            return;
        }
        let (armed, plain) = differential(&sb, "corrupt", |sb, name| {
            let repo = committed(sb, name);
            let root = sb.git_text(&repo, &["rev-parse", "HEAD^{tree}"]);
            let (dir, file) = root.split_at_checked(2).unwrap();
            let p = repo.join(".git/objects").join(dir).join(file);
            let z = std::fs::read(&p).unwrap();
            std::fs::remove_file(&p).unwrap();
            std::fs::write(&p, &z[..z.len() - 4]).unwrap();
            repo
        });
        assert!(plain.seen.error.is_some(), "{v}");
        assert_eq!(armed.ran("write-tree"), 0, "{v}");
        assert_eq!(armed.ran("diff-index"), 1, "{v}");
    }
}

/// Packed objects and an alternate store: git may read the trees elsewhere, so both commands run.
#[test]
fn packs_run_the_real_commands() {
    for (v, sb) in sandboxes() {
        if !safe(&sb.root) {
            return;
        }
        let (armed, _) = differential(&sb, "packed", |sb, name| {
            let repo = committed(sb, name);
            sb.git(&repo, &["repack", "-a", "-d", "-q"]);
            sb.git(&repo, &["prune-packed"]);
            repo
        });
        assert_eq!(armed.ran("write-tree"), 1, "{v}: {:?}", armed.calls);
        assert_eq!(armed.ran("diff-index"), 1, "{v}: {:?}", armed.calls);
        // a pack beside the loose copies: write-tree is still skipped (the loose copy exists), diff-index is not
        let (armed, _) = differential(&sb, "pack-beside", |sb, name| {
            let repo = committed(sb, name);
            sb.git(&repo, &["repack", "-a", "-q"]);
            repo
        });
        assert_eq!(armed.ran("write-tree"), 0, "{v}: {:?}", armed.calls);
        assert_eq!(armed.ran("diff-index"), 1, "{v}: {:?}", armed.calls);
    }
}

/// A held `index.lock`: the probe fails, the real `write-tree` runs and fails with git's message, and the lock is
/// left alone.
#[test]
fn a_held_index_lock_fails_with_gits_error() {
    for (v, sb) in sandboxes() {
        if !safe(&sb.root) {
            return;
        }
        let (armed, plain) = differential(&sb, "locked", |sb, name| {
            let repo = committed(sb, name);
            std::fs::write(repo.join(".git/index.lock"), b"").unwrap();
            repo
        });
        assert!(
            plain
                .seen
                .error
                .as_deref()
                .is_some_and(|e| e.contains("index.lock")),
            "{v}"
        );
        assert!(armed.seen.lock_left, "{v}");
        assert_eq!(armed.ran("write-tree"), 1, "{v}");
    }
}

/// An index that is a symbolic link: git locks (and discovery reports) its target, so the probe takes the lock git
/// takes, and the outcome is the same; no lock is left on either name.
#[test]
fn a_symlinked_index_is_locked_at_its_target() {
    for (v, sb) in sandboxes() {
        if !safe(&sb.root) {
            return;
        }
        let (armed, _) = differential(&sb, "symlinked", |sb, name| {
            let repo = committed(sb, name);
            std::fs::rename(repo.join(".git/index"), repo.join(".git/index.real")).unwrap();
            std::os::unix::fs::symlink("index.real", repo.join(".git/index")).unwrap();
            repo
        });
        assert!(armed.seen.clean, "{v}");
        let repo = sb.path("symlinked-armed");
        assert!(!repo.join(".git/index.real.lock").exists(), "{v}");
        assert!(
            std::fs::symlink_metadata(repo.join(".git/index"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "{v}"
        );
    }
}

/// Skip-worktree, assume-unchanged and gitlink entries keep `write-tree`'s elision but refuse the proof.
#[test]
fn skipped_entries_refuse_the_proof() {
    for (v, sb) in sandboxes() {
        if !safe(&sb.root) {
            return;
        }
        for flag in ["--assume-unchanged", "--skip-worktree"] {
            let (armed, _) = differential(&sb, &format!("flag{flag}"), |sb, name| {
                let repo = committed(sb, name);
                sb.git(&repo, &["update-index", flag, "b"]);
                repo
            });
            assert_eq!(armed.ran("diff-index"), 1, "{v} {flag}: {:?}", armed.calls);
        }
    }
}

/// `SNP_VERIFY_STASH_ELISION=1`: the real commands run as well, agree, and nothing is reported.
#[test]
fn the_verify_mode_runs_the_real_commands_too() {
    for (v, sb) in sandboxes() {
        if !safe(&sb.root) {
            return;
        }
        let repo = committed(&sb, "verify");
        let r = run(&sb, &repo, true, &[("SNP_VERIFY_STASH_ELISION", "1")]);
        assert_eq!(r.ran("write-tree"), 1, "{v}: {:?}", r.calls);
        assert_eq!(r.ran("diff-index"), 1, "{v}: {:?}", r.calls);
        assert!(r.seen.clean, "{v}");
        assert_eq!(r.seen.stderr, "", "{v}");
    }
}

/// `SNP_DIFF_MODE=git` and a `GIT_TEST_*` variable turn the elision off.
#[test]
fn environment_gates() {
    for (v, sb) in sandboxes() {
        if !safe(&sb.root) {
            return;
        }
        for (k, val) in [("SNP_DIFF_MODE", "git"), ("GIT_TEST_SPLIT_INDEX", "0")] {
            let repo = committed(&sb, &format!("env-{k}"));
            let r = run(&sb, &repo, true, &[(k, val)]);
            assert_eq!(r.ran("write-tree"), 1, "{v} {k}: {:?}", r.calls);
            assert_eq!(r.ran("diff-index"), 1, "{v} {k}: {:?}", r.calls);
        }
    }
}

/// `git commit -a` runs the hook with `GIT_INDEX_FILE=.git/index.lock` (the new index, under git's lock): the probe
/// takes `index.lock.lock`, as `write-tree` does, and leaves git's own `index.lock` alone.
#[test]
fn a_hook_index_under_gits_lock() {
    for (v, mut sb) in sandboxes() {
        if !safe(&sb.root) {
            return;
        }
        let repos: Vec<PathBuf> = ["commit-a-armed", "commit-a-plain"]
            .iter()
            .map(|n| {
                let repo = committed(&sb, n);
                std::fs::copy(repo.join(".git/index"), repo.join(".git/index.lock")).unwrap();
                repo
            })
            .collect();
        sb.env = sb
            .env
            .patched(&EnvPatch::default().set("GIT_INDEX_FILE", ".git/index.lock"));
        let armed = run(&sb, &repos[0], true, &[]);
        let plain = run(&sb, &repos[1], false, &[]);
        assert_eq!(armed.seen, plain.seen, "{v}");
        assert!(armed.seen.lock_left, "{v}: git's lock is the index here");
        assert!(!repos[0].join(".git/index.lock.lock").exists(), "{v}");
        assert_eq!(armed.ran("write-tree"), 0, "{v}: {:?}", armed.calls);
        assert_eq!(armed.ran("diff-index"), 0, "{v}: {:?}", armed.calls);
    }
}
