//! The `DiffOracle` (ARCHITECTURE §3.7, ADR-027, ADR-078): mode selection, zero spawns for unchanged hooks, native
//! fixer semantics, the show-diff gate, and the 10 000-iteration randomized soundness test against real `git diff`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use snp_core::env::{Env, EnvPatch};
use snp_git::{DiffFingerprint, Git, RepoInfo};
use snp_stash::diff_oracle::{Change, DiffMode, DiffOracle};

use crate::common::{Sandbox, Spy};

/// The fan-out inputs (`ls-files -s -z`, `config --list --show-origin -z`).
fn fanout(git: &Git) -> (Arc<snp_git::IndexEntries>, Arc<[PathBuf]>) {
    (
        Arc::new(git.tracked_with_modes().unwrap()),
        git.config_origins().unwrap().into(),
    )
}

fn repo_with_files(sb: &Sandbox) -> PathBuf {
    let repo = sb.init("r");
    for (p, c) in [
        ("a.txt", "alpha\n"),
        ("b.txt", "beta\nline\n"),
        ("dir/c.txt", "gamma\n"),
        ("dir/sub/d.c", "int x;\n"),
        ("e.bin", "\0\x01\x02"),
    ] {
        sb.write(&repo, p, p.as_bytes());
        sb.write(&repo, p, c.as_bytes());
    }
    sb.git(&repo, &["add", "."]);
    sb.commit(&repo, "c");
    repo
}

fn oracle(
    sb: &Sandbox,
    repo: &Path,
    env: &Env,
    backend: Option<Arc<Spy>>,
) -> (DiffOracle, Git, RepoInfo) {
    let (git, info) = sb.discover(repo);
    let git = git.with_env(env.clone());
    let (tracked, origins) = fanout(&git);
    let baseline = git.diff_fingerprint().unwrap();
    let og = match backend {
        Some(b) => git.with_backend(b),
        None => git.clone(),
    };
    let o = DiffOracle::new(
        &og,
        &info,
        env,
        Some(baseline),
        Some(tracked),
        Some(origins),
    )
    .unwrap();
    (o, git, info)
}

fn stat_mode_possible(path: &Path) -> bool {
    cfg!(unix) && snp_os::fsinfo::stat_oracle_safe(path).unwrap_or(false)
}

#[test]
fn mode_selection() {
    let sb = Sandbox::new();
    let repo = repo_with_files(&sb);
    let (git, info) = sb.discover(&repo);
    let (tracked, origins) = fanout(&git);
    let env = sb.env.clone();
    let mode = |env: &Env, t: Option<Arc<_>>, o: Option<Arc<[PathBuf]>>| {
        DiffOracle::new(&git, &info, env, None, t, o)
            .unwrap()
            .mode()
    };
    let expect = if stat_mode_possible(&repo) {
        DiffMode::Stat
    } else {
        DiffMode::Git
    };
    assert_eq!(
        mode(&env, Some(Arc::clone(&tracked)), Some(Arc::clone(&origins))),
        expect
    );
    assert_eq!(mode(&env, None, Some(Arc::clone(&origins))), DiffMode::Git);
    assert_eq!(mode(&env, Some(Arc::clone(&tracked)), None), DiffMode::Git);
    let with = |k: &'static str, v: &str| env.patched(&EnvPatch::default().set(k, v));
    assert_eq!(
        mode(
            &with("SNP_DIFF_MODE", "git"),
            Some(Arc::clone(&tracked)),
            Some(Arc::clone(&origins))
        ),
        DiffMode::Git
    );
    assert_eq!(
        mode(
            &with("GIT_ATTR_SOURCE", "HEAD"),
            Some(Arc::clone(&tracked)),
            Some(Arc::clone(&origins))
        ),
        DiffMode::Git
    );
    sb.git(&repo, &["config", "attr.tree", "HEAD"]);
    assert_eq!(mode(&env, Some(tracked), Some(origins)), DiffMode::Git);
}

#[test]
fn unchanged_hooks_cost_zero_spawns_in_stat_mode() {
    let sb = Sandbox::new();
    let repo = repo_with_files(&sb);
    if !stat_mode_possible(&repo) {
        return;
    }
    sb.write(&repo, "a.txt", b"pre-existing unstaged\n");
    let spy = Spy::new();
    let (mut o, _git, _) = oracle(&sb, &repo, &sb.env, Some(Arc::<Spy>::clone(&spy)));
    assert_eq!(o.mode(), DiffMode::Stat);
    for _ in 0..50 {
        o.before_hook().unwrap();
        // A hook that reads files and writes an untracked one.
        assert!(
            !std::fs::read(repo.join("b.txt")).unwrap().is_empty(),
            "the hook reads"
        );
        sb.write(&repo, "untracked.log", b"x");
        assert_eq!(o.after_hook().unwrap(), Change::Unchanged);
    }
    assert!(o.non_empty().unwrap());
    assert_eq!(spy.calls(), Vec::<String>::new());
    // A real modification costs exactly one `git diff`.
    o.before_hook().unwrap();
    sb.write(&repo, "b.txt", b"changed\n");
    assert_eq!(o.after_hook().unwrap(), Change::Changed);
    assert_eq!(
        spy.calls(),
        vec!["diff --no-ext-diff --no-textconv --ignore-submodules".to_owned()]
    );
    // Touch-only: one diff, unchanged.
    o.before_hook().unwrap();
    let f = std::fs::File::options()
        .write(true)
        .open(repo.join("b.txt"))
        .unwrap();
    f.set_modified(SystemTime::now() + Duration::from_secs(5))
        .unwrap();
    drop(f);
    assert_eq!(o.after_hook().unwrap(), Change::Unchanged);
    assert_eq!(spy.calls().len(), 2);
}

#[test]
fn git_mode_diffs_every_hook() {
    let sb = Sandbox::new();
    let repo = repo_with_files(&sb);
    let env = sb
        .env
        .patched(&EnvPatch::default().set("SNP_DIFF_MODE", "git"));
    let spy = Spy::new();
    let (mut o, _, _) = oracle(&sb, &repo, &env, Some(Arc::<Spy>::clone(&spy)));
    assert_eq!(o.mode(), DiffMode::Git);
    o.before_hook().unwrap();
    assert_eq!(o.after_hook().unwrap(), Change::Unchanged);
    o.before_hook().unwrap();
    sb.write(&repo, "a.txt", b"x\n");
    assert_eq!(o.after_hook().unwrap(), Change::Changed);
    assert_eq!(spy.calls().len(), 2);
}

#[test]
fn native_fixers_and_the_show_diff_gate() {
    for env in [
        Sandbox::new().env,
        Sandbox::new()
            .env
            .patched(&EnvPatch::default().set("SNP_DIFF_MODE", "git")),
    ] {
        let sb = Sandbox::new();
        let repo = repo_with_files(&sb);
        let env = sb.env.patched(
            &EnvPatch::default().set(
                "SNP_DIFF_MODE",
                env.get("SNP_DIFF_MODE")
                    .map_or(std::ffi::OsString::new(), std::ffi::OsStr::to_os_string),
            ),
        );
        let spy = Spy::new();
        let (mut o, _, _) = oracle(&sb, &repo, &env, Some(Arc::<Spy>::clone(&spy)));
        assert!(!o.non_empty().unwrap());
        // No modified paths: unchanged, no spawn.
        assert_eq!(o.after_native(false, false).unwrap(), Change::Unchanged);
        // Exact tracked paths reported: changed, no spawn; the digest is stale.
        sb.write(&repo, "a.txt", b"fixed\n");
        assert_eq!(o.after_native(true, false).unwrap(), Change::Changed);
        assert!(spy.calls().is_empty(), "{:?}", spy.calls());
        // The next comparison refreshes it lazily (one diff), then the caveat path diffs for real.
        o.before_hook().unwrap();
        assert_eq!(spy.calls().len(), 1);
        assert!(o.non_empty().unwrap());
        sb.write(&repo, "b.txt", b"fixed too\n");
        assert_eq!(o.after_native(true, true).unwrap(), Change::Changed);
        o.before_hook().unwrap();
        assert_eq!(o.after_native(false, true).unwrap(), Change::Unchanged);
        assert_eq!(spy.calls().len(), 3);
    }
}

/// xorshift64*: deterministic randomness for the soundness test.
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

const FILES: [&str; 8] = [
    "a.txt",
    "b.txt",
    "dir/c.txt",
    "dir/sub/d.c",
    "e.bin",
    "dir/f.txt",
    "g.md",
    "dir/sub/h.txt",
];
const CONTENTS: [&[u8]; 8] = [
    b"alpha\n",
    b"alpha\r\n",
    b"beta\nline\n",
    b"beta\r\nline\r\n",
    b"",
    b"\0\x01\x02",
    b"same size!\n",
    b"SAME SIZE!\n",
];
const ATTRS: [&str; 6] = [
    "*.txt text eol=crlf\n",
    "*.txt -text\n",
    "* text=auto\n",
    "*.c ident\n",
    "*.md eol=crlf\n",
    "",
];

/// Ignores the outcome of a best-effort mutation.
fn ign<T, E>(r: Result<T, E>) {
    drop(r);
}

fn set_mtime(p: &Path, t: SystemTime) {
    if let Ok(f) = std::fs::File::options().write(true).open(p) {
        ign(f.set_modified(t));
    }
}

/// One random "hook": a mutation of the worktree, the index, the attributes or the config.
fn mutate(sb: &Sandbox, repo: &Path, rng: &mut Rng) -> &'static str {
    let file = repo.join(rng.pick(&FILES));
    match rng.below(16) {
        0 | 1 => {
            ign(std::fs::create_dir_all(file.parent().unwrap()));
            ign(std::fs::remove_file(&file));
            ign(std::fs::write(&file, rng.pick(&CONTENTS)));
            "write"
        }
        2 => {
            // Same-size rewrite with the mtime restored.
            if let Ok(md) = std::fs::metadata(&file) {
                let mut new = std::fs::read(&file).unwrap_or_default();

                if let Some(b) = new.first_mut() {
                    *b = if *b == b'X' { b'Y' } else { b'X' };
                }
                ign(std::fs::write(&file, &new));
                set_mtime(&file, md.modified().unwrap());
            }
            "same-size"
        }
        3 => {
            set_mtime(
                &file,
                SystemTime::now() + Duration::from_secs(u64::try_from(rng.below(10)).unwrap()),
            );
            "touch"
        }
        4 => {
            ign(std::fs::remove_file(&file));
            "delete"
        }
        5 => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(md) = std::fs::metadata(&file) {
                    let mode = md.permissions().mode() ^ 0o100;
                    ign(std::fs::set_permissions(
                        &file,
                        std::fs::Permissions::from_mode(mode),
                    ));
                }
            }
            "chmod"
        }
        6 => {
            let dir = repo.join(rng.pick(&["", "dir", "dir/sub"]));
            let ga = dir.join(".gitattributes");
            if rng.below(3) == 0 {
                ign(std::fs::remove_file(&ga));
            } else {
                ign(std::fs::write(&ga, rng.pick(&ATTRS)));
            }
            "gitattributes"
        }
        7 => {
            ign(std::fs::create_dir_all(repo.join(".git/info")));
            ign(std::fs::write(
                repo.join(".git/info/attributes"),
                rng.pick(&ATTRS),
            ));
            "info/attributes"
        }
        8 => {
            let v = rng.pick(&["true", "false", "input"]);
            sb.git(repo, &["config", "core.autocrlf", v]);
            "config"
        }
        9 => {
            let inc = sb.path("inc.cfg");
            if rng.below(3) == 0 {
                ign(std::fs::remove_file(&inc));
            } else {
                ign(std::fs::write(
                    &inc,
                    format!(
                        "[core]\n\tautocrlf = {}\n",
                        rng.pick(&["true", "false", "input"])
                    ),
                ));
            }
            "include.path"
        }
        10 => {
            ign(std::fs::write(sb.path("attrs"), rng.pick(&ATTRS)));
            "core.attributesFile"
        }
        11 => {
            let rel = rng.pick(&FILES);
            drop(sb.git_status(repo, &["add", "-A", "--", rel]));
            "git add"
        }
        12 => {
            ign(std::fs::write(
                repo.join("untracked.txt"),
                rng.pick(&CONTENTS),
            ));
            "untracked"
        }
        13 => {
            #[cfg(unix)]
            {
                ign(std::fs::remove_file(&file));
                ign(std::os::unix::fs::symlink(
                    rng.pick(&["a.txt", "nowhere"]),
                    &file,
                ));
            }
            "symlink"
        }
        14 => {
            ign(std::fs::write(sb.path("global-attrs"), rng.pick(&ATTRS)));
            "xdg attributes"
        }
        _ => "noop",
    }
}

#[test]
fn randomized_soundness_against_real_git_diff() {
    let process = Env::capture_process();
    let knob = |k: &str| {
        process
            .get(k)
            .and_then(|v| v.to_str())
            .and_then(|v| v.parse::<u64>().ok())
    };
    let iterations = knob("SNP_ORACLE_ITERATIONS").unwrap_or(10_000);
    let seed = knob("SNP_ORACLE_SEED").unwrap_or(0x5eed_cafe);
    let sb = Sandbox::new();
    let repo = repo_with_files(&sb);
    for f in ["dir/f.txt", "g.md", "dir/sub/h.txt"] {
        sb.write(&repo, f, b"more\n");
    }
    sb.git(&repo, &["add", "."]);
    // include.path and core.attributesFile targets that do not exist yet; XDG attributes via the default path.
    sb.git(
        &repo,
        &[
            std::ffi::OsStr::new("config"),
            "include.path".as_ref(),
            sb.path("inc.cfg").as_os_str(),
        ],
    );
    sb.git(
        &repo,
        &[
            std::ffi::OsStr::new("config"),
            "core.attributesFile".as_ref(),
            sb.path("attrs").as_os_str(),
        ],
    );
    let xdg = sb.path("xdg");
    std::fs::create_dir_all(xdg.join("git")).unwrap();
    let env = sb
        .env
        .patched(&EnvPatch::default().set("XDG_CONFIG_HOME", xdg.as_os_str()));
    // `global-attrs` stands in for the XDG default attributes file through a symlink.
    #[cfg(unix)]
    std::os::unix::fs::symlink(sb.path("global-attrs"), xdg.join("git/attributes")).unwrap();
    let (mut o, truth_git, _) = oracle(&sb, &repo, &env, None);
    if !stat_mode_possible(&repo) {
        assert_eq!(o.mode(), DiffMode::Git);
    }
    let mut rng = Rng(seed);
    let mut truth: DiffFingerprint = truth_git.diff_fingerprint().unwrap();
    let mut misses = Vec::new();
    for i in 0..iterations {
        o.before_hook().unwrap();
        let what = mutate(&sb, &repo, &mut rng);
        let got = o.after_hook().unwrap();
        let now = truth_git.diff_fingerprint().unwrap();
        let want = if now == truth {
            Change::Unchanged
        } else {
            Change::Changed
        };
        truth = now;
        if got != want {
            misses.push((i, what, got, want));
        }
    }
    assert!(
        misses.is_empty(),
        "mode {:?}: {} mismatches, first: {:?}",
        o.mode(),
        misses.len(),
        &misses[..misses.len().min(10)]
    );
}
