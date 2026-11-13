//! `cargo xtask bench setup|run|gate`: the end-to-end performance and memory gates of ARCHITECTURE §12 (research 10
//! §0.2, §7.11; ADR-043, ADR-047, ADR-071, ADR-079, ADR-099, ADR-100).
//!
//! * `setup` installs the measuring tools (hyperfine 1.19.0 and prek 0.5.10, both pinned; the pre-commit 4.7.0 oracle
//!   comes from `cargo xtask oracle install`), mirrors pre-commit-hooks locally (every clone is redirected to the
//!   mirror through `url.<mirror>.insteadOf`), downloads the hash-locked wheelhouse every Python install resolves from
//!   with no index and puts the pinned uv on the hermetic PATH (so no scenario touches the network), generates the
//!   deterministic
//!   fixture repositories of B1–B12 under `target/bench/fixtures` (fixed author, committer and dates: identical trees
//!   and SHAs on every machine), builds snp with the `dist` profile and primes every tool's warm store. With
//!   `--base-snp`, the base commit's binary gets its own stores and B9 fixture, primed by that binary (ADR-099).
//! * `run` measures each scenario × tool with `hyperfine -N` in the hermetic environment, peak RSS with
//!   `/usr/bin/time` (median of 10; the largest process of the tree), and optionally the `strace -f -c` syscall count
//!   (`--syscalls`, B1/B3), the dhat totals and heap peak (`--dhat`, a `--features dhat-heap` build) and the
//!   deterministic counters (`--counts`: clones, `statx`, `newfstatat`, opens, dirents, execs); it writes one JSON run
//!   record. With `--base-snp`, snp and the base binary run in interleaved ABBA rounds per scenario, pre-commit and
//!   prek right after (ADR-099).
//! * `gate` checks a run record against `bench/budgets.toml` and a baseline: the paired base rounds of the record itself
//!   (a regression fails when the median per-round ratio exceeds 5 % and 0.1 ms and its 95 % interval excludes 1,
//!   ADR-099), else an explicit record or the last record of the JSON-lines history
//!   `bench/history/<platform>-<variant>.jsonl` (a median regression above 5 % and 0.1 ms whose 95 % confidence
//!   intervals do not overlap; against the history: also against the median of its last 10 records, the drift check;
//!   `--advisory-history` reports both as warnings when the history comes from other machines, ADR-090). It also fails
//!   on a hard-budget breach, on a counter of the head above the base's beyond its margin (ADR-100), and when snp's
//!   p50 is slower than pre-commit's or prek's or that comparison could not be made; `--comment` writes the Markdown
//!   PR comment, `--record` appends to the history. `gate --micro` enforces the exact micro thresholds of the W1/W2
//!   acceptance lists on criterion's results (`cargo bench -p snp`). `gate --compare A B` prints the per-scenario
//!   effect of B against A (ADR-047 speculative config load on/off, ADR-079 musl system allocator vs mimalloc).
//!
//! Every subcommand prints `--help`. The statistics are distribution-free: the median and its 95 % order-statistic
//! confidence interval (ranks `n/2 ∓ 1.96·√n/2`).
//!
//! Owner: WP-36.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, bail, ensure};
use serde_json::{Value, json};

use crate::workspace::{self, say};

/// The pinned hyperfine release (`cargo install --locked`).
const HYPERFINE_VERSION: &str = "1.19.0";
/// The pinned prek release (hash-locked in `bench/prek-requirements.txt`).
const PREK_VERSION: &str = "0.5.10";
/// The pre-commit-hooks tag of B6/B7 (the snp fast-path tag).
const PCH_REV: &str = "v6.0.0";
/// The upstream URL every pre-commit-hooks clone is redirected from.
const PCH_URL: &str = "https://github.com/pre-commit/pre-commit-hooks";
/// hyperfine's warm-up and measured runs (ARCHITECTURE §12.3).
const WARMUP: u32 = 5;
const RUNS: u32 = 50;
/// `/usr/bin/time` runs per RSS measurement (the median is kept).
const RSS_RUNS: u32 = 10;
/// The fixed identity and dates of every fixture commit.
const GIT_DATE: &str = "2024-01-01T00:00:00Z";
const GIT_NAME: &str = "snp bench";
const GIT_EMAIL: &str = "bench@snp.invalid";
/// The run-record schema version written; schema 1 records (before the paired rounds, counters and host facts of
/// ADR-099/098) are still read, so a history mixing both stays usable.
const SCHEMA: u64 = 2;
/// The run-record schema versions read.
const SCHEMAS_READ: [u64; 2] = [1, 2];
/// The default PATH of the hermetic environment on Unix (`--path` overrides it).
const UNIX_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

const USAGE: &str =
    "usage: cargo xtask bench <setup|run|gate> [options]   (each subcommand takes --help)";

const SETUP_HELP: &str = "\
usage: cargo xtask bench setup [options]

Installs hyperfine and prek, mirrors pre-commit-hooks, generates the fixtures, builds snp (dist) and primes the
warm stores. Everything lives under target/bench.

  --scenarios B1,B3,..   only the fixtures of these scenarios (default: all)
  --tools snp,prek,..    only prime these tools (default: snp,pre-commit,prek)
  --snp PATH             use this snp binary instead of building the dist profile
  --base-snp PATH        also prime this binary (the base commit's snp) with its own stores and B9 fixture
  --target TRIPLE        build snp for this target (e.g. x86_64-unknown-linux-musl)
  --allocator A          system (default) or mimalloc (feature musl-mimalloc; musl targets only)
  --mirror-from SRC      clone the pre-commit-hooks mirror from SRC (default: the GitHub URL)
  --path PATH            the PATH of the hermetic environment (Unix; default /usr/local/bin:/usr/bin:/bin)
  --jobs N               cargo --jobs of the snp build and the hyperfine install (default: CARGO_BUILD_JOBS or all cores)
  --no-install           fail instead of installing a missing hyperfine/prek";

const RUN_HELP: &str = "\
usage: cargo xtask bench run [options]

Measures every scenario x tool (hyperfine -N -w 5 -r 50; RSS by /usr/bin/time, median of 10: the largest process
of the tree) and writes a run record (default target/bench/results/<label>.json).

  --scenarios B1,B3,..   the scenarios (default: all)
  --tools snp,prek,..    the tools (default: snp,pre-commit,prek)
  --snp PATH             measure this snp binary instead of building the dist profile
  --base-snp PATH        the base commit's snp (primed by `setup --base-snp`): measured in interleaved ABBA rounds
                         with snp on every scenario, pre-commit and prek right after; the gate pairs the rounds
  --target TRIPLE        build snp for this target (the record's variant names it)
  --allocator A          system (default) or mimalloc (musl targets only)
  --label NAME           the record name (default: current)
  --out FILE             write the record here instead
  --runs N / --warmup N  override hyperfine's counts (50 / 5; the slow scenarios use fewer; with --base-snp, N is
                         split into the rounds)
  --syscalls             also count snp's syscalls with `strace -f -c` (Linux; B1, B3)
  --counts               also count clones, statx, newfstatat, openat, getdents64 and execve over the tree with
                         `strace -f --seccomp-bpf -c` (Linux; every scenario, snp and the base) and diff the base's
                         and snp's exec argv lists
  --dhat                 also measure the dhat totals and heap peak of B2/B3/B4/B6/B8/B10/B12u with a
                         `--features dhat-heap` build (a second, full release build in target/bench/dhat-target)
  --jobs N               cargo --jobs of the snp builds (default: CARGO_BUILD_JOBS or all cores)
  --path PATH            the PATH of the hermetic environment (Unix)";

const GATE_HELP: &str = "\
usage: cargo xtask bench gate [options]
       cargo xtask bench gate --micro --since SECS [--criterion DIR]
       cargo xtask bench gate --compare A.json B.json

Checks a run record against bench/budgets.toml and a baseline; exits non-zero on a failed check. A record measured
with `run --base-snp` carries its own baseline: the paired rounds gate the regression (and the counters of
`--counts` gate clones, statx and newfstatat against the base), and every comparison with another record is
advisory.

  --current FILE         the record to check (default <target-dir>/bench/results/current.json)
  --baseline FILE        the baseline record (default: the last history record of the same platform and variant)
  --history DIR          the JSON-lines history (default bench/history)
  --advisory-history     the history comes from other machines (hosted CI runners): the regression and drift checks
                         against it warn instead of failing; everything else still gates (not with --baseline)
  --comment FILE         write the Markdown PR comment here
  --record               append the current record to the history when the gate passes
  --allow-missing LIST   scenarios not measured on purpose (comma-separated): their budgets warn instead of failing
  --allow-missing-reference LIST
                         scenarios whose pre-commit/prek reference may fail or be missing: the never-slower rule
                         warns instead of failing
  --micro                check the criterion results of `cargo bench -p snp` against the [[micro]] thresholds
  --criterion DIR        criterion's output directory (default <target-dir>/criterion)
  --since SECS           --micro: ignore results older than this Unix time (stale results of an earlier run);
                         required unless --any-age
  --any-age              --micro: accept criterion results of any age
  --reference            --micro: this is the reference machine (`reference_only` thresholds are hard)
  --compare A B          print the per-scenario effect of record B against record A (no gate)";

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    let args: Vec<String> = args
        .iter()
        .map(|a| {
            a.to_str()
                .map(str::to_owned)
                .with_context(|| format!("non-UTF-8 argument {}", a.to_string_lossy()))
        })
        .collect::<anyhow::Result<_>>()?;
    let Some((sub, rest)) = args.split_first() else {
        bail!("{USAGE}");
    };
    let root = workspace::root()?;
    match sub.as_str() {
        "setup" => setup(&root, rest),
        "run" => measure(&root, rest),
        "gate" => gate(&root, rest),
        "-h" | "--help" | "help" => {
            say(USAGE);
            Ok(())
        }
        other => bail!("unknown subcommand {other:?}\n{USAGE}"),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Arguments
// ---------------------------------------------------------------------------------------------------------------

/// Parsed `--name value` / `--name=value` options and `--switch`es.
#[derive(Debug, Default)]
struct Opts {
    values: BTreeMap<&'static str, Vec<String>>,
    switches: BTreeSet<&'static str>,
    positional: Vec<String>,
}

impl Opts {
    /// Parses `args` against the known valued options and switches; `--help` returns `Ok(None)`.
    fn parse(
        args: &[String],
        valued: &[&'static str],
        switches: &[&'static str],
    ) -> anyhow::Result<Option<Self>> {
        let mut o = Self::default();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if a == "-h" || a == "--help" {
                return Ok(None);
            }
            let Some(body) = a.strip_prefix("--") else {
                o.positional.push(a.clone());
                continue;
            };
            let (name, inline) = match body.split_once('=') {
                Some((n, v)) => (n, Some(v.to_owned())),
                None => (body, None),
            };
            if let Some(&k) = valued.iter().find(|k| **k == name) {
                let v = match inline {
                    Some(v) => v,
                    None => it
                        .next()
                        .cloned()
                        .with_context(|| format!("--{k} needs a value"))?,
                };
                o.values.entry(k).or_default().push(v);
            } else if let Some(&k) = switches.iter().find(|k| **k == name) {
                ensure!(inline.is_none(), "--{k} takes no value");
                o.switches.insert(k);
            } else {
                bail!("unknown option --{name}");
            }
        }
        Ok(Some(o))
    }

    /// The last value of `--name`.
    fn one(&self, name: &str) -> Option<&str> {
        self.values
            .get(name)
            .and_then(|v| v.last())
            .map(String::as_str)
    }

    /// Every comma-separated item of every `--name`.
    fn list(&self, name: &str) -> Vec<String> {
        self.values
            .get(name)
            .into_iter()
            .flatten()
            .flat_map(|v| v.split(','))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    }

    fn has(&self, name: &str) -> bool {
        self.switches.contains(name)
    }

    fn number(&self, name: &str) -> anyhow::Result<Option<u32>> {
        self.one(name)
            .map(|v| {
                v.parse::<u32>()
                    .with_context(|| format!("--{name} {v:?}: not a count"))
            })
            .transpose()
    }

    fn no_positional(&self) -> anyhow::Result<()> {
        ensure!(
            self.positional.is_empty(),
            "unexpected argument(s): {}",
            self.positional.join(" ")
        );
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Tools, scenarios, fixtures
// ---------------------------------------------------------------------------------------------------------------

/// A measured tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Tool {
    Snp,
    /// The base commit's snp (`--base-snp`, ADR-099): its own stores and B9 fixture, never selected by `--tools`.
    SnpBase,
    PreCommit,
    Prek,
}

impl Tool {
    /// The tools `--tools` selects from (the base comes from `--base-snp`).
    const ALL: [Self; 3] = [Self::Snp, Self::PreCommit, Self::Prek];

    const fn name(self) -> &'static str {
        match self {
            Self::Snp => "snp",
            Self::SnpBase => "snp-base",
            Self::PreCommit => "pre-commit",
            Self::Prek => "prek",
        }
    }

    fn parse(s: &str) -> anyhow::Result<Self> {
        Self::ALL
            .into_iter()
            .find(|t| t.name() == s)
            .with_context(|| format!("unknown tool {s:?} (snp, pre-commit, prek)"))
    }

    /// snp itself or the base commit's snp: `SNP_BIN`, the scenario's snp environment and snp's store layout.
    const fn is_snp(self) -> bool {
        matches!(self, Self::Snp | Self::SnpBase)
    }

    /// The environment variable naming the tool's store.
    const fn store_var(self) -> &'static str {
        match self {
            Self::Snp | Self::SnpBase => "SNP_HOME",
            Self::PreCommit => "PRE_COMMIT_HOME",
            Self::Prek => "PREK_HOME",
        }
    }
}

/// The fixture repository a scenario runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Fixture {
    /// Ten local `true` hooks (`always_run`, `pass_filenames: false`), one tracked file (prek's noop corpus).
    Noop10,
    /// [`Fixture::Noop10`] with the tool's own `install`ed hook script (one copy per tool).
    Hooked,
    /// 50 000 tracked files; one `true` hook with filenames (`.pre-commit-config.yaml`) and the same hook with
    /// `types`/`types_or`/`exclude_types` (`types.yaml`).
    Files50k,
    /// 960 clean files and the 13 pre-commit-hooks hooks at v6.0.0 (prek's medium corpus).
    Pch960,
    /// A 10 MiB tracked file with a 10 MiB unstaged rewrite and one staged change.
    Stash10M,
    /// A configuration of 1 000 local hooks (`big.yaml`).
    Validate1000,
    /// 500 hook repositories referenced by 200 configurations, all installed in a dedicated store.
    Gc500,
    /// 20 000 non-ASCII paths and 20 `true` hooks with Unicode `files` patterns.
    Unicode20,
}

impl Fixture {
    const fn dir(self) -> &'static str {
        match self {
            Self::Noop10 => "noop10",
            Self::Hooked => "hooked",
            Self::Files50k => "files50k",
            Self::Pch960 => "pch960",
            Self::Stash10M => "stash10m",
            Self::Validate1000 => "validate1000",
            Self::Gc500 => "gc500",
            Self::Unicode20 => "unicode20",
        }
    }
}

/// Which store a scenario uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StoreMode {
    /// The tool's warm store `stores/<tool>`.
    Warm,
    /// `stores/<tool>-cold`, removed before every run (hyperfine `--prepare`).
    Cold,
    /// `stores/<tool>-gc`, primed with the 500 repositories of [`Fixture::Gc500`].
    Gc,
}

/// One end-to-end scenario of ARCHITECTURE §12.1.
#[derive(Debug)]
struct Scenario {
    id: &'static str,
    title: &'static str,
    fixture: Fixture,
    /// The tool's arguments; `None`: the command is `git commit` through the installed hook (B9).
    argv: Option<&'static [&'static str]>,
    store: StoreMode,
    /// Peak RSS is measured.
    rss: bool,
    /// `(warmup, runs)` replacing hyperfine's 5 / 50 for the slow scenarios.
    runs: Option<(u32, u32)>,
    /// Extra environment for snp only.
    snp_env: &'static [(&'static str, &'static str)],
}

const B9_GIT: &[&str] = &["commit", "--allow-empty", "-q", "-m", "bench"];

const SCENARIOS: &[Scenario] = &[
    Scenario {
        id: "B1",
        title: "--version",
        fixture: Fixture::Noop10,
        argv: Some(&["--version"]),
        store: StoreMode::Warm,
        rss: false,
        runs: None,
        snp_env: &[],
    },
    Scenario {
        id: "B2",
        title: "run, 1 local `true` hook (always_run, no filenames)",
        fixture: Fixture::Noop10,
        argv: Some(&["run", "noop-01"]),
        store: StoreMode::Warm,
        rss: false,
        runs: None,
        snp_env: &[],
    },
    Scenario {
        id: "B3",
        title: "run, 10 such hooks, nothing staged",
        fixture: Fixture::Noop10,
        argv: Some(&["run"]),
        store: StoreMode::Warm,
        rss: true,
        runs: None,
        snp_env: &[],
    },
    Scenario {
        id: "B4",
        title: "run -a, 1 hook with filenames, 50 k files",
        fixture: Fixture::Files50k,
        argv: Some(&["run", "--all-files"]),
        store: StoreMode::Warm,
        rss: true,
        runs: Some((2, 20)),
        snp_env: &[],
    },
    Scenario {
        id: "B5",
        title: "B4 + types/types_or/exclude_types",
        fixture: Fixture::Files50k,
        argv: Some(&["run", "--all-files", "--config", "types.yaml"]),
        store: StoreMode::Warm,
        rss: false,
        runs: Some((2, 20)),
        snp_env: &[],
    },
    Scenario {
        id: "B6",
        title: "13 pre-commit-hooks hooks, 960 files, warm",
        fixture: Fixture::Pch960,
        argv: Some(&["run", "--all-files"]),
        store: StoreMode::Warm,
        rss: true,
        runs: Some((2, 20)),
        snp_env: &[],
    },
    Scenario {
        id: "B7",
        title: "B6 cold (fresh store, local mirror)",
        fixture: Fixture::Pch960,
        argv: Some(&["run", "--all-files"]),
        store: StoreMode::Cold,
        rss: false,
        runs: Some((1, 5)),
        snp_env: &[],
    },
    Scenario {
        id: "B8",
        title: "stash with a 10 MB unstaged patch",
        fixture: Fixture::Stash10M,
        argv: Some(&["run"]),
        store: StoreMode::Warm,
        rss: true,
        runs: Some((2, 20)),
        snp_env: &[],
    },
    Scenario {
        id: "B9",
        title: "git commit through the installed hook",
        fixture: Fixture::Hooked,
        argv: None,
        store: StoreMode::Warm,
        rss: false,
        runs: None,
        snp_env: &[],
    },
    Scenario {
        id: "B10",
        title: "validate-config with 1 000 hooks",
        fixture: Fixture::Validate1000,
        argv: Some(&["validate-config", "big.yaml"]),
        store: StoreMode::Warm,
        rss: false,
        runs: None,
        snp_env: &[],
    },
    Scenario {
        id: "B11",
        title: "gc with 500 repos / 200 configs",
        fixture: Fixture::Gc500,
        argv: Some(&["gc"]),
        store: StoreMode::Gc,
        rss: false,
        runs: Some((2, 20)),
        snp_env: &[],
    },
    Scenario {
        id: "B12u",
        title: "B12 regex cache: 20 Unicode patterns x 32 jobs, non-ASCII paths",
        fixture: Fixture::Unicode20,
        argv: Some(&["run", "--all-files"]),
        store: StoreMode::Warm,
        rss: true,
        runs: Some((2, 20)),
        snp_env: &[("SNP_JOBS", "32")],
    },
];

fn scenario(id: &str) -> anyhow::Result<&'static Scenario> {
    SCENARIOS
        .iter()
        .find(|s| s.id == id)
        .with_context(|| format!("unknown scenario {id:?}"))
}

fn selected_scenarios(o: &Opts) -> anyhow::Result<Vec<&'static Scenario>> {
    let ids = o.list("scenarios");
    if ids.is_empty() {
        return Ok(SCENARIOS.iter().collect());
    }
    ids.iter().map(|id| scenario(id)).collect()
}

fn selected_tools(o: &Opts) -> anyhow::Result<Vec<Tool>> {
    let names = o.list("tools");
    if names.is_empty() {
        return Ok(Tool::ALL.to_vec());
    }
    let mut out: Vec<Tool> = names
        .iter()
        .map(|n| Tool::parse(n))
        .collect::<anyhow::Result<_>>()?;
    out.sort();
    out.dedup();
    Ok(out)
}

// ---------------------------------------------------------------------------------------------------------------
// The bench workspace: paths, the hermetic environment, process helpers
// ---------------------------------------------------------------------------------------------------------------

/// `target/bench` and the paths below it.
#[derive(Debug, Clone)]
struct Work {
    root: PathBuf,
    target_dir: PathBuf,
    dir: PathBuf,
    path_var: OsString,
}

/// cargo's target directory of the workspace at `root` (`CARGO_TARGET_DIR` and `build.target-dir` respected): `run`,
/// `setup` and `gate` must agree on where the records and criterion's results live.
fn target_dir(root: &Path) -> anyhow::Result<PathBuf> {
    let meta = workspace::metadata(root)?;
    Ok(meta
        .get("target_directory")
        .and_then(Value::as_str)
        .map_or_else(|| root.join("target"), PathBuf::from))
}

impl Work {
    fn new(root: &Path, o: &Opts) -> anyhow::Result<Self> {
        let target_dir = target_dir(root)?;
        let dir = target_dir.join("bench");
        let path_var = OsString::from(o.one("path").unwrap_or(UNIX_PATH));
        Ok(Self {
            root: root.to_path_buf(),
            target_dir,
            dir,
            path_var,
        })
    }

    fn home(&self) -> PathBuf {
        self.dir.join("home")
    }

    fn gitconfig(&self) -> PathBuf {
        self.home().join(".gitconfig")
    }

    fn mirror(&self) -> PathBuf {
        self.dir.join("mirror").join("pre-commit-hooks.git")
    }

    fn fixture(&self, f: Fixture, tool: Tool) -> PathBuf {
        match f {
            Fixture::Hooked => self.dir.join("fixtures").join(tool.name()).join(f.dir()),
            _ => self.dir.join("fixtures").join(f.dir()),
        }
    }

    fn store(&self, tool: Tool, mode: StoreMode) -> PathBuf {
        let name = match mode {
            StoreMode::Warm => tool.name().to_owned(),
            StoreMode::Cold => format!("{}-cold", tool.name()),
            StoreMode::Gc => format!("{}-gc", tool.name()),
        };
        self.dir.join("stores").join(name)
    }

    fn tools_dir(&self) -> PathBuf {
        self.dir.join("tools")
    }

    /// The local wheels every Python install of the measured tools resolves from (no index, no network).
    fn wheelhouse(&self) -> PathBuf {
        self.tools_dir().join("wheels")
    }

    /// The directory prepended to the hermetic PATH: the pinned `uv` (prek and snp would otherwise download one).
    fn path_dir(&self) -> PathBuf {
        self.tools_dir().join("path")
    }

    /// `GIT_CEILING_DIRECTORIES`: git never discovers a repository above `target/bench`. Every fixture and store
    /// repository lives below it, so a fixture whose `.git` is missing or broken (an interrupted setup) is an error
    /// instead of the enclosing snp checkout, where B9's `git commit` would land on a real branch and run its hooks.
    /// Both the given and the canonical spelling are listed: git compares the ceiling against the real path of the
    /// working directory.
    fn git_ceiling(&self) -> OsString {
        let mut dirs = vec![self.dir.clone()];
        if let Ok(real) = real_path(&self.dir)
            && real != self.dir
        {
            dirs.push(real);
        }
        std::env::join_paths(dirs).unwrap_or_else(|_| self.dir.clone().into_os_string())
    }

    fn setup_file(&self) -> PathBuf {
        self.dir.join("setup.json")
    }

    fn results(&self) -> PathBuf {
        self.dir.join("results")
    }

    /// The hermetic environment of every spawned tool (10 §7.3.2 shape): own HOME and caches, git config from
    /// `home/.gitconfig` only, fixed identity and dates, C.UTF-8, no colour, no prompts.
    fn env(&self) -> Vec<(OsString, OsString)> {
        let home = self.home();
        let mut v: Vec<(OsString, OsString)> = vec![
            ("HOME".into(), home.clone().into()),
            ("XDG_CACHE_HOME".into(), home.join(".cache").into()),
            ("XDG_CONFIG_HOME".into(), home.join(".config").into()),
            (
                "XDG_DATA_HOME".into(),
                home.join(".local").join("share").into(),
            ),
            ("GIT_CONFIG_GLOBAL".into(), self.gitconfig().into()),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ("GIT_TERMINAL_PROMPT".into(), "0".into()),
            ("GIT_AUTHOR_NAME".into(), GIT_NAME.into()),
            ("GIT_AUTHOR_EMAIL".into(), GIT_EMAIL.into()),
            ("GIT_AUTHOR_DATE".into(), GIT_DATE.into()),
            ("GIT_COMMITTER_NAME".into(), GIT_NAME.into()),
            ("GIT_COMMITTER_EMAIL".into(), GIT_EMAIL.into()),
            ("GIT_COMMITTER_DATE".into(), GIT_DATE.into()),
            ("LANG".into(), "C.UTF-8".into()),
            ("LC_ALL".into(), "C.UTF-8".into()),
            ("TZ".into(), "UTC".into()),
            ("TERM".into(), "dumb".into()),
            ("NO_COLOR".into(), "1".into()),
            ("GIT_CEILING_DIRECTORIES".into(), self.git_ceiling()),
            ("PIP_DISABLE_PIP_VERSION_CHECK".into(), "1".into()),
            // no measurement touches the network: Python installs resolve from the wheelhouse only (B6/B7's
            // ruamel.yaml and setuptools), virtualenv never checks for newer seed wheels
            ("PIP_NO_INDEX".into(), "1".into()),
            ("PIP_FIND_LINKS".into(), self.wheelhouse().into()),
            ("UV_NO_INDEX".into(), "1".into()),
            ("UV_FIND_LINKS".into(), self.wheelhouse().into()),
            ("UV_OFFLINE".into(), "1".into()),
            ("UV_PYTHON_DOWNLOADS".into(), "never".into()),
            ("VIRTUALENV_NO_PERIODIC_UPDATE".into(), "1".into()),
        ];
        if cfg!(windows) {
            v.push(("USERPROFILE".into(), home.into()));
        } else {
            let mut path = self.path_dir().into_os_string();
            path.push(":");
            path.push(&self.path_var);
            v.push(("PATH".into(), path));
        }
        v
    }

    /// The environment of `tool` with `mode`'s store (and snp's scenario extras).
    fn tool_env(
        &self,
        tool: Tool,
        mode: StoreMode,
        s: Option<&Scenario>,
    ) -> Vec<(OsString, OsString)> {
        let mut v = self.env();
        v.push((tool.store_var().into(), self.store(tool, mode).into()));
        if let (true, Some(s)) = (tool.is_snp(), s) {
            v.extend(
                s.snp_env
                    .iter()
                    .map(|(k, val)| (OsString::from(k), OsString::from(val))),
            );
        }
        v
    }

    /// A command in the hermetic environment: on Unix the inherited environment is cleared; on Windows (no
    /// portable PATH without reading the process environment) the hermetic variables override it.
    fn command(program: impl AsRef<OsStr>, env: &[(OsString, OsString)]) -> Command {
        let mut cmd = Command::new(program);
        if !cfg!(windows) {
            cmd.env_clear();
        }
        cmd.envs(env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null());
        cmd
    }
}

/// The OS's real path of `p`: what git resolves a working directory to when it compares it with
/// `GIT_CEILING_DIRECTORIES` or prints `--show-toplevel`.
#[expect(
    clippy::disallowed_methods,
    reason = "the comparison is with git's own (OS) realpath of the same directories, not Python's os.path.realpath; \
              both sides go through this function, so a Windows \\\\?\\ prefix cancels out"
)]
fn real_path(p: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(p)
}

/// Runs `cmd`; fails with its output when it exits non-zero; returns stdout.
fn run_ok(cmd: &mut Command, what: &str) -> anyhow::Result<String> {
    let out = cmd.output().with_context(|| format!("running {what}"))?;
    if !out.status.success() {
        bail!(
            "{what} failed ({}):\n{}{}",
            out.status,
            tail(&String::from_utf8_lossy(&out.stdout), 40),
            tail(&String::from_utf8_lossy(&out.stderr), 40)
        );
    }
    Ok(String::from_utf8_lossy_owned(out.stdout))
}

/// The last `n` lines of `s`.
fn tail(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let skip = lines.len().saturating_sub(n);
    let mut out = String::new();
    for l in lines.iter().skip(skip) {
        out.push_str(l);
        out.push('\n');
    }
    out
}

/// `git <args>` in `dir` with the hermetic environment.
fn git(work: &Work, dir: &Path, args: &[&str]) -> anyhow::Result<String> {
    let env = work.env();
    let mut cmd = Work::command("git", &env);
    cmd.args(args).current_dir(dir);
    run_ok(&mut cmd, &format!("git {}", args.join(" ")))
}

/// Writes `bytes` to `dir/rel`, creating parent directories.
fn write_file(dir: &Path, rel: &str, bytes: &[u8]) -> anyhow::Result<()> {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))
}

/// Marks `path` executable (Unix; git records the mode).
#[cfg_attr(
    not(unix),
    expect(
        clippy::unnecessary_wraps,
        reason = "fallible on Unix, a no-op elsewhere"
    )
)]
fn make_executable(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("chmod +x {}", path.display()))?;
    }
    #[cfg(not(unix))]
    {
        let _: &Path = path;
    }
    Ok(())
}

/// Removes `path` if it exists.
fn remove_tree(path: &Path) -> anyhow::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => {
            std::fs::remove_dir_all(path).with_context(|| format!("removing {}", path.display()))
        }
        Ok(_) => std::fs::remove_file(path).with_context(|| format!("removing {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("inspecting {}", path.display())),
    }
}

/// Copies the tree `from` to `to` (symlinks as symlinks on Unix, permissions kept by `fs::copy`).
fn copy_tree(from: &Path, to: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(to).with_context(|| format!("creating {}", to.display()))?;
    for entry in std::fs::read_dir(from).with_context(|| format!("reading {}", from.display()))? {
        let entry = entry?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_tree(&src, &dst)?;
        } else if ty.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(std::fs::read_link(&src)?, &dst)
                .with_context(|| format!("linking {}", dst.display()))?;
            #[cfg(not(unix))]
            bail!("symlink {} in a fixture (Unix only)", src.display());
        } else {
            std::fs::copy(&src, &dst).with_context(|| format!("copying {}", src.display()))?;
        }
    }
    Ok(())
}

/// One argument quoted for hyperfine's `--shell=none` splitter (POSIX shell words).
fn shell_quote(s: &str) -> String {
    let plain = !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./:=+,@%".contains(&b));
    if plain {
        return s.to_owned();
    }
    let mut out = String::from("'");
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

fn command_line(argv: &[OsString]) -> String {
    argv.iter()
        .map(|a| shell_quote(&a.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `<os>-<arch>` of the running host (the history key).
fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// A u64 as f64 without an `as` cast (exact below 2^53).
fn to_f64(n: u64) -> f64 {
    let hi = u32::try_from(n.checked_shr(32).unwrap_or(0)).unwrap_or(u32::MAX);
    let lo = u32::try_from(n & 0xffff_ffff).unwrap_or(u32::MAX);
    f64::from(hi).mul_add(4_294_967_296.0, f64::from(lo))
}

fn usize_f64(n: usize) -> f64 {
    to_f64(u64::try_from(n).unwrap_or(u64::MAX))
}

// ---------------------------------------------------------------------------------------------------------------
// Fixture generation (deterministic)
// ---------------------------------------------------------------------------------------------------------------

/// Numerical Recipes' LCG: a dependency-free deterministic stream.
#[derive(Debug, Clone)]
struct Lcg(u32);

impl Lcg {
    fn below(&mut self, n: usize) -> usize {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        usize::try_from(self.0.checked_shr(8).unwrap_or(0))
            .unwrap_or(0)
            .checked_rem(n)
            .unwrap_or(0)
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items.get(self.below(items.len())).copied().unwrap_or("x")
    }
}

const TOP: &[&str] = &[
    "src", "src", "src", "lib", "tests", "tests", "docs", "pkg", "app", "scripts", "tools",
    "assets", "examples",
];
const MID: &[&str] = &[
    "core",
    "util",
    "api",
    "models",
    "views",
    "handlers",
    "internal",
    "fixtures",
    "components",
    "commands",
    "io",
    "parser",
    "net",
    "store",
    "config",
    "cli",
    "web",
    "db",
    "auth",
    "data",
    "templates",
    "v1",
];
const STEM: &[&str] = &[
    "main", "mod", "lib", "index", "test_api", "conftest", "setup", "README", "utils", "helpers",
    "settings", "schema", "client", "server", "worker", "types", "errors", "app", "router",
    "styles",
];
const EXT: &[&str] = &[
    "py", "py", "py", "py", "js", "js", "ts", "tsx", "rs", "rs", "go", "md", "md", "rst", "txt",
    "yaml", "yml", "json", "toml", "cfg", "ini", "html", "css", "svg", "sh", "pyi", "xml", "c",
    "h",
];
const UNICODE: &[&str] = &[
    "données",
    "Ελληνικά",
    "русский",
    "日本語",
    "中文资料",
    "हिन्दी",
    "٣٤٥",
    "e\u{301}cole",
    "Straße",
    "naïve",
    "Ωmega",
    "über",
    "café",
    "测试",
    "Ünïcödé",
];

/// `n` unique repository-relative paths (the index suffix makes every name unique).
fn corpus(n: usize, seed: u32, unicode: bool) -> Vec<String> {
    let mut rng = Lcg(seed);
    let mut out = Vec::new();
    for i in 0..n {
        let mut p = String::from(rng.pick(if unicode { UNICODE } else { TOP }));
        for _ in 0..rng.below(4) {
            p.push('/');
            p.push_str(if unicode && rng.below(2) == 0 {
                rng.pick(UNICODE)
            } else {
                rng.pick(MID)
            });
        }
        let stem = if unicode {
            rng.pick(UNICODE)
        } else {
            rng.pick(STEM)
        };
        let ext = rng.pick(EXT);
        _ = write!(p, "/{stem}_{i}.{ext}");
        out.push(p);
    }
    out
}

/// The noop configuration of B2/B3 (prek's benchmark corpus): `n` local `system` hooks running `true`.
fn noop_config(n: usize) -> String {
    let mut s = String::from("repos:\n-   repo: local\n    hooks:\n");
    for i in 1..=n {
        _ = write!(
            s,
            "    -   id: noop-{i:02}\n        name: no-op {i:02}\n        entry: 'true'\n        \
             language: system\n        always_run: true\n        pass_filenames: false\n"
        );
    }
    s
}

/// The 13 pre-commit-hooks hooks of B6/B7.
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

/// The 20 Unicode-aware `files` patterns of B12u (the ADR-079 regex-cache case; YAML single-quoted, no `'`).
const UNICODE_PATTERNS: [&str; 20] = [
    "(?i)^données/",
    r"^\w+/\w+_\d+\.py$",
    "(?i)straße|strasse",
    r"[^\W\d_]{6,}_\d+\.",
    "^(?:日本語|中文资料)/",
    r"\b\w{3,8}\b/\w+\.md$",
    "(?i)ünïcödé",
    r"^[\w\-]+(?:/[\w\-]+){2,}\.json$",
    r"\d{3,}\.(?:ts|tsx|js)$",
    "(?i)^(?:ελληνικά|русский)/",
    "[αβγδεζηθ]",
    r"(?:\w+/){3}\w+_\d{2}\.",
    r"[\u4e00-\u9fff]{2,}",
    "(?i)caf[ée]",
    r"^\w*\d\w*/",
    "(?:^|/)測試|测试/",
    r"\W\w+_\d+\.min\.js$",
    r"(?i)naïve/.+\.ya?ml$",
    "[ऀ-ॿ]+",
    r"^\w+(?:/\w+)*/\w+_1\d*\.\w+$",
];

/// `git init` + the fixture files + one commit.
fn init_repo(work: &Work, dir: &Path) -> anyhow::Result<()> {
    remove_tree(dir)?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    git(work, dir, &["-c", "init.defaultBranch=main", "init", "-q"])?;
    Ok(())
}

/// `dir` must be the top level of its own git repository: git (with the bench's ceiling) resolves it to exactly
/// `dir`, so no command run there can reach another repository.
fn ensure_own_repo(work: &Work, dir: &Path) -> anyhow::Result<()> {
    let top = git(work, dir, &["rev-parse", "--show-toplevel"]).with_context(|| {
        format!(
            "{} is not a git repository (run `cargo xtask bench setup`)",
            dir.display()
        )
    })?;
    let top = PathBuf::from(top.trim_end_matches(['\n', '\r']));
    let real = |p: &Path| real_path(p).with_context(|| format!("resolving {}", p.display()));
    ensure!(
        real(&top)? == real(dir)?,
        "{} belongs to the repository at {}, not its own (run `cargo xtask bench setup`)",
        dir.display(),
        top.display()
    );
    Ok(())
}

fn commit_all(work: &Work, dir: &Path, msg: &str) -> anyhow::Result<()> {
    git(work, dir, &["add", "-A"])?;
    git(work, dir, &["commit", "-q", "--no-verify", "-m", msg])?;
    Ok(())
}

fn gen_noop10(work: &Work, dir: &Path) -> anyhow::Result<()> {
    init_repo(work, dir)?;
    write_file(dir, ".pre-commit-config.yaml", noop_config(10).as_bytes())?;
    write_file(dir, "README.md", b"# bench\n")?;
    commit_all(work, dir, "init")
}

fn gen_files50k(work: &Work, dir: &Path) -> anyhow::Result<()> {
    init_repo(work, dir)?;
    let hook = |extra: &str| {
        format!(
            "repos:\n-   repo: local\n    hooks:\n    -   id: true-files\n        name: true with files\n        \
             entry: 'true'\n        language: system\n{extra}"
        )
    };
    write_file(dir, ".pre-commit-config.yaml", hook("").as_bytes())?;
    write_file(
        dir,
        "types.yaml",
        hook(
            "        types: [text]\n        types_or: [python, javascript, markdown, yaml, json]\n        \
             exclude_types: [svg]\n",
        )
        .as_bytes(),
    )?;
    for (i, p) in corpus(50_000, 0x5eed_0001, false).iter().enumerate() {
        write_file(dir, p, format!("line {i}\n").as_bytes())?;
    }
    commit_all(work, dir, "init")
}

/// 960 files that pass all 13 hooks of [`B6_HOOKS`] (LF, final newline, no trailing whitespace, valid syntax).
fn gen_pch960(work: &Work, dir: &Path) -> anyhow::Result<()> {
    init_repo(work, dir)?;
    let mut config = format!("repos:\n-   repo: {PCH_URL}\n    rev: {PCH_REV}\n    hooks:\n");
    for id in B6_HOOKS {
        _ = writeln!(config, "    -   id: {id}");
    }
    write_file(dir, ".pre-commit-config.yaml", config.as_bytes())?;
    for i in 0..960_usize {
        let pkg = i.checked_rem(24).unwrap_or(0);
        match i.checked_rem(8).unwrap_or(0) {
            0 => write_file(
                dir,
                &format!("pkg{pkg:02}/mod_{i:03}.py"),
                format!("\"\"\"Module {i}.\"\"\"\n\n\ndef f_{i}(x):\n    return x + {i}\n")
                    .as_bytes(),
            )?,
            1 => write_file(
                dir,
                &format!("docs/sec{pkg:02}/page_{i:03}.md"),
                format!("# Page {i}\n\nSome text for page {i}.\n").as_bytes(),
            )?,
            2 => write_file(
                dir,
                &format!("data/set{pkg:02}/rec_{i:03}.json"),
                format!("{{\"id\": {i}, \"tags\": [\"a\", \"b\"], \"ok\": true}}\n").as_bytes(),
            )?,
            3 => write_file(
                dir,
                &format!("conf/env{pkg:02}/c_{i:03}.yaml"),
                format!("name: c{i}\nitems:\n  - one\n  - two\nnested:\n  key: {i}\n").as_bytes(),
            )?,
            4 => write_file(
                dir,
                &format!("conf/env{pkg:02}/t_{i:03}.toml"),
                format!("[tool]\nname = \"t{i}\"\nvalues = [1, 2, 3]\n").as_bytes(),
            )?,
            5 => write_file(
                dir,
                &format!("xml/x{pkg:02}/doc_{i:03}.xml"),
                format!("<?xml version=\"1.0\"?>\n<root><item id=\"{i}\">value</item></root>\n")
                    .as_bytes(),
            )?,
            6 => {
                let rel = format!("scripts/s{pkg:02}/run_{i:03}.sh");
                write_file(dir, &rel, format!("#!/bin/sh\necho {i}\n").as_bytes())?;
                make_executable(&dir.join(&rel))?;
            }
            _ => write_file(
                dir,
                &format!("notes/n{pkg:02}/note_{i:03}.txt"),
                format!("note {i}\nsecond line\n").as_bytes(),
            )?,
        }
    }
    commit_all(work, dir, "init")
}

/// ~10 MiB of numbered lines; `salt` changes every line.
fn big_text(salt: u32) -> String {
    let mut rng = Lcg(salt);
    let mut s = String::new();
    let mut i = 0_u64;
    while s.len() < 10 << 20 {
        _ = writeln!(
            s,
            "line {i:08} {:06} lorem ipsum dolor sit amet",
            rng.below(1_000_000)
        );
        i = i.saturating_add(1);
    }
    s
}

fn gen_stash10m(work: &Work, dir: &Path) -> anyhow::Result<()> {
    init_repo(work, dir)?;
    write_file(
        dir,
        ".pre-commit-config.yaml",
        b"repos:\n-   repo: local\n    hooks:\n    -   id: true-small\n        name: true on small.txt\n        \
          entry: 'true'\n        language: system\n        files: '^small\\.txt$'\n",
    )?;
    write_file(dir, "big.txt", big_text(1).as_bytes())?;
    write_file(dir, "small.txt", b"small\n")?;
    commit_all(work, dir, "init")?;
    // a staged change for the hook, a 10 MiB unstaged rewrite for the stash
    write_file(dir, "small.txt", b"small, changed\n")?;
    git(work, dir, &["add", "small.txt"])?;
    write_file(dir, "big.txt", big_text(2).as_bytes())
}

fn gen_validate1000(work: &Work, dir: &Path) -> anyhow::Result<()> {
    init_repo(work, dir)?;
    let mut s = String::from("repos:\n");
    for r in 0..10 {
        s.push_str("-   repo: local\n    hooks:\n");
        for h in 0..100 {
            _ = write!(
                s,
                "    -   id: r{r}-h{h:03}\n        name: hook {r}/{h}\n        entry: echo {h}\n        \
                 language: system\n        files: '\\.(py|pyi)$'\n        exclude: '^vendor/'\n        \
                 types_or: [python, pyi]\n        args: [--flag, '{h}']\n"
            );
        }
    }
    write_file(dir, "big.yaml", s.as_bytes())?;
    write_file(dir, ".pre-commit-config.yaml", noop_config(1).as_bytes())?;
    commit_all(work, dir, "init")
}

/// 500 hook repositories under `repos/` and 200 configurations under `configs/`: configuration `i` uses
/// repositories `i`, `200 + i` and (for `i < 100`) `400 + i`, so every repository is used.
fn gen_gc500(work: &Work, dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    init_repo(work, dir)?;
    let repos = dir.join("repos");
    let mut revs = Vec::new();
    for r in 0..500 {
        let rd = repos.join(format!("r{r:03}"));
        std::fs::create_dir_all(&rd).with_context(|| format!("creating {}", rd.display()))?;
        git(work, &rd, &["-c", "init.defaultBranch=main", "init", "-q"])?;
        write_file(
            &rd,
            ".pre-commit-hooks.yaml",
            format!(
                "-   id: h{r:03}\n    name: hook {r}\n    entry: 'true'\n    language: system\n"
            )
            .as_bytes(),
        )?;
        commit_all(work, &rd, "hook")?;
        revs.push(git(work, &rd, &["rev-parse", "HEAD"])?.trim().to_owned());
    }
    let mut configs = Vec::new();
    for c in 0..200_usize {
        let mut s = String::from("repos:\n");
        let mut used = vec![c, c.saturating_add(200)];
        if c < 100 {
            used.push(c.saturating_add(400));
        }
        for r in used {
            let rev = revs.get(r).context("500 revisions")?;
            let path = repos.join(format!("r{r:03}"));
            _ = write!(
                s,
                "-   repo: {}\n    rev: {rev}\n    hooks:\n    -   id: h{r:03}\n",
                path.display()
            );
        }
        let rel = format!("configs/c{c:03}.yaml");
        write_file(dir, &rel, s.as_bytes())?;
        configs.push(dir.join(rel));
    }
    write_file(dir, ".pre-commit-config.yaml", noop_config(1).as_bytes())?;
    // the hook repositories are separate repositories, not part of this one
    write_file(dir, ".gitignore", b"/repos/\n")?;
    commit_all(work, dir, "init")?;
    Ok(configs)
}

fn gen_unicode20(work: &Work, dir: &Path) -> anyhow::Result<()> {
    init_repo(work, dir)?;
    let mut s = String::from("repos:\n-   repo: local\n    hooks:\n");
    for (i, pat) in UNICODE_PATTERNS.iter().enumerate() {
        _ = write!(
            s,
            "    -   id: u{i:02}\n        name: unicode {i:02}\n        entry: 'true'\n        \
             language: system\n        files: '{pat}'\n"
        );
    }
    write_file(dir, ".pre-commit-config.yaml", s.as_bytes())?;
    for (i, p) in corpus(20_000, 0x5eed_0002, true).iter().enumerate() {
        write_file(dir, p, format!("ligne {i}\n").as_bytes())?;
    }
    git(work, dir, &["config", "core.quotePath", "false"])?;
    commit_all(work, dir, "init")
}

// ---------------------------------------------------------------------------------------------------------------
// setup
// ---------------------------------------------------------------------------------------------------------------

/// The resolved measuring tools.
#[derive(Debug, Clone)]
struct Tools {
    hyperfine: PathBuf,
    exes: BTreeMap<Tool, PathBuf>,
}

fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

fn venv_exe(venv: &Path, name: &str) -> PathBuf {
    venv.join(if cfg!(windows) { "Scripts" } else { "bin" })
        .join(exe_name(name))
}

/// Whether `program --version` runs.
fn runs(program: &Path) -> bool {
    Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .is_ok_and(|o| o.status.success())
}

/// hyperfine [`HYPERFINE_VERSION`]: `target/bench/tools/bin/hyperfine`, else `hyperfine` on PATH, else `cargo install
/// --locked` it. Another hyperfine release is never used: its JSON and `-N` semantics (e.g. `--prepare` under `-N`)
/// differ, and the records would come from a different measuring tool.
fn ensure_hyperfine(work: &Work, install: bool, jobs: Option<u32>) -> anyhow::Result<PathBuf> {
    let local = work.tools_dir().join("bin").join(exe_name("hyperfine"));
    if hyperfine_version_ok(&local) {
        return Ok(local);
    }
    if hyperfine_version_ok(Path::new("hyperfine")) {
        return Ok(PathBuf::from("hyperfine"));
    }
    ensure!(
        install,
        "hyperfine {HYPERFINE_VERSION} not found at {} or on PATH (run `cargo xtask bench setup` to install it)",
        local.display()
    );
    say(&format!("bench: installing hyperfine {HYPERFINE_VERSION}"));
    let mut cmd = Command::new("cargo");
    cmd.args(["install", "--locked", "--root"])
        .arg(work.tools_dir())
        .arg(format!("hyperfine@{HYPERFINE_VERSION}"))
        .env("CARGO_TARGET_DIR", work.tools_dir().join("build"));
    cargo_jobs(&mut cmd, jobs);
    run_ok(&mut cmd, "cargo install hyperfine")?;
    ensure!(
        hyperfine_version_ok(&local),
        "{} is not hyperfine {HYPERFINE_VERSION} after the install",
        local.display()
    );
    Ok(local)
}

/// Whether `exe --version` runs and prints `hyperfine <HYPERFINE_VERSION>`.
fn hyperfine_version_ok(exe: &Path) -> bool {
    Command::new(exe)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .is_ok_and(|o| {
            o.status.success() && is_hyperfine_version(&String::from_utf8_lossy(&o.stdout))
        })
}

/// `hyperfine --version` output names exactly [`HYPERFINE_VERSION`].
fn is_hyperfine_version(stdout: &str) -> bool {
    let mut words = stdout.split_whitespace();
    words.next() == Some("hyperfine") && words.next() == Some(HYPERFINE_VERSION)
}

/// prek: a venv `target/bench/tools/prek` from the hash-locked `bench/prek-requirements.txt` (wheels only).
fn ensure_prek(work: &Work, install: bool) -> anyhow::Result<PathBuf> {
    let venv = work.tools_dir().join("prek");
    let exe = venv_exe(&venv, "prek");
    if prek_version_ok(&exe) {
        return Ok(exe);
    }
    ensure!(
        install,
        "prek {PREK_VERSION} not found at {}",
        exe.display()
    );
    say(&format!("bench: installing prek {PREK_VERSION}"));
    remove_tree(&venv)?;
    run_ok(
        Command::new("uv")
            .args(["venv", "--no-config", "--quiet", "--python", "3.12"])
            .arg(&venv),
        "uv venv (prek)",
    )?;
    run_ok(
        Command::new("uv")
            .args([
                "pip",
                "install",
                "--no-config",
                "--quiet",
                "--require-hashes",
                "--only-binary",
                ":all:",
            ])
            .arg("--python")
            .arg(venv_exe(&venv, "python"))
            .arg("-r")
            .arg(work.root.join("bench").join("prek-requirements.txt")),
        "uv pip install prek",
    )?;
    ensure!(
        prek_version_ok(&exe),
        "{} is not prek {PREK_VERSION}",
        exe.display()
    );
    Ok(exe)
}

/// The pinned bench helpers and the wheelhouse that make every measurement offline: a venv `tools/bench-tools` from the
/// hash-locked `bench/tools-requirements.txt` (uv 0.13.5 — linked as `tools/path/uv`, the head of the hermetic PATH —
/// and pip), and `tools/wheels`, the hash-checked wheels of `bench/wheelhouse-requirements.txt` that pre-commit's pip,
/// prek's uv and snp install B6/B7's hook repository from (`PIP_NO_INDEX`, `UV_NO_INDEX`, `UV_OFFLINE`). Without
/// them, every cold B7 run would resolve ruamel.yaml and setuptools on PyPI and prek would download uv.
fn ensure_offline_tools(work: &Work, install: bool) -> anyhow::Result<()> {
    let venv = work.tools_dir().join("bench-tools");
    let uv = venv_exe(&venv, "uv");
    let python = venv_exe(&venv, "python");
    let wheels = work.wheelhouse();
    let lock = std::fs::read_to_string(work.root.join("bench").join("wheelhouse-requirements.txt"))
        .context("reading bench/wheelhouse-requirements.txt")?;
    let stamp = wheels.join(".requirements.txt");
    let wheels_ok = std::fs::read_to_string(&stamp).is_ok_and(|t| t == lock);
    if runs(&uv) && runs(&python) && wheels_ok {
        return link_uv(work, &uv);
    }
    ensure!(
        install,
        "the bench helper tools or the wheelhouse are missing under {} (run `cargo xtask bench setup`)",
        work.tools_dir().display()
    );
    if !(runs(&uv) && runs(&python)) {
        say("bench: installing the bench helper tools (uv, pip)");
        remove_tree(&venv)?;
        run_ok(
            Command::new("uv")
                .args(["venv", "--no-config", "--quiet", "--python", "3.12"])
                .arg(&venv),
            "uv venv (bench tools)",
        )?;
        run_ok(
            Command::new("uv")
                .args([
                    "pip",
                    "install",
                    "--no-config",
                    "--quiet",
                    "--require-hashes",
                    "--only-binary",
                    ":all:",
                ])
                .arg("--python")
                .arg(&python)
                .arg("-r")
                .arg(work.root.join("bench").join("tools-requirements.txt")),
            "uv pip install (bench tools)",
        )?;
    }
    say("bench: downloading the wheelhouse");
    remove_tree(&wheels)?;
    std::fs::create_dir_all(&wheels).with_context(|| format!("creating {}", wheels.display()))?;
    run_ok(
        Command::new(&python)
            .args([
                "-I",
                "-m",
                "pip",
                "download",
                "--quiet",
                "--disable-pip-version-check",
                "--no-deps",
                "--require-hashes",
                "--only-binary",
                ":all:",
                "-d",
            ])
            .arg(&wheels)
            .arg("-r")
            .arg(work.root.join("bench").join("wheelhouse-requirements.txt"))
            .stdin(Stdio::null()),
        "pip download (wheelhouse)",
    )?;
    std::fs::write(&stamp, lock).with_context(|| format!("writing {}", stamp.display()))?;
    link_uv(work, &uv)
}

/// `tools/path/uv` → the pinned uv.
fn link_uv(work: &Work, uv: &Path) -> anyhow::Result<()> {
    let dir = work.path_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let link = dir.join(exe_name("uv"));
    remove_tree(&link)?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(uv, &link).with_context(|| format!("linking {}", link.display()))?;
    #[cfg(not(unix))]
    std::fs::copy(uv, &link).with_context(|| format!("copying {}", link.display()))?;
    Ok(())
}

fn prek_version_ok(exe: &Path) -> bool {
    Command::new(exe)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .is_ok_and(|o| {
            o.status.success() && String::from_utf8_lossy(&o.stdout).contains(PREK_VERSION)
        })
}

/// The pre-commit 4.7.0 oracle `target/oracle/pc470-py312` (`cargo xtask oracle install`).
fn oracle_pre_commit(work: &Work) -> anyhow::Result<PathBuf> {
    let exe = venv_exe(
        &work.target_dir.join("oracle").join("pc470-py312"),
        "pre-commit",
    );
    ensure!(
        runs(&exe),
        "{} does not run: create the oracle with `cargo xtask oracle install`",
        exe.display()
    );
    Ok(exe)
}

/// The snp build variant: `<gnu|musl|msvc|…>-<allocator>`.
#[derive(Debug, Clone)]
struct Build {
    target: Option<String>,
    mimalloc: bool,
    /// `--jobs N` of every cargo invocation (`None`: cargo's default, i.e. `CARGO_BUILD_JOBS` or every core).
    jobs: Option<u32>,
}

impl Build {
    fn from_opts(o: &Opts) -> anyhow::Result<Self> {
        let target = o.one("target").map(str::to_owned);
        let mimalloc = match o.one("allocator").unwrap_or("system") {
            "system" => false,
            "mimalloc" => true,
            other => bail!("--allocator {other:?}: system or mimalloc"),
        };
        ensure!(
            !mimalloc || target.as_deref().is_some_and(|t| t.ends_with("-musl")),
            "--allocator mimalloc applies to musl targets only (ADR-079)"
        );
        Ok(Self {
            target,
            mimalloc,
            jobs: o.number("jobs")?,
        })
    }

    fn variant(&self) -> String {
        let env = self
            .target
            .as_deref()
            .and_then(|t| t.rsplit('-').next())
            .unwrap_or(if cfg!(target_env = "musl") {
                "musl"
            } else if cfg!(target_env = "msvc") {
                "msvc"
            } else {
                "gnu"
            });
        format!(
            "{env}-{}",
            if self.mimalloc { "mimalloc" } else { "system" }
        )
    }

    /// `cargo build --profile dist -p snp --bin snp` → the binary.
    fn build(&self, work: &Work) -> anyhow::Result<PathBuf> {
        let mut cmd = Command::new("cargo");
        cmd.args([
            "build",
            "--profile",
            "dist",
            "--locked",
            "-p",
            "snp",
            "--bin",
            "snp",
        ])
        .current_dir(&work.root);
        let mut dir = work.target_dir.clone();
        if let Some(t) = &self.target {
            cmd.args(["--target", t]);
            dir.push(t);
        }
        if self.mimalloc {
            cmd.args(["--features", "musl-mimalloc"]);
        }
        cargo_jobs(&mut cmd, self.jobs);
        say(&format!("bench: building snp ({}, dist)", self.variant()));
        run_ok(&mut cmd, "cargo build --profile dist")?;
        Ok(dir.join("dist").join(exe_name("snp")))
    }
}

/// `--jobs N` on a cargo command (the bench builds share the machine with everything else running on it).
fn cargo_jobs(cmd: &mut Command, jobs: Option<u32>) {
    if let Some(n) = jobs {
        cmd.arg("--jobs").arg(n.to_string());
    }
}

fn resolve_snp(work: &Work, o: &Opts, build: &Build) -> anyhow::Result<PathBuf> {
    match o.one("snp") {
        Some(p) => existing_binary("snp", p),
        None => build.build(work),
    }
}

/// `--<option> PATH` made absolute (hyperfine and the tools run in the fixture directories); it must run.
fn existing_binary(option: &str, p: &str) -> anyhow::Result<PathBuf> {
    let p = std::path::absolute(p).with_context(|| format!("--{option} {p}"))?;
    ensure!(runs(&p), "--{option} {}: does not run", p.display());
    Ok(p)
}

/// The base commit's snp of `--base-snp` (ADR-099), if given.
fn resolve_base(o: &Opts) -> anyhow::Result<Option<PathBuf>> {
    o.one("base-snp")
        .map(|p| existing_binary("base-snp", p))
        .transpose()
}

/// The binary of a `--tools` tool.
fn resolve_tool(
    work: &Work,
    o: &Opts,
    build: &Build,
    t: Tool,
    install: bool,
) -> anyhow::Result<PathBuf> {
    match t {
        Tool::Snp => resolve_snp(work, o, build),
        Tool::SnpBase => resolve_base(o)?.context("--base-snp is required for the base"),
        Tool::PreCommit => oracle_pre_commit(work),
        Tool::Prek => ensure_prek(work, install),
    }
}

/// `home/.gitconfig`: identity, `main`, no autocrlf, and the redirect of pre-commit-hooks to the mirror.
fn write_gitconfig(work: &Work) -> anyhow::Result<()> {
    let mirror = work.mirror();
    let url = if cfg!(windows) {
        format!(
            "file:///{}",
            mirror.display().to_string().replace('\\', "/")
        )
    } else {
        format!("file://{}", mirror.display())
    };
    let text = format!(
        "[user]\n\tname = {GIT_NAME}\n\temail = {GIT_EMAIL}\n[init]\n\tdefaultBranch = main\n[core]\n\t\
         autocrlf = false\n[advice]\n\tdetachedHead = false\n[url \"{url}\"]\n\tinsteadOf = {PCH_URL}\n"
    );
    std::fs::create_dir_all(work.home().join(".cache"))?;
    std::fs::write(work.gitconfig(), text).context("writing the bench .gitconfig")
}

/// The bare pre-commit-hooks mirror (cloned once; the source must have `PCH_REV`). A mirror directory without `HEAD`
/// is what an interrupted clone (or a partially restored cache) leaves behind: it is removed and cloned again, since
/// `git clone` refuses an existing non-empty destination.
fn ensure_mirror(work: &Work, from: Option<&str>) -> anyhow::Result<()> {
    let mirror = work.mirror();
    if mirror.join("HEAD").is_file() {
        return Ok(());
    }
    if std::fs::symlink_metadata(&mirror).is_ok() {
        say(&format!(
            "bench: removing the incomplete mirror {} (no HEAD)",
            mirror.display()
        ));
        remove_tree(&mirror)?;
    }
    let src = from.unwrap_or(PCH_URL);
    say(&format!("bench: mirroring pre-commit-hooks from {src}"));
    // a partial mirror (an interrupted clone, or a CI cache restore that kept the directory without its HEAD) would
    // make `git clone` refuse the non-empty destination: start over
    if mirror.exists() {
        std::fs::remove_dir_all(&mirror)
            .with_context(|| format!("removing the incomplete mirror {}", mirror.display()))?;
    }
    if let Some(parent) = mirror.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // the clone must not see the bench gitconfig, which redirects the upstream URL to this very mirror
    let mut cmd = Command::new("git");
    cmd.args(["clone", "--quiet", "--bare", src])
        .arg(&mirror)
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null());
    run_ok(&mut cmd, "git clone --bare (pre-commit-hooks mirror)")?;
    git(
        work,
        &mirror,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/tags/{PCH_REV}"),
        ],
    )
    .with_context(|| format!("the mirror has no tag {PCH_REV}"))?;
    Ok(())
}

/// What `setup` leaves for `run`.
#[derive(Debug, Default)]
struct SetupState {
    unavailable: BTreeMap<String, String>,
}

impl SetupState {
    fn key(s: &str, t: Tool) -> String {
        format!("{s}/{}", t.name())
    }

    fn load(work: &Work) -> anyhow::Result<Self> {
        let path = work.setup_file();
        let text = std::fs::read_to_string(&path).with_context(|| {
            format!(
                "reading {} (run `cargo xtask bench setup` first)",
                path.display()
            )
        })?;
        let v: Value =
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        let unavailable = v
            .get("unavailable")
            .and_then(Value::as_object)
            .map_or_default(|m| {
                m.iter()
                    .map(|(k, r)| (k.clone(), r.as_str().unwrap_or_default().to_owned()))
                    .collect()
            });
        Ok(Self { unavailable })
    }

    fn save(&self, work: &Work, tools: &Tools) -> anyhow::Result<()> {
        let exes: serde_json::Map<String, Value> = tools
            .exes
            .iter()
            .map(|(t, p)| (t.name().to_owned(), json!(p.display().to_string())))
            .collect();
        let v = json!({
            "schema": SCHEMA,
            "hyperfine": tools.hyperfine.display().to_string(),
            "tools": exes,
            "unavailable": self.unavailable,
        });
        std::fs::write(work.setup_file(), serde_json::to_string_pretty(&v)? + "\n")
            .context("writing setup.json")
    }
}

/// The `unavailable` reason of a scenario whose setup has not finished (an interrupted setup leaves it behind).
const SETUP_IN_PROGRESS: &str =
    "setup did not finish (interrupted?): run `cargo xtask bench setup` again";

/// The fixtures `scenarios` need: B9's per-tool hooked copy is made from `noop10`, so it needs that one too.
fn fixtures_of(scenarios: &[&Scenario]) -> BTreeSet<Fixture> {
    let mut f: BTreeSet<Fixture> = scenarios.iter().map(|s| s.fixture).collect();
    if f.contains(&Fixture::Hooked) {
        f.insert(Fixture::Noop10);
    }
    f
}

fn setup(root: &Path, args: &[String]) -> anyhow::Result<()> {
    let Some(o) = Opts::parse(
        args,
        &[
            "scenarios",
            "tools",
            "snp",
            "target",
            "allocator",
            "mirror-from",
            "path",
            "jobs",
            "base-snp",
        ],
        &["no-install"],
    )?
    else {
        say(SETUP_HELP);
        return Ok(());
    };
    o.no_positional()?;
    let work = Work::new(root, &o)?;
    let scenarios = selected_scenarios(&o)?;
    let tool_list = selected_tools(&o)?;
    let install = !o.has("no-install");
    std::fs::create_dir_all(&work.dir)?;

    let build = Build::from_opts(&o)?;
    let hyperfine = ensure_hyperfine(&work, install, build.jobs)?;
    let mut exes = BTreeMap::new();
    for t in &tool_list {
        let exe = resolve_tool(&work, &o, &build, *t, install)?;
        exes.insert(*t, exe);
    }
    if let Some(base) = resolve_base(&o)? {
        exes.insert(Tool::SnpBase, base);
    }
    let tools = Tools { hyperfine, exes };

    // keep what an earlier setup found for the scenarios and tools not prepared now; the ones prepared now are
    // unavailable until their priming succeeds, so an interrupted setup (half-rebuilt fixtures) leaves nothing that
    // `run` would measure
    let mut state = SetupState::load(&work).unwrap_or_default();
    for s in &scenarios {
        for t in tools.exes.keys() {
            state
                .unavailable
                .insert(SetupState::key(s.id, *t), SETUP_IN_PROGRESS.to_owned());
        }
    }
    state.save(&work, &tools)?;

    write_gitconfig(&work)?;
    ensure_offline_tools(&work, install)?;
    let fixtures = fixtures_of(&scenarios);
    if fixtures.contains(&Fixture::Pch960) {
        ensure_mirror(&work, o.one("mirror-from"))?;
    }
    let mut gc_configs = Vec::new();
    for f in &fixtures {
        say(&format!("bench: generating fixture {}", f.dir()));
        let dir = work.fixture(*f, Tool::Snp);
        match f {
            Fixture::Noop10 => gen_noop10(&work, &dir)?,
            Fixture::Hooked => {}
            Fixture::Files50k => gen_files50k(&work, &dir)?,
            Fixture::Pch960 => gen_pch960(&work, &dir)?,
            Fixture::Stash10M => gen_stash10m(&work, &dir)?,
            Fixture::Validate1000 => gen_validate1000(&work, &dir)?,
            Fixture::Gc500 => gc_configs = gen_gc500(&work, &dir)?,
            Fixture::Unicode20 => gen_unicode20(&work, &dir)?,
        }
    }

    // per-tool preparation: B9's installed hook, B6's warm store, B11's populated store
    for (tool, exe) in &tools.exes {
        for s in &scenarios {
            let key = SetupState::key(s.id, *tool);
            match prime(&work, *tool, exe, s, &gc_configs) {
                Ok(()) => {
                    state.unavailable.remove(&key);
                }
                Err(e) => {
                    say(&format!(
                        "bench: {} unavailable for {}: {e:#}",
                        s.id,
                        tool.name()
                    ));
                    state.unavailable.insert(key, format!("{e:#}"));
                }
            }
        }
    }
    state.save(&work, &tools)?;
    say(&format!("bench: setup complete in {}", work.dir.display()));
    Ok(())
}

/// Prepares `tool` for scenario `s` once (outside the measurement).
fn prime(
    work: &Work,
    tool: Tool,
    exe: &Path,
    s: &Scenario,
    gc_configs: &[PathBuf],
) -> anyhow::Result<()> {
    let env = work.tool_env(tool, s.store, Some(s));
    let tool_cmd = |dir: &Path, args: &[&OsStr]| -> anyhow::Result<()> {
        let mut cmd = Work::command(exe, &env);
        cmd.args(args).current_dir(dir);
        let what = format!(
            "{} {}",
            tool.name(),
            args.iter()
                .map(|a| a.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ")
        );
        run_ok(&mut cmd, &what).map(drop)
    };
    match s.fixture {
        Fixture::Hooked => {
            let dir = work.fixture(Fixture::Hooked, tool);
            remove_tree(&dir)?;
            copy_tree(&work.fixture(Fixture::Noop10, tool), &dir)?;
            ensure_own_repo(work, &dir)?;
            tool_cmd(&dir, &[OsStr::new("install")])?;
        }
        Fixture::Pch960 if s.store == StoreMode::Warm => {
            ensure_own_repo(work, &work.fixture(Fixture::Pch960, tool))?;
            tool_cmd(
                &work.fixture(Fixture::Pch960, tool),
                &[OsStr::new("install-hooks")],
            )?;
        }
        Fixture::Gc500 => {
            remove_tree(&work.store(tool, StoreMode::Gc))?;
            let dir = work.fixture(Fixture::Gc500, tool);
            ensure_own_repo(work, &dir)?;
            // B11 like for like (P18): every tool must install the 500 repositories and track the 200 configs.
            // prek tracks a config only on `run` (its `install-hooks` installs without tracking), so it is primed
            // with `run --config <absolute config>`, which installs, finds no file to check and exits 0.
            let prime = if tool == Tool::Prek {
                "run"
            } else {
                "install-hooks"
            };
            for c in gc_configs {
                tool_cmd(
                    &dir,
                    &[OsStr::new(prime), OsStr::new("--config"), c.as_os_str()],
                )?;
            }
        }
        _ => {}
    }
    // the scenario's command must succeed once before it is measured
    let dir = work.fixture(s.fixture, tool);
    ensure_own_repo(work, &dir)?;
    if s.store != StoreMode::Cold {
        let (program, argv) = scenario_command(s, exe);
        let mut cmd = Work::command(&program, &env);
        cmd.args(&argv).current_dir(&dir);
        if tool.is_snp() {
            cmd.env("SNP_BIN", exe);
        }
        run_ok(&mut cmd, &format!("{} ({})", s.id, tool.name()))?;
    }
    if s.fixture == Fixture::Gc500 {
        gc_equal_work(work, tool, gc_configs.len())?;
    }
    if s.fixture == Fixture::Stash10M {
        clean_patches(work, tool, s, false)?;
    }
    Ok(())
}

/// The repositories in each tool's store (the B11 equal-work count).
const GC_REPOS: usize = 500;

/// B11's equal-work assertion after the warm-up `gc` (P18): every store holds the 500 repositories, and snp and prek
/// track every config (prek's `config-tracking.json`, snp's `configs-v1` entries; pre-commit's tracking is in SQLite
/// and is covered by its repository count, since its `gc` would remove the repositories of an untracked config).
///
/// snp's half is a warning, not an error, until the fresh-store tracking fix (C1: `Store::mark_config_used` returns
/// early when the store does not exist yet, so the first `install-hooks` into an empty store is not tracked and its 3
/// repositories are collected: 199 configs and 497 repositories) is in both binaries.
fn gc_equal_work(work: &Work, tool: Tool, configs: usize) -> anyhow::Result<()> {
    let store = work.store(tool, StoreMode::Gc);
    let (tracked, repos) = gc_store_counts(tool, &store)?;
    let mut problems = Vec::new();
    if let Some(n) = tracked
        && n != configs
    {
        problems.push(format!("tracks {n} configs, not {configs}"));
    }
    if repos != GC_REPOS {
        problems.push(format!("holds {repos} repositories, not {GC_REPOS}"));
    }
    if problems.is_empty() {
        return Ok(());
    }
    let msg = format!(
        "B11 is not like for like: the {} store {} {}",
        tool.name(),
        store.display(),
        problems.join(" and ")
    );
    if tool.is_snp() {
        say(&format!(
            "bench: warning: {msg} (C1: the fresh-store `install-hooks` is not tracked)"
        ));
        return Ok(());
    }
    bail!("{msg}")
}

/// `(tracked configs if the store records them in a file, repositories)` of a primed gc store.
fn gc_store_counts(tool: Tool, store: &Path) -> anyhow::Result<(Option<usize>, usize)> {
    let dirs = |dir: &Path, keep: &dyn Fn(&str) -> bool| -> anyhow::Result<usize> {
        let mut n = 0_usize;
        for e in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
            let e = e?;
            if e.file_type()?.is_dir() && keep(&e.file_name().to_string_lossy()) {
                n = n.saturating_add(1);
            }
        }
        Ok(n)
    };
    Ok(match tool {
        Tool::Snp | Tool::SnpBase => {
            let configs = store.join("configs-v1");
            let mut tracked = 0_usize;
            for e in std::fs::read_dir(&configs)
                .with_context(|| format!("reading {}", configs.display()))?
            {
                let e = e?;
                if !e.file_type()?.is_dir() && !e.file_name().to_string_lossy().starts_with('.') {
                    tracked = tracked.saturating_add(1);
                }
            }
            (Some(tracked), dirs(&store.join("repos-v1"), &|_| true)?)
        }
        Tool::PreCommit => (None, dirs(store, &|n| n.starts_with("repo"))?),
        Tool::Prek => {
            let path = store.join("config-tracking.json");
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let v: Value = serde_json::from_str(&text)
                .with_context(|| format!("parsing {}", path.display()))?;
            let tracked = v
                .as_array()
                .with_context(|| format!("{}: not a JSON array", path.display()))?
                .len();
            (Some(tracked), dirs(&store.join("repos"), &|_| true)?)
        }
    })
}

/// The program and arguments of scenario `s` for the tool binary `exe`.
fn scenario_command(s: &Scenario, exe: &Path) -> (PathBuf, Vec<OsString>) {
    match s.argv {
        Some(argv) => (exe.to_path_buf(), argv.iter().map(OsString::from).collect()),
        None => (
            PathBuf::from("git"),
            B9_GIT.iter().map(OsString::from).collect(),
        ),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Statistics and parsers of the measuring tools' output
// ---------------------------------------------------------------------------------------------------------------

/// The median of `xs` (any order; `None` when empty).
fn median(xs: &[f64]) -> Option<f64> {
    let mut v = xs.to_vec();
    v.sort_by(f64::total_cmp);
    let n = v.len();
    let mid = n / 2;
    if n == 0 {
        None
    } else if n % 2 == 1 {
        v.get(mid).copied()
    } else {
        Some(f64::midpoint(*v.get(mid.checked_sub(1)?)?, *v.get(mid)?))
    }
}

/// The nearest integer to `x` in `0..=n`.
fn nearest_index(x: f64, n: usize) -> usize {
    (0..=n)
        .min_by(|a, b| {
            (usize_f64(*a) - x)
                .abs()
                .total_cmp(&(usize_f64(*b) - x).abs())
        })
        .unwrap_or(0)
}

/// The distribution-free 95 % confidence interval of the median: the order statistics of ranks
/// `n/2 − 1.96·√n/2` and `1 + n/2 + 1.96·√n/2` (1-based, clamped).
fn median_ci95(xs: &[f64]) -> Option<(f64, f64)> {
    let mut v = xs.to_vec();
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n == 0 {
        return None;
    }
    let nf = usize_f64(n);
    let half = 1.96 * nf.sqrt() / 2.0;
    let lo = nearest_index(nf / 2.0 - half, n).clamp(1, n);
    let hi = nearest_index(1.0 + nf / 2.0 + half, n).clamp(1, n);
    Some((*v.get(lo.checked_sub(1)?)?, *v.get(hi.checked_sub(1)?)?))
}

/// One tool's timing on one scenario (milliseconds).
#[derive(Debug, Clone, Default, PartialEq)]
struct Measure {
    median_ms: f64,
    ci95_ms: (f64, f64),
    mean_ms: f64,
    stddev_ms: f64,
    min_ms: f64,
    max_ms: f64,
    runs: u64,
    /// The peak RSS of the largest process of the tree (`/usr/bin/time` reports a maximum, not a sum).
    rss_mib: Option<f64>,
    syscalls: Option<u64>,
    alloc_mib: Option<f64>,
    /// dhat's total blocks allocated (`--dhat`; the ceiling of the multi-threaded scenarios, ADR-100).
    alloc_blocks: Option<u64>,
    /// dhat's heap peak: the bytes live at the global maximum (`--dhat`; B8's budget, ADR-100).
    heap_peak_mib: Option<f64>,
    /// The median of each interleaved round's block, in round order (`--base-snp`, ADR-099; empty otherwise).
    rounds_ms: Vec<f64>,
    /// The deterministic counters of one traced run (`--counts`, ADR-100; empty otherwise).
    counts: BTreeMap<String, u64>,
    /// The exec argv lines only the head (`+`) or only the base (`-`) ran (`--counts --base-snp`; snp only).
    exec_diff: Vec<String>,
}

impl Measure {
    /// The statistics of `times` (milliseconds, any order): the pooled runs of every block of one tool.
    fn from_times(times: &[f64]) -> anyhow::Result<Self> {
        let med = median(times).context("no run measured")?;
        let ci = median_ci95(times).context("no run measured")?;
        let n = usize_f64(times.len());
        let mean = times.iter().sum::<f64>() / n;
        // the sample standard deviation, as hyperfine reports it (undefined for one run)
        let stddev = if times.len() > 1 {
            (times.iter().map(|t| (t - mean).powi(2)).sum::<f64>() / (n - 1.0)).sqrt()
        } else {
            f64::NAN
        };
        Ok(Self {
            median_ms: med,
            ci95_ms: ci,
            mean_ms: mean,
            stddev_ms: stddev,
            min_ms: times.iter().copied().fold(f64::INFINITY, f64::min),
            max_ms: times.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            runs: u64::try_from(times.len()).unwrap_or(u64::MAX),
            ..Self::default()
        })
    }
}

/// The run times (milliseconds) of hyperfine's `--export-json` (one command).
fn parse_hyperfine_times(text: &str) -> anyhow::Result<Vec<f64>> {
    let v: Value = serde_json::from_str(text).context("parsing hyperfine JSON")?;
    let r = v
        .get("results")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .context("hyperfine JSON has no result")?;
    let times: Vec<f64> = r
        .get("times")
        .and_then(Value::as_array)
        .context("hyperfine JSON has no times")?
        .iter()
        .filter_map(Value::as_f64)
        .map(|s| s * 1000.0)
        .collect();
    ensure!(!times.is_empty(), "hyperfine measured no run");
    Ok(times)
}

/// Peak RSS in KiB from `/usr/bin/time -v` (GNU: `Maximum resident set size (kbytes): N`) or `-l` (BSD/macOS:
/// `N  maximum resident set size`, bytes).
fn parse_time_rss_kib(stderr: &str) -> Option<u64> {
    for line in stderr.lines() {
        let l = line.trim();
        if let Some(v) = l.strip_prefix("Maximum resident set size (kbytes):") {
            return v.trim().parse().ok();
        }
        if let Some(v) = l.strip_suffix("maximum resident set size") {
            return v.trim().parse::<u64>().ok().map(|b| b / 1024);
        }
    }
    None
}

/// The `calls` column of the `total` row of `strace -c`.
fn parse_strace_total(summary: &str) -> Option<u64> {
    summary
        .lines()
        .rev()
        .find(|l| l.trim_end().ends_with(" total"))
        .and_then(|l| l.split_whitespace().nth(3))
        .and_then(|c| c.parse().ok())
}

/// Every syscall row of a `strace -c` table: name → calls (the `errors` column may be empty, so the calls are the
/// fourth word and the name the last; the `total` row is left out).
fn parse_strace_table(summary: &str) -> BTreeMap<String, u64> {
    summary
        .lines()
        .filter_map(|l| {
            let words: Vec<&str> = l.split_whitespace().collect();
            if !(5..=6).contains(&words.len()) {
                return None;
            }
            let name = *words.last()?;
            let calls = words.get(3)?.parse::<u64>().ok()?;
            // a data row starts with the `% time` number (the header and the rules do not)
            if words.first()?.parse::<f64>().is_err() || name == "total" {
                return None;
            }
            Some((name.to_owned(), calls))
        })
        .collect()
}

/// dhat's totals of one run (`dhat-heap.json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DhatSummary {
    /// The sum of `tb`: total bytes allocated.
    total_bytes: u64,
    /// The sum of `tbk`: total blocks allocated.
    total_blocks: u64,
    /// The sum of `gb`: the bytes live at the global heap maximum (t-gmax).
    peak_bytes: u64,
}

/// dhat's totals: `tb`, `tbk` and `gb` summed over the program points of `dhat-heap.json` (each must be there).
fn parse_dhat(text: &str) -> anyhow::Result<DhatSummary> {
    let v: Value = serde_json::from_str(text).context("parsing dhat-heap.json")?;
    let pps = v
        .get("pps")
        .and_then(Value::as_array)
        .context("dhat-heap.json has no pps")?;
    let sum = |key: &str| {
        pps.iter()
            .map(|p| {
                p.get(key)
                    .and_then(Value::as_u64)
                    .with_context(|| format!("a dhat program point without {key}"))
            })
            .try_fold(0_u64, |acc, n| anyhow::Ok(acc.saturating_add(n?)))
    };
    Ok(DhatSummary {
        total_bytes: sum("tb")?,
        total_blocks: sum("tbk")?,
        peak_bytes: sum("gb")?,
    })
}

fn bytes_to_mib(b: u64) -> f64 {
    to_f64(b) / (1024.0 * 1024.0)
}

// ---------------------------------------------------------------------------------------------------------------
// Run records
// ---------------------------------------------------------------------------------------------------------------

/// One tool on one scenario: measured or failed.
#[derive(Debug, Clone, PartialEq)]
enum Outcome {
    Ok(Measure),
    Failed(String),
}

/// A run record (the history line, the gate's input).
#[derive(Debug, Clone, Default, PartialEq)]
struct Record {
    platform: String,
    variant: String,
    label: String,
    commit: String,
    unix_time: u64,
    /// The runner's logical CPUs (snp's default job count and partition count follow it; ADR-100).
    cpu_count: Option<u64>,
    /// `git --version` of the runner (tree-wide counters and timings depend on it; ADR-100).
    git_version: String,
    /// The optional metrics this run was asked to measure (`syscalls` for `--syscalls`; `alloc_mib`, `alloc_blocks`
    /// and `heap_peak_mib` for `--dhat`; `counts` for `--counts`): a budget on a requested metric that has no value
    /// fails instead of being skipped.
    extras: BTreeSet<String>,
    scenarios: BTreeMap<String, BTreeMap<String, Outcome>>,
}

impl Record {
    fn os(&self) -> &str {
        self.platform.split('-').next().unwrap_or_default()
    }

    fn get(&self, scenario: &str, tool: &str) -> Option<&Measure> {
        match self.scenarios.get(scenario)?.get(tool)? {
            Outcome::Ok(m) => Some(m),
            Outcome::Failed(_) => None,
        }
    }

    fn measure_json(m: &Measure) -> Value {
        let mut v = json!({
            "median_ms": m.median_ms,
            "ci95_ms": [m.ci95_ms.0, m.ci95_ms.1],
            "mean_ms": m.mean_ms,
            "stddev_ms": m.stddev_ms,
            "min_ms": m.min_ms,
            "max_ms": m.max_ms,
            "runs": m.runs,
            "rss_mib": m.rss_mib,
            "syscalls": m.syscalls,
            "alloc_mib": m.alloc_mib,
        });
        // the schema-2 additions are written only when measured, so a history line stays as short as before
        if let Some(map) = v.as_object_mut() {
            if let Some(b) = m.alloc_blocks {
                map.insert("alloc_blocks".into(), json!(b));
            }
            if let Some(p) = m.heap_peak_mib {
                map.insert("heap_peak_mib".into(), json!(p));
            }
            if !m.rounds_ms.is_empty() {
                map.insert("rounds_ms".into(), json!(m.rounds_ms));
            }
            if !m.counts.is_empty() {
                map.insert("counts".into(), json!(m.counts));
            }
            if !m.exec_diff.is_empty() {
                map.insert("exec_diff".into(), json!(m.exec_diff));
            }
        }
        v
    }

    fn to_json(&self) -> Value {
        let scenarios: serde_json::Map<String, Value> = self
            .scenarios
            .iter()
            .map(|(s, tools)| {
                let t: serde_json::Map<String, Value> = tools
                    .iter()
                    .map(|(name, o)| {
                        let v = match o {
                            Outcome::Ok(m) => Self::measure_json(m),
                            Outcome::Failed(e) => json!({ "error": e }),
                        };
                        (name.clone(), v)
                    })
                    .collect();
                (s.clone(), Value::Object(t))
            })
            .collect();
        json!({
            "schema": SCHEMA,
            "platform": self.platform,
            "variant": self.variant,
            "label": self.label,
            "commit": self.commit,
            "unix_time": self.unix_time,
            "cpu_count": self.cpu_count,
            "git_version": self.git_version,
            "extras": self.extras.iter().collect::<Vec<_>>(),
            "scenarios": scenarios,
        })
    }

    /// A schema 1 or 2 record (unknown keys are ignored; a schema 1 record has no rounds, counters or host facts).
    fn from_json(v: &Value) -> anyhow::Result<Self> {
        let schema = v.get("schema").and_then(Value::as_u64);
        ensure!(
            schema.is_some_and(|n| SCHEMAS_READ.contains(&n)),
            "run record schema {schema:?} (want one of {SCHEMAS_READ:?})"
        );
        let s = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let mut scenarios = BTreeMap::new();
        for (id, tools) in v
            .get("scenarios")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            let mut out = BTreeMap::new();
            for (tool, m) in tools.as_object().into_iter().flatten() {
                let o = if let Some(e) = m.get("error").and_then(Value::as_str) {
                    Outcome::Failed(e.to_owned())
                } else {
                    let f = |k: &str| m.get(k).and_then(Value::as_f64);
                    let ci = m.get("ci95_ms").and_then(Value::as_array);
                    let ci_at = |i: usize| ci.and_then(|a| a.get(i)).and_then(Value::as_f64);
                    Outcome::Ok(Measure {
                        median_ms: f("median_ms")
                            .with_context(|| format!("{id}/{tool}: no median_ms"))?,
                        ci95_ms: (ci_at(0).unwrap_or(f64::NAN), ci_at(1).unwrap_or(f64::NAN)),
                        mean_ms: f("mean_ms").unwrap_or(f64::NAN),
                        stddev_ms: f("stddev_ms").unwrap_or(f64::NAN),
                        min_ms: f("min_ms").unwrap_or(f64::NAN),
                        max_ms: f("max_ms").unwrap_or(f64::NAN),
                        runs: m.get("runs").and_then(Value::as_u64).unwrap_or(0),
                        rss_mib: f("rss_mib"),
                        syscalls: m.get("syscalls").and_then(Value::as_u64),
                        alloc_mib: f("alloc_mib"),
                        alloc_blocks: m.get("alloc_blocks").and_then(Value::as_u64),
                        heap_peak_mib: f("heap_peak_mib"),
                        rounds_ms: m
                            .get("rounds_ms")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_f64)
                            .collect(),
                        counts: m
                            .get("counts")
                            .and_then(Value::as_object)
                            .into_iter()
                            .flatten()
                            .filter_map(|(k, n)| Some((k.clone(), n.as_u64()?)))
                            .collect(),
                        exec_diff: m
                            .get("exec_diff")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect(),
                    })
                };
                out.insert(tool.clone(), o);
            }
            scenarios.insert(id.clone(), out);
        }
        Ok(Self {
            platform: s("platform"),
            variant: s("variant"),
            label: s("label"),
            commit: s("commit"),
            unix_time: v.get("unix_time").and_then(Value::as_u64).unwrap_or(0),
            cpu_count: v.get("cpu_count").and_then(Value::as_u64),
            git_version: s("git_version"),
            extras: v
                .get("extras")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            scenarios,
        })
    }

    fn read(path: &Path) -> anyhow::Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let v: Value =
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        Self::from_json(&v).with_context(|| format!("in {}", path.display()))
    }
}

// ---------------------------------------------------------------------------------------------------------------
// run
// ---------------------------------------------------------------------------------------------------------------

/// The scenarios `--syscalls` counts (the ceilings of ADR-083).
const SYSCALL_SCENARIOS: [&str; 2] = ["B1", "B3"];
/// The scenarios `--dhat` profiles (ADR-100): B3 and B8 for B8's heap-peak budget (relative to B3's), and the absolute
/// allocation ceilings of the single-threaded (bytes) and multi-threaded (blocks) scenarios.
const DHAT_SCENARIOS: [&str; 7] = ["B2", "B3", "B4", "B6", "B8", "B10", "B12u"];
/// The most interleaved rounds of a paired measurement (ADR-099).
const ROUNDS: u32 = 10;

/// `(rounds, runs per block)` of a paired measurement of `total` runs per binary (ADR-099): at most [`ROUNDS`]
/// rounds, every block at least one run. 50 runs: 10 × 5; 20: 10 × 2; B7's 5: 5 × 1.
fn paired_rounds(total: u32) -> (u32, u32) {
    let rounds = total.clamp(1, ROUNDS);
    (rounds, total.checked_div(rounds).unwrap_or(1).max(1))
}

/// Whether the base runs first in round `r`: ABBA order (base, head; head, base; …), so a drift of the runner during
/// the scenario's window falls on both binaries alike.
const fn base_first(r: u32) -> bool {
    r.is_multiple_of(2)
}

fn measure(root: &Path, args: &[String]) -> anyhow::Result<()> {
    let Some(o) = Opts::parse(
        args,
        &[
            "scenarios",
            "tools",
            "snp",
            "base-snp",
            "target",
            "allocator",
            "label",
            "out",
            "runs",
            "warmup",
            "path",
            "jobs",
        ],
        &["syscalls", "dhat", "counts"],
    )?
    else {
        say(RUN_HELP);
        return Ok(());
    };
    o.no_positional()?;
    let work = Work::new(root, &o)?;
    let state = SetupState::load(&work)?;
    let scenarios = selected_scenarios(&o)?;
    let tool_list = selected_tools(&o)?;
    let build = Build::from_opts(&o)?;
    let hyperfine = ensure_hyperfine(&work, false, None)?;
    let mut exes = BTreeMap::new();
    for t in &tool_list {
        exes.insert(*t, resolve_tool(&work, &o, &build, *t, false)?);
    }
    let base = resolve_base(&o)?;
    ensure!(
        base.is_none() || exes.contains_key(&Tool::Snp),
        "--base-snp is paired with snp: select snp in --tools"
    );
    let label = o.one("label").unwrap_or("current").to_owned();
    let counts = (o.number("warmup")?, o.number("runs")?);
    let raw = work.results().join("raw").join(&label);
    std::fs::create_dir_all(&raw)?;

    let mut record = Record {
        platform: platform(),
        variant: build.variant(),
        label: label.clone(),
        commit: git(&work, root, &["rev-parse", "HEAD"]).map_or_default(|s| s.trim().to_owned()),
        unix_time: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        cpu_count: std::thread::available_parallelism()
            .ok()
            .and_then(|n| u64::try_from(n.get()).ok()),
        git_version: git(&work, root, &["--version"]).map_or_default(|s| s.trim().to_owned()),
        extras: [
            ("syscalls", "syscalls"),
            ("dhat", "alloc_mib"),
            ("dhat", "alloc_blocks"),
            ("dhat", "heap_peak_mib"),
            ("counts", "counts"),
        ]
        .into_iter()
        .filter(|(switch, _)| o.has(switch))
        .map(|(_, metric)| metric.to_owned())
        .collect(),
        scenarios: BTreeMap::new(),
    };
    for (switch, needs) in [
        ("syscalls", &SYSCALL_SCENARIOS[..]),
        ("dhat", &DHAT_SCENARIOS[..]),
    ] {
        if o.has(switch) && !scenarios.iter().any(|s| needs.contains(&s.id)) {
            say(&format!(
                "bench: warning: --{switch} measures {} only, which this run does not select (the gate reports the \
                 budget as not measured)",
                needs.join("/")
            ));
        }
    }
    let mut all_tools: Vec<Tool> = exes.keys().copied().collect();
    if base.is_some() {
        all_tools.push(Tool::SnpBase);
    }
    // the patches earlier runs left in every store (B8, P07); an interrupted run's is kept, and its B8 then fails
    for s in scenarios.iter().filter(|s| s.fixture == Fixture::Stash10M) {
        for t in &all_tools {
            if let Err(e) = clean_patches(&work, *t, s, false) {
                say(&format!("bench: warning: {}: {e:#}", t.name()));
            }
        }
    }
    for s in &scenarios {
        let mut per_tool = BTreeMap::new();
        let unavailable = |t: Tool| state.unavailable.get(&SetupState::key(s.id, t));
        // snp first (paired with the base when there is one), then the references in the same window
        for (tool, exe) in &exes {
            if let Some(reason) = unavailable(*tool) {
                say(&format!(
                    "bench: {} {}: skipped (setup: {})",
                    s.id,
                    tool.name(),
                    one_line(reason)
                ));
                per_tool.insert(
                    tool.name().to_owned(),
                    Outcome::Failed(format!("setup: {reason}")),
                );
                continue;
            }
            let base_exe = base.as_deref().filter(|_| *tool == Tool::Snp);
            if let Some(base_exe) = base_exe {
                let base_exe = match unavailable(Tool::SnpBase) {
                    Some(reason) => Err(format!("setup: {reason}")),
                    None => Ok(base_exe),
                };
                say(&format!(
                    "bench: {} snp and snp-base (interleaved) — {}",
                    s.id, s.title
                ));
                let (head, base_out) =
                    measure_paired(&work, &hyperfine, exe, &base_exe, s, counts, &raw);
                log_outcome(s, Tool::SnpBase, &base_out);
                log_outcome(s, Tool::Snp, &head);
                per_tool.insert(Tool::SnpBase.name().to_owned(), base_out);
                per_tool.insert(Tool::Snp.name().to_owned(), head);
                continue;
            }
            say(&format!("bench: {} {} — {}", s.id, tool.name(), s.title));
            let outcome = match measure_single(&work, &hyperfine, *tool, exe, s, counts, &raw) {
                Ok(m) => Outcome::Ok(m),
                Err(e) => Outcome::Failed(format!("{e:#}")),
            };
            log_outcome(s, *tool, &outcome);
            per_tool.insert(tool.name().to_owned(), outcome);
        }
        if o.has("counts") {
            let pairs = [
                (Tool::Snp, exes.get(&Tool::Snp).cloned()),
                (Tool::SnpBase, base.clone()),
            ];
            let mut execs = BTreeMap::new();
            for (tool, exe) in pairs {
                let Some(exe) = exe else { continue };
                let Some(Outcome::Ok(m)) = per_tool.get_mut(tool.name()) else {
                    continue;
                };
                // a failure leaves the counters empty (the gate fails a head without them, warns for the base)
                match count_pass(&work, tool, &exe, s, &raw) {
                    Ok((c, argv)) => {
                        say(&format!("bench: {} {}: counts {c:?}", s.id, tool.name()));
                        m.counts = c;
                        execs.insert(tool, argv);
                    }
                    Err(e) => say(&format!(
                        "bench: warning: {} {}: not counted: {}",
                        s.id,
                        tool.name(),
                        one_line(&format!("{e:#}"))
                    )),
                }
            }
            if let (Some(head), Some(base_argv)) =
                (execs.get(&Tool::Snp), execs.get(&Tool::SnpBase))
                && let Some(Outcome::Ok(m)) = per_tool.get_mut(Tool::Snp.name())
            {
                m.exec_diff = exec_diff(head, base_argv);
            }
        }
        // once after the scenario (P07)
        if s.fixture == Fixture::Stash10M {
            for t in &all_tools {
                if let Err(e) = clean_patches(&work, *t, s, false) {
                    say(&format!("bench: warning: {}: {e:#}", t.name()));
                }
            }
        }
        record.scenarios.insert(s.id.to_owned(), per_tool);
    }

    if let Some(snp) = exes.get(&Tool::Snp) {
        if o.has("syscalls") {
            for id in SYSCALL_SCENARIOS {
                if let Some(s) = scenarios.iter().find(|s| s.id == id)
                    && let Some(Outcome::Ok(m)) =
                        record.scenarios.get_mut(id).and_then(|t| t.get_mut("snp"))
                {
                    // a failure leaves the metric unmeasured (the gate fails its budget as such) and keeps the
                    // timings of the whole run
                    match count_syscalls(&work, snp, s) {
                        Ok(n) => {
                            say(&format!("bench: {id} snp: {n} syscalls (strace -f -c)"));
                            m.syscalls = Some(n);
                        }
                        Err(e) => say(&format!(
                            "bench: warning: {id} snp: syscalls not measured: {}",
                            one_line(&format!("{e:#}"))
                        )),
                    }
                }
            }
        }
        if o.has("dhat") {
            measure_dhat(&work, &build, &scenarios, &mut record);
        }
    }

    let out = o.one("out").map_or_else(
        || work.results().join(format!("{label}.json")),
        PathBuf::from,
    );
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        &out,
        serde_json::to_string_pretty(&record.to_json())? + "\n",
    )
    .with_context(|| format!("writing {}", out.display()))?;
    say(&format!("bench: wrote {}", out.display()));
    Ok(())
}

/// The log line of one tool's outcome on `s`.
fn log_outcome(s: &Scenario, tool: Tool, o: &Outcome) {
    match o {
        Outcome::Ok(m) => say(&format!(
            "bench: {} {}: p50 {:.2} ms [{:.2}, {:.2}]{}",
            s.id,
            tool.name(),
            m.median_ms,
            m.ci95_ms.0,
            m.ci95_ms.1,
            m.rss_mib.map_or_default(|r| format!(", RSS {r:.1} MiB"))
        )),
        Outcome::Failed(e) => say(&format!(
            "bench: {} {}: FAILED: {}",
            s.id,
            tool.name(),
            one_line(e)
        )),
    }
}

/// `s` on one line (whitespace runs collapsed), at most 240 characters: a table cell or a log line.
fn one_line(s: &str) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= 240 {
        flat
    } else {
        let mut cut: String = flat.chars().take(239).collect();
        cut.push('…');
        cut
    }
}

/// The position of scenario `id` in [`SCENARIOS`] (unknown ids last): B2 sorts before B10.
fn scenario_rank(id: &str) -> usize {
    SCENARIOS
        .iter()
        .position(|s| s.id == id)
        .unwrap_or(usize::MAX)
}

/// `(warm-up runs, measured runs)` of `s` per binary, with the `--warmup`/`--runs` overrides.
fn run_counts(s: &Scenario, counts: (Option<u32>, Option<u32>)) -> (u32, u32) {
    let (default_w, default_r) = s.runs.unwrap_or((WARMUP, RUNS));
    (counts.0.unwrap_or(default_w), counts.1.unwrap_or(default_r))
}

/// The environment of `tool` on `s`; snp's carries `SNP_BIN`, since B9's hook script runs `$SNP_BIN` first (ADR-048):
/// the measured binary, whichever one setup installed.
fn scenario_env(work: &Work, tool: Tool, exe: &Path, s: &Scenario) -> Vec<(OsString, OsString)> {
    let mut env = work.tool_env(tool, s.store, Some(s));
    if tool.is_snp() {
        env.push(("SNP_BIN".into(), exe.as_os_str().to_owned()));
    }
    env
}

/// One hyperfine invocation of `tool` on `s` (`warmup` + `runs`), written to `json_out`; the run times (ms).
fn hyperfine_block(
    work: &Work,
    hyperfine: &Path,
    tool: Tool,
    exe: &Path,
    s: &Scenario,
    (warmup, runs): (u32, u32),
    json_out: &Path,
) -> anyhow::Result<Vec<f64>> {
    let dir = work.fixture(s.fixture, tool);
    ensure!(
        dir.is_dir(),
        "fixture {} missing (run `cargo xtask bench setup`)",
        dir.display()
    );
    // B9 commits in the fixture 56 times: it must be the fixture's own repository, never an enclosing one
    ensure_own_repo(work, &dir)?;
    // B8: refuse on an interrupted run, and the directory `find` starts from must exist
    clean_patches(work, tool, s, true)?;
    let env = scenario_env(work, tool, exe, s);
    let (program, argv) = scenario_command(s, exe);
    let mut line = vec![program.into_os_string()];
    line.extend(argv);
    let mut cmd = Work::command(hyperfine, &env);
    cmd.args([
        "-N",
        "--style",
        "none",
        "-w",
        &warmup.to_string(),
        "-r",
        &runs.to_string(),
    ])
    .arg("--command-name")
    .arg(format!("{} {}", tool.name(), s.id))
    .arg("--export-json")
    .arg(json_out)
    .current_dir(&dir);
    if let Some(prepare) = prepare_command(work, tool, s) {
        cmd.arg("--prepare").arg(prepare);
    }
    cmd.arg(command_line(&line));
    run_ok(&mut cmd, "hyperfine")?;
    let text = std::fs::read_to_string(json_out)
        .with_context(|| format!("reading {}", json_out.display()))?;
    parse_hyperfine_times(&text)
}

/// hyperfine's `--prepare` of `tool` on `s` (Unix; ARCHITECTURE §12.3): B7 removes the cold store before every run,
/// B8 the stash patch the previous run left (P07: 50 runs × 3 tools leave about 2.8 GB of patches otherwise).
fn prepare_command(work: &Work, tool: Tool, s: &Scenario) -> Option<String> {
    if cfg!(windows) {
        return None;
    }
    if s.store == StoreMode::Cold {
        return Some(command_line(&[
            OsString::from("rm"),
            OsString::from("-rf"),
            work.store(tool, StoreMode::Cold).into_os_string(),
        ]));
    }
    if s.fixture == Fixture::Stash10M {
        let (dir, pattern) = patch_location(work, tool, s.store);
        return Some(command_line(&[
            OsString::from("find"),
            dir.into_os_string(),
            OsString::from("-maxdepth"),
            OsString::from("1"),
            OsString::from("-type"),
            OsString::from("f"),
            OsString::from("-name"),
            OsString::from(pattern),
            OsString::from("-delete"),
        ]));
    }
    None
}

/// hyperfine (and the RSS runs) of `tool` alone on `s`.
fn measure_single(
    work: &Work,
    hyperfine: &Path,
    tool: Tool,
    exe: &Path,
    s: &Scenario,
    counts: (Option<u32>, Option<u32>),
    raw: &Path,
) -> anyhow::Result<Measure> {
    let json_out = raw.join(format!("{}-{}.json", s.id, tool.name()));
    let times = hyperfine_block(
        work,
        hyperfine,
        tool,
        exe,
        s,
        run_counts(s, counts),
        &json_out,
    )?;
    let mut m = Measure::from_times(&times)?;
    add_rss(work, tool, exe, s, &mut m)?;
    Ok(m)
}

/// The RSS runs of `tool` on an RSS scenario (not on Windows).
fn add_rss(
    work: &Work,
    tool: Tool,
    exe: &Path,
    s: &Scenario,
    m: &mut Measure,
) -> anyhow::Result<()> {
    if s.rss && !cfg!(windows) {
        let env = scenario_env(work, tool, exe, s);
        let (program, argv) = scenario_command(s, exe);
        let dir = work.fixture(s.fixture, tool);
        let before = || clean_patches(work, tool, s, false);
        m.rss_mib = Some(rss_median_kib(&env, &dir, &program, &argv, &before)? / 1024.0);
    }
    Ok(())
}

/// snp and the base commit's snp on `s` in interleaved ABBA rounds (ADR-099), then snp's RSS runs (the gate never
/// reads the base's). Each round runs one block of each binary; each binary has one warm-up, in its first block.
/// A failure of the base marks it not measured and the head still runs every round; a failure of the head ends the
/// scenario.
fn measure_paired(
    work: &Work,
    hyperfine: &Path,
    head: &Path,
    base: &Result<&Path, String>,
    s: &Scenario,
    counts: (Option<u32>, Option<u32>),
    raw: &Path,
) -> (Outcome, Outcome) {
    let (warmup, total) = run_counts(s, counts);
    let (rounds, per_block) = paired_rounds(total);
    let mut base_err = base.as_ref().err().cloned();
    let mut runs: BTreeMap<Tool, (Vec<f64>, Vec<f64>)> = BTreeMap::new();
    for r in 0..rounds {
        let order = if base_first(r) {
            [Tool::SnpBase, Tool::Snp]
        } else {
            [Tool::Snp, Tool::SnpBase]
        };
        for tool in order {
            let exe = match (tool, base) {
                (Tool::SnpBase, Ok(b)) if base_err.is_none() => *b,
                (Tool::SnpBase, _) => continue,
                _ => head,
            };
            let entry = runs.entry(tool).or_default();
            let w = if entry.0.is_empty() { warmup } else { 0 };
            let json_out = raw.join(format!("{}-{}-r{r:02}.json", s.id, tool.name()));
            match hyperfine_block(work, hyperfine, tool, exe, s, (w, per_block), &json_out) {
                Ok(times) => {
                    if let Some(med) = median(&times) {
                        entry.1.push(med);
                    }
                    entry.0.extend(times);
                }
                Err(e) if tool == Tool::SnpBase => base_err = Some(format!("{e:#}")),
                Err(e) => {
                    return (
                        Outcome::Failed(format!("{e:#}")),
                        Outcome::Failed("not measured: snp failed this scenario".into()),
                    );
                }
            }
        }
    }
    let pooled = |tool: Tool| -> anyhow::Result<Measure> {
        let (times, medians) = runs.get(&tool).context("no block ran")?;
        let mut m = Measure::from_times(times)?;
        m.rounds_ms.clone_from(medians);
        Ok(m)
    };
    let head_out = match pooled(Tool::Snp).and_then(|mut m| {
        add_rss(work, Tool::Snp, head, s, &mut m)?;
        Ok(m)
    }) {
        Ok(m) => Outcome::Ok(m),
        Err(e) => Outcome::Failed(format!("{e:#}")),
    };
    let base_out = match base_err {
        Some(e) => Outcome::Failed(e),
        None => match pooled(Tool::SnpBase) {
            Ok(m) => Outcome::Ok(m),
            Err(e) => Outcome::Failed(format!("{e:#}")),
        },
    };
    (head_out, base_out)
}

/// The median peak RSS (KiB) of [`RSS_RUNS`] runs under `/usr/bin/time` (the largest process of the tree: a maximum
/// over the processes the tool waited for, not a sum); `before` runs before each (B8's patch cleanup).
fn rss_median_kib(
    env: &[(OsString, OsString)],
    dir: &Path,
    program: &Path,
    argv: &[OsString],
    before: &dyn Fn() -> anyhow::Result<()>,
) -> anyhow::Result<f64> {
    let flag = if cfg!(target_os = "macos") {
        "-l"
    } else {
        "-v"
    };
    let mut samples = Vec::new();
    for _ in 0..RSS_RUNS {
        before()?;
        let mut cmd = Work::command("/usr/bin/time", env);
        cmd.arg(flag).arg(program).args(argv).current_dir(dir);
        let out = cmd.output().context("running /usr/bin/time")?;
        ensure!(
            out.status.success(),
            "/usr/bin/time {}: {}",
            program.display(),
            out.status
        );
        let kib = parse_time_rss_kib(&String::from_utf8_lossy(&out.stderr))
            .context("/usr/bin/time printed no maximum resident set size")?;
        samples.push(to_f64(kib));
    }
    median(&samples).context("no RSS sample")
}

/// Where `tool` leaves its stash patches and the `find -name` pattern of their names (P07): snp
/// `<store>/patches/patch<secs>-<pid>[-<n>]`, pre-commit `<store>/patch<secs>-<pid>` (the store root), prek
/// `<store>/patches/<secs>-<pid>.patch`. Nothing else in a store matches (journals are `run-*.snp-journal`).
fn patch_location(work: &Work, tool: Tool, mode: StoreMode) -> (PathBuf, &'static str) {
    let store = work.store(tool, mode);
    match tool {
        Tool::Snp | Tool::SnpBase => (store.join("patches"), "patch[0-9]*-[0-9]*"),
        Tool::PreCommit => (store, "patch[0-9]*-[0-9]*"),
        Tool::Prek => (store.join("patches"), "[0-9]*-[0-9]*.patch"),
    }
}

/// `find -name`'s match of `pattern` (literals, `*` and `[0-9]` only) against a whole file name.
fn name_matches(pattern: &[u8], name: &[u8]) -> bool {
    if let Some(rest) = pattern.strip_prefix(b"*") {
        return (0..=name.len())
            .any(|i| name.get(i..).is_some_and(|tail| name_matches(rest, tail)));
    }
    if let Some(rest) = pattern.strip_prefix(b"[0-9]") {
        return match name.split_first() {
            Some((c, tail)) => c.is_ascii_digit() && name_matches(rest, tail),
            None => false,
        };
    }
    match (pattern.split_first(), name.split_first()) {
        (Some((p, prest)), Some((c, nrest))) => p == c && name_matches(prest, nrest),
        (None, None) => true,
        _ => false,
    }
}

/// The trace of an interrupted snp run that B8's cleanup must not hide: a store journal `run-*.snp-journal` or a
/// git-dir pointer `.git/snp-stash*.journal` of the fixture.
fn interrupted_run(work: &Work, tool: Tool, s: &Scenario) -> Option<PathBuf> {
    let mut places = vec![(
        work.fixture(s.fixture, tool).join(".git"),
        &b"snp-stash*.journal"[..],
    )];
    if tool.is_snp() {
        places.push((
            work.store(tool, s.store).join("patches"),
            &b"run-*.snp-journal"[..],
        ));
    }
    places.into_iter().find_map(|(dir, pattern)| {
        std::fs::read_dir(&dir).ok()?.find_map(|e| {
            let e = e.ok()?;
            name_matches(pattern, e.file_name().as_encoded_bytes()).then(|| e.path())
        })
    })
}

/// Removes the stash patches `tool` left in its store on B8 (P07; a no-op on every other scenario), exactly the
/// files [`prepare_command`]'s `find` removes. Refuses when a run was interrupted (its patch holds the user's
/// changes in a real repository; here it means the measurement is broken), and with `require_dir` when the
/// directory `find` starts from is missing (`find` would exit 1 and abort hyperfine).
fn clean_patches(work: &Work, tool: Tool, s: &Scenario, require_dir: bool) -> anyhow::Result<()> {
    if s.fixture != Fixture::Stash10M {
        return Ok(());
    }
    if let Some(trace) = interrupted_run(work, tool, s) {
        bail!(
            "{} exists: a run of {} was interrupted, and its patch is kept (inspect it, then remove it and run \
             `cargo xtask bench setup --scenarios {}`)",
            trace.display(),
            tool.name(),
            s.id
        );
    }
    let (dir, pattern) = patch_location(work, tool, s.store);
    if !dir.is_dir() {
        ensure!(
            !require_dir,
            "{} is missing (run `cargo xtask bench setup --scenarios {}`)",
            dir.display(),
            s.id
        );
        return Ok(());
    }
    for e in std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
        let e = e?;
        // `find -type f`: regular files only, never a directory or a symlink
        let ty = e.file_type()?;
        if !ty.is_dir()
            && !ty.is_symlink()
            && name_matches(pattern.as_bytes(), e.file_name().as_encoded_bytes())
        {
            std::fs::remove_file(e.path())
                .with_context(|| format!("removing {}", e.path().display()))?;
        }
    }
    Ok(())
}

/// `strace -f -c` of snp on `s` (Linux).
fn count_syscalls(work: &Work, snp: &Path, s: &Scenario) -> anyhow::Result<u64> {
    let summary = work.results().join(format!("strace-{}.txt", s.id));
    ensure_own_repo(work, &work.fixture(s.fixture, Tool::Snp))?;
    let env = scenario_env(work, Tool::Snp, snp, s);
    let (program, argv) = scenario_command(s, snp);
    clean_patches(work, Tool::Snp, s, false)?;
    let mut cmd = Work::command("strace", &env);
    cmd.args(["-f", "-c", "-o"])
        .arg(&summary)
        .arg(program)
        .args(argv)
        .current_dir(work.fixture(s.fixture, Tool::Snp));
    run_ok(&mut cmd, "strace -f -c")?;
    clean_patches(work, Tool::Snp, s, false)?;
    let text = std::fs::read_to_string(&summary)?;
    parse_strace_total(&text).context("strace printed no total row")
}

/// The syscalls `--counts` traces (ADR-100); `?` ignores a name the architecture lacks (aarch64 has no `fork`).
const COUNTED_SYSCALLS: &str =
    "trace=?clone,?clone3,?fork,?vfork,?execve,?statx,?newfstatat,?openat,?getdents64";

/// The counters of a `strace -c` table: `clones` (`clone` + `clone3` + `fork` + `vfork`, the whole tree) and each of
/// `execve`, `statx`, `newfstatat`, `openat`, `getdents64` (0 when the table has no row for it).
fn counters_of(table: &BTreeMap<String, u64>) -> BTreeMap<String, u64> {
    let get = |k: &str| table.get(k).copied().unwrap_or(0);
    let clones = ["clone", "clone3", "fork", "vfork"]
        .into_iter()
        .map(get)
        .fold(0_u64, u64::saturating_add);
    let mut out = BTreeMap::from([("clones".to_owned(), clones)]);
    for k in ["execve", "statx", "newfstatat", "openat", "getdents64"] {
        out.insert(k.to_owned(), get(k));
    }
    out
}

/// One counting pass of `tool` on `s` after the timing runs (ADR-100, P15): `strace -f --seccomp-bpf -c` over the
/// counted syscalls, then one `strace -f -e trace=execve` pass for the exec argv list (normalized: paths of the
/// bench directory, store and binary replaced, long digit runs folded). B7 replays the cold store's removal before
/// each pass, B8 its patch cleanup.
fn count_pass(
    work: &Work,
    tool: Tool,
    exe: &Path,
    s: &Scenario,
    raw: &Path,
) -> anyhow::Result<(BTreeMap<String, u64>, Vec<String>)> {
    let dir = work.fixture(s.fixture, tool);
    ensure_own_repo(work, &dir)?;
    let env = scenario_env(work, tool, exe, s);
    let (program, argv) = scenario_command(s, exe);
    let replay = || -> anyhow::Result<()> {
        if s.store == StoreMode::Cold {
            remove_tree(&work.store(tool, StoreMode::Cold))?;
        }
        clean_patches(work, tool, s, false)
    };
    replay()?;
    let summary = raw.join(format!("{}-{}.counts.txt", s.id, tool.name()));
    let mut cmd = Work::command("strace", &env);
    cmd.args(["-f", "--seccomp-bpf", "-c", "-e", COUNTED_SYSCALLS, "-o"])
        .arg(&summary)
        .arg(&program)
        .args(&argv)
        .current_dir(&dir);
    run_ok(&mut cmd, "strace -f --seccomp-bpf -c")?;
    let table = parse_strace_table(&std::fs::read_to_string(&summary)?);
    ensure!(!table.is_empty(), "strace counted no syscall");
    replay()?;
    let trace = raw.join(format!("{}-{}.execve.txt", s.id, tool.name()));
    let mut cmd = Work::command("strace", &env);
    cmd.args([
        "-f",
        "-qq",
        "-s",
        "4096",
        "-e",
        "trace=execve",
        "-e",
        "status=successful",
        "-e",
        "signal=none",
        "-o",
    ])
    .arg(&trace)
    .arg(&program)
    .args(&argv)
    .current_dir(&dir);
    run_ok(&mut cmd, "strace -f -e trace=execve")?;
    replay()?;
    let subs = [
        (exe.display().to_string(), "$SNP"),
        (dir.display().to_string(), "$FIXTURE"),
        (work.store(tool, s.store).display().to_string(), "$STORE"),
        (work.dir.display().to_string(), "$BENCH"),
    ];
    let execs = parse_execve(&std::fs::read_to_string(&trace)?)
        .iter()
        .map(|argv| normalize_argv(argv, &subs))
        .collect();
    Ok((counters_of(&table), execs))
}

/// The argv of every `execve` line of an strace log (`[pid] execve("path", ["a", "b"], …) = 0`), each argument as
/// strace printed it (quoted C escapes kept, so the comparison is exact).
fn parse_execve(log: &str) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for line in log.lines() {
        let Some((_, rest)) = line.split_once("execve(") else {
            continue;
        };
        let Some((_, rest)) = rest.split_once(", [") else {
            continue;
        };
        let mut args = Vec::new();
        let mut cur = String::new();
        let (mut quoted, mut escaped) = (false, false);
        for c in rest.chars() {
            if quoted {
                cur.push(c);
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    quoted = false;
                }
            } else if c == '"' {
                cur.push(c);
                quoted = true;
            } else if c == ',' || c == ']' {
                let arg = cur.trim();
                if !arg.is_empty() {
                    args.push(arg.to_owned());
                }
                cur.clear();
                if c == ']' {
                    break;
                }
            } else {
                cur.push(c);
            }
        }
        out.push(args);
    }
    out
}

/// One exec argv as a comparable line: `subs` replaced, digit runs of 5 or more (times, pids) folded to `N`, and
/// more than 8 arguments (a hook's file list) cut to the first 8 and a count.
fn normalize_argv(argv: &[String], subs: &[(String, &str)]) -> String {
    const SHOWN: usize = 8;
    let mut parts: Vec<String> = argv
        .iter()
        .take(SHOWN)
        .map(|a| {
            let mut a = a.clone();
            for (from, to) in subs {
                if !from.is_empty() {
                    a = a.replace(from.as_str(), to);
                }
            }
            fold_digits(&a)
        })
        .collect();
    if argv.len() > SHOWN {
        parts.push(format!("… ({} arguments)", argv.len()));
    }
    parts.join(" ")
}

/// Runs of 5 or more ASCII digits replaced by `N`.
fn fold_digits(s: &str) -> String {
    let mut out = String::new();
    let mut run = String::new();
    let flush = |out: &mut String, run: &mut String| {
        if run.len() >= 5 {
            out.push('N');
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in s.chars() {
        if c.is_ascii_digit() {
            run.push(c);
        } else {
            flush(&mut out, &mut run);
            out.push(c);
        }
    }
    flush(&mut out, &mut run);
    out
}

/// The most exec-diff lines a record keeps per scenario.
const EXEC_DIFF_MAX: usize = 40;

/// The exec argv lines of `head` and `base` as a multiset difference: `+n× line` ran more often in the head, `-n× line`
/// more often in the base (both directions, so a dropped parity or safety call shows), sorted, at most
/// [`EXEC_DIFF_MAX`].
fn exec_diff(head: &[String], base: &[String]) -> Vec<String> {
    let mut count: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
    for l in head {
        let e = count.entry(l).or_default();
        e.0 = e.0.saturating_add(1);
    }
    for l in base {
        let e = count.entry(l).or_default();
        e.1 = e.1.saturating_add(1);
    }
    let mut out: Vec<String> = count
        .into_iter()
        .filter(|(_, (h, b))| h != b)
        .map(|(l, (h, b))| {
            if h > b {
                format!("+{}× {l}", h.saturating_sub(b))
            } else {
                format!("-{}× {l}", b.saturating_sub(h))
            }
        })
        .collect();
    if out.len() > EXEC_DIFF_MAX {
        let more = out.len().saturating_sub(EXEC_DIFF_MAX);
        out.truncate(EXEC_DIFF_MAX);
        out.push(format!("… and {more} more"));
    }
    out
}

/// Builds snp with `--features dhat-heap` (release, own target dir): the binary.
fn dhat_build(work: &Work, build: &Build) -> anyhow::Result<PathBuf> {
    let target_dir = work.dir.join("dhat-target");
    let mut cmd = Command::new("cargo");
    cmd.args([
        "build",
        "--release",
        "--locked",
        "-p",
        "snp",
        "--bin",
        "snp",
        "--features",
        "dhat-heap",
    ])
    .arg("--target-dir")
    .arg(&target_dir)
    .current_dir(&work.root);
    let mut bin_dir = target_dir;
    if let Some(t) = &build.target {
        cmd.args(["--target", t]);
        bin_dir.push(t);
    }
    cargo_jobs(&mut cmd, build.jobs);
    say("bench: building snp (dhat-heap)");
    run_ok(&mut cmd, "cargo build --features dhat-heap")?;
    Ok(bin_dir.join("release").join(exe_name("snp")))
}

/// One run of the dhat build on `s`: its totals and heap peak. The profile `dhat-heap.json` lands in the fixture (the
/// working directory) and is removed.
fn dhat_run(work: &Work, exe: &Path, s: &Scenario) -> anyhow::Result<DhatSummary> {
    let dir = work.fixture(s.fixture, Tool::Snp);
    ensure_own_repo(work, &dir)?;
    let profile = dir.join("dhat-heap.json");
    remove_tree(&profile)?;
    clean_patches(work, Tool::Snp, s, false)?;
    let env = scenario_env(work, Tool::Snp, exe, s);
    let (program, argv) = scenario_command(s, exe);
    let mut run = Work::command(program, &env);
    run.args(argv).current_dir(&dir);
    let ran = run_ok(&mut run, "snp (dhat-heap)");
    let text =
        std::fs::read_to_string(&profile).with_context(|| format!("reading {}", profile.display()));
    remove_tree(&profile)?;
    clean_patches(work, Tool::Snp, s, false)?;
    ran?;
    parse_dhat(&text?)
}

/// `--dhat`: the dhat build once, then one profiled run per selected scenario of [`DHAT_SCENARIOS`] that snp measured.
/// A failure leaves the metrics unmeasured (the gate fails their hard budgets as such).
fn measure_dhat(work: &Work, build: &Build, scenarios: &[&Scenario], record: &mut Record) {
    let wanted: Vec<&Scenario> = scenarios
        .iter()
        .copied()
        .filter(|s| DHAT_SCENARIOS.contains(&s.id))
        .collect();
    if wanted.is_empty() {
        return;
    }
    let exe = match dhat_build(work, build) {
        Ok(exe) => exe,
        Err(e) => {
            say(&format!(
                "bench: warning: allocations not measured: {}",
                one_line(&format!("{e:#}"))
            ));
            return;
        }
    };
    for s in wanted {
        let Some(Outcome::Ok(m)) = record
            .scenarios
            .get_mut(s.id)
            .and_then(|t| t.get_mut("snp"))
        else {
            continue;
        };
        match dhat_run(work, &exe, s) {
            Ok(d) => {
                say(&format!(
                    "bench: {} snp: {} bytes in {} blocks allocated, heap peak {} bytes (dhat)",
                    s.id, d.total_bytes, d.total_blocks, d.peak_bytes
                ));
                m.alloc_mib = Some(bytes_to_mib(d.total_bytes));
                m.alloc_blocks = Some(d.total_blocks);
                m.heap_peak_mib = Some(bytes_to_mib(d.peak_bytes));
            }
            Err(e) => say(&format!(
                "bench: warning: {} snp: allocation not measured: {}",
                s.id,
                one_line(&format!("{e:#}"))
            )),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Budgets and the gate
// ---------------------------------------------------------------------------------------------------------------

/// A gated metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Metric {
    MedianMs,
    /// The peak RSS of the largest process of the tree (`/usr/bin/time`): it can only overstate snp's own.
    RssMib,
    Syscalls,
    AllocMib,
    /// dhat's total blocks allocated (ADR-100).
    AllocBlocks,
    /// dhat's heap peak (ADR-100): what snp itself holds at once, whatever its children use.
    HeapPeakMib,
}

impl Metric {
    fn parse(s: &str) -> anyhow::Result<Self> {
        Ok(match s {
            "median_ms" => Self::MedianMs,
            "rss_mib" => Self::RssMib,
            "syscalls" => Self::Syscalls,
            "alloc_mib" => Self::AllocMib,
            "alloc_blocks" => Self::AllocBlocks,
            "heap_peak_mib" => Self::HeapPeakMib,
            other => bail!("unknown metric {other:?}"),
        })
    }

    const fn unit(self) -> &'static str {
        match self {
            Self::MedianMs => "ms",
            Self::RssMib | Self::AllocMib | Self::HeapPeakMib => "MiB",
            Self::Syscalls => "syscalls",
            Self::AllocBlocks => "blocks",
        }
    }

    /// A count, printed without decimals.
    const fn is_count(self) -> bool {
        matches!(self, Self::Syscalls | Self::AllocBlocks)
    }

    /// The start of the check's name.
    const fn label(self) -> &'static str {
        match self {
            Self::MedianMs => "p50",
            Self::RssMib => "RSS",
            Self::Syscalls => "syscalls",
            Self::AllocMib => "allocated",
            Self::AllocBlocks => "blocks allocated",
            Self::HeapPeakMib => "heap peak",
        }
    }

    /// The `Record::extras` name of a metric measured only on request (`--syscalls`, `--dhat`).
    const fn extra(self) -> Option<&'static str> {
        match self {
            Self::Syscalls => Some("syscalls"),
            Self::AllocMib => Some("alloc_mib"),
            Self::AllocBlocks => Some("alloc_blocks"),
            Self::HeapPeakMib => Some("heap_peak_mib"),
            Self::MedianMs | Self::RssMib => None,
        }
    }

    fn of(self, m: &Measure) -> Option<f64> {
        match self {
            Self::MedianMs => Some(m.median_ms),
            Self::RssMib => m.rss_mib,
            Self::Syscalls => m.syscalls.map(to_f64),
            Self::AllocMib => m.alloc_mib,
            Self::AllocBlocks => m.alloc_blocks.map(to_f64),
            Self::HeapPeakMib => m.heap_peak_mib,
        }
    }
}

/// One `[[budget]]`.
#[derive(Debug, Clone, PartialEq)]
struct Budget {
    os: String,
    scenario: String,
    metric: Metric,
    max: f64,
    plus: f64,
    vs_tool: Option<String>,
    vs_scenario: Option<String>,
    hard: bool,
}

/// One `[[micro]]`.
#[derive(Debug, Clone, PartialEq)]
struct MicroThreshold {
    id: String,
    max_ns: Option<f64>,
    min_per_sec: Option<f64>,
    source: String,
    /// The limit is the target on the reference machine (`--reference`); elsewhere a breach is a warning
    /// (ADR-086 amendment of the budgets: shared CI runners are slower than the reference machine).
    reference_only: bool,
}

/// One `[[known_miss]]`: a check that fails today for a recorded reason (ADR-091).
#[derive(Debug, Clone, PartialEq)]
struct KnownMiss {
    scenario: String,
    /// The start of the check's name (`p50 <= prek`).
    check: String,
    reason: String,
}

/// `bench/budgets.toml`.
#[derive(Debug, Clone, PartialEq)]
struct Budgets {
    regression_pct: f64,
    regression_min_ms: f64,
    never_slower_than: Vec<String>,
    musl_adopted: String,
    limits: Vec<Budget>,
    micro: Vec<MicroThreshold>,
    known_misses: Vec<KnownMiss>,
}

fn toml_f64(v: &toml::Value) -> Option<f64> {
    v.as_float().or_else(|| {
        v.as_integer()
            .and_then(|i| i32::try_from(i).ok())
            .map(f64::from)
    })
}

impl Budgets {
    fn parse(text: &str) -> anyhow::Result<Self> {
        let t: toml::Table = text.parse().context("parsing budgets TOML")?;
        let gate = t
            .get("gate")
            .and_then(toml::Value::as_table)
            .context("no [gate] table")?;
        let regression_pct = gate
            .get("regression_pct")
            .and_then(toml_f64)
            .context("[gate] regression_pct")?;
        let regression_min_ms = gate
            .get("regression_min_ms")
            .and_then(toml_f64)
            .context("[gate] regression_min_ms")?;
        let mut never_slower_than = Vec::new();
        for v in gate
            .get("never_slower_than")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
        {
            // a misspelt tool would never be measured, and the rule would never compare anything
            let name = v
                .as_str()
                .with_context(|| format!("[gate] never_slower_than: {v} is not a tool name"))?;
            let tool = Tool::parse(name).context("[gate] never_slower_than")?;
            ensure!(
                tool != Tool::Snp,
                "[gate] never_slower_than: snp is the measured tool"
            );
            never_slower_than.push(name.to_owned());
        }
        let musl_adopted = t
            .get("musl")
            .and_then(|m| m.get("adopted"))
            .and_then(toml::Value::as_str)
            .unwrap_or("system")
            .to_owned();
        ensure!(
            musl_adopted == "system" || musl_adopted == "mimalloc",
            "[musl] adopted = {musl_adopted:?}: system or mimalloc"
        );
        let mut limits = Vec::new();
        for (i, b) in t
            .get("budget")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let s = |k: &str| b.get(k).and_then(toml::Value::as_str).map(str::to_owned);
            let at = || format!("[[budget]] #{}", i.saturating_add(1));
            let os = s("os").with_context(|| format!("{}: os", at()))?;
            ensure!(
                matches!(os.as_str(), "linux" | "macos" | "windows"),
                "{}: os {os:?}",
                at()
            );
            let vs_tool = s("vs_tool");
            let vs_scenario = s("vs_scenario");
            ensure!(
                vs_tool.is_none() || vs_scenario.is_none(),
                "{}: vs_tool and vs_scenario",
                at()
            );
            if let Some(t) = &vs_tool {
                Tool::parse(t).with_context(at)?;
            }
            limits.push(Budget {
                os,
                scenario: s("scenario").with_context(|| format!("{}: scenario", at()))?,
                metric: Metric::parse(&s("metric").with_context(|| format!("{}: metric", at()))?)
                    .with_context(at)?,
                max: b
                    .get("max")
                    .and_then(toml_f64)
                    .with_context(|| format!("{}: max", at()))?,
                plus: b.get("plus").and_then(toml_f64).unwrap_or(0.0),
                vs_tool,
                vs_scenario,
                hard: b
                    .get("hard")
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(false),
            });
        }
        let mut micro = Vec::new();
        for (i, m) in t
            .get("micro")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let id = m
                .get("id")
                .and_then(toml::Value::as_str)
                .with_context(|| format!("[[micro]] #{}: id", i.saturating_add(1)))?
                .to_owned();
            let max_ns = m.get("max_ns").and_then(toml_f64);
            let min_per_sec = m.get("min_per_sec").and_then(toml_f64);
            ensure!(
                max_ns.is_some() != min_per_sec.is_some(),
                "[[micro]] {id}: exactly one of max_ns, min_per_sec"
            );
            micro.push(MicroThreshold {
                id,
                max_ns,
                min_per_sec,
                source: m
                    .get("source")
                    .and_then(toml::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                reference_only: m
                    .get("reference_only")
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(false),
            });
        }
        let mut known_misses = Vec::new();
        for (i, m) in t
            .get("known_miss")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let field = |key: &str| {
                m.get(key)
                    .and_then(toml::Value::as_str)
                    .filter(|v| !v.is_empty())
                    .map(str::to_owned)
                    .with_context(|| format!("[[known_miss]] #{}: {key}", i.saturating_add(1)))
            };
            let scenario = field("scenario")?;
            ensure!(
                SCENARIOS.iter().any(|s| s.id == scenario),
                "[[known_miss]] #{}: unknown scenario {scenario}",
                i.saturating_add(1)
            );
            known_misses.push(KnownMiss {
                scenario,
                check: field("check")?,
                reason: field("reason")?,
            });
        }
        Ok(Self {
            regression_pct,
            regression_min_ms,
            never_slower_than,
            musl_adopted,
            limits,
            micro,
            known_misses,
        })
    }

    fn load(root: &Path) -> anyhow::Result<Self> {
        let path = root.join("bench").join("budgets.toml");
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("in {}", path.display()))
    }
}

/// The verdict of one check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Verdict {
    Pass,
    /// A soft budget breached, or a check that could not run.
    Warn,
    Fail,
}

/// One gate check.
#[derive(Debug, Clone, PartialEq)]
struct Check {
    scenario: String,
    what: String,
    verdict: Verdict,
    detail: String,
}

/// How many of the latest history records the drift check takes the median of.
const DRIFT_WINDOW: usize = 10;
/// The fewest history measurements of a scenario the drift check needs.
const DRIFT_MIN: usize = 3;

/// The record whose snp median on `id` is the median of `history`'s (the lower middle; `None` below [`DRIFT_MIN`]).
fn drift_reference<'a>(history: &'a [Record], id: &str) -> Option<(&'a Measure, usize)> {
    let mut ms: Vec<&Measure> = history.iter().filter_map(|r| r.get(id, "snp")).collect();
    if ms.len() < DRIFT_MIN {
        return None;
    }
    ms.sort_by(|a, b| a.median_ms.total_cmp(&b.median_ms));
    let n = ms.len();
    ms.get(n.saturating_sub(1) / 2).map(|m| (*m, n))
}

/// The regression rule of `snp` against `prev`: slower by more than `[gate] regression_pct` and
/// `regression_min_ms` with disjoint 95 % CIs fails, slower with overlapping CIs warns.
fn regression(snp: &Measure, prev: &Measure, b: &Budgets, against: &str) -> (Verdict, String) {
    let limit = (prev.median_ms * (1.0 + b.regression_pct / 100.0))
        .max(prev.median_ms + b.regression_min_ms);
    let slower = snp.median_ms > limit;
    let disjoint = snp.ci95_ms.0 > prev.ci95_ms.1;
    let verdict = if slower && disjoint {
        Verdict::Fail
    } else if slower {
        Verdict::Warn
    } else {
        Verdict::Pass
    };
    (
        verdict,
        format!(
            "{:.2} ms [{:.2}, {:.2}] vs {against} {:.2} ms [{:.2}, {:.2}] ({:+.1}%)",
            snp.median_ms,
            snp.ci95_ms.0,
            snp.ci95_ms.1,
            prev.median_ms,
            prev.ci95_ms.0,
            prev.ci95_ms.1,
            (snp.median_ms / prev.median_ms - 1.0) * 100.0
        ),
    )
}

/// The name of the paired regression check (ADR-099). It does not start with `no regression`, so
/// `--advisory-history`, which is about the history, never softens it.
const PAIRED_CHECK: &str = "no paired regression";

/// The paired regression rule of ADR-099 on the interleaved rounds of `head` and `base`: with
/// `d_r = ln(head_r / base_r)`, it fails when `exp(median d) > 1 + regression_pct`, the effect
/// `base × (exp(median d) − 1)` exceeds `regression_min_ms`, and the 95 % interval of the median of `d` lies above 0;
/// the point estimate alone above the threshold warns. `None` when the rounds cannot be paired (missing, unequal in
/// number, or not positive).
fn paired_regression(head: &Measure, base: &Measure, b: &Budgets) -> Option<(Verdict, String)> {
    if head.rounds_ms.is_empty() || head.rounds_ms.len() != base.rounds_ms.len() {
        return None;
    }
    let d: Vec<f64> = head
        .rounds_ms
        .iter()
        .zip(&base.rounds_ms)
        .map(|(h, b)| (h / b).ln())
        .collect();
    if d.iter().any(|x| !x.is_finite()) {
        return None;
    }
    let ratio = median(&d)?.exp();
    let (lo, hi) = median_ci95(&d)?;
    let (lo, hi) = (lo.exp(), hi.exp());
    let effect_ms = base.median_ms * (ratio - 1.0);
    let slower = ratio > 1.0 + b.regression_pct / 100.0 && effect_ms.abs() > b.regression_min_ms;
    let verdict = if slower && lo > 1.0 {
        Verdict::Fail
    } else if slower {
        Verdict::Warn
    } else {
        Verdict::Pass
    };
    Some((
        verdict,
        format!(
            "{:.2} ms vs base {:.2} ms: paired ratio {:+.1}% [{:+.1}%, {:+.1}%] over {} rounds ({:+.2} ms)",
            head.median_ms,
            base.median_ms,
            (ratio - 1.0) * 100.0,
            (lo - 1.0) * 100.0,
            (hi - 1.0) * 100.0,
            d.len(),
            effect_ms
        ),
    ))
}

/// The counters gated base against head on the same runner (ADR-100): `(counter, percent, at least)`; the head fails
/// when it exceeds the base by more than the percentage **and** by at least the absolute amount. Opens, dirents and
/// execs are reported only (`openat` varies by ±1; an exec change is reviewed through the argv diff), and nothing is
/// gated against the history (runner images change git and glibc).
const COUNTER_GATES: [(&str, f64, u64); 3] = [
    ("clones", 10.0, 5),
    ("statx", 2.0, 50),
    ("newfstatat", 2.0, 50),
];

/// The counter checks of `head` against `base` (both counted).
fn counter_checks(head: &Measure, base: &Measure) -> Vec<(String, Verdict, String)> {
    COUNTER_GATES
        .iter()
        .filter_map(|&(key, pct, min)| {
            let h = *head.counts.get(key)?;
            let b = *base.counts.get(key)?;
            let over = to_f64(h) > to_f64(b) * (1.0 + pct / 100.0) && h.saturating_sub(b) >= min;
            Some((
                format!("{key} <= base + {pct}%"),
                if over { Verdict::Fail } else { Verdict::Pass },
                format!("{h} vs base {b} ({})", signed_diff(h, b)),
            ))
        })
        .collect()
}

/// `h − b` with its sign (`+3`, `-2`, `+0`).
fn signed_diff(h: u64, b: u64) -> String {
    if h >= b {
        format!("+{}", h.saturating_sub(b))
    } else {
        format!("-{}", b.saturating_sub(h))
    }
}

/// `--advisory-history`: the checks that compare with a baseline warn instead of failing. A record from another
/// machine says nothing at this resolution: one binary measured 0.76 to 1.19 ms for B1 across hosted runner VMs
/// (ADR-090). Only those checks change; budgets and the never-slower rule are measured within the run.
fn advisory_history(checks: &mut [Check]) {
    for c in checks {
        if c.verdict == Verdict::Fail
            && (c.what.starts_with("no regression") || c.what.starts_with("no drift"))
        {
            c.verdict = Verdict::Warn;
            c.detail.push_str(" (--advisory-history)");
        }
    }
}

/// `[[known_miss]]`: a listed check that fails warns, with its reason, and one that passes says the entry can go.
/// Only the budget or never-slower check itself is covered: getting worse still fails the regression check.
fn apply_known_misses(checks: &mut [Check], misses: &[KnownMiss]) {
    for m in misses {
        for c in checks
            .iter_mut()
            .filter(|c| c.scenario == m.scenario && c.what.starts_with(&m.check))
        {
            match c.verdict {
                Verdict::Fail => {
                    c.verdict = Verdict::Warn;
                    write!(c.detail, " (known miss: {})", m.reason).unwrap_or_default();
                }
                Verdict::Pass => c
                    .detail
                    .push_str(" (a known miss: remove the entry once this holds on every runner)"),
                Verdict::Warn => {}
            }
        }
    }
}

/// Every check of `cur` (and against `base`) under `b` (the e2e gate; pure). Nothing passes silently: a budget whose
/// scenario was not measured fails when hard (a warning when its scenario is in `allow_missing` — the scenarios a
/// workflow knowingly does not run yet), a requested metric without a value fails, a competitor that failed or was not
/// measured fails the never-slower rule (a warning when the scenario is in `allow_missing_reference`, or for the
/// non-adopted musl allocator), and a missing baseline is a warning. `history` (the latest records of the JSON-lines
/// history; empty for an explicit baseline) adds the drift check: the regression rule against the median record of
/// the window, so a series of slowdowns each just under the threshold of the previous record still fails.
fn evaluate(
    cur: &Record,
    base: Option<&Record>,
    b: &Budgets,
    allow_missing: &BTreeSet<String>,
    allow_missing_reference: &BTreeSet<String>,
    history: &[Record],
) -> Vec<Check> {
    let mut out = Vec::new();
    let push = |out: &mut Vec<Check>, s: &str, what: String, verdict, detail: String| {
        out.push(Check {
            scenario: s.to_owned(),
            what,
            verdict,
            detail,
        });
    };
    let musl_other = cur.variant.starts_with("musl-") && !cur.variant.ends_with(&b.musl_adopted);
    for (id, tools) in &cur.scenarios {
        // snp itself must have worked
        match tools.get("snp") {
            Some(Outcome::Failed(e)) => {
                push(&mut out, id, "snp ran".into(), Verdict::Fail, one_line(e));
                continue;
            }
            None => continue,
            Some(Outcome::Ok(_)) => {}
        }
        let Some(snp) = cur.get(id, "snp") else {
            continue;
        };
        // never slower than pre-commit or prek at p50 (the non-adopted musl allocator is only reported)
        for other in &b.never_slower_than {
            let o = match tools.get(other) {
                Some(Outcome::Ok(o)) => o,
                failed_or_absent => {
                    let why = match failed_or_absent {
                        Some(Outcome::Failed(e)) => {
                            format!("reference {other} failed: {}", one_line(e))
                        }
                        _ => format!("reference {other} not measured"),
                    };
                    let (verdict, why) = if allow_missing_reference.contains(id) {
                        (Verdict::Warn, format!("{why} (--allow-missing-reference)"))
                    } else if musl_other {
                        (Verdict::Warn, why)
                    } else {
                        (Verdict::Fail, why)
                    };
                    push(&mut out, id, format!("p50 <= {other}"), verdict, why);
                    continue;
                }
            };
            let ok = snp.median_ms <= o.median_ms;
            let verdict = if ok {
                Verdict::Pass
            } else if musl_other {
                Verdict::Warn
            } else {
                Verdict::Fail
            };
            push(
                &mut out,
                id,
                format!("p50 <= {other}"),
                verdict,
                format!(
                    "{:.2} ms vs {:.2} ms ({:.2}x)",
                    snp.median_ms,
                    o.median_ms,
                    o.median_ms / snp.median_ms
                ),
            );
        }
        // the paired regression against the base binary measured in the same rounds (ADR-099): it gates, and the
        // unpaired comparisons below are then advisory (they compare measurements minutes or VMs apart)
        let paired_base = tools.get(Tool::SnpBase.name());
        match paired_base {
            Some(Outcome::Ok(bm)) => {
                let (verdict, detail) = paired_regression(snp, bm, b).unwrap_or_else(|| {
                    (
                        Verdict::Warn,
                        format!(
                            "the rounds cannot be paired ({} head, {} base)",
                            snp.rounds_ms.len(),
                            bm.rounds_ms.len()
                        ),
                    )
                });
                push(
                    &mut out,
                    id,
                    format!("{PAIRED_CHECK} > {}%", b.regression_pct),
                    verdict,
                    detail,
                );
            }
            Some(Outcome::Failed(e)) => push(
                &mut out,
                id,
                format!("{PAIRED_CHECK} > {}%", b.regression_pct),
                Verdict::Warn,
                format!("the base was not measured: {}", one_line(e)),
            ),
            None => {}
        }
        let first_unpaired = out.len();
        // regression against the baseline: > pct and non-overlapping 95 % CIs
        if base.is_some_and(|r| r.get(id, "snp").is_none()) {
            push(
                &mut out,
                id,
                format!("no regression > {}%", b.regression_pct),
                Verdict::Warn,
                "the baseline has no snp measurement of this scenario".into(),
            );
        }
        if let Some(prev) = base.and_then(|r| r.get(id, "snp")) {
            let (verdict, detail) = regression(snp, prev, b, "baseline");
            push(
                &mut out,
                id,
                format!("no regression > {}%", b.regression_pct),
                verdict,
                detail,
            );
        }
        if let Some((prev, n)) = drift_reference(history, id) {
            let (verdict, detail) =
                regression(snp, prev, b, &format!("median of the last {n} records"));
            push(
                &mut out,
                id,
                format!("no drift > {}%", b.regression_pct),
                verdict,
                detail,
            );
        }
        if paired_base.is_some() {
            for c in out.iter_mut().skip(first_unpaired) {
                if c.verdict == Verdict::Fail {
                    c.verdict = Verdict::Warn;
                    c.detail.push_str(" (advisory: the paired base gates)");
                }
            }
        }
        // the deterministic counters against the base, same runner (ADR-100); never against the history
        if cur.extras.contains("counts") {
            match (snp.counts.is_empty(), paired_base) {
                (true, _) => push(
                    &mut out,
                    id,
                    "counted".into(),
                    Verdict::Fail,
                    "--counts was requested, but snp was not counted".into(),
                ),
                (false, Some(Outcome::Ok(bm))) if !bm.counts.is_empty() => {
                    for (what, verdict, detail) in counter_checks(snp, bm) {
                        push(&mut out, id, what, verdict, detail);
                    }
                }
                (false, Some(Outcome::Ok(_))) => push(
                    &mut out,
                    id,
                    "counters <= base".into(),
                    Verdict::Warn,
                    "the base was not counted".into(),
                ),
                (false, Some(Outcome::Failed(_)) | None) => {}
            }
        }
    }
    // budgets of this OS
    let paired_anywhere = cur
        .scenarios
        .values()
        .any(|t| t.contains_key(Tool::SnpBase.name()));
    if base.is_none() && !paired_anywhere {
        push(
            &mut out,
            "*",
            format!("no regression > {}%", b.regression_pct),
            Verdict::Warn,
            format!(
                "no baseline for {}-{}: the regression check is skipped",
                cur.platform, cur.variant
            ),
        );
    }
    for bud in b.limits.iter().filter(|x| x.os == cur.os()) {
        // syscalls and allocations are measured only on request (the nightly); a requested one must be there
        if bud.metric.extra().is_some_and(|e| !cur.extras.contains(e)) {
            continue;
        }
        let what = format!(
            "{} <= {}{}",
            bud.metric.label(),
            budget_text(bud),
            if bud.hard { " (hard)" } else { "" }
        );
        let snp = match cur.scenarios.get(&bud.scenario).and_then(|t| t.get("snp")) {
            Some(Outcome::Ok(m)) => m,
            Some(Outcome::Failed(_)) => continue, // already a "snp ran" failure
            None => {
                let allowed = allow_missing.contains(&bud.scenario);
                let (verdict, detail) = if allowed {
                    (Verdict::Warn, "scenario not measured (--allow-missing)")
                } else if bud.hard {
                    (Verdict::Fail, "scenario not measured")
                } else {
                    (Verdict::Warn, "scenario not measured")
                };
                push(&mut out, &bud.scenario, what, verdict, detail.into());
                continue;
            }
        };
        let Some(value) = bud.metric.of(snp) else {
            push(
                &mut out,
                &bud.scenario,
                what,
                if bud.hard {
                    Verdict::Fail
                } else {
                    Verdict::Warn
                },
                "metric not measured".into(),
            );
            continue;
        };
        let reference = match (&bud.vs_tool, &bud.vs_scenario) {
            (Some(t), _) => cur.get(&bud.scenario, t).and_then(|m| bud.metric.of(m)),
            (_, Some(s)) => cur.get(s, "snp").and_then(|m| bud.metric.of(m)),
            (None, None) => Some(1.0),
        };
        let Some(reference) = reference else {
            push(
                &mut out,
                &bud.scenario,
                what,
                Verdict::Warn,
                "reference not measured".into(),
            );
            continue;
        };
        let limit = bud.max.mul_add(reference, bud.plus);
        let verdict = if value <= limit {
            Verdict::Pass
        } else if bud.hard {
            Verdict::Fail
        } else {
            Verdict::Warn
        };
        let unit = bud.metric.unit();
        let detail = if bud.metric.is_count() {
            format!("{value:.0} {unit} (limit {limit:.0})")
        } else {
            format!("{value:.2} {unit} (limit {limit:.2} {unit})")
        };
        push(&mut out, &bud.scenario, what, verdict, detail);
    }
    out.sort_by_key(|c| scenario_rank(&c.scenario));
    out
}

fn budget_text(b: &Budget) -> String {
    let unit = b.metric.unit();
    let plus = if b.plus > 0.0 {
        format!(" + {} {unit}", b.plus)
    } else {
        String::new()
    };
    match (&b.vs_tool, &b.vs_scenario) {
        (Some(t), _) => format!("{}x {t}{plus}", b.max),
        (_, Some(s)) => format!("{}x {s}{plus}", b.max),
        (None, None) => format!("{} {unit}", b.max),
    }
}

/// The history file of a platform and variant.
fn history_file(dir: &Path, platform: &str, variant: &str) -> PathBuf {
    dir.join(format!("{platform}-{variant}.jsonl"))
}

/// The last `n` records of a JSON-lines history, oldest first (empty when the file is missing or empty).
fn history_records(path: &Path, n: usize) -> anyhow::Result<Vec<Record>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let skip = lines.len().saturating_sub(n);
    lines
        .iter()
        .enumerate()
        .skip(skip)
        .map(|(i, line)| {
            let v: Value = serde_json::from_str(line).with_context(|| {
                format!(
                    "parsing record {} of {}",
                    i.saturating_add(1),
                    path.display()
                )
            })?;
            Record::from_json(&v)
        })
        .collect()
}

fn fmt_measure(m: Option<&Measure>) -> String {
    m.map_or_else(|| "—".to_owned(), |m| format!("{:.2}", m.median_ms))
}

/// The counters of the comment: `clones / statx / newfstatat / execve` of snp, each with its difference to the base.
fn counts_cell(snp: Option<&Measure>, base: Option<&Measure>) -> String {
    let Some(h) = snp.filter(|m| !m.counts.is_empty()) else {
        return "—".to_owned();
    };
    ["clones", "statx", "newfstatat", "execve"]
        .iter()
        .map(|k| {
            let hv = h.counts.get(*k).copied().unwrap_or(0);
            match base.and_then(|b| b.counts.get(*k)) {
                Some(bv) => format!("{hv} ({})", signed_diff(hv, *bv)),
                None => hv.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join(" / ")
}

/// The Markdown PR comment.
fn comment(cur: &Record, base: Option<&Record>, checks: &[Check]) -> String {
    let worst = checks
        .iter()
        .map(|c| c.verdict)
        .max()
        .unwrap_or(Verdict::Pass);
    let mut s = String::new();
    _ = writeln!(
        s,
        "### snp benchmarks ({}, {})\n\n**Gate: {}** — commit `{}`{}\n",
        cur.platform,
        cur.variant,
        match worst {
            Verdict::Pass => "pass",
            Verdict::Warn => "pass with warnings",
            Verdict::Fail => "FAIL",
        },
        cur.commit.get(..12).unwrap_or(&cur.commit),
        base.map_or_default(|b| format!(
            ", baseline `{}` ({})",
            b.commit.get(..12).unwrap_or(&b.commit),
            b.label
        ))
    );
    let paired = cur
        .scenarios
        .values()
        .any(|t| t.contains_key(Tool::SnpBase.name()));
    let counted = cur
        .scenarios
        .keys()
        .any(|id| cur.get(id, "snp").is_some_and(|m| !m.counts.is_empty()));
    if paired {
        s.push_str(
            "The baseline is the base commit's snp, measured in the same interleaved rounds (Δ: the median per-round \
             ratio, ADR-099).\n\n",
        );
    }
    let mut header = String::from(
        "| Scenario | snp p50 ms (95% CI) | baseline | Δ | pre-commit | prek | RSS MiB (largest process) |",
    );
    let mut rule = String::from("|---|---|---|---|---|---|---|");
    if counted {
        header.push_str(" clones / statx / newfstatat / execve (Δ vs base) |");
        rule.push_str("---|");
    }
    _ = writeln!(s, "{header}\n{rule}");
    let mut rows: Vec<(&String, &BTreeMap<String, Outcome>)> = cur.scenarios.iter().collect();
    rows.sort_by_key(|(id, _)| scenario_rank(id));
    for (id, tools) in &rows {
        let snp = cur.get(id, "snp");
        let paired_base = cur.get(id, Tool::SnpBase.name());
        let prev = paired_base.or_else(|| base.and_then(|b| b.get(id, "snp")));
        let snp_cell = match (snp, tools.get("snp")) {
            (Some(m), _) => format!("{:.2} ({:.2}–{:.2})", m.median_ms, m.ci95_ms.0, m.ci95_ms.1),
            (None, Some(Outcome::Failed(_))) => "failed".to_owned(),
            (None, _) => "—".to_owned(),
        };
        let paired_ratio = snp
            .zip(paired_base)
            .and_then(|(h, b)| {
                let d: Vec<f64> = h
                    .rounds_ms
                    .iter()
                    .zip(&b.rounds_ms)
                    .map(|(x, y)| (x / y).ln())
                    .collect();
                (h.rounds_ms.len() == b.rounds_ms.len())
                    .then(|| median(&d))
                    .flatten()
            })
            .map(f64::exp);
        let delta = match (snp, prev, paired_ratio) {
            (_, _, Some(r)) => format!("{:+.1}% (paired)", (r - 1.0) * 100.0),
            (Some(a), Some(b), None) => {
                format!("{:+.1}%", (a.median_ms / b.median_ms - 1.0) * 100.0)
            }
            _ => "—".to_owned(),
        };
        let title = SCENARIOS
            .iter()
            .find(|x| x.id == id.as_str())
            .map_or("", |x| x.title);
        _ = write!(
            s,
            "| {id} {title} | {snp_cell} | {} | {delta} | {} | {} | {} |",
            fmt_measure(prev),
            fmt_measure(cur.get(id, "pre-commit")),
            fmt_measure(cur.get(id, "prek")),
            snp.and_then(|m| m.rss_mib)
                .map_or_else(|| "—".to_owned(), |r| format!("{r:.1}"))
        );
        if counted {
            _ = write!(s, " {} |", counts_cell(snp, paired_base));
        }
        s.push('\n');
    }
    let diffs: Vec<(&String, &Vec<String>)> = rows
        .iter()
        .filter_map(|(id, _)| {
            cur.get(id, "snp")
                .filter(|m| !m.exec_diff.is_empty())
                .map(|m| (*id, &m.exec_diff))
        })
        .collect();
    if !diffs.is_empty() {
        s.push_str(
            "\n<details><summary>Exec changes against the base (+ only in this commit, - only in the base)</summary>\n\n",
        );
        for (id, lines) in diffs {
            _ = writeln!(s, "**{id}**\n\n```");
            for l in lines {
                _ = writeln!(s, "{l}");
            }
            s.push_str("```\n\n");
        }
        s.push_str("</details>\n");
    }
    let notable: Vec<&Check> = checks
        .iter()
        .filter(|c| c.verdict != Verdict::Pass)
        .collect();
    if !notable.is_empty() {
        s.push_str("\n| Scenario | Check | Verdict | Detail |\n|---|---|---|---|\n");
        for c in notable {
            _ = writeln!(
                s,
                "| {} | {} | {} | {} |",
                c.scenario,
                c.what,
                if c.verdict == Verdict::Fail {
                    "**fail**"
                } else {
                    "warn"
                },
                c.detail.replace('|', "\\|")
            );
        }
    }
    s
}

fn gate(root: &Path, args: &[String]) -> anyhow::Result<()> {
    let Some(o) = Opts::parse(
        args,
        &[
            "current",
            "baseline",
            "history",
            "comment",
            "criterion",
            "allow-missing",
            "allow-missing-reference",
            "since",
        ],
        &[
            "record",
            "micro",
            "compare",
            "any-age",
            "reference",
            "advisory-history",
        ],
    )?
    else {
        say(GATE_HELP);
        return Ok(());
    };
    let budgets = Budgets::load(root)?;
    let target = target_dir(root)?;
    if o.has("micro") {
        o.no_positional()?;
        let dir = o
            .one("criterion")
            .map_or_else(|| target.join("criterion"), PathBuf::from);
        let since = o
            .one("since")
            .map(|v| {
                v.parse::<u64>()
                    .ok()
                    .and_then(|secs| {
                        std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(secs))
                    })
                    .with_context(|| format!("--since {v:?}: not a Unix time in seconds"))
            })
            .transpose()?;
        // criterion keeps the results of every earlier `cargo bench`: without a cut-off a benchmark that no longer
        // runs (renamed, removed, not selected) would pass on stale numbers
        ensure!(
            since.is_some() || o.has("any-age"),
            "gate --micro needs --since <unix-secs> (the time `cargo bench` started), or --any-age to accept results \
             of any age"
        );
        return gate_micro(&budgets, &dir, since, o.has("reference"));
    }
    if o.has("compare") {
        let [a, b] = o.positional.as_slice() else {
            bail!("--compare takes two run records");
        };
        compare(&Record::read(Path::new(a))?, &Record::read(Path::new(b))?);
        return Ok(());
    }
    o.no_positional()?;
    let work_results = target.join("bench").join("results");
    let current_path = o
        .one("current")
        .map_or_else(|| work_results.join("current.json"), PathBuf::from);
    let cur = Record::read(&current_path)?;
    let history = o
        .one("history")
        .map_or_else(|| root.join("bench").join("history"), PathBuf::from);
    let hist_file = history_file(&history, &cur.platform, &cur.variant);
    // an explicit baseline (a pull request's base commit) has no history; otherwise the last record is the baseline
    // and the window of the latest ones the drift reference
    // (an explicit baseline is measured on this machine: nothing may turn that comparison off)
    ensure!(
        !(o.has("advisory-history") && o.one("baseline").is_some()),
        "--advisory-history is about the history: it does not apply to --baseline"
    );
    let (base, recent) = if let Some(p) = o.one("baseline") {
        (Some(Record::read(Path::new(p))?), Vec::new())
    } else {
        let recent = history_records(&hist_file, DRIFT_WINDOW)?;
        (recent.last().cloned(), recent)
    };
    let allow_missing: BTreeSet<String> = o.list("allow-missing").into_iter().collect();
    let allow_missing_reference: BTreeSet<String> =
        o.list("allow-missing-reference").into_iter().collect();
    let mut checks = evaluate(
        &cur,
        base.as_ref(),
        &budgets,
        &allow_missing,
        &allow_missing_reference,
        &recent,
    );
    if o.has("advisory-history") {
        advisory_history(&mut checks);
    }
    apply_known_misses(&mut checks, &budgets.known_misses);
    for c in &checks {
        say(&format!(
            "{:<5} {:<5} {:<34} {}",
            match c.verdict {
                Verdict::Pass => "ok",
                Verdict::Warn => "WARN",
                Verdict::Fail => "FAIL",
            },
            c.scenario,
            c.what,
            c.detail
        ));
    }
    if let Some(path) = o.one("comment") {
        std::fs::write(path, comment(&cur, base.as_ref(), &checks))
            .with_context(|| format!("writing {path}"))?;
    }
    let failed: Vec<&Check> = checks
        .iter()
        .filter(|c| c.verdict == Verdict::Fail)
        .collect();
    ensure!(
        failed.is_empty(),
        "bench gate: {} failed check(s)",
        failed.len()
    );
    if o.has("record") {
        std::fs::create_dir_all(&history)?;
        let mut text = match std::fs::read_to_string(&hist_file) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", hist_file.display())),
        };
        text.push_str(&serde_json::to_string(&cur.to_json())?);
        text.push('\n');
        std::fs::write(&hist_file, text)
            .with_context(|| format!("writing {}", hist_file.display()))?;
        say(&format!("bench: recorded in {}", hist_file.display()));
    }
    say(&format!("bench gate: pass ({} checks)", checks.len()));
    Ok(())
}

/// The effect of record `b` against `a` per scenario (snp only): ADR-047, ADR-079 decisions.
fn compare(a: &Record, b: &Record) {
    say(&format!(
        "{:<6} {:>12} {:>12} {:>9}  {}",
        "scen", a.label, b.label, "change", "significant"
    ));
    let ids: BTreeSet<&String> = a.scenarios.keys().chain(b.scenarios.keys()).collect();
    for id in ids {
        if let (Some(x), Some(y)) = (a.get(id, "snp"), b.get(id, "snp")) {
            let disjoint = y.ci95_ms.0 > x.ci95_ms.1 || y.ci95_ms.1 < x.ci95_ms.0;
            say(&format!(
                "{id:<6} {:>12.2} {:>12.2} {:>+8.1}%  {}{}",
                x.median_ms,
                y.median_ms,
                (y.median_ms / x.median_ms - 1.0) * 100.0,
                if disjoint {
                    "yes (95% CIs disjoint)"
                } else {
                    "no"
                },
                match (x.rss_mib, y.rss_mib) {
                    (Some(r1), Some(r2)) => format!("; RSS {r1:.1} -> {r2:.1} MiB"),
                    _ => String::new(),
                }
            ));
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// gate --micro (criterion)
// ---------------------------------------------------------------------------------------------------------------

/// One criterion benchmark result.
#[derive(Debug, Clone, PartialEq)]
struct CritResult {
    median_ns: f64,
    elements: Option<u64>,
}

/// Every `<dir>/**/new/{benchmark,estimates}.json` pair, keyed by the full benchmark id.
/// With `since`, a result whose `estimates.json` is older is stale (an earlier `cargo bench`, e.g. of a benchmark
/// that no longer runs) and is left out, so its threshold fails as unmeasured.
fn read_criterion(
    dir: &Path,
    since: Option<std::time::SystemTime>,
) -> anyhow::Result<BTreeMap<String, CritResult>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries {
            let e = e?;
            if e.file_type()?.is_dir() {
                stack.push(e.path());
            }
        }
        if d.file_name() != Some(OsStr::new("new")) {
            continue;
        }
        let (bench, est) = (d.join("benchmark.json"), d.join("estimates.json"));
        if !bench.is_file() || !est.is_file() {
            continue;
        }
        if let Some(since) = since {
            let modified = std::fs::metadata(&est)
                .and_then(|m| m.modified())
                .with_context(|| format!("reading the mtime of {}", est.display()))?;
            if modified < since {
                say(&format!(
                    "bench gate --micro: ignoring the stale result {}",
                    est.display()
                ));
                continue;
            }
        }
        let b: Value = serde_json::from_str(&std::fs::read_to_string(&bench)?)
            .with_context(|| format!("parsing {}", bench.display()))?;
        let e: Value = serde_json::from_str(&std::fs::read_to_string(&est)?)
            .with_context(|| format!("parsing {}", est.display()))?;
        let id = b
            .get("full_id")
            .and_then(Value::as_str)
            .with_context(|| format!("{}: no full_id", bench.display()))?;
        let median_ns = e
            .get("median")
            .and_then(|m| m.get("point_estimate"))
            .and_then(Value::as_f64)
            .with_context(|| format!("{}: no median estimate", est.display()))?;
        let elements = b
            .get("throughput")
            .and_then(|t| t.get("Elements"))
            .and_then(Value::as_u64);
        out.insert(
            id.to_owned(),
            CritResult {
                median_ns,
                elements,
            },
        );
    }
    Ok(out)
}

/// The micro checks (pure): a missing benchmark fails.
fn evaluate_micro(
    thresholds: &[MicroThreshold],
    results: &BTreeMap<String, CritResult>,
    reference: bool,
) -> Vec<Check> {
    thresholds
        .iter()
        .map(|t| {
            let (verdict, what, detail) = match (results.get(&t.id), t.max_ns, t.min_per_sec) {
                (None, _, _) => (
                    Verdict::Fail,
                    "measured".to_owned(),
                    "no criterion result".to_owned(),
                ),
                (Some(r), Some(max), _) => (
                    if r.median_ns <= max {
                        Verdict::Pass
                    } else {
                        Verdict::Fail
                    },
                    format!("median <= {}", fmt_ns(max)),
                    fmt_ns(r.median_ns),
                ),
                (Some(r), None, Some(min)) => match r.elements {
                    Some(n) if r.median_ns > 0.0 => {
                        let rate = to_f64(n) / (r.median_ns / 1e9);
                        (
                            if rate >= min {
                                Verdict::Pass
                            } else {
                                Verdict::Fail
                            },
                            format!(">= {min:.0}/s"),
                            format!("{rate:.0}/s"),
                        )
                    }
                    _ => (
                        Verdict::Fail,
                        "throughput".to_owned(),
                        "no element throughput declared".to_owned(),
                    ),
                },
                (Some(_), None, None) => (
                    Verdict::Fail,
                    "threshold".to_owned(),
                    "empty threshold".to_owned(),
                ),
            };
            // a measured breach of a reference-machine limit off the reference machine is a warning
            let (verdict, detail) = if verdict == Verdict::Fail
                && t.reference_only
                && !reference
                && results.contains_key(&t.id)
            {
                (
                    Verdict::Warn,
                    format!("{detail} (hard on the reference machine only, `--reference`)"),
                )
            } else {
                (verdict, detail)
            };
            Check {
                scenario: t.id.clone(),
                what,
                verdict,
                detail: if t.source.is_empty() {
                    detail
                } else {
                    format!("{detail} — {}", t.source)
                },
            }
        })
        .collect()
}

fn fmt_ns(ns: f64) -> String {
    if ns >= 1e6 {
        format!("{:.2} ms", ns / 1e6)
    } else if ns >= 1e3 {
        format!("{:.2} µs", ns / 1e3)
    } else {
        format!("{ns:.0} ns")
    }
}

fn gate_micro(
    b: &Budgets,
    dir: &Path,
    since: Option<std::time::SystemTime>,
    reference: bool,
) -> anyhow::Result<()> {
    let results = read_criterion(dir, since)?;
    let checks = evaluate_micro(&b.micro, &results, reference);
    for c in &checks {
        say(&format!(
            "{:<5} {:<40} {:<18} {}",
            match c.verdict {
                Verdict::Pass => "ok",
                Verdict::Warn => "WARN",
                Verdict::Fail => "FAIL",
            },
            c.scenario,
            c.what,
            c.detail
        ));
    }
    let failed = checks.iter().filter(|c| c.verdict == Verdict::Fail).count();
    ensure!(
        failed == 0,
        "bench gate --micro: {failed} failed threshold(s) (criterion results in {})",
        dir.display()
    );
    say(&format!(
        "bench gate --micro: pass ({} thresholds)",
        checks.len()
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(median: f64, ci: (f64, f64)) -> Measure {
        Measure {
            median_ms: median,
            ci95_ms: ci,
            runs: 50,
            ..Measure::default()
        }
    }

    fn record(variant: &str, rows: &[(&str, &str, Outcome)]) -> Record {
        let mut r = Record {
            platform: "linux-x86_64".into(),
            variant: variant.into(),
            label: "t".into(),
            commit: "0123456789abcdef".into(),
            ..Record::default()
        };
        for (s, t, o) in rows {
            r.scenarios
                .entry((*s).to_owned())
                .or_default()
                .insert((*t).to_owned(), o.clone());
        }
        r
    }

    fn budgets() -> Budgets {
        Budgets::load(&workspace::root().unwrap()).unwrap()
    }

    fn none() -> BTreeSet<String> {
        BTreeSet::new()
    }

    fn verdict_of<'a>(checks: &'a [Check], scenario: &str, what: &str) -> Option<&'a Check> {
        checks
            .iter()
            .find(|c| c.scenario == scenario && c.what.starts_with(what))
    }

    #[test]
    fn median_and_ci_are_order_statistics() {
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 1.0, 3.0, 2.0]), Some(2.5));
        let xs: Vec<f64> = (1..=50).map(f64::from).collect();
        // n = 50: ranks round(25 - 6.93) = 18 and round(26 + 6.93) = 33
        assert_eq!(median_ci95(&xs), Some((18.0, 33.0)));
        assert_eq!(median_ci95(&[7.0]), Some((7.0, 7.0)));
        let ten: Vec<f64> = (1..=10).map(f64::from).collect();
        let (lo, hi) = median_ci95(&ten).unwrap();
        assert!(lo <= 5.5 && hi >= 5.5 && lo >= 1.0 && hi <= 10.0);
    }

    #[test]
    fn conversions_are_exact() {
        assert_eq!(to_f64(0), 0.0);
        assert_eq!(to_f64(1 << 40), 1_099_511_627_776.0);
        assert_eq!(to_f64(u64::from(u32::MAX) + 7), 4_294_967_302.0);
        assert_eq!(bytes_to_mib(2 << 20), 2.0);
    }

    #[test]
    fn hyperfine_json_is_parsed() {
        let text = r#"{"results":[{"command":"snp B1","mean":0.0012,"stddev":0.0001,"median":0.0011,
            "min":0.0010,"max":0.0020,"times":[0.001,0.0011,0.0012,0.002,0.0011],"exit_codes":[0,0,0,0,0]}]}"#;
        let m = Measure::from_times(&parse_hyperfine_times(text).unwrap()).unwrap();
        assert!((m.median_ms - 1.1).abs() < 1e-9);
        // the statistics come from the times (pooled over blocks), not from hyperfine's per-invocation fields
        assert!((m.mean_ms - 1.28).abs() < 1e-9);
        assert!((m.min_ms - 1.0).abs() < 1e-9 && (m.max_ms - 2.0).abs() < 1e-9);
        assert!((m.stddev_ms - 0.167_f64.sqrt()).abs() < 1e-9);
        assert_eq!(m.runs, 5);
        assert!(m.ci95_ms.0 <= m.median_ms && m.median_ms <= m.ci95_ms.1);
        assert!(parse_hyperfine_times(r#"{"results":[]}"#).is_err());
        assert!(parse_hyperfine_times(r#"{"results":[{"times":[]}]}"#).is_err());
        assert!(parse_hyperfine_times("not json").is_err());
        // one run (a B7 block): no standard deviation
        let one = Measure::from_times(&[5.0]).unwrap();
        assert_eq!((one.median_ms, one.runs), (5.0, 1));
        assert!(one.stddev_ms.is_nan());
        assert!(Measure::from_times(&[]).is_err());
    }

    #[test]
    fn rss_strace_and_dhat_outputs_are_parsed() {
        let gnu = "\tCommand being timed: \"snp run\"\n\tMaximum resident set size (kbytes): 12345\n\tExit status: 0\n";
        assert_eq!(parse_time_rss_kib(gnu), Some(12345));
        let bsd =
            "        0.01 real         0.00 user\n           10485760  maximum resident set size\n";
        assert_eq!(parse_time_rss_kib(bsd), Some(10240));
        assert_eq!(parse_time_rss_kib("nothing"), None);

        let strace = "% time     seconds  usecs/call     calls    errors syscall\n\
            ------ ----------- ----------- --------- --------- ------------------\n\
             60.00    0.000060          10         6           mmap\n\
             40.00    0.000040          20         2         2 openat\n\
            ------ ----------- ----------- --------- --------- ------------------\n\
            100.00    0.000772          12        62         2 total\n";
        assert_eq!(parse_strace_total(strace), Some(62));
        let no_errors = "100.00    0.000100          10        10           total\n";
        assert_eq!(parse_strace_total(no_errors), Some(10));
        assert_eq!(parse_strace_total(""), None);

        let table = parse_strace_table(strace);
        assert_eq!(
            table,
            BTreeMap::from([("mmap".to_owned(), 6), ("openat".to_owned(), 2)])
        );
        let c = counters_of(&BTreeMap::from([
            ("clone3".to_owned(), 4),
            ("vfork".to_owned(), 2),
            ("statx".to_owned(), 9),
        ]));
        assert_eq!(c.get("clones"), Some(&6));
        assert_eq!(c.get("statx"), Some(&9));
        assert_eq!(c.get("newfstatat"), Some(&0));
        assert_eq!(c.len(), 6);

        let dhat = r#"{"dhatFileVersion":2,"mode":"rust-heap","pps":[{"tb":1000,"tbk":3,"gb":600},
            {"tb":24,"tbk":1,"gb":24}]}"#;
        assert_eq!(
            parse_dhat(dhat).unwrap(),
            DhatSummary {
                total_bytes: 1024,
                total_blocks: 4,
                peak_bytes: 624
            }
        );
        // every field must be there: a heap peak without `gb` would read as 0
        assert!(parse_dhat(r#"{"pps":[{"tbk":1,"gb":1}]}"#).is_err());
        assert!(parse_dhat(r#"{"pps":[{"tb":1,"tbk":1}]}"#).is_err());
    }

    #[test]
    fn shell_quoting_round_trips_hyperfine_words() {
        assert_eq!(shell_quote("/usr/bin/snp"), "/usr/bin/snp");
        assert_eq!(shell_quote("--all-files"), "--all-files");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(
            command_line(&["rm".into(), "-rf".into(), "/tmp/x y".into()]),
            "rm -rf '/tmp/x y'"
        );
    }

    #[test]
    fn options_parse_values_switches_and_help() {
        let args: Vec<String> = [
            "--scenarios",
            "B1,B3",
            "--tools=snp",
            "--syscalls",
            "--scenarios",
            "B6",
        ]
        .map(str::to_owned)
        .to_vec();
        let o = Opts::parse(&args, &["scenarios", "tools"], &["syscalls"])
            .unwrap()
            .unwrap();
        assert_eq!(o.list("scenarios"), ["B1", "B3", "B6"]);
        assert_eq!(o.one("tools"), Some("snp"));
        assert!(o.has("syscalls"));
        assert!(
            Opts::parse(&["--help".to_owned()], &[], &[])
                .unwrap()
                .is_none()
        );
        assert!(Opts::parse(&["--bogus".to_owned()], &[], &[]).is_err());
        assert!(Opts::parse(&["--tools".to_owned()], &["tools"], &[]).is_err());
        assert!(Opts::parse(&["--syscalls=1".to_owned()], &[], &["syscalls"]).is_err());
        assert!(selected_scenarios(&o).is_ok());
        let bad = Opts::parse(&["--scenarios=B99".to_owned()], &["scenarios"], &[])
            .unwrap()
            .unwrap();
        assert!(selected_scenarios(&bad).is_err());
    }

    #[test]
    fn scenario_table_is_consistent() {
        let ids: BTreeSet<&str> = SCENARIOS.iter().map(|s| s.id).collect();
        assert_eq!(ids.len(), SCENARIOS.len(), "duplicate scenario id");
        for id in [
            "B1", "B2", "B3", "B4", "B5", "B6", "B7", "B8", "B9", "B10", "B11", "B12u",
        ] {
            assert!(ids.contains(id), "{id}");
        }
        // every budget names a known scenario and every micro threshold a criterion id shape
        let b = budgets();
        for bud in &b.limits {
            assert!(ids.contains(bud.scenario.as_str()), "{bud:?}");
            if let Some(s) = &bud.vs_scenario {
                assert!(ids.contains(s.as_str()), "{bud:?}");
            }
        }
        for t in &b.micro {
            assert!(t.id.contains('/'), "{t:?}");
        }
    }

    #[test]
    fn committed_budgets_hold_the_adr_043_hard_gates() {
        let b = budgets();
        assert!((b.regression_pct - 5.0).abs() < f64::EPSILON);
        assert!((b.regression_min_ms - 0.1).abs() < f64::EPSILON);
        assert_eq!(b.never_slower_than, ["pre-commit", "prek"]);
        let hard = |s: &str, metric: Metric| {
            b.limits
                .iter()
                .find(|x| x.os == "linux" && x.scenario == s && x.metric == metric && x.hard)
                .map(|x| x.max)
        };
        assert_eq!(hard("B1", Metric::MedianMs), Some(3.0));
        assert_eq!(hard("B2", Metric::MedianMs), Some(10.0));
        assert_eq!(hard("B3", Metric::MedianMs), Some(25.0));
        // the memory budgets are MB (10^6 bytes) in ARCHITECTURE §12.1: their MiB value may not exceed it
        let mib_of_mb = |mb: f64| mb * 1e6 / (1024.0 * 1024.0);
        for (s, metric, mb) in [
            ("B4", Metric::RssMib, 30.0),
            ("B6", Metric::RssMib, 25.0),
            ("B12u", Metric::RssMib, 30.0),
            ("B3", Metric::AllocMib, 2.0),
        ] {
            let max = hard(s, metric).unwrap();
            assert!(
                max <= mib_of_mb(mb) && max > mib_of_mb(mb) - 0.01,
                "{s} {metric:?}: {max} MiB is not {mb} MB"
            );
        }
        // B8 (ADR-100): snp's own heap peak, at most B3's + 2 MB, hard (nightly); the tree RSS, which measures
        // git's diff of the 10 MB file, is no longer budgeted
        let b8 = b
            .limits
            .iter()
            .find(|x| x.scenario == "B8" && x.metric == Metric::HeapPeakMib)
            .unwrap();
        assert!(b8.hard && b8.plus <= mib_of_mb(2.0) && b8.plus > mib_of_mb(2.0) - 0.01);
        assert_eq!((b8.max, b8.vs_scenario.as_deref()), (1.0, Some("B3")));
        assert!(
            !b.limits
                .iter()
                .any(|x| x.scenario == "B8" && x.metric == Metric::RssMib)
        );
        assert_eq!(hard("B1", Metric::Syscalls), Some(64.0));
        // ARCHITECTURE §12.3: committed, hard syscall ceilings on B1 and B3 (B3 measured + ~10 %, ADR-083)
        assert_eq!(hard("B3", Metric::Syscalls), Some(4230.0));
        let micro = |id: &str| b.micro.iter().find(|t| t.id == id).cloned();
        assert_eq!(
            micro("argparse/hook_impl_cli_compat").unwrap().max_ns,
            Some(5000.0)
        );
        assert_eq!(
            micro("pyre_tier1/typical_12x100k").unwrap().min_per_sec,
            Some(1_000_000.0)
        );
        assert_eq!(
            micro("classify/50k_files_10_filters").unwrap().max_ns,
            Some(40_000_000.0)
        );
    }

    #[test]
    fn budgets_reject_malformed_entries() {
        let base = "[gate]\nregression_pct = 5\nregression_min_ms = 0\nnever_slower_than = []\n";
        assert!(Budgets::parse(base).is_ok());
        assert!(Budgets::parse("").is_err());
        let bad_os = format!(
            "{base}[[budget]]\nos = \"beos\"\nscenario = \"B1\"\nmetric = \"median_ms\"\nmax = 1\n"
        );
        assert!(Budgets::parse(&bad_os).is_err());
        let bad_metric = format!(
            "{base}[[budget]]\nos = \"linux\"\nscenario = \"B1\"\nmetric = \"x\"\nmax = 1\n"
        );
        assert!(Budgets::parse(&bad_metric).is_err());
        let both = format!(
            "{base}[[budget]]\nos = \"linux\"\nscenario = \"B1\"\nmetric = \"median_ms\"\nmax = 1\n\
             vs_tool = \"prek\"\nvs_scenario = \"B2\"\n"
        );
        assert!(Budgets::parse(&both).is_err());
        let micro_both = format!("{base}[[micro]]\nid = \"a/b\"\nmax_ns = 1\nmin_per_sec = 1\n");
        assert!(Budgets::parse(&micro_both).is_err());
        let musl = format!("{base}[musl]\nadopted = \"jemalloc\"\n");
        assert!(Budgets::parse(&musl).is_err());
    }

    #[test]
    fn records_round_trip_through_json() {
        let mut snp = m(1.5, (1.4, 1.6));
        snp.rss_mib = Some(7.5);
        snp.syscalls = Some(60);
        let mut r = record(
            "gnu-system",
            &[
                ("B1", "snp", Outcome::Ok(snp)),
                ("B1", "pre-commit", Outcome::Ok(m(170.0, (168.0, 172.0)))),
                ("B3", "prek", Outcome::Failed("boom".into())),
            ],
        );
        r.extras.insert("syscalls".into());
        let back = Record::from_json(&r.to_json()).unwrap();
        assert_eq!(back, r);
        assert_eq!(back.os(), "linux");
        let mut v = r.to_json();
        v["schema"] = json!(99);
        assert!(Record::from_json(&v).is_err());
    }

    #[test]
    fn gate_fails_a_hard_budget_and_a_slower_p50() {
        let b = budgets();
        let cur = record(
            "gnu-system",
            &[
                ("B1", "snp", Outcome::Ok(m(3.5, (3.4, 3.6)))),
                ("B1", "pre-commit", Outcome::Ok(m(170.0, (169.0, 171.0)))),
                ("B3", "snp", Outcome::Ok(m(20.0, (19.0, 21.0)))),
                ("B3", "prek", Outcome::Ok(m(18.0, (17.0, 19.0)))),
            ],
        );
        let checks = evaluate(&cur, None, &b, &none(), &none(), &[]);
        assert_eq!(
            verdict_of(&checks, "B1", "p50 <= 3").unwrap().verdict,
            Verdict::Fail
        );
        assert_eq!(
            verdict_of(&checks, "B1", "p50 <= pre-commit")
                .unwrap()
                .verdict,
            Verdict::Pass
        );
        assert_eq!(
            verdict_of(&checks, "B3", "p50 <= prek").unwrap().verdict,
            Verdict::Fail
        );
        assert_eq!(
            verdict_of(&checks, "B3", "p50 <= 25").unwrap().verdict,
            Verdict::Pass
        );
        // B2 was not measured: its hard budget fails rather than passing silently
        assert_eq!(
            verdict_of(&checks, "B2", "p50 <= 10").unwrap().verdict,
            Verdict::Fail
        );
    }

    #[test]
    fn gate_reports_unmeasured_scenarios_and_metrics() {
        let b = budgets();
        let cur = record(
            "gnu-system",
            &[
                ("B1", "snp", Outcome::Ok(m(1.0, (0.9, 1.1)))),
                ("B1", "pre-commit", Outcome::Ok(m(170.0, (169.0, 171.0)))),
                ("B1", "prek", Outcome::Ok(m(4.0, (3.9, 4.1)))),
            ],
        );
        // every hard Linux budget of an unmeasured scenario fails: B2/B3 p50, B4/B6/B12u RSS
        let checks = evaluate(&cur, None, &b, &none(), &none(), &[]);
        for (s, what) in [
            ("B2", "p50"),
            ("B3", "p50"),
            ("B4", "RSS"),
            ("B6", "RSS"),
            ("B12u", "RSS"),
        ] {
            let c = checks
                .iter()
                .find(|c| c.scenario == s && c.what.starts_with(what) && c.what.ends_with("(hard)"))
                .unwrap_or_else(|| panic!("no {s} {what} check: {checks:?}"));
            assert_eq!(c.verdict, Verdict::Fail, "{c:?}");
        }
        // a soft budget of an unmeasured scenario warns
        assert_eq!(
            verdict_of(&checks, "B7", "p50 <= 1000").unwrap().verdict,
            Verdict::Warn
        );
        // the scenarios a workflow knowingly skips only warn
        let pending: BTreeSet<String> = [
            "B2", "B3", "B4", "B5", "B6", "B7", "B8", "B9", "B10", "B11", "B12u",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let checks = evaluate(&cur, None, &b, &pending, &none(), &[]);
        assert!(
            checks.iter().all(|c| c.verdict != Verdict::Fail),
            "{checks:?}"
        );
        assert_eq!(
            verdict_of(&checks, "B3", "p50 <= 25").unwrap().verdict,
            Verdict::Warn
        );
        // B1 syscalls / B3 allocations: skipped when not requested, a failure when requested and missing
        assert!(verdict_of(&checks, "B1", "syscalls").is_none());
        assert!(verdict_of(&checks, "B3", "allocated").is_none());
        let mut dhat = cur.clone();
        dhat.extras.insert("alloc_mib".into());
        assert_eq!(
            verdict_of(
                &evaluate(&dhat, None, &b, &none(), &none(), &[]),
                "B3",
                "allocated"
            )
            .unwrap()
            .verdict,
            Verdict::Fail
        );
        assert_eq!(
            verdict_of(
                &evaluate(&dhat, None, &b, &pending, &none(), &[]),
                "B3",
                "allocated"
            )
            .unwrap()
            .verdict,
            Verdict::Warn
        );
        let mut nightly = cur;
        nightly.extras.insert("syscalls".into());
        let checks = evaluate(&nightly, None, &b, &pending, &none(), &[]);
        assert_eq!(
            verdict_of(&checks, "B1", "syscalls").unwrap().verdict,
            Verdict::Fail
        );
        if let Some(Outcome::Ok(snp)) = nightly
            .scenarios
            .get_mut("B1")
            .and_then(|t| t.get_mut("snp"))
        {
            snp.syscalls = Some(62);
        }
        let checks = evaluate(&nightly, None, &b, &pending, &none(), &[]);
        assert_eq!(
            verdict_of(&checks, "B1", "syscalls").unwrap().verdict,
            Verdict::Pass
        );
        // no baseline: the skipped regression check is visible
        assert_eq!(
            verdict_of(&checks, "*", "no regression").unwrap().verdict,
            Verdict::Warn
        );
    }

    #[test]
    fn never_slower_rule_fails_on_a_failed_or_missing_reference() {
        let b = budgets();
        let cur = record(
            "gnu-system",
            &[
                ("B1", "snp", Outcome::Ok(m(1.0, (0.9, 1.1)))),
                (
                    "B1",
                    "pre-commit",
                    Outcome::Failed("setup: oracle broken\nline 2".into()),
                ),
            ],
        );
        let checks = evaluate(&cur, None, &b, &none(), &none(), &[]);
        let pc = verdict_of(&checks, "B1", "p50 <= pre-commit").unwrap();
        assert_eq!(pc.verdict, Verdict::Fail);
        assert_eq!(
            pc.detail,
            "reference pre-commit failed: setup: oracle broken line 2"
        );
        let prek = verdict_of(&checks, "B1", "p50 <= prek").unwrap();
        assert_eq!(prek.verdict, Verdict::Fail);
        assert_eq!(prek.detail, "reference prek not measured");
        let text = comment(&cur, None, &checks);
        assert!(text.contains("reference pre-commit failed"), "{text}");
        // an exemption is explicit (and visible in the workflow), per scenario
        let exempt: BTreeSet<String> = ["B1".to_owned()].into();
        let checks = evaluate(&cur, None, &b, &none(), &exempt, &[]);
        let prek = verdict_of(&checks, "B1", "p50 <= prek").unwrap();
        assert_eq!(prek.verdict, Verdict::Warn);
        assert!(prek.detail.ends_with("(--allow-missing-reference)"));
        // the non-adopted musl allocator is only reported
        let musl = record(
            "musl-mimalloc",
            &[("B1", "snp", Outcome::Ok(m(1.0, (0.9, 1.1))))],
        );
        let checks = evaluate(&musl, None, &b, &none(), &none(), &[]);
        assert_eq!(
            verdict_of(&checks, "B1", "p50 <= prek").unwrap().verdict,
            Verdict::Warn
        );
    }

    #[test]
    fn never_slower_than_names_real_tools() {
        let ok = "[gate]\nregression_pct = 5\nregression_min_ms = 0\nnever_slower_than = [\"pre-commit\", \"prek\"]\n";
        assert_eq!(
            Budgets::parse(ok).unwrap().never_slower_than,
            ["pre-commit", "prek"]
        );
        for bad in ["[\"prekk\"]", "[\"snp\"]", "[1]"] {
            let text = format!(
                "[gate]\nregression_pct = 5\nregression_min_ms = 0\nnever_slower_than = {bad}\n"
            );
            assert!(Budgets::parse(&text).is_err(), "{bad}");
        }
    }

    #[test]
    fn drift_against_the_history_median_catches_a_slow_creep() {
        let b = budgets();
        // every record 4.9 % slower than the one before: each step passes the regression rule
        let mut history = Vec::new();
        let mut t = 10.0_f64;
        for _ in 0..8 {
            history.push(record(
                "gnu-system",
                &[("B1", "snp", Outcome::Ok(m(t, (t * 0.99, t * 1.01))))],
            ));
            t *= 1.049;
        }
        let cur = record(
            "gnu-system",
            &[
                ("B1", "snp", Outcome::Ok(m(t, (t * 0.99, t * 1.01)))),
                ("B1", "pre-commit", Outcome::Ok(m(100.0, (99.0, 101.0)))),
                ("B1", "prek", Outcome::Ok(m(100.0, (99.0, 101.0)))),
            ],
        );
        let checks = evaluate(&cur, history.last(), &b, &none(), &none(), &history);
        assert_eq!(
            verdict_of(&checks, "B1", "no regression").unwrap().verdict,
            Verdict::Pass
        );
        let drift = verdict_of(&checks, "B1", "no drift").unwrap();
        assert_eq!(drift.verdict, Verdict::Fail, "{}", drift.detail);
        assert!(
            drift.detail.contains("median of the last 8 records"),
            "{}",
            drift.detail
        );
        // a stable history passes; fewer than DRIFT_MIN measurements skip the check
        let flat: Vec<Record> = (0..5)
            .map(|_| {
                record(
                    "gnu-system",
                    &[("B1", "snp", Outcome::Ok(m(t, (t * 0.99, t * 1.01))))],
                )
            })
            .collect();
        let checks = evaluate(&cur, flat.last(), &b, &none(), &none(), &flat);
        assert_eq!(
            verdict_of(&checks, "B1", "no drift").unwrap().verdict,
            Verdict::Pass
        );
        let checks = evaluate(&cur, flat.last(), &b, &none(), &none(), &flat[..2]);
        assert!(verdict_of(&checks, "B1", "no drift").is_none());
    }

    #[test]
    fn the_regression_floor_absorbs_same_runner_noise() {
        let b = budgets();
        let base = m(0.769, (0.739, 0.781));
        // the false alarm of run 41492075808: one runner, +5.8 %, disjoint intervals, but only 0.045 ms
        let noise = m(0.814, (0.792, 0.825));
        assert_eq!(regression(&noise, &base, &b, "baseline").0, Verdict::Pass);
        // past the floor the rule is the old one
        let real = m(0.95, (0.93, 0.97));
        assert_eq!(regression(&real, &base, &b, "baseline").0, Verdict::Fail);
        // from 2 ms up the percentage decides: +4 % passes, +6 % fails
        let slow = m(10.0, (9.9, 10.1));
        let within = m(10.4, (10.3, 10.5));
        assert_eq!(regression(&within, &slow, &b, "baseline").0, Verdict::Pass);
        let beyond = m(10.6, (10.5, 10.7));
        assert_eq!(regression(&beyond, &slow, &b, "baseline").0, Verdict::Fail);
    }

    #[test]
    fn advisory_history_softens_only_the_comparisons_with_other_machines() {
        let b = budgets();
        let snp = |t: f64| ("B1", "snp", Outcome::Ok(m(t, (t * 0.99, t * 1.01))));
        let history: Vec<Record> = (0..4).map(|_| record("gnu-system", &[snp(0.76)])).collect();
        // over the hard 3 ms budget and slower than prek, both measured within this run
        let cur = record(
            "gnu-system",
            &[
                snp(4.0),
                ("B1", "pre-commit", Outcome::Ok(m(100.0, (99.0, 101.0)))),
                ("B1", "prek", Outcome::Ok(m(2.0, (1.9, 2.1)))),
            ],
        );
        let mut checks = evaluate(&cur, history.last(), &b, &none(), &none(), &history);
        for what in ["no regression", "no drift"] {
            let c = verdict_of(&checks, "B1", what).unwrap();
            assert_eq!(c.verdict, Verdict::Fail, "{what}: {}", c.detail);
        }
        advisory_history(&mut checks);
        for what in ["no regression", "no drift"] {
            let c = verdict_of(&checks, "B1", what).unwrap();
            assert_eq!(c.verdict, Verdict::Warn, "{what}: {}", c.detail);
            assert!(c.detail.ends_with("(--advisory-history)"), "{}", c.detail);
        }
        for what in ["p50 <= prek", "p50 <= 3"] {
            let c = verdict_of(&checks, "B1", what).unwrap();
            assert_eq!(c.verdict, Verdict::Fail, "{what}: {}", c.detail);
        }
    }

    #[test]
    fn known_misses_warn_with_their_reason_and_nothing_else_changes() {
        let check = |scenario: &str, what: &str, verdict| Check {
            scenario: scenario.into(),
            what: what.into(),
            verdict,
            detail: String::from("d"),
        };
        let misses = [KnownMiss {
            scenario: "B8".into(),
            check: "p50 <= prek".into(),
            reason: "the re-check".into(),
        }];
        let mut checks = [
            check("B8", "p50 <= prek", Verdict::Fail),
            check("B8", "p50 <= pre-commit", Verdict::Fail),
            check("B3", "p50 <= prek", Verdict::Fail),
            check("B8", "no regression > 5%", Verdict::Fail),
        ];
        apply_known_misses(&mut checks, &misses);
        assert_eq!(checks[0].verdict, Verdict::Warn);
        assert_eq!(checks[0].detail, "d (known miss: the re-check)");
        // the other check, the other scenario, and getting worse all still fail
        assert!(checks[1..].iter().all(|c| c.verdict == Verdict::Fail));
        let mut fixed = [check("B8", "p50 <= prek", Verdict::Pass)];
        apply_known_misses(&mut fixed, &misses);
        assert_eq!(fixed[0].verdict, Verdict::Pass);
        assert!(
            fixed[0].detail.contains("remove the entry"),
            "{}",
            fixed[0].detail
        );
    }

    #[test]
    fn known_misses_must_name_a_scenario_and_a_reason() {
        let base = "[gate]\nregression_pct = 5\nregression_min_ms = 0\nnever_slower_than = []\n";
        let with = |entry: &str| Budgets::parse(&format!("{base}[[known_miss]]\n{entry}"));
        let ok = with("scenario = \"B2\"\ncheck = \"p50\"\nreason = \"r\"\n").unwrap();
        assert_eq!(ok.known_misses.len(), 1);
        assert!(with("scenario = \"B99\"\ncheck = \"p50\"\nreason = \"r\"\n").is_err());
        assert!(with("scenario = \"B2\"\ncheck = \"p50\"\nreason = \"\"\n").is_err());
        assert!(with("scenario = \"B2\"\nreason = \"r\"\n").is_err());
    }

    #[test]
    fn hyperfine_version_must_match_the_pin() {
        assert!(is_hyperfine_version("hyperfine 1.19.0\n"));
        assert!(!is_hyperfine_version("hyperfine 1.18.0\n"));
        assert!(!is_hyperfine_version("hyperfine 1.19.01\n"));
        assert!(!is_hyperfine_version(""));
    }

    #[test]
    fn gate_regression_needs_5_percent_and_disjoint_intervals() {
        let b = budgets();
        let base = record(
            "gnu-system",
            &[("B3", "snp", Outcome::Ok(m(10.0, (9.8, 10.2))))],
        );
        let check = |median: f64, ci: (f64, f64)| {
            let cur = record("gnu-system", &[("B3", "snp", Outcome::Ok(m(median, ci)))]);
            verdict_of(
                &evaluate(&cur, Some(&base), &b, &none(), &none(), &[]),
                "B3",
                "no regression",
            )
            .unwrap()
            .verdict
        };
        assert_eq!(check(10.4, (10.3, 10.5)), Verdict::Pass); // +4 %
        assert_eq!(check(10.6, (10.1, 11.0)), Verdict::Warn); // +6 %, intervals overlap
        assert_eq!(check(10.6, (10.3, 10.9)), Verdict::Fail); // +6 %, disjoint
        assert_eq!(check(9.0, (8.9, 9.1)), Verdict::Pass);
    }

    #[test]
    fn gate_relative_budgets_and_failed_snp_runs() {
        let b = budgets();
        let mut b3 = m(20.0, (19.0, 21.0));
        b3.rss_mib = Some(8.0);
        let mut b8 = m(40.0, (39.0, 41.0));
        b8.rss_mib = Some(10.5);
        let cur = record(
            "gnu-system",
            &[
                ("B3", "snp", Outcome::Ok(b3)),
                ("B8", "snp", Outcome::Ok(b8)),
                ("B9", "snp", Outcome::Ok(m(22.5, (22.0, 23.0)))),
                ("B4", "snp", Outcome::Ok(m(100.0, (99.0, 101.0)))),
                ("B6", "snp", Outcome::Failed("exit 3".into())),
            ],
        );
        let checks = evaluate(&cur, None, &b, &none(), &none(), &[]);
        // B8's tree RSS (git's) is information only (ADR-100); its heap peak is a nightly metric
        assert!(verdict_of(&checks, "B8", "RSS").is_none());
        assert!(verdict_of(&checks, "B8", "heap peak").is_none());
        // B9 ≤ B3 + 3 ms = 23 ms
        assert_eq!(
            verdict_of(&checks, "B9", "p50 <= 1x B3").unwrap().verdict,
            Verdict::Pass
        );
        // B4 vs pre-commit: not measured → warn, never a silent pass
        assert_eq!(
            verdict_of(&checks, "B4", "p50 <= 0.2x pre-commit")
                .unwrap()
                .verdict,
            Verdict::Warn
        );
        assert_eq!(
            verdict_of(&checks, "B6", "snp ran").unwrap().verdict,
            Verdict::Fail
        );
    }

    #[test]
    fn non_adopted_musl_allocator_is_only_reported() {
        let b = budgets();
        let rows = [
            ("B3", "snp", Outcome::Ok(m(20.0, (19.0, 21.0)))),
            ("B3", "prek", Outcome::Ok(m(18.0, (17.0, 19.0)))),
        ];
        let adopted = format!("musl-{}", b.musl_adopted);
        let other = if b.musl_adopted == "system" {
            "musl-mimalloc"
        } else {
            "musl-system"
        };
        let v = |variant: &str| {
            verdict_of(
                &evaluate(&record(variant, &rows), None, &b, &none(), &none(), &[]),
                "B3",
                "p50 <= prek",
            )
            .unwrap()
            .verdict
        };
        assert_eq!(v(&adopted), Verdict::Fail);
        assert_eq!(v(other), Verdict::Warn);
    }

    #[test]
    fn comment_lists_scenarios_and_failures() {
        let b = budgets();
        let cur = record(
            "gnu-system",
            &[
                ("B1", "snp", Outcome::Ok(m(3.5, (3.4, 3.6)))),
                ("B1", "prek", Outcome::Ok(m(4.0, (3.9, 4.1)))),
            ],
        );
        let base = record(
            "gnu-system",
            &[("B1", "snp", Outcome::Ok(m(1.0, (0.9, 1.1))))],
        );
        let checks = evaluate(&cur, Some(&base), &b, &none(), &none(), &[]);
        let text = comment(&cur, Some(&base), &checks);
        assert!(text.contains("**Gate: FAIL**"), "{text}");
        assert!(
            text.contains("| B1 --version | 3.50 (3.40–3.60) | 1.00 | +250.0% | — | 4.00 | — |"),
            "{text}"
        );
        assert!(text.contains("**fail**"), "{text}");
    }

    #[test]
    fn micro_thresholds_fail_missing_and_slow_benchmarks() {
        let b = budgets();
        let mut results = BTreeMap::new();
        results.insert(
            "argparse/hook_impl_cli_compat".to_owned(),
            CritResult {
                median_ns: 1500.0,
                elements: None,
            },
        );
        results.insert(
            "pyre_tier1/typical_12x100k".to_owned(),
            CritResult {
                median_ns: 2e9,
                elements: Some(1_200_000),
            },
        );
        let checks = evaluate_micro(&b.micro, &results, true);
        let v = |id: &str| checks.iter().find(|c| c.scenario == id).unwrap().verdict;
        assert_eq!(v("argparse/hook_impl_cli_compat"), Verdict::Pass);
        // 1.2 M searches in 2 s = 600 k/s < 1 M/s
        assert_eq!(v("pyre_tier1/typical_12x100k"), Verdict::Fail);
        assert_eq!(v("classify/50k_files_10_filters"), Verdict::Fail);
        // Off the reference machine a `reference_only` breach warns; the others still fail.
        let checks = evaluate_micro(&b.micro, &results, false);
        let v = |id: &str| checks.iter().find(|c| c.scenario == id).unwrap().verdict;
        assert_eq!(v("pyre_tier1/typical_12x100k"), Verdict::Fail);
        let classify = checks
            .iter()
            .find(|c| c.scenario == "classify/50k_files_10_filters")
            .unwrap();
        if b.micro
            .iter()
            .any(|t| t.id == classify.scenario && t.reference_only)
        {
            assert!(matches!(classify.verdict, Verdict::Warn | Verdict::Fail));
        }
    }

    #[test]
    fn criterion_directories_are_read() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target/tmp/xtask-bench-criterion")
            .join(std::process::id().to_string());
        let new = dir
            .join("argparse")
            .join("hook_impl_cli_compat")
            .join("new");
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(
            new.join("benchmark.json"),
            r#"{"group_id":"argparse","function_id":"hook_impl_cli_compat","value_str":null,
               "throughput":{"Elements":7},"full_id":"argparse/hook_impl_cli_compat",
               "directory_name":"argparse/hook_impl_cli_compat","title":"argparse/hook_impl_cli_compat"}"#,
        )
        .unwrap();
        std::fs::write(
            new.join("estimates.json"),
            r#"{"mean":{"point_estimate":1300.0},"median":{"point_estimate":1234.5}}"#,
        )
        .unwrap();
        // a `base/` sibling is ignored
        std::fs::create_dir_all(
            dir.join("argparse")
                .join("hook_impl_cli_compat")
                .join("base"),
        )
        .unwrap();
        let got = read_criterion(&dir, None).unwrap();
        // a result older than the `--since` time of this run is stale and left out
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
        let stale = read_criterion(&dir, Some(later)).unwrap();
        let fresh = read_criterion(&dir, Some(std::time::UNIX_EPOCH)).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(stale.is_empty(), "{stale:?}");
        assert_eq!(fresh, got);
        assert_eq!(
            got.get("argparse/hook_impl_cli_compat"),
            Some(&CritResult {
                median_ns: 1234.5,
                elements: Some(7)
            })
        );
        assert_eq!(got.len(), 1);
        assert!(
            read_criterion(Path::new("/nonexistent/criterion"), None)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn history_returns_the_last_record() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target/tmp/xtask-bench-history")
            .join(std::process::id().to_string());
        std::fs::create_dir_all(&dir).unwrap();
        let file = history_file(&dir, "linux-x86_64", "gnu-system");
        assert_eq!(history_records(&file, 10).unwrap().len(), 0);
        let a = record(
            "gnu-system",
            &[("B1", "snp", Outcome::Ok(m(1.0, (0.9, 1.1))))],
        );
        let mut b = a.clone();
        b.label = "second".into();
        let text = format!("{}\n{}\n\n", a.to_json(), b.to_json());
        std::fs::write(&file, text).unwrap();
        let got = history_records(&file, 10).unwrap().pop().unwrap();
        let window = history_records(&file, 10).unwrap();
        let last_one = history_records(&file, 1).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(got.label, "second");
        assert_eq!(window.len(), 2);
        assert_eq!(window[0].label, "t");
        assert_eq!(last_one.len(), 1);
        assert_eq!(last_one[0].label, "second");
    }

    /// A `Work` rooted in a scratch directory.
    fn scratch_work(name: &str) -> (PathBuf, Work) {
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target/tmp")
            .join(format!("xtask-bench-{name}-{}", std::process::id()));
        let _stale = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let base = real_path(&base).unwrap();
        let target_dir = base.join("target");
        let work = Work {
            root: base.clone(),
            dir: target_dir.join("bench"),
            target_dir,
            path_var: UNIX_PATH.into(),
        };
        std::fs::create_dir_all(&work.dir).unwrap();
        (base, work)
    }

    #[test]
    fn the_hermetic_environment_is_offline_and_pinned_below_target_bench() {
        let (base, work) = scratch_work("env");
        let env: BTreeMap<String, String> = work
            .env()
            .into_iter()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.to_string_lossy().into_owned(),
                )
            })
            .collect();
        std::fs::remove_dir_all(&base).unwrap();
        let wheels = work.wheelhouse().display().to_string();
        for (k, v) in [
            ("PIP_NO_INDEX", "1"),
            ("UV_NO_INDEX", "1"),
            ("UV_OFFLINE", "1"),
            ("UV_PYTHON_DOWNLOADS", "never"),
            ("VIRTUALENV_NO_PERIODIC_UPDATE", "1"),
            ("PIP_FIND_LINKS", wheels.as_str()),
            ("UV_FIND_LINKS", wheels.as_str()),
            ("GIT_CEILING_DIRECTORIES", work.dir.to_str().unwrap()),
        ] {
            assert_eq!(env.get(k).map(String::as_str), Some(v), "{k}");
        }
        if !cfg!(windows) {
            let path = &env["PATH"];
            assert!(
                path.starts_with(&format!("{}:", work.path_dir().display())),
                "{path}"
            );
            assert!(path.ends_with(UNIX_PATH), "{path}");
        }
    }

    /// An interrupted setup leaves a fixture whose `.git` is not a repository: git must not walk up to the enclosing
    /// repository (B9 would commit there), and the measurement must refuse the directory.
    #[test]
    fn a_fixture_never_resolves_to_an_enclosing_repository() {
        let (base, work) = scratch_work("ceiling");
        // the enclosing repository: the scratch root (in the real case, the developer's snp checkout)
        git(
            &work,
            &base,
            &["-c", "init.defaultBranch=main", "init", "-q"],
        )
        .unwrap();
        let broken = work.fixture(Fixture::Hooked, Tool::Snp);
        std::fs::create_dir_all(broken.join(".git").join("objects")).unwrap();
        let err = ensure_own_repo(&work, &broken).unwrap_err();
        assert!(
            format!("{err:#}").contains("not a git repository"),
            "{err:#}"
        );
        // a directory below a fixture is not the fixture's top level either
        let good = work.fixture(Fixture::Noop10, Tool::Snp);
        gen_noop10(&work, &good).unwrap();
        ensure_own_repo(&work, &good).unwrap();
        let sub = good.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let err = ensure_own_repo(&work, &sub).unwrap_err();
        assert!(format!("{err:#}").contains("not its own"), "{err:#}");
        // and the enclosing repository received nothing
        let log = git(&work, &base, &["rev-list", "--all", "--count"]).unwrap();
        std::fs::remove_dir_all(&base).unwrap();
        assert_eq!(log.trim(), "0");
    }

    #[test]
    fn b9_alone_generates_the_noop10_fixture_it_copies() {
        let b9 = scenario("B9").unwrap();
        assert_eq!(b9.fixture, Fixture::Hooked);
        assert_eq!(
            fixtures_of(&[b9]),
            [Fixture::Noop10, Fixture::Hooked].into()
        );
        let b4 = scenario("B4").unwrap();
        assert_eq!(fixtures_of(&[b4]), [Fixture::Files50k].into());
    }

    #[test]
    fn corpora_are_deterministic_and_unique() {
        let a = corpus(2000, 1, false);
        assert_eq!(a, corpus(2000, 1, false));
        assert_eq!(a.iter().collect::<BTreeSet<_>>().len(), 2000);
        let u = corpus(500, 2, true);
        assert!(u.iter().all(|p| !p.is_ascii()));
        assert!(noop_config(10).contains("noop-10"));
        assert_eq!(big_text(1).len() >> 20, 10);
        assert_ne!(big_text(1), big_text(2));
    }

    #[test]
    fn variants_name_the_target_and_allocator() {
        let o = |args: &[&str]| {
            let v: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
            Opts::parse(&v, &["target", "allocator"], &[])
                .unwrap()
                .unwrap()
        };
        let musl = Build::from_opts(&o(&[
            "--target",
            "x86_64-unknown-linux-musl",
            "--allocator",
            "mimalloc",
        ]))
        .unwrap();
        assert_eq!(musl.variant(), "musl-mimalloc");
        assert_eq!(
            Build::from_opts(&o(&["--target", "x86_64-unknown-linux-musl"]))
                .unwrap()
                .variant(),
            "musl-system"
        );
        assert!(Build::from_opts(&o(&["--allocator", "mimalloc"])).is_err());
        assert!(Build::from_opts(&o(&["--allocator", "jemalloc"])).is_err());
    }

    /// A measure with interleaved block medians.
    fn rounds(medians: &[f64]) -> Measure {
        let mut x = Measure::from_times(medians).unwrap();
        x.rounds_ms = medians.to_vec();
        x
    }

    #[test]
    fn paired_rounds_split_the_runs_into_abba_blocks() {
        assert_eq!(paired_rounds(50), (10, 5));
        assert_eq!(paired_rounds(20), (10, 2));
        assert_eq!(paired_rounds(5), (5, 1));
        assert_eq!(paired_rounds(1), (1, 1));
        assert_eq!(paired_rounds(0), (1, 1));
        let order: Vec<bool> = (0..4).map(base_first).collect();
        assert_eq!(order, [true, false, true, false]);
        // one warm-up per binary per scenario: run_counts keeps the scenario's, the rounds take the runs
        let b4 = scenario("B4").unwrap();
        assert_eq!(run_counts(b4, (None, None)), (2, 20));
        assert_eq!(run_counts(b4, (Some(0), Some(4))), (0, 4));
        assert_eq!(
            run_counts(scenario("B1").unwrap(), (None, None)),
            (WARMUP, RUNS)
        );
    }

    #[test]
    fn the_paired_rule_needs_5_percent_the_floor_and_an_interval_above_1() {
        let b = budgets();
        let base = rounds(&[10.0; 10]);
        // +10 % in every round: fails
        let slow = rounds(&[11.0; 10]);
        let (v, detail) = paired_regression(&slow, &base, &b).unwrap();
        assert_eq!(v, Verdict::Fail, "{detail}");
        assert!(
            detail.contains("+10.0%") && detail.contains("10 rounds"),
            "{detail}"
        );
        // +10 % in most rounds, faster in some: the point estimate is over, the interval is not above 1
        let mixed = rounds(&[11.0, 9.0, 11.0, 9.0, 11.0, 11.0, 9.0, 11.0, 11.0, 9.0]);
        assert_eq!(
            paired_regression(&mixed, &base, &b).unwrap().0,
            Verdict::Warn
        );
        // +4 %: passes
        assert_eq!(
            paired_regression(&rounds(&[10.4; 10]), &base, &b)
                .unwrap()
                .0,
            Verdict::Pass
        );
        // +20 % of 0.3 ms is 0.06 ms, below the 0.1 ms floor of ADR-090
        assert_eq!(
            paired_regression(&rounds(&[0.36; 10]), &rounds(&[0.3; 10]), &b)
                .unwrap()
                .0,
            Verdict::Pass
        );
        // faster passes
        assert_eq!(
            paired_regression(&rounds(&[8.0; 10]), &base, &b).unwrap().0,
            Verdict::Pass
        );
        // rounds that cannot be paired
        assert!(paired_regression(&rounds(&[11.0; 9]), &base, &b).is_none());
        assert!(paired_regression(&m(11.0, (10.0, 12.0)), &base, &b).is_none());
        assert!(paired_regression(&rounds(&[0.0; 10]), &base, &b).is_none());
    }

    #[test]
    fn the_paired_base_gates_and_the_unpaired_baseline_only_advises() {
        let b = budgets();
        let mut head = rounds(&[11.0; 10]);
        head.median_ms = 11.0;
        let cur = record(
            "gnu-system",
            &[
                ("B3", "snp", Outcome::Ok(head)),
                ("B3", "snp-base", Outcome::Ok(rounds(&[10.0; 10]))),
                ("B3", "pre-commit", Outcome::Ok(m(240.0, (239.0, 241.0)))),
                ("B3", "prek", Outcome::Ok(m(30.0, (29.0, 31.0)))),
            ],
        );
        // a history baseline 20 % faster from another VM
        let old = record(
            "gnu-system",
            &[("B3", "snp", Outcome::Ok(m(9.0, (8.9, 9.1))))],
        );
        let checks = evaluate(&cur, Some(&old), &b, &none(), &none(), &[]);
        let paired = verdict_of(&checks, "B3", PAIRED_CHECK).unwrap();
        assert_eq!(paired.verdict, Verdict::Fail, "{paired:?}");
        let unpaired = verdict_of(&checks, "B3", "no regression").unwrap();
        assert_eq!(unpaired.verdict, Verdict::Warn);
        assert!(unpaired.detail.contains("advisory: the paired base gates"));
        // --advisory-history does not soften the paired check
        let mut soft = checks.clone();
        advisory_history(&mut soft);
        assert_eq!(
            verdict_of(&soft, "B3", PAIRED_CHECK).unwrap().verdict,
            Verdict::Fail
        );
        // no "no baseline" warning when the record carries its own base
        let alone = evaluate(&cur, None, &b, &none(), &none(), &[]);
        assert!(verdict_of(&alone, "*", "no regression").is_none());

        // a failed base is "not measured" (a warning); the head's own checks still run
        let mut failed = cur;
        failed
            .scenarios
            .get_mut("B3")
            .unwrap()
            .insert("snp-base".into(), Outcome::Failed("orphan pointer".into()));
        let checks = evaluate(&failed, None, &b, &none(), &none(), &[]);
        let c = verdict_of(&checks, "B3", PAIRED_CHECK).unwrap();
        assert_eq!(c.verdict, Verdict::Warn);
        assert!(c.detail.contains("orphan pointer"));
        assert!(verdict_of(&checks, "B3", "p50 <= prek").is_some());
    }

    #[test]
    fn counters_gate_the_head_against_the_base_only() {
        let b = budgets();
        let counted = |clones: u64, statx: u64, newfstatat: u64| {
            let mut x = rounds(&[10.0; 10]);
            x.counts = BTreeMap::from([
                ("clones".to_owned(), clones),
                ("statx".to_owned(), statx),
                ("newfstatat".to_owned(), newfstatat),
                ("execve".to_owned(), 9),
                ("openat".to_owned(), 100),
                ("getdents64".to_owned(), 4),
            ]);
            x
        };
        let rec = |head: Measure, base: Option<Measure>| {
            let mut rows = vec![("B3", "snp", Outcome::Ok(head))];
            if let Some(base) = base {
                rows.push(("B3", "snp-base", Outcome::Ok(base)));
            }
            let mut r = record("gnu-system", &rows);
            r.extras.insert("counts".into());
            r
        };
        let of = |r: &Record, what: &str| {
            verdict_of(&evaluate(r, None, &b, &none(), &none(), &[]), "B3", what).map(|c| c.verdict)
        };
        // +1 thread per spawn (20 → 40 clones) fails; +4 clones on 20 is below the absolute floor of 5
        let base = counted(20, 1000, 5000);
        assert_eq!(
            of(&rec(counted(40, 1000, 5000), Some(base.clone())), "clones"),
            Some(Verdict::Fail)
        );
        assert_eq!(
            of(&rec(counted(24, 1000, 5000), Some(base.clone())), "clones"),
            Some(Verdict::Pass)
        );
        // statx and newfstatat separately: +2 % and +50
        assert_eq!(
            of(&rec(counted(20, 1060, 5000), Some(base.clone())), "statx"),
            Some(Verdict::Fail)
        );
        assert_eq!(
            of(&rec(counted(20, 1040, 5000), Some(base.clone())), "statx"),
            Some(Verdict::Pass)
        );
        assert_eq!(
            of(
                &rec(counted(20, 1000, 5099), Some(base.clone())),
                "newfstatat"
            ),
            Some(Verdict::Pass)
        );
        assert_eq!(
            of(
                &rec(counted(20, 1000, 5101), Some(base.clone())),
                "newfstatat"
            ),
            Some(Verdict::Fail)
        );
        // no base: reported only, never compared with the history
        assert_eq!(of(&rec(counted(99, 9999, 99999), None), "clones"), None);
        // requested but not counted: fails; the base not counted: warns
        assert_eq!(
            of(&rec(rounds(&[10.0; 10]), Some(base)), "counted"),
            Some(Verdict::Fail)
        );
        assert_eq!(
            of(
                &rec(counted(20, 1000, 5000), Some(rounds(&[10.0; 10]))),
                "counters"
            ),
            Some(Verdict::Warn)
        );
    }

    #[test]
    fn a_mixed_history_of_schema_1_and_2_records_is_read() {
        let (base, work) = scratch_work("history");
        let old = r#"{"schema":1,"platform":"linux-x86_64","variant":"gnu-system","label":"current","commit":"a","unix_time":1,"extras":[],"scenarios":{"B1":{"snp":{"median_ms":1.0,"ci95_ms":[0.9,1.1],"runs":50,"rss_mib":null,"syscalls":null,"alloc_mib":null}}}}"#;
        let mut head = rounds(&[1.0, 1.1]);
        head.counts = BTreeMap::from([("clones".to_owned(), 3)]);
        head.exec_diff = vec!["+1× git status".into()];
        head.alloc_blocks = Some(10);
        head.heap_peak_mib = Some(0.5);
        let mut new = record(
            "gnu-system",
            &[
                ("B1", "snp", Outcome::Ok(head.clone())),
                ("B1", "snp-base", Outcome::Ok(rounds(&[1.0, 1.0]))),
            ],
        );
        new.cpu_count = Some(4);
        new.git_version = "git version 2.43.0".into();
        let path = work.dir.join("h.jsonl");
        std::fs::write(
            &path,
            format!(
                "{old}\n{}\n",
                serde_json::to_string(&new.to_json()).unwrap()
            ),
        )
        .unwrap();
        let recs = history_records(&path, 10).unwrap();
        std::fs::remove_dir_all(&base).unwrap();
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].get("B1", "snp").unwrap().median_ms, 1.0);
        assert_eq!(
            recs[0].get("B1", "snp").unwrap().rounds_ms,
            Vec::<f64>::new()
        );
        assert_eq!(recs[0].cpu_count, None);
        assert_eq!(recs[1], new);
        assert_eq!(recs[1].get("B1", "snp").unwrap(), &head);
        // the drift reference reads both
        assert!(drift_reference(&recs, "B1").is_none());
        // a schema-2 record without measured extras writes none of the new keys
        let plain = Record::measure_json(&m(1.0, (0.9, 1.1)));
        for k in [
            "rounds_ms",
            "counts",
            "exec_diff",
            "alloc_blocks",
            "heap_peak_mib",
        ] {
            assert!(plain.get(k).is_none(), "{k}");
        }
    }

    #[test]
    fn patch_names_match_exactly_what_each_tool_writes() {
        let snp = b"patch[0-9]*-[0-9]*";
        let prek = b"[0-9]*-[0-9]*.patch";
        for name in ["patch1791403656-4242", "patch1791403656-4242-1"] {
            assert!(name_matches(snp, name.as_bytes()), "{name}");
        }
        for name in [
            "run-4242-ab.snp-journal",
            "patch-1",
            "patchX-1",
            "patch1-",
            "README",
            "1791403656-4242.patch",
        ] {
            assert!(!name_matches(snp, name.as_bytes()), "{name}");
        }
        assert!(name_matches(prek, b"1791403656-4242.patch"));
        for name in [
            "old.patch",
            "1791403656-4242.patch.tmp",
            "x1-2.patch",
            "1-.patch",
        ] {
            assert!(!name_matches(prek, name.as_bytes()), "{name}");
        }
        assert!(name_matches(b"snp-stash*.journal", b"snp-stash.journal"));
        assert!(name_matches(b"snp-stash*.journal", b"snp-stash-2.journal"));
        assert!(!name_matches(
            b"snp-stash*.journal",
            b"snp-stash.journal.ita"
        ));
        assert!(name_matches(b"run-*.snp-journal", b"run-1-ab.snp-journal"));
    }

    #[test]
    fn b8_cleanup_removes_only_patches_and_refuses_an_interrupted_run() {
        let (base, work) = scratch_work("patches");
        let b8 = scenario("B8").unwrap();
        let fixture = work.fixture(Fixture::Stash10M, Tool::Snp);
        std::fs::create_dir_all(fixture.join(".git")).unwrap();
        let files = |dir: &Path| -> Vec<String> {
            let mut v: Vec<String> = std::fs::read_dir(dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        };
        for tool in [Tool::Snp, Tool::SnpBase, Tool::PreCommit, Tool::Prek] {
            let (dir, _) = patch_location(&work, tool, StoreMode::Warm);
            // a missing directory: a no-op, unless `find` is about to start there
            clean_patches(&work, tool, b8, false).unwrap();
            assert!(clean_patches(&work, tool, b8, true).is_err());
            std::fs::create_dir_all(&dir).unwrap();
            for name in [
                "patch1791403656-11",
                "patch1791403656-11-1",
                "1791403656-11.patch",
                "README",
                "db.db",
            ] {
                std::fs::write(dir.join(name), b"x").unwrap();
            }
            clean_patches(&work, tool, b8, true).unwrap();
            let left = files(&dir);
            match tool {
                Tool::Prek => assert_eq!(
                    left,
                    [
                        "README",
                        "db.db",
                        "patch1791403656-11",
                        "patch1791403656-11-1"
                    ]
                ),
                _ => assert_eq!(left, ["1791403656-11.patch", "README", "db.db"]),
            }
            // never on another scenario
            std::fs::write(dir.join("patch1-2"), b"x").unwrap();
            clean_patches(&work, tool, scenario("B3").unwrap(), true).unwrap();
            assert!(dir.join("patch1-2").exists() || tool == Tool::Prek);
            // the B8 prepare deletes the same names with find (Unix)
            let prepare = prepare_command(&work, tool, b8);
            if cfg!(unix) {
                let p = prepare.unwrap();
                assert!(p.starts_with("find ") && p.ends_with(" -delete"), "{p}");
                assert!(p.contains("-maxdepth 1 -type f -name"), "{p}");
            }
        }
        // an interrupted run: a store journal of snp, or a git-dir pointer of the fixture
        let journal = work
            .store(Tool::Snp, StoreMode::Warm)
            .join("patches")
            .join("run-4242-ab.snp-journal");
        std::fs::write(&journal, b"{}").unwrap();
        let err = clean_patches(&work, Tool::Snp, b8, true).unwrap_err();
        assert!(format!("{err:#}").contains("interrupted"), "{err:#}");
        assert!(
            work.store(Tool::Snp, StoreMode::Warm)
                .join("patches/patch1-2")
                .exists()
        );
        // the base's store is its own
        clean_patches(&work, Tool::SnpBase, b8, true).unwrap();
        std::fs::remove_file(&journal).unwrap();
        std::fs::write(fixture.join(".git").join("snp-stash.journal"), b"{}").unwrap();
        assert!(clean_patches(&work, Tool::PreCommit, b8, true).is_err());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn the_base_has_its_own_stores_and_b9_fixture() {
        let (base, work) = scratch_work("base-stores");
        std::fs::remove_dir_all(&base).unwrap();
        for mode in [StoreMode::Warm, StoreMode::Cold, StoreMode::Gc] {
            assert_ne!(work.store(Tool::Snp, mode), work.store(Tool::SnpBase, mode));
        }
        assert_ne!(
            work.fixture(Fixture::Hooked, Tool::Snp),
            work.fixture(Fixture::Hooked, Tool::SnpBase)
        );
        assert_eq!(
            work.fixture(Fixture::Stash10M, Tool::Snp),
            work.fixture(Fixture::Stash10M, Tool::SnpBase)
        );
        let env = scenario_env(
            &work,
            Tool::SnpBase,
            Path::new("/b/snp"),
            scenario("B12u").unwrap(),
        );
        let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("SNP_BIN"), Some(OsString::from("/b/snp")));
        assert_eq!(get("SNP_JOBS"), Some(OsString::from("32")));
        assert_eq!(
            get("SNP_HOME"),
            Some(work.store(Tool::SnpBase, StoreMode::Warm).into_os_string())
        );
        assert!(get("GIT_CEILING_DIRECTORIES").is_some());
        // `--tools` never selects the base
        assert!(Tool::parse("snp-base").is_err());
        assert_eq!(Tool::SnpBase.store_var(), "SNP_HOME");
    }

    #[test]
    fn exec_argv_lists_are_parsed_normalized_and_diffed() {
        let log = r#"4242 execve("/bench/snp", ["/bench/snp", "run"], 0x7ffd /* 30 vars */) = 0
4243 execve("/usr/bin/git", ["git", "diff", "--quiet", "a b\"c"], 0x5 /* 30 vars */) = 0
4244 execve("/usr/bin/git", ["git", "apply", "/bench/stores/snp/patches/patch1791403656-4242"], 0x5 /* 3 vars */) = 0
+++ exited with 0 +++
"#;
        let argvs = parse_execve(log);
        assert_eq!(argvs.len(), 3);
        assert_eq!(
            argvs[1],
            ["\"git\"", "\"diff\"", "\"--quiet\"", "\"a b\\\"c\""]
        );
        let subs = [
            ("/bench/stores/snp".to_owned(), "$STORE"),
            ("/bench/snp".to_owned(), "$SNP"),
        ];
        let lines: Vec<String> = argvs.iter().map(|a| normalize_argv(a, &subs)).collect();
        assert_eq!(lines[0], "\"$SNP\" \"run\"");
        assert_eq!(lines[2], "\"git\" \"apply\" \"$STORE/patches/patchN-4242\"");
        let many: Vec<String> = (0..20).map(|i| format!("\"f{i}\"")).collect();
        assert!(normalize_argv(&many, &[]).ends_with("… (20 arguments)"));
        assert_eq!(fold_digits("a1234b12345c"), "a1234bNc");
        let head = vec!["git a".to_owned(), "git b".to_owned(), "git b".to_owned()];
        let base = vec!["git a".to_owned(), "git b".to_owned(), "git c".to_owned()];
        assert_eq!(exec_diff(&head, &base), ["+1× git b", "-1× git c"]);
        assert_eq!(exec_diff(&head, &head), Vec::<String>::new());
        let wide: Vec<String> = (0..50).map(|i| format!("git {i}")).collect();
        let d = exec_diff(&wide, &[]);
        assert_eq!(d.len(), EXEC_DIFF_MAX + 1);
        assert_eq!(d.last().unwrap(), "… and 10 more");
    }

    #[test]
    fn heap_peak_and_block_budgets_are_nightly_metrics() {
        let b = budgets();
        let mut b3 = m(10.0, (9.0, 11.0));
        b3.heap_peak_mib = Some(0.25);
        let mut b8 = m(400.0, (390.0, 410.0));
        b8.heap_peak_mib = Some(20.0); // a buffered 21 MB patch
        let mut cur = record(
            "gnu-system",
            &[
                ("B3", "snp", Outcome::Ok(b3)),
                ("B8", "snp", Outcome::Ok(b8)),
            ],
        );
        // not requested: not checked
        let checks = evaluate(&cur, None, &b, &none(), &none(), &[]);
        assert!(verdict_of(&checks, "B8", "heap peak").is_none());
        cur.extras.insert("heap_peak_mib".into());
        let checks = evaluate(&cur, None, &b, &none(), &none(), &[]);
        let c = verdict_of(&checks, "B8", "heap peak").unwrap();
        assert_eq!(c.verdict, Verdict::Fail, "{c:?}");
        assert!(c.detail.contains("limit 2.16 MiB"), "{c:?}");
        // the streamed patch: 0.48 MiB passes
        cur.scenarios.get_mut("B8").unwrap().insert("snp".into(), {
            let mut x = m(400.0, (390.0, 410.0));
            x.heap_peak_mib = Some(0.48);
            Outcome::Ok(x)
        });
        let checks = evaluate(&cur, None, &b, &none(), &none(), &[]);
        assert_eq!(
            verdict_of(&checks, "B8", "heap peak").unwrap().verdict,
            Verdict::Pass
        );
        assert_eq!(Metric::parse("alloc_blocks").unwrap().unit(), "blocks");
        assert_eq!(
            Metric::parse("heap_peak_mib").unwrap().extra(),
            Some("heap_peak_mib")
        );
    }

    #[test]
    fn the_comment_shows_the_paired_delta_counts_and_exec_changes() {
        let b = budgets();
        let mut head = rounds(&[11.0; 10]);
        head.counts = BTreeMap::from([
            ("clones".to_owned(), 22),
            ("statx".to_owned(), 5),
            ("newfstatat".to_owned(), 7),
            ("execve".to_owned(), 9),
        ]);
        head.exec_diff = vec!["+1× \"git\" \"status\"".into()];
        let mut base = rounds(&[10.0; 10]);
        base.counts = BTreeMap::from([
            ("clones".to_owned(), 20),
            ("statx".to_owned(), 5),
            ("newfstatat".to_owned(), 8),
            ("execve".to_owned(), 8),
        ]);
        let mut cur = record(
            "gnu-system",
            &[
                ("B3", "snp", Outcome::Ok(head)),
                ("B3", "snp-base", Outcome::Ok(base)),
            ],
        );
        cur.extras.insert("counts".into());
        let checks = evaluate(&cur, None, &b, &none(), &none(), &[]);
        let text = comment(&cur, None, &checks);
        assert!(text.contains("+10.0% (paired)"), "{text}");
        assert!(
            text.contains("22 (+2) / 5 (+0) / 7 (-1) / 9 (+1)"),
            "{text}"
        );
        assert!(text.contains("RSS MiB (largest process)"), "{text}");
        assert!(text.contains("\"git\" \"status\""), "{text}");
    }

    #[test]
    fn gc_stores_are_counted_per_tool() {
        let (base, work) = scratch_work("gc-counts");
        let mk = |p: PathBuf| std::fs::create_dir_all(p).unwrap();
        let snp = work.store(Tool::Snp, StoreMode::Gc);
        for i in 0..3 {
            mk(snp.join("repos-v1").join(format!("k{i}")));
            std::fs::write(snp.join("repos-v1").join(format!("k{i}.lock")), b"").unwrap();
        }
        mk(snp.join("configs-v1"));
        std::fs::write(snp.join("configs-v1").join("0123"), b"x").unwrap();
        assert_eq!(gc_store_counts(Tool::Snp, &snp).unwrap(), (Some(1), 3));
        let pc = work.store(Tool::PreCommit, StoreMode::Gc);
        mk(pc.join("repoabc"));
        mk(pc.join("repodef"));
        mk(pc.join("other"));
        std::fs::write(pc.join("db.db"), b"").unwrap();
        assert_eq!(gc_store_counts(Tool::PreCommit, &pc).unwrap(), (None, 2));
        let pk = work.store(Tool::Prek, StoreMode::Gc);
        mk(pk.join("repos").join("a"));
        std::fs::write(
            pk.join("config-tracking.json"),
            br#"["/c/1.yaml", "/c/2.yaml"]"#,
        )
        .unwrap();
        assert_eq!(gc_store_counts(Tool::Prek, &pk).unwrap(), (Some(2), 1));
        // pre-commit and prek fail loudly; snp warns until the C1 fix is in both binaries
        assert!(gc_equal_work(&work, Tool::Prek, 2).is_err());
        assert!(gc_equal_work(&work, Tool::Snp, 1).is_ok());
        std::fs::remove_dir_all(&base).unwrap();
    }
}
