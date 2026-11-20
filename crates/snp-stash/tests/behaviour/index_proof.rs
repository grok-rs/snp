//! The index-stat proof (ADR-134): a clean, non-racy index decides "`git diff` is empty" with no spawn, every other
//! state falls back to the real `git diff`, and a randomized differential against real `git diff` (racy, touched,
//! mode, symlinked directories, empty files, assume-unchanged, skip-worktree, intent-to-add, split index, sparse
//! checkout, index v4, unmerged entries, gitlinks, `git add` by a hook, configuration edits) never finds the proof
//! wrong or missing a side effect. Every test runs with the git on `PATH` and every `bin` directory of
//! `SNP_TEST_GIT_MATRIX` (`:`-separated; locally 2.18.5 and 2.56.0 beside the system git).

#![cfg(unix)]

use std::ffi::OsString;
use std::num::NonZeroUsize;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use snp_core::cancel::CancelToken;
use snp_core::env::{Env, EnvPatch};
use snp_git::{DiffFingerprint, Git, RepoInfo};
use snp_proc::pool::JobPool;
use snp_stash::diff_oracle::{Change, DiffMode, DiffOracle, ProofStats};

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

/// `(major, minor)` of `git version X.Y.Z…`.
fn version(v: &str) -> (u32, u32) {
    let mut it = v
        .trim_start_matches("git version ")
        .split('.')
        .map(|p| p.parse::<u32>().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0))
}

fn safe(repo: &Path) -> bool {
    snp_os::fsinfo::stat_oracle_safe(repo).unwrap_or(false)
}

fn pool() -> Arc<JobPool> {
    let four = NonZeroUsize::new(4).unwrap();
    JobPool::with_cores(four, four, CancelToken::new())
}

/// A Git-mode oracle with the proof armed (when its gates allow); `None` as the baseline (lazy).
fn proof_oracle(git: &Git, info: &RepoInfo, env: &Env) -> (DiffOracle, bool) {
    let mut o = DiffOracle::new(git, info, env, None, None, None).unwrap();
    assert_eq!(o.mode(), DiffMode::Git);
    let listing = git.config_listing().unwrap();
    let armed = o.enable_index_proof(info, env, pool(), &listing, false);
    (o, armed)
}

/// The `git diff` calls a spy saw (`proof_oracle` asks G through it too).
fn diffs(spy: &Spy) -> Vec<String> {
    spy.calls()
        .into_iter()
        .filter(|c| c.starts_with("diff"))
        .collect()
}

fn stats(o: &DiffOracle) -> ProofStats {
    o.proof_stats().unwrap_or_default()
}

/// Sets the mtime of `p` itself (a symlink is not followed) to `t`.
fn set_lmtime(p: &Path, t: SystemTime) {
    use rustix::fs::{AtFlags, Timespec, Timestamps};
    let d = t.duration_since(SystemTime::UNIX_EPOCH).unwrap();
    let ts = Timespec {
        tv_sec: i64::try_from(d.as_secs()).unwrap(),
        tv_nsec: d.subsec_nanos().into(),
    };
    let _ignored = rustix::fs::utimensat(
        rustix::fs::CWD,
        p,
        &Timestamps {
            last_access: ts,
            last_modification: ts,
        },
        AtFlags::SYMLINK_NOFOLLOW,
    );
}

/// Makes the worktree and the index HEAD's tree with exact, non-racy stat data: `git reset --hard`, every tracked path's
/// mtime 10 s in the past, then `git
/// read-tree HEAD` (entries without stat data) and `git update-index --refresh` (which stats every entry and writes the
/// index now). A plain refresh would not do: git compares whole seconds only, so stat data changed within the same
/// second (the ctime of the mtime change itself) stays stale in the index at nanosecond precision, which the proof
/// compares.
fn settle(sb: &Sandbox, repo: &Path) {
    drop(sb.git_status(repo, &["reset", "-q", "--hard"]));
    // flags cleared, every file (a skip-worktree one too) written back
    drop(sb.git_status(repo, &["read-tree", "HEAD"]));
    drop(sb.git_status(repo, &["checkout-index", "-a", "-f"]));
    let past = SystemTime::now() - Duration::from_secs(10);
    let listed = sb.git(repo, &["ls-files", "-z"]);
    for p in listed.split(|&b| b == 0).filter(|p| !p.is_empty()) {
        set_lmtime(&repo.join(std::str::from_utf8(p).unwrap()), past);
    }
    sb.git(repo, &["read-tree", "HEAD"]);
    drop(sb.git_status(repo, &["update-index", "-q", "--refresh"]));
}

/// The index file's identity: a rewrite (`index.lock` renamed over it) changes the inode and the times.
fn index_stamp(info: &RepoInfo) -> Option<(u64, i64, i64)> {
    std::fs::metadata(&info.index_file)
        .ok()
        .map(|m| (m.ino(), m.mtime(), m.mtime_nsec()))
}

fn repo_with_tree(sb: &Sandbox, name: &str, init: &[&str]) -> PathBuf {
    let repo = sb.path(name);
    std::fs::create_dir_all(&repo).unwrap();
    let mut args = vec!["init", "-q"];
    args.extend_from_slice(init);
    args.push(".");
    sb.git(&repo, &args);
    for (p, c) in [
        ("a.txt", &b"alpha\n"[..]),
        ("b.txt", b"beta\nline\n"),
        ("empty", b""),
        ("dir/c.txt", b"gamma\n"),
        ("dir/sub/d.c", b"int x;\n"),
        ("dir/sub/deep/e.md", b"# e\n"),
        ("x/y.txt", b"y\n"),
    ] {
        sb.write(&repo, p, c);
    }
    std::os::unix::fs::symlink("a.txt", repo.join("link")).unwrap();
    {
        use std::os::unix::fs::PermissionsExt as _;
        sb.write(&repo, "run.sh", b"#!/bin/sh\n");
        std::fs::set_permissions(repo.join("run.sh"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
    }
    sb.git(&repo, &["add", "."]);
    sb.commit(&repo, "c");
    settle(sb, &repo);
    repo
}

/// A clean, non-racy index proves the baseline and every unchanged hook with no `git diff` at all (a counting probe),
/// for index versions 2, 3 (an assume-unchanged and a skip-worktree entry) and 4, and an empty tracked file; a real
/// modification, a touch and an intent-to-add entry each fall back to exactly one real `git diff`.
#[test]
fn clean_index_proves_without_spawning() {
    for (v, sb) in sandboxes() {
        let repo = repo_with_tree(&sb, "r", &[]);
        if !safe(&repo) {
            return;
        }
        for (case, setup) in [
            ("v2", &[][..]),
            ("v4", &["update-index", "--index-version", "4"][..]),
            (
                "assume-unchanged",
                &["update-index", "--assume-unchanged", "b.txt"][..],
            ),
            (
                "skip-worktree",
                &["update-index", "--skip-worktree", "x/y.txt"][..],
            ),
        ] {
            settle(&sb, &repo);
            if !setup.is_empty() {
                sb.git(&repo, setup);
            }
            if case == "assume-unchanged" {
                // git never looks at it again
                sb.write(&repo, "b.txt", b"changed but assumed unchanged\n");
            }
            if case == "skip-worktree" {
                std::fs::remove_file(repo.join("x/y.txt")).unwrap();
            }
            let (git, info) = sb.discover(&repo);
            let spy = Spy::new();
            let (mut o, armed) =
                proof_oracle(&git.with_backend(Arc::<Spy>::clone(&spy)), &info, &sb.env);
            assert!(armed, "{v} {case}");
            for _ in 0..5 {
                o.before_hook().unwrap();
                sb.write(&repo, "untracked.log", b"x");
                assert_eq!(o.after_hook().unwrap(), Change::Unchanged, "{v} {case}");
            }
            assert!(!o.non_empty().unwrap(), "{v} {case}");
            assert_eq!(diffs(&spy), Vec::<String>::new(), "{v} {case}");
            assert_eq!(stats(&o).proven, 6, "{v} {case}: baseline and five hooks");
            // a modification: the proof fails, one real diff
            o.before_hook().unwrap();
            sb.write(&repo, "dir/c.txt", b"changed\n");
            assert_eq!(o.after_hook().unwrap(), Change::Changed, "{v} {case}");
            assert_eq!(diffs(&spy).len(), 1, "{v} {case}");
            // a non-empty last fingerprint: no pass at all
            let passes = stats(&o).passes;
            o.before_hook().unwrap();
            assert_eq!(o.after_hook().unwrap(), Change::Unchanged, "{v} {case}");
            assert_eq!(stats(&o).passes, passes, "{v} {case}");
            sb.write(&repo, "dir/c.txt", b"gamma\n");
        }
        // touch-only: git prints nothing but would refresh the index, so the proof must not hold
        settle(&sb, &repo);
        let (git, info) = sb.discover(&repo);
        let spy = Spy::new();
        let (mut o, _) = proof_oracle(&git.with_backend(Arc::<Spy>::clone(&spy)), &info, &sb.env);
        o.before_hook().unwrap();
        assert!(diffs(&spy).is_empty(), "{v}");
        let f = std::fs::File::options()
            .write(true)
            .open(repo.join("a.txt"))
            .unwrap();
        f.set_modified(SystemTime::now() + Duration::from_secs(5))
            .unwrap();
        drop(f);
        assert_eq!(o.after_hook().unwrap(), Change::Unchanged, "{v}");
        assert_eq!(diffs(&spy).len(), 1, "{v}: touch-only takes the real diff");
        // intent-to-add: never proven
        sb.write(&repo, "new.txt", b"n\n");
        sb.git(&repo, &["add", "-N", "new.txt"]);
        let (git, info) = sb.discover(&repo);
        let (mut o, _) = proof_oracle(&git, &info, &sb.env);
        o.before_hook().unwrap();
        assert!(o.non_empty().unwrap(), "{v}: intent-to-add shows");
        assert_eq!(stats(&o).proven, 0, "{v}");
    }
}

/// The index git reads is `GIT_INDEX_FILE`'s (`git commit` hooks run with `index.lock` or `next-index-*`): a clean
/// alternate index proves, a modification in it does not, whatever `.git/index` says.
#[test]
fn the_proof_reads_git_index_file() {
    for (v, mut sb) in sandboxes() {
        let repo = repo_with_tree(&sb, "r", &[]);
        if !safe(&repo) {
            return;
        }
        let alt = sb.path("alt-index");
        std::fs::copy(repo.join(".git/index"), &alt).unwrap();
        let env = sb
            .env
            .patched(&EnvPatch::default().set("GIT_INDEX_FILE", alt.as_os_str()));
        // `.git/index` goes stale: a modification staged there only
        sb.write(&repo, "a.txt", b"staged in the main index\n");
        sb.git(&repo, &["add", "a.txt"]);
        sb.write(&repo, "a.txt", b"alpha\n");
        set_lmtime(
            &repo.join("a.txt"),
            SystemTime::now() - Duration::from_secs(10),
        );
        sb.env = env.clone();
        settle(&sb, &repo);
        let (git, info) = sb.discover(&repo);
        assert_eq!(info.index_file, alt, "{v}");
        let (mut o, armed) = proof_oracle(&git, &info, &env);
        assert!(armed, "{v}");
        o.before_hook().unwrap();
        assert!(!o.non_empty().unwrap(), "{v}");
        assert_eq!(stats(&o).proven, 1, "{v}");
        assert!(sb.git(&repo, &["diff"]).is_empty(), "{v}");
        // a modification the alternate index does not have
        o.before_hook().unwrap();
        sb.write(&repo, "x/y.txt", b"changed\n");
        assert_eq!(o.after_hook().unwrap(), Change::Changed, "{v}");
    }
}

/// A SHA-256 repository (git ≥ 2.29): the object format from the configuration sizes the entries.
#[test]
fn sha256_repositories_prove() {
    for (v, sb) in sandboxes() {
        if version(&v) < (2, 29) {
            continue;
        }
        let repo = repo_with_tree(&sb, "r256", &["--object-format=sha256"]);
        if !safe(&repo) {
            return;
        }
        let (git, info) = sb.discover(&repo);
        let (mut o, armed) = proof_oracle(&git, &info, &sb.env);
        assert!(armed, "{v}");
        o.before_hook().unwrap();
        assert_eq!(stats(&o).proven, 1, "{v}");
        o.before_hook().unwrap();
        sb.write(&repo, "empty", b"no longer empty\n");
        assert_eq!(o.after_hook().unwrap(), Change::Changed, "{v}");
    }
}

/// Configuration gates: fsmonitor (config or environment), ignoreStat and a sparse checkout refuse at arming; a
/// split index refuses at the parse.
#[test]
fn gates_refuse() {
    for (v, sb) in sandboxes() {
        let repo = repo_with_tree(&sb, "r", &[]);
        if !safe(&repo) {
            return;
        }
        for (key, value) in [
            ("core.ignoreStat", "true"),
            ("core.sparseCheckout", "true"),
            ("core.fsmonitor", "/nonexistent-hook"),
        ] {
            sb.git(&repo, &["config", key, value]);
            let (git, info) = sb.discover(&repo);
            let (_, armed) = proof_oracle(&git, &info, &sb.env);
            assert!(!armed, "{v} {key}");
            sb.git(&repo, &["config", "--unset", key]);
        }
        let env = sb
            .env
            .patched(&EnvPatch::default().set("GIT_TEST_FSMONITOR", "x"));
        let (git, info) = sb.discover(&repo);
        assert!(!proof_oracle(&git, &info, &env).1, "{v}");
        let env = sb
            .env
            .patched(&EnvPatch::default().set("SNP_DIFF_MODE", "git"));
        assert!(!proof_oracle(&git, &info, &env).1, "{v}");
        settle(&sb, &repo);
        sb.git(&repo, &["update-index", "--split-index"]);
        let (git, info) = sb.discover(&repo);
        let (mut o, armed) = proof_oracle(&git, &info, &sb.env);
        assert!(armed, "{v}");
        o.before_hook().unwrap();
        assert_eq!(
            stats(&o),
            ProofStats::default(),
            "{v}: a split index is never passed"
        );
    }
}

/// xorshift64*.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(n).unwrap()).unwrap()
    }

    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

const FILES: [&str; 9] = [
    "a.txt",
    "b.txt",
    "empty",
    "dir/c.txt",
    "dir/sub/d.c",
    "dir/sub/deep/e.md",
    "x/y.txt",
    "link",
    "run.sh",
];
const CONTENTS: [&[u8]; 5] = [
    b"alpha\n",
    b"",
    b"beta\nline\n",
    b"same size!\n",
    b"SAME SIZE!\n",
];

fn ign<T, E>(r: Result<T, E>) {
    drop(r);
}

/// One random "hook" of the differential: a worktree, index or configuration change, or a settle back to a clean,
/// non-racy index (so the proof gets its chances).
fn mutate(sb: &Sandbox, repo: &Path, rng: &mut Rng) -> &'static str {
    let rel = *rng.pick(&FILES);
    let file = repo.join(rel);
    match rng.below(32) {
        0..=4 => {
            // back to the committed tree, settled
            for d in ["dir/sub", "dir", "x"] {
                if std::fs::symlink_metadata(repo.join(d)).is_ok_and(|m| m.file_type().is_symlink())
                {
                    ign(std::fs::remove_file(repo.join(d)));
                }
            }
            drop(sb.git_status(repo, &["reset", "-q", "--hard"]));
            drop(sb.git_status(repo, &["clean", "-q", "-fd"]));
            for key in ["core.ignoreStat", "core.sparseCheckout"] {
                drop(sb.git_status(repo, &["config", "--unset", key]));
            }
            settle(sb, repo);
            "settle"
        }
        5 => {
            ign(std::fs::remove_file(&file));
            ign(std::fs::write(&file, rng.pick(&CONTENTS)));
            "write"
        }
        6 => {
            if let Ok(md) = std::fs::metadata(&file) {
                let mut new = std::fs::read(&file).unwrap_or_default();
                if let Some(b) = new.first_mut() {
                    *b = if *b == b'X' { b'Y' } else { b'X' };
                }
                ign(std::fs::write(&file, &new));
                set_lmtime(&file, md.modified().unwrap());
            }
            "same-size, mtime restored"
        }
        7 => {
            set_lmtime(&file, SystemTime::now() + Duration::from_secs(3));
            "touch"
        }
        8 => {
            ign(std::fs::remove_file(&file));
            "delete"
        }
        9 => {
            use std::os::unix::fs::PermissionsExt as _;
            if let Ok(md) = std::fs::symlink_metadata(&file)
                && md.is_file()
            {
                ign(std::fs::set_permissions(
                    &file,
                    std::fs::Permissions::from_mode(md.permissions().mode() ^ 0o100),
                ));
            }
            "chmod"
        }
        10 => {
            ign(std::fs::remove_file(&file));
            ign(std::os::unix::fs::symlink("a.txt", &file));
            "symlink"
        }
        11 => {
            // a leading directory replaced by a symlink to an identical copy
            let d = *rng.pick(&["dir", "x", "dir/sub"]);
            let real = repo.join(format!("{}.real", d.replace('/', "_")));
            if std::fs::rename(repo.join(d), &real).is_ok() {
                ign(std::os::unix::fs::symlink(&real, repo.join(d)));
            }
            "symlinked directory"
        }
        12 => {
            drop(sb.git_status(repo, &["update-index", "--assume-unchanged", rel]));
            "assume-unchanged"
        }
        13 => {
            drop(sb.git_status(repo, &["update-index", "--skip-worktree", rel]));
            "skip-worktree"
        }
        14 => {
            drop(sb.git_status(repo, &["update-index", "--no-assume-unchanged", rel]));
            drop(sb.git_status(repo, &["update-index", "--no-skip-worktree", rel]));
            "flags cleared"
        }
        15 => {
            ign(std::fs::write(repo.join("ita.txt"), b"ita\n"));
            drop(sb.git_status(repo, &["add", "-N", "ita.txt"]));
            "intent-to-add"
        }
        16 => {
            let v = *rng.pick(&["2", "3", "4"]);
            drop(sb.git_status(repo, &["update-index", "--index-version", v]));
            "index version"
        }
        17 => {
            let on = *rng.pick(&["--split-index", "--no-split-index"]);
            drop(sb.git_status(repo, &["update-index", on]));
            "split index"
        }
        18 => {
            drop(sb.git_status(repo, &["add", "-A", "--", rel]));
            "git add"
        }
        19 => {
            let (key, v) = *rng.pick(&[
                ("core.ignoreStat", "true"),
                ("core.ignoreStat", "false"),
                ("core.sparseCheckout", "true"),
                ("core.sparseCheckout", "false"),
                ("core.fileMode", "false"),
                ("core.fileMode", "true"),
                ("core.trustctime", "false"),
                ("core.checkStat", "minimal"),
            ]);
            drop(sb.git_status(repo, &["config", key, v]));
            "config"
        }
        20 => {
            // an unmerged path: stages 1-3 of a new path
            let oid = String::from_utf8(sb.git_status(repo, &["rev-parse", "HEAD:a.txt"]).1)
                .unwrap_or_default();
            let oid = oid.trim();
            if !oid.is_empty() {
                let info = format!(
                    "100644 {oid} 1\tconflict.txt\n100644 {oid} 2\tconflict.txt\n100644 {oid} 3\tconflict.txt\n"
                );
                let mut c = std::process::Command::new(
                    snp_proc::resolve::find_executable("git".as_ref(), &sb.env, repo).unwrap(),
                );
                sb.env.apply_to(&mut c);
                c.args(["update-index", "--index-info"])
                    .current_dir(repo)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null());
                if let Ok(mut child) = c.spawn() {
                    use std::io::Write as _;
                    if let Some(mut s) = child.stdin.take() {
                        ign(s.write_all(info.as_bytes()));
                    }
                    ign(child.wait());
                }
            }
            "unmerged"
        }
        21 => {
            drop(sb.git_status(
                repo,
                &[
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    "160000,78981922613b2afb6025042ff6bd878ac1994e85,sub",
                ],
            ));
            "gitlink"
        }
        22 => {
            // a directory replaced by a file of the same name
            let d = *rng.pick(&["x", "dir/sub/deep"]);
            ign(std::fs::remove_dir_all(repo.join(d)));
            ign(std::fs::write(repo.join(d), b"now a file\n"));
            "directory to file"
        }
        23..=27 => {
            ign(std::fs::write(repo.join("untracked.log"), b"log\n"));
            "untracked"
        }
        _ => "read-only",
    }
}

/// The differential: one oracle per git, Git mode with the proof armed, a long random sequence of hooks. Every
/// verdict equals real `git diff`'s (stdout compared before and after each hook), and whenever the proof held, the
/// real `git diff` printed nothing and left the index untouched (no refresh write the proof would have skipped). The
/// proof must also hold often enough that the test is not vacuous.
#[test]
fn randomized_differential_against_real_git_diff() {
    let process = Env::capture_process();
    let knob = |k: &str| {
        process
            .get(k)
            .and_then(|v| v.to_str())
            .and_then(|v| v.parse::<u64>().ok())
    };
    let iterations = knob("SNP_INDEX_PROOF_ITERATIONS").unwrap_or(400);
    let seed = knob("SNP_INDEX_PROOF_SEED").unwrap_or(0x1d3c_5eed);
    for (v, sb) in sandboxes() {
        let repo = repo_with_tree(&sb, "r", &[]);
        if !safe(&repo) {
            return;
        }
        let (git, info) = sb.discover(&repo);
        let (mut o, armed) = proof_oracle(&git, &info, &sb.env);
        assert!(armed, "{v}");
        let mut rng = Rng(seed);
        let mut truth: DiffFingerprint = git.diff_fingerprint().unwrap();
        let mut misses = Vec::new();
        let mut held = 0_u32;
        for i in 0..iterations {
            o.before_hook().unwrap();
            let what = mutate(&sb, &repo, &mut rng);
            let before = stats(&o).proven;
            let got = o.after_hook().unwrap();
            let proved = stats(&o).proven > before;
            let stamp = index_stamp(&info);
            let now = git.diff_fingerprint().unwrap();
            let want = if now == truth {
                Change::Unchanged
            } else {
                Change::Changed
            };
            truth = now;
            if got != want {
                misses.push((i, what, "verdict", got, want));
            }
            if proved {
                held += 1;
                if now != DiffFingerprint::EMPTY || index_stamp(&info) != stamp {
                    misses.push((i, what, "proof held but git diff disagreed", got, want));
                }
            }
        }
        assert!(
            misses.is_empty(),
            "{v}: {} mismatches, first: {:?}",
            misses.len(),
            &misses[..misses.len().min(10)]
        );
        assert!(
            u64::from(held) * 20 >= iterations,
            "{v}: the proof held only {held} times in {iterations} hooks"
        );
    }
}
