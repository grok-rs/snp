//! Integration tests: spawn-count budgets via `SNP_TEST_SPAWN_LOG` (ARCHITECTURE §12.1, §12.3).
//!
//! The timing gates of `cargo xtask bench` run the `dist` build; these tests run against the `--features testing`
//! build, whose `snp_proc::spawn_log` appends `<role> <group> <argv0 basename>` for every process snp starts. They
//! assert upper bounds per scenario shape and so catch an accidental O(files) or O(hooks²) spawn long before it shows
//! up as a timing regression.
//!
//! The documented bounds (Linux/macOS; ARCHITECTURE §12.1 "B3 spawn model"):
//!
//! | Shape | git spawns | hook spawns | total |
//! |---|---|---|---|
//! | B1 `--version` | 0 | 0 | **0** |
//! | B2 `run noop-01` (1 `always_run` hook, nothing staged) | 1 combined `rev-parse` + F, G (F answers A), B, C + the stash's `write-tree` and `diff-index` (Stat mode from one hook on a small tree, ADR-104); with fewer than 3 CPUs A, B, C + 1 `git diff` | 1 | **≤ 8** |
//! | B3 `run` (10 such hooks) | 1 combined `rev-parse` + F, G, B, C (E skipped, P17) + `write-tree` + `diff-index`; with fewer than 3 CPUs or a large tree A as well | 10 | **≤ 17** (≤ 18) |
//! | B4 `run --all-files`, 1 hook with filenames, 2 000 files, `PRE_COMMIT_NO_CONCURRENCY=1` | 7 | exactly 1 (one partition) | **≤ 8** |
//! | B9 `git commit --allow-empty` through the installed hook (noop10; pinned exactly on Linux) | B3's | 10 | **≤ 17** |
//! | B12u `run --all-files`, 20 Unicode `files` patterns, 2 000 non-ASCII paths, `SNP_JOBS=32` | ≤ 7 | 20..=640 (≤ 32 partitions per hook) | — |
//! | B6 13 pre-commit-hooks hooks, Tier A fast path, `--all-files` | 7 + ≤ 4 per-run pre-commit-hooks facts (`diff --staged --diff-filter=A`, `check-attr --stdin`, `ls-files`, `ls-files --stage`) + ≤ 2 slack | 0 (all native, no Python) | **≤ 13** |
//!
//! The stash's `write-tree` + `diff-index` are pre-commit's own calls (`_unstaged_changes_cleared`, 02 §7), outside
//! the fan-out (ADR-082 corrects the B3 model, which had left them out). The baseline `git diff` is elided on the warm
//! path (`diff-index` rc 0 + the `DiffOracle` stat path, ADR-078), so no `git diff` runs on Unix once the Stat mode
//! applies (three process hooks or more). On Windows there is no stat oracle: every hook that ran is followed by a real
//! `git diff` and the baseline is taken (ADR-043), so the hook-proportional bounds grow by `hooks + 1`.
//!
//! Every count is taken on the second of two identical runs (the first warms the store), as the bounds describe the
//! warm path. B1's zero count is asserted only after a positive control proved the log armed. Without
//! `--features testing` the spawn log does not exist, so those tests are ignored with a reason that says so.
//!
//! Owner: WP-36.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use snp_testkit::{Bins, Output, TestEnv};

/// The binaries under test: this package's `snp` and the workspace's `snp-probe` when built.
fn bins() -> Bins {
    let snp = PathBuf::from(env!("CARGO_BIN_EXE_snp"));
    let probe = Bins::locate().map_or_else(|| snp.clone(), |b| b.probe);
    Bins { snp, probe }
}

/// One parsed spawn-log line.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Spawn {
    role: String,
    group: String,
    exe: String,
}

impl Spawn {
    /// `git` / `git.exe`.
    fn is_git(&self) -> bool {
        self.exe == "git" || self.exe.eq_ignore_ascii_case("git.exe")
    }
}

/// The spawn log of one invocation.
#[derive(Debug)]
struct SpawnLog {
    spawns: Vec<Spawn>,
}

impl SpawnLog {
    fn read(path: &Path) -> Self {
        let spawns = snp_proc_log_lines(path)
            .into_iter()
            .map(|line| {
                let mut parts = line.splitn(3, ' ');
                let mut next = || parts.next().unwrap_or_default().to_owned();
                Spawn {
                    role: next(),
                    group: next(),
                    exe: next(),
                }
            })
            .collect();
        Self { spawns }
    }

    fn total(&self) -> usize {
        self.spawns.len()
    }

    fn git(&self) -> usize {
        self.spawns.iter().filter(|s| s.is_git()).count()
    }

    fn hooks(&self) -> usize {
        self.spawns.iter().filter(|s| s.role == "hook").count()
    }

    /// A readable dump for assertion messages.
    fn dump(&self) -> String {
        let mut s = String::new();
        for (i, sp) in self.spawns.iter().enumerate() {
            writeln!(s, "  {i:>3}: {} {} {}", sp.role, sp.group, sp.exe).unwrap();
        }
        s
    }
}

/// The lines of the spawn log at `path` (a missing file: no spawns). Read directly: `snp_proc::spawn_log` exists
/// only in the `testing` build of snp-proc, and the format is fixed by ARCHITECTURE §1.4.10.
fn snp_proc_log_lines(path: &Path) -> Vec<String> {
    std::fs::read(path).map_or_default(|b| {
        String::from_utf8_lossy(&b)
            .lines()
            .map(str::to_owned)
            .collect()
    })
}

/// Runs `snp <args>` in `cwd` twice — once unlogged to warm the store (the cached `git --version` of
/// `GitExe::resolve`, the store layout, the baseline-elision state), then with the spawn log armed — and returns the
/// second run's output and log: the bounds of ARCHITECTURE §12.1 are those of the warm path that B2/B3 measure.
fn run_logged(
    env: &TestEnv,
    cwd: &Path,
    args: &[&str],
    extra: &[(&str, &str)],
) -> (Output, SpawnLog) {
    let log = env.root().join(format!(
        "spawns-{}.log",
        args.join("_").replace(['/', ' '], "-")
    ));
    let mut warm = env.snp(cwd).args(args);
    for (k, v) in extra {
        warm = warm.env(k, v);
    }
    let _warm_up = warm.output();
    let _ignored = std::fs::remove_file(&log);
    let mut cmd = env.snp(cwd).args(args).env("SNP_TEST_SPAWN_LOG", &log);
    for (k, v) in extra {
        cmd = cmd.env(k, v);
    }
    let out = cmd.output();
    (out, SpawnLog::read(&log))
}

/// The `noop` configuration of B2/B3 (prek's benchmark corpus): `n` local `system` hooks running `true`.
fn noop_config(n: usize) -> String {
    let mut s = String::from("repos:\n-   repo: local\n    hooks:\n");
    for i in 1..=n {
        write!(
            s,
            "    -   id: noop-{i:02}\n        name: no-op {i:02}\n        entry: 'true'\n        \
             language: system\n        always_run: true\n        pass_filenames: false\n"
        )
        .unwrap();
    }
    s
}

/// A committed repository holding `config` and one tracked file, nothing staged.
fn noop_repo(env: &TestEnv, name: &str, config: &str) -> PathBuf {
    env.git_repo(name)
        .file(".pre-commit-config.yaml", config.as_bytes())
        .file("README.md", b"# bench\n")
        .stage_all()
        .commit("init")
        .build()
}

/// The hook-proportional extra spawns on Windows: one `git diff` per hook that ran plus the baseline (ADR-043).
const fn windows_diff_spawns(hooks: usize) -> usize {
    if cfg!(windows) { hooks + 1 } else { 0 }
}

/// The fixed git cost of a warm run: one combined `rev-parse` and at most six overlapped fan-out queries.
const WARM_RUN_GIT: usize = 1 + 6;

/// The git cost of a warm stashing `run` whose hooks read no files (P17: no staged list E), `process_hooks` of them
/// executed: one combined `rev-parse`; fan-out A, B, C, plus F and G when the Stat mode applies (Unix, from three
/// process hooks on, ARCHITECTURE §3.1 step 14); the stash's `write-tree` + `diff-index` (02 §7); one `git diff` per
/// executed hook without the Stat mode (and on Windows the baseline as well, ADR-043).
///
/// On a `small` tree (at most 256 index entries) with three or more CPUs, F and G start first and F answers A, and
/// the Stat mode applies from one process hook on (ADR-104): B, C, F and G, and no `git diff`.
fn stash_run_git(process_hooks: usize, small: bool) -> usize {
    if cfg!(unix) && small && snp_proc::xargs::cpu_count().get() >= 3 && process_hooks >= 1 {
        return 1 + 4 + 2;
    }
    let stat = cfg!(unix) && process_hooks >= 3;
    let fan_out = if stat { 5 } else { 3 };
    let diffs = if stat {
        0
    } else if cfg!(windows) {
        process_hooks + 1
    } else {
        process_hooks
    };
    1 + fan_out + 2 + diffs
}

fn assert_bounds(what: &str, out: &Output, log: &SpawnLog, max_total: usize, hooks: usize) {
    assert_eq!(
        out.code,
        0,
        "{what}: snp failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        log.total() > 0,
        "{what}: the spawn log is empty — is the binary a `--features testing` build that arms SNP_TEST_SPAWN_LOG?"
    );
    assert_eq!(log.hooks(), hooks, "{what}: hook spawns\n{}", log.dump());
    assert!(
        log.total() <= max_total,
        "{what}: {} spawns > budget {max_total} ({} git)\n{}",
        log.total(),
        log.git(),
        log.dump()
    );
}

/// B1: `--version` starts no process at all (T1: answered before any initialisation). A zero count proves nothing
/// unless the log is armed, so a `snp run` that must spawn is logged first as the positive control; that needs the
/// run pipeline (WP-32) and the binary entry arming `SNP_TEST_SPAWN_LOG` (WP-33).
#[test]
#[cfg_attr(
    not(feature = "testing"),
    ignore = "needs a `--features testing` build (spawn log)"
)]
fn b1_version_spawns_nothing() {
    let env = TestEnv::new(bins());
    let repo = noop_repo(&env, "b1-control", &noop_config(1));
    let (out, control) = run_logged(&env, &repo, &["run"], &[]);
    assert_bounds("B1 positive control", &out, &control, usize::MAX, 1);
    let (out, log) = run_logged(&env, env.root(), &["--version"], &[]);
    assert_eq!(out.code, 0);
    // argparse's `print` (text mode: CRLF on Windows)
    let nl = if cfg!(windows) { "\r\n" } else { "\n" };
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("snp 2.0.1{nl}")
    );
    assert_eq!(log.total(), 0, "--version spawned:\n{}", log.dump());
}

/// B2: one `always_run` noop hook, nothing staged: ≤ 8 spawns (7 git + 1 hook).
#[test]
#[cfg_attr(
    not(feature = "testing"),
    ignore = "needs a `--features testing` build (spawn log)"
)]
fn b2_one_noop_hook_within_8_spawns() {
    let env = TestEnv::new(bins());
    let repo = noop_repo(&env, "b2", &noop_config(10));
    let (out, log) = run_logged(&env, &repo, &["run", "noop-01"], &[]);
    assert_bounds("B2", &out, &log, stash_run_git(1, true) + 1, 1);
}

/// B3: ten `always_run` noop hooks, nothing staged: ≤ 17 spawns (7 git + 10 hooks, no `git diff`; 18 with fewer
/// than three CPUs).
#[test]
#[cfg_attr(
    not(feature = "testing"),
    ignore = "needs a `--features testing` build (spawn log)"
)]
fn b3_ten_noop_hooks_within_18_spawns() {
    let env = TestEnv::new(bins());
    let repo = noop_repo(&env, "b3", &noop_config(10));
    let (out, log) = run_logged(&env, &repo, &["run"], &[]);
    assert_bounds("B3", &out, &log, stash_run_git(10, true) + 10, 10);
    // the hooks run in their own process groups only when isolation is asked for; `true` inherits
    assert!(
        log.spawns
            .iter()
            .filter(|s| s.role == "hook")
            .all(|s| s.exe == "true" || s.exe.eq_ignore_ascii_case("true.exe")),
        "{}",
        log.dump()
    );
}

/// B3 must not grow with unrelated tracked files: 2 000 tracked files change no count (the run lists no files when
/// every hook is `always_run` with `pass_filenames: false`, P17).
#[test]
#[cfg_attr(
    not(feature = "testing"),
    ignore = "needs a `--features testing` build (spawn log)"
)]
fn b3_spawns_do_not_grow_with_tracked_files() {
    let env = TestEnv::new(bins());
    let mut b = env
        .git_repo("b3-big")
        .file(".pre-commit-config.yaml", noop_config(10).as_bytes());
    for i in 0..2_000 {
        b = b.file(&format!("src/pkg{:02}/f{i:04}.py", i % 40), b"x = 1\n");
    }
    let repo = b.stage_all().commit("init").build();
    let (out, log) = run_logged(&env, &repo, &["run"], &[]);
    assert_bounds(
        "B3 (2k files)",
        &out,
        &log,
        stash_run_git(10, false) + 10,
        10,
    );
}

/// B4 shape: `run --all-files`, one hook with filenames over 2 000 files and one job: exactly one partition, so one
/// hook spawn; no per-file process of any kind.
#[test]
#[cfg_attr(
    not(feature = "testing"),
    ignore = "needs a `--features testing` build (spawn log)"
)]
fn b4_shape_one_partition_no_per_file_spawns() {
    let env = TestEnv::new(bins());
    let config = "repos:\n-   repo: local\n    hooks:\n    -   id: true-files\n        name: true with files\n        \
                  entry: 'true'\n        language: system\n";
    let mut b = env
        .git_repo("b4")
        .file(".pre-commit-config.yaml", config.as_bytes());
    for i in 0..2_000 {
        b = b.file(&format!("src/pkg{:02}/f{i:04}.py", i % 40), b"x = 1\n");
    }
    let repo = b.stage_all().commit("init").build();
    let (out, log) = run_logged(
        &env,
        &repo,
        &["run", "--all-files", "true-files"],
        &[("PRE_COMMIT_NO_CONCURRENCY", "1")],
    );
    // Windows: CreateProcess takes at most 32 767 characters, so the 2 000 paths (~44 k) need two partitions
    // (pre-commit's xargs splits the same way there).
    let partitions = if cfg!(windows) { 2 } else { 1 };
    assert_bounds(
        "B4 (2k files)",
        &out,
        &log,
        WARM_RUN_GIT + partitions + windows_diff_spawns(1),
        partitions,
    );
}

/// The 13 pre-commit-hooks hooks of B6 (prek's benchmark corpus shape).
const B6_HOOKS: [&str; 13] = [
    "trailing-whitespace",
    "end-of-file-fixer",
    "mixed-line-ending",
    "fix-byte-order-marker",
    "check-json",
    "check-yaml",
    "check-toml",
    "check-xml",
    "check-merge-conflict",
    "check-added-large-files",
    "detect-private-key",
    "check-executables-have-shebangs",
    "check-case-conflict",
];

/// B6 shape: 13 pre-commit-hooks hooks at a known tag take the Tier A fast path — no clone, no Python, no hook
/// process; only the fixed git queries plus the per-run pre-commit-hooks facts: ≤ 13 spawns.
#[test]
#[cfg_attr(
    not(feature = "testing"),
    ignore = "needs a `--features testing` build (spawn log)"
)]
fn b6_shape_fast_path_within_13_spawns() {
    let env = TestEnv::new(bins());
    let mut config = String::from(
        "repos:\n-   repo: https://github.com/pre-commit/pre-commit-hooks\n    rev: v6.0.0\n    hooks:\n",
    );
    for id in B6_HOOKS {
        writeln!(config, "    -   id: {id}").unwrap();
    }
    let mut b = env
        .git_repo("b6")
        .file(".pre-commit-config.yaml", config.as_bytes());
    for i in 0..96 {
        b = match i % 6 {
            0 => b.file(&format!("pkg/m{i:03}.py"), b"x = 1\n"),
            1 => b.file(&format!("docs/d{i:03}.md"), b"# title\n\ntext\n"),
            2 => b.file(&format!("data/j{i:03}.json"), b"{\"a\": [1, 2]}\n"),
            3 => b.file(&format!("conf/y{i:03}.yaml"), b"a: 1\nb: [x, y]\n"),
            4 => b.file(&format!("conf/t{i:03}.toml"), b"[a]\nb = 1\n"),
            _ => b.file(
                &format!("xml/x{i:03}.xml"),
                b"<?xml version=\"1.0\"?>\n<a><b/></a>\n",
            ),
        };
    }
    let repo = b.stage_all().commit("init").build();
    // hermetic: a regressed fast path that tries to clone fails at once instead of reaching GitHub (or hanging
    // offline), since git refuses every transport but file://. TestEnv sets this already; it is restated here because
    // this test's correctness depends on it.
    let no_network = [("GIT_ALLOW_PROTOCOL", "file")];
    let (out, log) = run_logged(&env, &repo, &["run", "--all-files"], &no_network);
    // Windows: no stat oracle, so the baseline `git diff` is taken (ADR-043).
    assert_bounds(
        "B6",
        &out,
        &log,
        WARM_RUN_GIT + 4 + 2 + usize::from(cfg!(windows)),
        0,
    );
    // The python default-version probe is persisted after the warm-up run, so the measured run spawns no Python,
    // except on macOS, whose runners' `python3` is a proxy (the `/usr/bin` xcrun shim, or a version manager's
    // `shims/`): snp never persists a proxy's facts (`ToolProbe::fact_file`, ADR-079) and probes it once per run.
    let shim_probe =
        |s: &Spawn| cfg!(target_os = "macos") && s.role == "tool" && s.exe == "python3";
    assert!(
        log.spawns.iter().filter(|s| shim_probe(s)).count() <= 1
            && log.spawns.iter().all(|s| s.is_git() || shim_probe(s)),
        "B6 started a non-git process:\n{}",
        log.dump()
    );
    // the fast path never clones (no network): the store's repository bucket holds no clone directory (only the
    // `<key>.lock` the `Initializing` line is printed under, ADR-032)
    let store = env.root().join("snp-store").join("repos-v1");
    let repos = std::fs::read_dir(&store).map_or(0, |d| {
        d.filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|f| f.is_dir()))
            .count()
    });
    assert_eq!(
        repos,
        0,
        "B6 materialised a repository in {}",
        store.display()
    );
}

/// B9 shape pin (P15): `git commit --allow-empty` through the installed hook of a `noop` repository with ten hooks.
/// hook-impl reuses its own discovery (one combined `rev-parse`, ADR-084) and runs the same pipeline as B3, so on
/// Linux the git count is exactly B3's and every hook spawns once; elsewhere the B3 bounds apply.
#[test]
#[cfg_attr(
    not(feature = "testing"),
    ignore = "needs a `--features testing` build (spawn log)"
)]
fn b9_hook_impl_shape_is_pinned() {
    let env = TestEnv::new(bins());
    let repo = noop_repo(&env, "b9", &noop_config(10));
    let installed = env.snp(&repo).args(["install"]).output();
    assert_eq!(installed.code, 0, "{installed:?}");
    let commit = |log: Option<&Path>| {
        let mut cmd = env
            .git(&repo)
            .args(["commit", "-q", "--allow-empty", "-m", "b9"]);
        if let Some(log) = log {
            cmd = cmd.env("SNP_TEST_SPAWN_LOG", log);
        }
        cmd.output()
    };
    let _warm_up = commit(None);
    let log_path = env.root().join("spawns-b9.log");
    let out = commit(Some(&log_path));
    let log = SpawnLog::read(&log_path);
    let git = stash_run_git(10, true);
    assert_bounds("B9", &out, &log, git + 10, 10);
    if cfg!(target_os = "linux") {
        assert_eq!(log.git(), git, "B9 git spawns\n{}", log.dump());
        assert_eq!(log.total(), git + 10, "B9 spawns\n{}", log.dump());
    }
}

/// Twenty `files` patterns over non-ASCII paths: the B12u configuration's shape (`unicode20`), scaled down.
const UNICODE_PATTERNS: [&str; 20] = [
    "^données/",
    "^日本語/",
    "^Ελληνικά/",
    "^кириллица/",
    "é",
    "ü",
    "ß",
    "ñ",
    "ç",
    "ø",
    "語",
    "λ",
    "ж",
    r"\.txt$",
    r"\.md$",
    "(?i)Ä",
    "^[^/]+/[^/]+$",
    "[àâäÄ]",
    "[ёжз]",
    "[αβγ]",
];

/// B12u shape pin (P15): `run --all-files` with twenty Unicode `files` patterns over 2 000 non-ASCII paths and
/// `SNP_JOBS=32`. No stash, so only the warm-run git queries; each hook runs in at most 32 partitions (never once per
/// file), and only `true` is spawned for the hooks.
#[test]
#[cfg_attr(
    not(feature = "testing"),
    ignore = "needs a `--features testing` build (spawn log)"
)]
fn b12u_shape_is_pinned() {
    let env = TestEnv::new(bins());
    let mut config = String::from("repos:\n-   repo: local\n    hooks:\n");
    for (i, pat) in UNICODE_PATTERNS.iter().enumerate() {
        write!(
            config,
            "    -   id: u{i:02}\n        name: unicode {i:02}\n        entry: 'true'\n        \
             language: system\n        files: '{pat}'\n"
        )
        .unwrap();
    }
    let dirs = ["données", "日本語", "Ελληνικά", "кириллица"];
    let names = [
        "éte", "über", "straße", "año", "garçon", "søster", "Äpfel", "ёж", "αβγ", "語",
    ];
    let mut b = env
        .git_repo("b12u")
        .file(".pre-commit-config.yaml", config.as_bytes());
    for i in 0..2_000_usize {
        let dir = dirs[i % dirs.len()];
        let name = names[i % names.len()];
        let ext = if i % 2 == 0 { "txt" } else { "md" };
        b = b.file(&format!("{dir}/{name}-{i:04}.{ext}"), b"ligne\n");
    }
    let repo = b.stage_all().commit("init").build();
    let (out, log) = run_logged(&env, &repo, &["run", "--all-files"], &[("SNP_JOBS", "32")]);
    let max_hooks = UNICODE_PATTERNS.len() * 32;
    assert_eq!(out.code, 0, "{out:?}");
    assert!(
        log.git() <= WARM_RUN_GIT + windows_diff_spawns(UNICODE_PATTERNS.len()),
        "B12u git spawns\n{}",
        log.dump()
    );
    assert!(
        (UNICODE_PATTERNS.len()..=max_hooks).contains(&log.hooks()),
        "B12u: {} hook spawns, expected 20..={max_hooks}\n{}",
        log.hooks(),
        log.dump()
    );
    assert!(
        log.spawns
            .iter()
            .filter(|s| s.role == "hook")
            .all(|s| s.exe == "true" || s.exe.eq_ignore_ascii_case("true.exe")),
        "{}",
        log.dump()
    );
}

/// The spawn log records the group of each spawn; a hook never runs in the `critical` role.
#[test]
#[cfg_attr(
    not(feature = "testing"),
    ignore = "needs a `--features testing` build (spawn log)"
)]
fn spawn_roles_are_well_formed() {
    let env = TestEnv::new(bins());
    let repo = noop_repo(&env, "roles", &noop_config(3));
    let (out, log) = run_logged(&env, &repo, &["run"], &[]);
    assert_eq!(out.code, 0, "{}", String::from_utf8_lossy(&out.stderr));
    for s in &log.spawns {
        assert!(
            matches!(s.role.as_str(), "hook" | "tool" | "critical"),
            "{s:?}"
        );
        assert!(matches!(s.group.as_str(), "inherit" | "isolated"), "{s:?}");
        assert!(!(s.role == "hook" && s.is_git()), "{s:?}");
    }
}

/// The definition of done's 3× margin over a timing budget of ARCHITECTURE §12.1 for this OS (Linux, macOS, Windows): the
/// perf-smoke job runs these release-mode tests; `cargo xtask bench gate` enforces the budgets themselves on the
/// `dist` build with hyperfine.
fn smoke_budget(linux_ms: u64, macos_ms: u64, windows_ms: u64) -> std::time::Duration {
    let ms = if cfg!(target_os = "macos") {
        macos_ms
    } else if cfg!(windows) {
        windows_ms
    } else {
        linux_ms
    };
    std::time::Duration::from_millis(ms * 3)
}

/// The median wall time of `runs` invocations of `snp <args>` in `cwd` (after two warm-up runs).
fn median_wall(env: &TestEnv, cwd: &Path, args: &[&str], runs: usize) -> std::time::Duration {
    for _ in 0..2 {
        let out = env.snp(cwd).args(args).output();
        assert_eq!(out.code, 0, "{}", String::from_utf8_lossy(&out.stderr));
    }
    let mut times: Vec<std::time::Duration> = (0..runs)
        .map(|_| {
            let t = std::time::Instant::now();
            let out = env.snp(cwd).args(args).output();
            let elapsed = t.elapsed();
            assert_eq!(out.code, 0, "{}", String::from_utf8_lossy(&out.stderr));
            elapsed
        })
        .collect();
    times.sort();
    times[runs / 2]
}

/// B1 (`--version`, ≤ 3 ms on Linux, 5 ms on macOS, 15 ms on Windows) with the 3× margin.
#[test]
#[ignore = "perf"]
fn perf_b1_version_within_3x_budget() {
    let env = TestEnv::new(bins());
    let median = median_wall(&env, env.root(), &["--version"], 31);
    let budget = smoke_budget(3, 5, 15);
    assert!(median <= budget, "B1 median {median:?} > {budget:?}");
}

/// B2 (one `always_run` noop hook, ≤ 10 ms on Linux) with the 3× margin. A wall-clock bound: ignored in the default
/// gate (it must be deterministic) and run by the perf-smoke job in release mode.
#[test]
#[ignore = "perf"]
fn perf_b2_one_noop_hook_within_3x_budget() {
    let env = TestEnv::new(bins());
    let repo = noop_repo(&env, "b2-timing", &noop_config(10));
    let median = median_wall(&env, &repo, &["run", "noop-01"], 21);
    let budget = smoke_budget(10, 15, 100);
    assert!(median <= budget, "B2 median {median:?} > {budget:?}");
}

/// B3 (ten `always_run` noop hooks, ≤ 25 ms on Linux, 40 ms on macOS) with the 3× margin. A wall-clock bound:
/// ignored in the default gate and run by the perf-smoke job in release mode.
#[test]
#[ignore = "perf"]
fn perf_b3_ten_noop_hooks_within_3x_budget() {
    let env = TestEnv::new(bins());
    let repo = noop_repo(&env, "b3-timing", &noop_config(10));
    let median = median_wall(&env, &repo, &["run"], 21);
    let budget = smoke_budget(25, 40, 400);
    assert!(median <= budget, "B3 median {median:?} > {budget:?}");
}
