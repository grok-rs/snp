//! Runs a scenario under snp and the oracle and compares (ARCHITECTURE §13.2, research 10 §7.4.4).
//!
//! Protocol per scenario and variant (brand × with/without `SNP_COMPAT_STRICT=1`): for each side **sequentially**
//! the fixed root `R = <runner.root>/<hash of the name>` is wiped (only when it carries the harness marker),
//! the scenario is materialised there with the hermetic environment, the side's entry point is linked as
//! `R/bin/pre-commit` (compat brand) or `R/bin/snp` (snp brand), and every step runs with `GIT_*_DATE` fixed per
//! step and (Linux) `taskset -c 0-3` so `os.cpu_count()`-derived job counts agree. After each step the observable
//! state is snapshotted. Then the streams are normalised (brand map on the oracle's non-argparse text in snp-brand
//! runs, the scenario's compare tier, the store roots, the foreign class-DIAG lines of snp's stderr) and compared,
//! and the divergence file's `expected:` checks are evaluated on the raw captures.
//!
//! Verdicts: no `expect_divergence` ⇒ any difference is unlisted. With an ID ⇒ the scenario must still differ and
//! its `expected:` checks must hold (else [`Verdict::StaleDivergence`]); a class-DIAG ID may differ on stderr only
//! (a difference elsewhere is unlisted). The strict variant of a class-EXT scenario must be identical to the
//! oracle (strict mode turns the extension off).
//!
//! Owner: WP-35.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use snp_core::env::{Env, EnvPatch};

use crate::compare::{self, Difference};
use crate::exec::{self, Captured, Invocation};
use crate::expect::{self, CheckCx};
use crate::materialise::{self, World};
use crate::normalise::{self, BrandMap, Tier};
use crate::scenario::{Brand, Compare, Scenario, Status, Step, StepKind, StreamMode};
use crate::snapshot::{self, Entry, State, StoreKind, Wanted};
use crate::text::{fnv64, replace_bytes, show};

/// `oracle_snp_prog` = the pc470 oracle launched with argparse prog patched to `snp` (ADR-058), used for snp-brand
/// runs.
#[derive(Debug)]
pub struct Runner {
    /// The snp binary under test.
    pub snp_bin: PathBuf,
    /// `target/oracle/pc470-py312/bin/pre-commit`.
    pub oracle_bin: PathBuf,
    /// The `pre-commit-snp-prog` launcher.
    pub oracle_snp_prog: PathBuf,
    /// The fixed absolute root both tools run under.
    pub root: PathBuf,
}

/// The comparison result.
#[derive(Debug)]
#[non_exhaustive]
pub enum Verdict {
    /// Identical after normalisation (also: nothing to compare, see [`Outcome::skipped`]).
    Same,
    /// A difference; `id` = the registered divergence it matches, if any.
    Diverged {
        /// The matching register ID.
        id: Option<String>,
        /// The rendered difference.
        diff: String,
    },
    /// A listed divergence that no longer reproduces.
    StaleDivergence(String),
}

impl Verdict {
    /// Whether the verdict is acceptable for a scenario registered under `expect` (`None`: unregistered).
    pub fn is_pass(&self, expect: Option<&str>) -> bool {
        match self {
            Self::Same => expect.is_none(),
            Self::Diverged { id: Some(id), .. } => expect == Some(id.as_str()),
            Self::Diverged { id: None, .. } | Self::StaleDivergence(_) => false,
        }
    }

    /// A one-line label: `same`, `listed D-…`, `UNLISTED`, `STALE D-…`.
    pub fn label(&self) -> String {
        match self {
            Self::Same => String::from("same"),
            Self::Diverged { id: Some(id), .. } => format!("listed {id}"),
            Self::Diverged { id: None, .. } => String::from("UNLISTED DIFFERENCE"),
            Self::StaleDivergence(id) => format!("STALE {id}"),
        }
    }

    fn copy(&self) -> Self {
        match self {
            Self::Same => Self::Same,
            Self::Diverged { id, diff } => Self::Diverged {
                id: id.clone(),
                diff: diff.clone(),
            },
            Self::StaleDivergence(id) => Self::StaleDivergence(id.clone()),
        }
    }
}

/// Run-wide settings beyond the four frozen [`Runner`] fields.
#[derive(Debug, Clone)]
pub struct Options {
    /// The snp-probe binary (`${probe}`); scenarios that need it are skipped without it.
    pub probe: Option<PathBuf>,
    /// The oracle venv's Python (reads pre-commit's SQLite store for the logical store comparison).
    pub python: Option<PathBuf>,
    /// `tests/differential/brand_map.yaml`.
    pub brand_map: BrandMap,
    /// Run `requires: {net: true}` scenarios.
    pub allow_net: bool,
    /// `taskset -c 0-N` prefix (Linux, when `taskset` works).
    pub taskset: Option<(PathBuf, String)>,
    /// Where failing scenarios write their artifacts (`<dir>/<name>/…`, incl. `repro.sh`).
    pub artifacts: Option<PathBuf>,
    /// Restrict the brands (empty: the scenario's own list).
    pub brands: Vec<Brand>,
    /// The parent environment the hermetic child environment inherits `PATH` & co. from.
    pub parent: Env,
    /// The host's git version.
    pub git_version: Option<[u32; 3]>,
    /// Oracle against itself (self-consistency): snp's side also runs the oracle programs.
    pub self_check: bool,
    /// Keep the scenario root after the run (default: removed after a passing scenario).
    pub keep: bool,
}

impl Options {
    /// Settings derived from the runner's paths and the host: snp-probe next to `snp_bin`, the oracle venv's
    /// Python next to `oracle_bin`, the workspace brand map, taskset (Linux), the host git version.
    pub fn detect(r: &Runner) -> Result<Self, String> {
        let parent = Env::capture_process();
        let exe = std::env::consts::EXE_SUFFIX;
        let probe = r
            .snp_bin
            .parent()
            .map(|d| d.join(format!("snp-probe{exe}")))
            .filter(|p| p.is_file())
            // Hooks run in the scenario's work directory: a relative probe path would not resolve there.
            .and_then(|p| std::path::absolute(p).ok());
        let python = r
            .oracle_bin
            .parent()
            .map(|d| {
                d.join(if cfg!(windows) {
                    "python.exe"
                } else {
                    "python"
                })
            })
            .filter(|p| p.exists());
        Ok(Self {
            probe,
            python,
            brand_map: BrandMap::workspace()?,
            allow_net: false,
            taskset: detect_taskset(&parent),
            artifacts: None,
            brands: Vec::new(),
            git_version: detect_git_version(&parent),
            parent,
            self_check: false,
            keep: false,
        })
    }
}

/// `taskset -c 0-N` with N = min(4, CPUs) - 1, when `taskset -c … true` works (Linux only).
fn detect_taskset(parent: &Env) -> Option<(PathBuf, String)> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let ts = exec::which("taskset", parent)?;
    let truth = exec::which("true", parent)?;
    let n = std::thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .min(4);
    let cpus = format!("0-{}", n.saturating_sub(1));
    let out = exec::run(&Invocation {
        program: ts.clone(),
        args: vec!["-c".into(), cpus.clone().into(), truth.into_os_string()],
        env: parent.clone(),
        cwd: std::env::temp_dir(),
        stdin: Vec::new(),
        tty: false,
        timeout: Duration::from_secs(30),
        signal: None,
    })
    .ok()?;
    (out.exit == Some(0)).then_some((ts, cpus))
}

/// `git --version` of the host.
pub(crate) fn detect_git_version(parent: &Env) -> Option<[u32; 3]> {
    let git = exec::which("git", parent)?;
    let out = exec::run(&Invocation {
        program: git,
        args: vec!["--version".into()],
        env: parent.clone(),
        cwd: std::env::temp_dir(),
        stdin: Vec::new(),
        tty: false,
        timeout: Duration::from_secs(30),
        signal: None,
    })
    .ok()?;
    let text = show(&out.stdout);
    crate::scenario::parse_version(text.trim().rsplit(' ').next().unwrap_or_default())
}

/// One run configuration of a scenario.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Variant {
    /// The brand.
    pub brand: Brand,
    /// snp runs with `SNP_COMPAT_STRICT=1`.
    pub strict: bool,
}

impl Variant {
    /// `compat`, `snp`, `compat+strict`, `snp+strict`.
    pub fn label(self) -> String {
        if self.strict {
            format!("{}+strict", self.brand.name())
        } else {
            self.brand.name().to_owned()
        }
    }
}

/// One step of one side.
#[derive(Debug, Clone)]
pub struct StepReport {
    /// Streams and exit.
    pub captured: Captured,
    /// The state after the step.
    pub state: State,
}

/// One side's run.
#[derive(Debug, Clone, Default)]
pub struct SideReport {
    /// Per step.
    pub steps: Vec<StepReport>,
    /// `.git/config` before the first step.
    pub initial_git_config: Option<Vec<u8>>,
}

/// One variant's comparison.
#[derive(Debug)]
pub struct VariantReport {
    /// Which variant.
    pub variant: Variant,
    /// Observed differences after normalisation.
    pub differences: Vec<Difference>,
    /// Failed `expected:` checks (`oracle step 1: exit: …`).
    pub expectation_failures: Vec<String>,
    /// DIAG IDs whose lines were stripped from snp's stderr.
    pub stripped_diag: Vec<&'static str>,
    /// The variant's verdict.
    pub verdict: Verdict,
    /// pre-commit's side.
    pub oracle: SideReport,
    /// snp's side.
    pub snp: SideReport,
}

/// Everything one scenario produced.
#[derive(Debug)]
pub struct Outcome {
    /// The scenario name.
    pub name: String,
    /// The register ID it is registered under.
    pub expect_divergence: Option<String>,
    /// Why it was not run (stub, `status: unit`, a `requires:` the host lacks, an unsupported key).
    pub skipped: Option<String>,
    /// A harness error (materialisation or spawn failure): always a failure.
    pub error: Option<String>,
    /// Per variant.
    pub variants: Vec<VariantReport>,
    /// The combined verdict (worst variant).
    pub verdict: Verdict,
    /// The run was an oracle self-check (every scenario must be identical).
    pub self_check: bool,
}

impl Outcome {
    /// Whether the scenario passes: skipped, identical, or diverging exactly as registered.
    pub fn passed(&self) -> bool {
        let expect = if self.self_check {
            None
        } else {
            self.expect_divergence.as_deref()
        };
        self.error.is_none() && (self.skipped.is_some() || self.verdict.is_pass(expect))
    }

    /// A human report: the verdict line plus every difference and failed expectation.
    pub fn report(&self) -> String {
        let mut o = String::new();
        let status = if let Some(s) = &self.skipped {
            format!("skipped ({s})")
        } else if let Some(e) = &self.error {
            format!("ERROR: {e}")
        } else {
            self.verdict.label()
        };
        writeln!(o, "{}: {status}", self.name).unwrap_or_default();
        for v in &self.variants {
            if v.verdict.is_pass(self.expect_divergence.as_deref())
                && !matches!(v.verdict, Verdict::Same)
            {
                continue;
            }
            if matches!(v.verdict, Verdict::Same) && self.expect_divergence.is_none() {
                continue;
            }
            writeln!(o, "  [{}] {}", v.variant.label(), v.verdict.label()).unwrap_or_default();
            for d in &v.differences {
                writeln!(o, "  --- {} (oracle -, snp +)", d.what).unwrap_or_default();
                for line in d.diff.lines() {
                    writeln!(o, "    {line}").unwrap_or_default();
                }
            }
            for f in &v.expectation_failures {
                writeln!(o, "  expectation failed: {f}").unwrap_or_default();
            }
        }
        o
    }
}

/// Which tool a side runs.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Side {
    Oracle,
    Snp,
}

impl Side {
    const fn name(self) -> &'static str {
        match self {
            Self::Oracle => "oracle",
            Self::Snp => "snp",
        }
    }
}

/// A side's run: the report and the world it ran in.
#[derive(Debug)]
struct SideRun {
    report: SideReport,
    world: World,
}

impl Runner {
    /// Materialises the scenario for both tools and compares every observed effect. Settings come from
    /// [`Options::detect`]; a skipped scenario is [`Verdict::Same`] (see [`Runner::run_with`] for the reason) and a
    /// harness error is an unlisted [`Verdict::Diverged`] naming the error.
    pub fn run(&self, s: &Scenario) -> Verdict {
        match Options::detect(self) {
            Ok(opts) => self.run_with(s, &opts).verdict,
            Err(e) => Verdict::Diverged {
                id: None,
                diff: format!("harness error: {e}"),
            },
        }
    }

    /// [`Runner::run`] with explicit settings and the full report.
    pub fn run_with(&self, s: &Scenario, opts: &Options) -> Outcome {
        let mut out = Outcome {
            name: s.name.clone(),
            expect_divergence: s.expect_divergence.clone(),
            skipped: skip_reason(s, opts),
            error: None,
            variants: Vec::new(),
            verdict: Verdict::Same,
            self_check: opts.self_check,
        };
        if out.skipped.is_some() {
            return out;
        }
        let root = match self.scenario_root(s) {
            Ok(r) => r,
            Err(e) => return errored(out, e),
        };
        let brands: Vec<Brand> = s
            .brands
            .iter()
            .copied()
            .filter(|b| opts.brands.is_empty() || opts.brands.contains(b))
            .collect();
        if brands.is_empty() {
            out.skipped = Some(String::from("no selected brand"));
            return out;
        }
        for brand in brands {
            let oracle = match self.run_side(s, &root, brand, Side::Oracle, false, opts) {
                Ok(r) => r,
                Err(e) => return errored(out, format!("[{}] oracle: {e}", brand.name())),
            };
            let stricts: &[bool] = if s.strict_variants && !opts.self_check {
                &[false, true]
            } else {
                &[false]
            };
            for &strict in stricts {
                let snp = match self.run_side(s, &root, brand, Side::Snp, strict, opts) {
                    Ok(r) => r,
                    Err(e) => {
                        return errored(
                            out,
                            format!("[{}] snp: {e}", Variant { brand, strict }.label()),
                        );
                    }
                };
                out.variants.push(judge(
                    s,
                    Variant { brand, strict },
                    &oracle,
                    snp,
                    &root,
                    opts,
                ));
            }
        }
        out.verdict = combine(&out.variants);
        if !out.passed()
            && let Some(dir) = &opts.artifacts
            && let Err(e) = write_artifacts(dir, s, &out, self, &root)
        {
            out.error = Some(format!("writing artifacts: {e}"));
        }
        if !opts.keep {
            // Best effort: a leftover root is wiped by the next run anyway.
            drop(materialise::remove_root(&root));
        }
        out
    }

    /// `R` for `s`: `<root>/<16 hex digits of the name's hash>`, canonical (both tools see the same bytes).
    pub fn scenario_root(&self, s: &Scenario) -> Result<PathBuf, String> {
        std::fs::create_dir_all(&self.root)
            .map_err(|e| format!("creating {}: {e}", self.root.display()))?;
        let base = materialise::canonical(&self.root)?;
        Ok(base.join(format!("{:016x}", fnv64(s.name.as_bytes()))))
    }

    /// The program a side runs for `brand`.
    fn program(&self, brand: Brand, side: Side, opts: &Options) -> &Path {
        match (side, brand) {
            (Side::Snp, _) if !opts.self_check => &self.snp_bin,
            (_, Brand::Compat) => &self.oracle_bin,
            (_, Brand::Snp) => &self.oracle_snp_prog,
        }
    }

    fn run_side(
        &self,
        s: &Scenario,
        root: &Path,
        brand: Brand,
        side: Side,
        strict: bool,
        opts: &Options,
    ) -> Result<SideRun, String> {
        materialise::reset_root(root)?;
        let mut w = materialise::materialise(s, root, opts.probe.as_deref(), &opts.parent)?;
        let tool = link_tool(&w, self.program(brand, side, opts), brand)?;
        w.vars
            .push((String::from("tool"), tool.to_string_lossy().into_owned()));
        // The plain pre-commit 4.7.0 oracle on both sides: interop scenarios (11 §A.20) run it explicitly next to the
        // tool under test (`- shell: ${oracle} install` then `- tool: [install]`), so the oracle side is the baseline.
        let oracle = std::path::absolute(&self.oracle_bin)
            .map_err(|e| format!("resolving {}: {e}", self.oracle_bin.display()))?;
        w.vars.push((
            String::from("oracle"),
            crate::text::sh_quote(&oracle.to_string_lossy()),
        ));
        let work = w.layout.work.clone();
        let want = Wanted {
            tree: s.compare.tree || needs_tree(s),
            index: s.compare.index,
            refs: s.compare.refs,
            hooks: s.compare.hooks_dir,
            git_config: true,
            store: s.compare.store,
        };
        let store_kind = if side == Side::Snp && !opts.self_check {
            StoreKind::Snp
        } else {
            StoreKind::PreCommit
        };
        let mut report = SideReport {
            steps: Vec::new(),
            initial_git_config: std::fs::read(work.join(".git/config")).ok(),
        };
        for (project, step) in flat_steps(s) {
            if let Some(p) = project {
                w.layout.work = root.join(p);
            }
            let inv = step_invocation(&mut w, step, &tool, side == Side::Snp && strict, opts)?;
            let captured = exec::run(&inv)?;
            let state = snapshot::capture(&mut w, want, store_kind, opts.python.as_deref())?;
            report.steps.push(StepReport { captured, state });
        }
        if let Some(first) = s.projects.first() {
            w.layout.work = root.join(first);
        }
        Ok(SideRun { report, world: w })
    }
}

fn errored(mut out: Outcome, e: String) -> Outcome {
    out.verdict = Verdict::Diverged {
        id: None,
        diff: format!("harness error: {e}"),
    };
    out.error = Some(e);
    out
}

/// The steps in execution order: every step once, or (with `projects:`) every step per project.
fn flat_steps(s: &Scenario) -> Vec<(Option<&str>, &Step)> {
    if s.projects.is_empty() {
        s.steps.iter().map(|st| (None, st)).collect()
    } else {
        s.projects
            .iter()
            .flat_map(|p| s.steps.iter().map(move |st| (Some(p.as_str()), st)))
            .collect()
    }
}

/// The flat index of step `n` (1-based) of the project an expectation names (`project:`), if any.
fn flat_index(
    s: &Scenario,
    n: usize,
    checks: &[(String, crate::yamlv::Yv)],
) -> Result<usize, String> {
    let i = n.checked_sub(1).ok_or("step 0")?;
    let project = checks
        .iter()
        .find(|(k, _)| k == "project")
        .and_then(|(_, v)| v.to_text());
    match project {
        None => Ok(i),
        Some(p) => {
            let pi = s
                .projects
                .iter()
                .position(|x| *x == p)
                .ok_or_else(|| format!("unknown project {p:?}"))?;
            Ok(pi.saturating_mul(s.steps.len()).saturating_add(i))
        }
    }
}

/// Whether an `expected:` check needs the tree snapshot although `compare.tree` is off.
fn needs_tree(s: &Scenario) -> bool {
    s.expected.as_ref().is_some_and(|e| {
        e.oracle
            .iter()
            .chain(&e.snp)
            .any(|(_, se)| se.checks.iter().any(|(k, _)| k.starts_with("tree")))
    })
}

/// Why `s` cannot run here, if it cannot.
fn skip_reason(s: &Scenario, opts: &Options) -> Option<String> {
    match s.status {
        Status::Stub => return Some(String::from("stub divergence file")),
        Status::Unit => {
            return Some(format!(
                "covered by unit tests: {}",
                s.unit_tests.join(", ")
            ));
        }
        Status::Active => {}
    }
    if !s.unsupported.is_empty() {
        return Some(format!("unsupported: {}", s.unsupported.join("; ")));
    }
    if let Some(since) = &s.since
        && crate::scenario::parse_version(since).is_some_and(|v| v > [4, 7, 0])
    {
        return Some(format!("needs pre-commit {since}"));
    }
    let r = &s.requires;
    let os = if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };
    if !r.os.is_empty()
        && !r
            .os
            .iter()
            .any(|o| o == os || (o == "posix" && !cfg!(windows)))
    {
        return Some(format!("os {os} not in {:?}", r.os));
    }
    if !r.git.terms.is_empty() {
        match opts.git_version {
            Some(v) if r.git.matches(v) => {}
            Some(v) => {
                return Some(format!(
                    "git {}.{}.{} does not satisfy the requirement",
                    v[0], v[1], v[2]
                ));
            }
            None => return Some(String::from("git version unknown")),
        }
    }
    if r.net && !opts.allow_net {
        return Some(String::from("needs the network (--net)"));
    }
    if r.pty && cfg!(windows) {
        return Some(String::from("needs a pty"));
    }
    if r.probe && opts.probe.is_none() {
        return Some(String::from("needs snp-probe"));
    }
    if let Some(t) = r
        .tools
        .iter()
        .find(|t| exec::which(t, &opts.parent).is_none())
    {
        return Some(format!("tool {t:?} not on PATH"));
    }
    if let Some(want_root) = r.root
        && is_root() != want_root
    {
        return Some(String::from(if want_root {
            "needs root"
        } else {
            "must not run as root"
        }));
    }
    if let Some(min) = r.logical_cpus_min {
        let n = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
        if u32::try_from(n).unwrap_or(u32::MAX) < min {
            return Some(format!("needs {min} logical CPUs"));
        }
    }
    None
}

fn is_root() -> bool {
    #[cfg(unix)]
    {
        rustix::process::geteuid().is_root()
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Links the side's program as `R/bin/pre-commit` (compat) or `R/bin/snp` (snp brand); Windows copies it.
fn link_tool(w: &World, program: &Path, brand: Brand) -> Result<PathBuf, String> {
    let name = match brand {
        Brand::Compat => "pre-commit",
        Brand::Snp => "snp",
    };
    // The link lives under the scenario root: a relative program path (`--snp-bin target/debug/snp`) would dangle.
    let absolute = std::path::absolute(program)
        .map_err(|e| format!("resolving {}: {e}", program.display()))?;
    let program = absolute.as_path();
    if !program.exists() {
        return Err(format!("{} does not exist", program.display()));
    }
    #[cfg(unix)]
    {
        let link = w.layout.bin.join(name);
        std::os::unix::fs::symlink(program, &link)
            .map_err(|e| format!("linking {}: {e}", link.display()))?;
        Ok(link)
    }
    #[cfg(not(unix))]
    {
        // A pip console-script launcher only works from its own Scripts directory: the oracle runs in place.
        if program
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
            && program.ends_with("snp.exe")
        {
            let copy = w.layout.bin.join(format!("{name}.exe"));
            std::fs::copy(program, &copy)
                .map_err(|e| format!("copying {}: {e}", program.display()))?;
            Ok(copy)
        } else {
            let _: &World = w;
            Ok(program.to_path_buf())
        }
    }
}

fn step_invocation(
    w: &mut World,
    step: &Step,
    tool: &Path,
    strict: bool,
    opts: &Options,
) -> Result<Invocation, String> {
    w.tick = w.tick.saturating_add(1);
    let date = materialise::git_date(w.tick);
    let mut patch = EnvPatch::default()
        .set("GIT_AUTHOR_DATE", date.clone())
        .set("GIT_COMMITTER_DATE", date)
        .set("PRE_COMMIT", tool.as_os_str().to_os_string());
    if strict {
        patch = patch.set("SNP_COMPAT_STRICT", "1");
    }
    for (k, v) in &step.env {
        match v {
            None => patch = patch.unset(k.clone()),
            Some(v) if k == "PATH_PREPEND" => {
                let dirs: Vec<PathBuf> = std::env::split_paths(&w.subst(v)?).collect();
                patch = patch.prepend_path("PATH", dirs);
            }
            Some(v) => patch = patch.set(k.clone(), w.subst(v)?),
        }
    }
    let env = w.env.patched(&patch);
    let (program, args): (PathBuf, Vec<std::ffi::OsString>) = match &step.kind {
        StepKind::Tool(a) => (tool.to_path_buf(), subst_all(w, a)?),
        StepKind::Git(a) => (w.git.clone(), subst_all(w, a)?),
        StepKind::Shell(c) => {
            let sh = w.sh.clone().ok_or("sh is not on PATH")?;
            (sh, vec!["-c".into(), w.subst(c)?.into()])
        }
    };
    let (program, args) = match &opts.taskset {
        Some((ts, cpus)) => {
            let mut a: Vec<std::ffi::OsString> =
                vec!["-c".into(), cpus.into(), program.into_os_string()];
            a.extend(args);
            (ts.clone(), a)
        }
        None => (program, args),
    };
    let cwd = match &step.cwd {
        Some(c) => w.layout.work.join(w.subst(c)?),
        None => w.layout.work.clone(),
    };
    let signal = match &step.signal {
        Some(sig) => Some((
            exec::signal_number(&sig.name)
                .ok_or_else(|| format!("unknown signal {:?}", sig.name))?,
            exec::Trigger {
                file: sig.after_file.as_ref().map(|f| cwd.join(f)),
                delay: Duration::from_millis(sig.after_ms),
            },
        )),
        None => None,
    };
    Ok(Invocation {
        program,
        args,
        env,
        cwd,
        stdin: w.subst_bytes(&step.stdin)?,
        tty: step.tty,
        timeout: step
            .timeout_ms
            .map_or(exec::DEFAULT_TIMEOUT, Duration::from_millis),
        signal,
    })
}

/// Substituted step arguments. An argument whose `\xNN` escapes make it invalid UTF-8 is a byte name (posix: the
/// raw bytes, as `files_bytes` and `commit` entries); any other argument is passed literally.
fn subst_all(w: &World, a: &[String]) -> Result<Vec<std::ffi::OsString>, String> {
    a.iter()
        .map(|x| {
            let t = w.subst(x)?;
            let bytes = crate::text::expand_byte_escapes(&t);
            if std::str::from_utf8(&bytes).is_err() {
                materialise::os_from_bytes(&bytes)
            } else {
                Ok(t.into())
            }
        })
        .collect()
}

/// A step's short description for reports: `tool run --all-files`.
fn describe(step: &Step) -> String {
    let (kind, rest) = match &step.kind {
        StepKind::Tool(a) => ("tool", a.join(" ")),
        StepKind::Git(a) => ("git", a.join(" ")),
        StepKind::Shell(c) => ("shell", c.clone()),
    };
    let mut d = format!("{kind} {rest}");
    if d.chars().count() > 60 {
        d = d.chars().take(57).collect::<String>() + "...";
    }
    d
}

/// The normalisation of one stream for one side. `reduce`: the brand map whose snp-only parser blocks are reduced
/// (both sides of snp-brand runs).
fn norm_stream(
    mode: StreamMode,
    bytes: &[u8],
    (map, reduce): (&BrandMap, Option<&BrandMap>),
    stores: &[&[u8]],
    eol: bool,
) -> Vec<u8> {
    let tier = match mode {
        StreamMode::Raw | StreamMode::Ignore => Tier::A,
        StreamMode::Exact | StreamMode::LinesUnordered => Tier::C,
        StreamMode::TierB => Tier::B,
    };
    let mut b = normalise::normalise(tier, bytes, map);
    if let Some(r) = reduce {
        b = r.reduce_snp_only_parsers(&b);
    }
    if !matches!(mode, StreamMode::Raw) {
        b = normalise::mask_paths(&b, stores);
    }
    if eol {
        b = replace_bytes(&b, b"\r\n", b"\n");
    }
    b
}

fn stream_diff(out: &mut Vec<Difference>, what: String, mode: StreamMode, (a, b): (&[u8], &[u8])) {
    if !compare::streams_equal(mode, a, b) {
        let (x, y) = if matches!(mode, StreamMode::LinesUnordered) {
            (sorted_lines(a), sorted_lines(b))
        } else {
            (a.to_vec(), b.to_vec())
        };
        out.push(Difference {
            what,
            diff: compare::unified(&x, &y),
        });
    }
}

fn sorted_lines(b: &[u8]) -> Vec<u8> {
    let mut l = crate::text::lines_with_ends(b);
    l.sort_unstable();
    l.concat()
}

/// Compares one variant and evaluates its expectations.
#[expect(
    clippy::many_single_char_names,
    reason = "s(cenario), c(ompare), a/b (the two sides' steps)"
)]
fn judge(
    s: &Scenario,
    variant: Variant,
    oracle: &SideRun,
    snp: SideRun,
    root: &Path,
    opts: &Options,
) -> VariantReport {
    let c: &Compare = &s.compare;
    let store_a = root.join("store");
    let store_b = root.join("snp-store");
    let stores: [&[u8]; 2] = [
        store_a.as_os_str().as_encoded_bytes(),
        store_b.as_os_str().as_encoded_bytes(),
    ];
    let identity = BrandMap::identity();
    let (map_oracle, map_snp) = match (variant.brand, opts.self_check) {
        (Brand::Compat, _) => (&identity, &identity),
        (Brand::Snp, false) => (&opts.brand_map, &identity),
        (Brand::Snp, true) => (&opts.brand_map, &opts.brand_map),
    };
    let reduce = (variant.brand == Brand::Snp && !opts.self_check).then_some(&opts.brand_map);
    // A class-BRAND divergence is exactly what the brand map removes: its own scenario compares unmapped texts,
    // or the divergence could never be observed.
    let brand_id = s
        .expect_divergence
        .as_deref()
        .is_some_and(|i| i.starts_with("D-BRAND-"));
    // (Both sides: under --self-check the snp side is the oracle too, and mapping only one of them would invent the
    // very difference the scenario registers.)
    let (map_oracle, map_snp, reduce) = if brand_id {
        (&identity, &identity, None)
    } else {
        (map_oracle, map_snp, reduce)
    };
    let (map_oracle, map_snp) = ((map_oracle, reduce), (map_snp, reduce));
    let keep_diag = s.expect_divergence.as_deref().filter(|_| s.is_diag());
    // The generated suite's known differences go first, on the raw bytes: the rest of the comparison stays exact.
    let generated = s
        .tags
        .iter()
        .any(|t| t == "generated")
        .then_some(variant.brand);
    let known = |bytes: &[u8]| {
        generated.map_or_else(
            || bytes.to_vec(),
            |brand| normalise::reduce_generated(bytes, brand),
        )
    };
    let mut diffs = Vec::new();
    let mut stripped: Vec<&'static str> = Vec::new();
    for (i, (project, step)) in flat_steps(s).into_iter().enumerate() {
        let n = i
            .checked_rem(s.steps.len().max(1))
            .unwrap_or(i)
            .saturating_add(1);
        let label = match project {
            Some(p) => format!("{p}: step {n} ({})", describe(step)),
            None => format!("step {n} ({})", describe(step)),
        };
        let (Some(a), Some(b)) = (oracle.report.steps.get(i), snp.report.steps.get(i)) else {
            continue;
        };
        let (ca, cb) = (&a.captured, &b.captured);
        // A shell step that runs the plain oracle by name (interop: `${oracle} install`) runs pre-commit on both
        // sides: its text is pre-commit's on both, so the snp-brand map does not apply to it.
        let oracle_only = matches!(&step.kind, StepKind::Shell(cmd)
            if cmd.contains(concat!("$", "{oracle}")) && !cmd.contains(concat!("$", "{tool}")));
        let map_oracle = if oracle_only { map_snp } else { map_oracle };
        // A stream cut at MAX_CAPTURE is never "equal", even when both sides flooded.
        if ca.truncated || cb.truncated {
            diffs.push(Difference {
                what: format!("{label} output"),
                diff: format!(
                    "-output exceeded {} bytes: {}\n+output exceeded {} bytes: {}\n",
                    exec::MAX_CAPTURE,
                    ca.truncated,
                    exec::MAX_CAPTURE,
                    cb.truncated
                ),
            });
            // Normalising and diffing two 64 MiB prefixes would only restate that; the step's state still counts.
            step_state_diffs(&mut diffs, &label, c, (&a.state, &b.state), generated);
            continue;
        }
        if ca.timed_out != cb.timed_out {
            diffs.push(Difference {
                what: format!("{label} timeout"),
                diff: format!(
                    "-timed out: {}\n+timed out: {}\n",
                    ca.timed_out, cb.timed_out
                ),
            });
        }
        if !compare::exits_equal(c.exit, ca.exit, cb.exit) {
            diffs.push(Difference {
                what: format!("{label} exit"),
                diff: format!("-{:?}\n+{:?}\n", ca.exit, cb.exit),
            });
        }
        // snp's foreign DIAG lines are stripped from stderr (and from the merged stream under a pty).
        let (snp_err, ids) = normalise::strip_diag(&cb.stderr, keep_diag);
        let snp_out = if step.tty {
            let (o, more) = normalise::strip_diag(&cb.stdout, keep_diag);
            stripped.extend(more);
            o
        } else {
            cb.stdout.clone()
        };
        stripped.extend(ids);
        let (snp_out, snp_err) = (known(&snp_out), known(&snp_err));
        let (oracle_stdout, oracle_stderr) = (known(&ca.stdout), known(&ca.stderr));
        let eol = c.eol_insensitive;
        // Under a pty stdout and stderr are one stream: a difference that is only the kept DIAG ID's own lines is
        // that ID's stderr difference (it is labelled as such, so the class-DIAG rule holds).
        let oracle_out = norm_stream(c.stdout, &oracle_stdout, map_oracle, &stores, eol);
        let snp_norm = norm_stream(c.stdout, &snp_out, map_snp, &stores, eol);
        let diag_only = step.tty
            && keep_diag.is_some()
            && !compare::streams_equal(c.stdout, &oracle_out, &snp_norm)
            && compare::streams_equal(
                c.stdout,
                &oracle_out,
                &norm_stream(
                    c.stdout,
                    &normalise::strip_diag(&snp_out, None).0,
                    map_snp,
                    &stores,
                    eol,
                ),
            );
        let what = if diag_only {
            format!("{label} stdout (pty: merged stderr)")
        } else {
            format!("{label} stdout")
        };
        stream_diff(&mut diffs, what, c.stdout, (&oracle_out, &snp_norm));
        stream_diff(
            &mut diffs,
            format!("{label} stderr"),
            c.stderr,
            (
                &norm_stream(c.stderr, &oracle_stderr, map_oracle, &stores, eol),
                &norm_stream(c.stderr, &snp_err, map_snp, &stores, eol),
            ),
        );
        step_state_diffs(&mut diffs, &label, c, (&a.state, &b.state), generated);
    }
    stripped.sort_unstable();
    stripped.dedup();
    let mut failures = Vec::new();
    if let Some(e) = &s.expected {
        let snp_map = (variant.brand == Brand::Snp && !opts.self_check).then_some(&opts.brand_map);
        // Self-check runs the oracle on both sides: only pre-commit's expectations apply (to either side).
        let snp_checks = if opts.self_check { &e.oracle } else { &e.snp };
        for (side_name, checks, run, map) in [
            ("oracle", &e.oracle, oracle, None),
            ("snp", snp_checks, &snp, snp_map),
        ] {
            for (n, se) in checks {
                let Some(st) = flat_index(s, *n, &se.checks)
                    .ok()
                    .and_then(|i| run.report.steps.get(i))
                else {
                    failures.push(format!("{side_name} step {n}: no such step"));
                    continue;
                };
                let cx = CheckCx {
                    cap: &st.captured,
                    state: &st.state,
                    initial_git_config: run.report.initial_git_config.as_deref(),
                    world: &run.world,
                    brand: variant.brand,
                    map,
                };
                for f in expect::evaluate(&se.checks, &cx) {
                    failures.push(format!("{side_name} step {n}: {f}"));
                }
            }
        }
    }
    let verdict = if opts.self_check {
        // Oracle against itself: every scenario, registered or not, must show no difference.
        let mut plain = Scenario::new(&s.name);
        plain.expect_divergence = None;
        variant_verdict(&plain, variant, &diffs, &failures)
    } else {
        variant_verdict(s, variant, &diffs, &failures)
    };
    VariantReport {
        variant,
        differences: diffs,
        expectation_failures: failures,
        stripped_diag: stripped,
        verdict,
        oracle: oracle.report.clone(),
        snp: snp.report,
    }
}

/// [`state_diffs`] of one step; in a generated scenario (`generated` = its brand) the worktree's files are reduced
/// first, since a hook's `log_file` holds what the hook printed.
fn step_state_diffs(
    diffs: &mut Vec<Difference>,
    label: &str,
    c: &Compare,
    (a, b): (&State, &State),
    generated: Option<Brand>,
) {
    let Some(brand) = generated else {
        return state_diffs(diffs, label, c, a, b);
    };
    let reduced = |s: &State| {
        let mut s = s.clone();
        for e in s.tree.values_mut() {
            if let Entry::File { bytes, .. } = e {
                *bytes = normalise::reduce_generated(bytes, brand);
            }
        }
        s
    };
    state_diffs(diffs, label, c, &reduced(a), &reduced(b));
}

#[expect(
    clippy::many_single_char_names,
    reason = "a/b are the oracle/snp states, x/y their renderings, c the compare block"
)]
fn state_diffs(diffs: &mut Vec<Difference>, label: &str, c: &Compare, a: &State, b: &State) {
    let eolfix = |v: &[u8]| {
        if c.eol_insensitive {
            replace_bytes(v, b"\r\n", b"\n")
        } else {
            v.to_vec()
        }
    };
    if c.tree && a.tree != b.tree {
        let (x, y) = (
            snapshot::render_tree(&a.tree),
            snapshot::render_tree(&b.tree),
        );
        if !c.eol_insensitive || eolfix(&x) != eolfix(&y) {
            diffs.push(Difference {
                what: format!("{label} tree"),
                diff: compare::unified(&x, &y),
            });
        }
    }
    if c.index && a.index != b.index {
        diffs.push(Difference {
            what: format!("{label} index"),
            diff: compare::unified(&a.index, &b.index),
        });
    }
    if c.refs && a.refs != b.refs {
        diffs.push(Difference {
            what: format!("{label} refs"),
            diff: compare::unified(&a.refs, &b.refs),
        });
    }
    if c.hooks_dir && a.hooks != b.hooks {
        diffs.push(Difference {
            what: format!("{label} hooks"),
            diff: format!("-{:?}\n+{:?}\n", a.hooks, b.hooks),
        });
    }
    if !matches!(c.config_file, StreamMode::Ignore) {
        let x = eolfix(a.config_file.as_deref().unwrap_or(b"<absent>"));
        let y = eolfix(b.config_file.as_deref().unwrap_or(b"<absent>"));
        if !compare::streams_equal(c.config_file, &x, &y) {
            diffs.push(Difference {
                what: format!("{label} config file"),
                diff: compare::unified(&x, &y),
            });
        }
    }
    if c.git_config && a.git_config != b.git_config {
        let (x, y) = (
            a.git_config.clone().unwrap_or_default(),
            b.git_config.clone().unwrap_or_default(),
        );
        diffs.push(Difference {
            what: format!("{label} .git/config"),
            diff: compare::unified(&x, &y),
        });
    }
    if c.store && a.store != b.store {
        let x = a.store.join("\n");
        let y = b.store.join("\n");
        diffs.push(Difference {
            what: format!("{label} store"),
            diff: compare::unified(x.as_bytes(), y.as_bytes()),
        });
    }
}

/// Whether a difference's channel is stderr (DIAG may differ there only); under a pty, the merged stream when the
/// difference is only DIAG lines.
fn is_stderr(d: &Difference) -> bool {
    d.what.ends_with(" stderr") || d.what.ends_with(" stdout (pty: merged stderr)")
}

fn render_diffs(diffs: &[Difference], failures: &[String]) -> String {
    let mut o = String::new();
    for d in diffs {
        writeln!(o, "--- {}", d.what).unwrap_or_default();
        o.push_str(&d.diff);
    }
    for f in failures {
        writeln!(o, "expectation failed: {f}").unwrap_or_default();
    }
    o
}

fn variant_verdict(
    s: &Scenario,
    variant: Variant,
    diffs: &[Difference],
    failures: &[String],
) -> Verdict {
    let id = s.expect_divergence.as_deref();
    // The strict variant of an extension scenario must equal pre-commit: SNP_COMPAT_STRICT turns the extension off.
    let id = if variant.strict && id.is_some_and(|i| i.starts_with("D-EXT-")) {
        None
    } else {
        id
    };
    let rendered = render_diffs(diffs, failures);
    match id {
        None if diffs.is_empty() && failures.is_empty() => Verdict::Same,
        None => Verdict::Diverged {
            id: None,
            diff: rendered,
        },
        Some(id) => {
            if id.starts_with("D-DIAG-") && diffs.iter().any(|d| !is_stderr(d)) {
                let outside: Vec<Difference> =
                    diffs.iter().filter(|d| !is_stderr(d)).cloned().collect();
                return Verdict::Diverged {
                    id: None,
                    diff: render_diffs(&outside, &[]),
                };
            }
            if diffs.is_empty() || !failures.is_empty() {
                Verdict::StaleDivergence(id.to_owned())
            } else {
                Verdict::Diverged {
                    id: Some(id.to_owned()),
                    diff: rendered,
                }
            }
        }
    }
}

/// The worst verdict: unlisted > stale > listed > same.
fn combine(vs: &[VariantReport]) -> Verdict {
    let rank = |v: &Verdict| match v {
        Verdict::Diverged { id: None, .. } => 3,
        Verdict::StaleDivergence(_) => 2,
        Verdict::Diverged { id: Some(_), .. } => 1,
        Verdict::Same => 0,
    };
    vs.iter()
        .map(|v| &v.verdict)
        .max_by_key(|v| rank(v))
        .map_or(Verdict::Same, Verdict::copy)
}

/// A file-system-safe directory name for a scenario: one path component that is never empty and never starts
/// with a dot (so `.` and `..` cannot name the output directory or its parent).
pub(crate) fn safe_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    // Windows drops a trailing `.` from a path component (`_..` would name `_`): end in `_` instead.
    let s = if s.ends_with('.') { format!("{s}_") } else { s };
    if s.is_empty() || s.starts_with('.') {
        format!("_{s}")
    } else {
        s
    }
}

/// Written into every artifact directory; [`write_artifacts`] removes only directories that carry it.
const ARTIFACT_MARKER: &str = ".snp-difftest-artifacts";

/// `dir/<safe name>`, emptied: an existing directory is removed only when it carries [`ARTIFACT_MARKER`] (or is a
/// pre-marker artifact directory); the fresh one gets the marker.
fn artifact_dir(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let base = dir.join(safe_name(name));
    if base.parent() != Some(dir) {
        return Err(format!(
            "{}: not a directory below {}",
            base.display(),
            dir.display()
        ));
    }
    if std::fs::symlink_metadata(&base).is_ok() {
        // Directories written before the marker existed carry scenario.yaml and repro.sh.
        let legacy = base.join("scenario.yaml").is_file() && base.join("repro.sh").is_file();
        if !base.join(ARTIFACT_MARKER).is_file() && !legacy {
            return Err(format!(
                "refusing to replace {}: not a difftest artifact directory (no {ARTIFACT_MARKER})",
                base.display()
            ));
        }
        std::fs::remove_dir_all(&base).map_err(|e| format!("{}: {e}", base.display()))?;
    }
    materialise::write_file(&base.join(ARTIFACT_MARKER), b"")?;
    Ok(base)
}

fn write_artifacts(
    dir: &Path,
    s: &Scenario,
    out: &Outcome,
    r: &Runner,
    root: &Path,
) -> Result<(), String> {
    let base = artifact_dir(dir, &s.name)?;
    let w = |p: PathBuf, b: &[u8]| materialise::write_file(&p, b);
    w(base.join("scenario.yaml"), s.to_yaml().as_bytes())?;
    w(base.join("report.txt"), out.report().as_bytes())?;
    for v in &out.variants {
        let vd = base.join(v.variant.label());
        for (side, rep) in [(Side::Oracle, &v.oracle), (Side::Snp, &v.snp)] {
            for (i, st) in rep.steps.iter().enumerate() {
                let p = vd
                    .join(side.name())
                    .join(format!("step-{}", i.saturating_add(1)));
                w(p.join("stdout"), &st.captured.stdout)?;
                w(p.join("stderr"), &st.captured.stderr)?;
                let exit = format!(
                    "{:?}{}\n",
                    st.captured.exit,
                    if st.captured.timed_out {
                        " (timed out)"
                    } else {
                        ""
                    }
                );
                w(p.join("exit"), exit.as_bytes())?;
                w(p.join("tree.txt"), &snapshot::render_tree(&st.state.tree))?;
                w(p.join("index.txt"), &st.state.index)?;
                w(p.join("refs.txt"), &st.state.refs)?;
            }
        }
        w(
            vd.join("diff.txt"),
            render_diffs(&v.differences, &v.expectation_failures).as_bytes(),
        )?;
    }
    let script = crate::shrink::repro_script(s, r, root);
    w(base.join("repro.sh"), script.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            base.join("repro.sh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .map_err(|e| format!("chmod repro.sh: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(what: &str) -> Difference {
        Difference {
            what: what.to_owned(),
            diff: String::from("-a\n+b\n"),
        }
    }

    fn sc(id: Option<&str>) -> Scenario {
        let mut s = Scenario::new("t");
        s.expect_divergence = id.map(str::to_owned);
        s
    }

    const PLAIN: Variant = Variant {
        brand: Brand::Compat,
        strict: false,
    };

    #[test]
    fn verdicts() {
        assert!(matches!(
            variant_verdict(&sc(None), PLAIN, &[], &[]),
            Verdict::Same
        ));
        assert!(matches!(
            variant_verdict(&sc(None), PLAIN, &[diff("step 1 (x) stdout")], &[]),
            Verdict::Diverged { id: None, .. }
        ));
        let listed = variant_verdict(
            &sc(Some("D-CRASH-04")),
            PLAIN,
            &[diff("step 1 (x) stdout")],
            &[],
        );
        assert!(matches!(&listed, Verdict::Diverged { id: Some(i), .. } if i == "D-CRASH-04"));
        assert!(listed.is_pass(Some("D-CRASH-04")));
        assert!(!listed.is_pass(None));
        assert!(matches!(
            variant_verdict(&sc(Some("D-CRASH-04")), PLAIN, &[], &[]),
            Verdict::StaleDivergence(_)
        ));
        let failed = variant_verdict(
            &sc(Some("D-CRASH-04")),
            PLAIN,
            &[diff("step 1 (x) stdout")],
            &[String::from("x")],
        );
        assert!(matches!(failed, Verdict::StaleDivergence(_)));
    }

    #[test]
    fn diag_may_differ_on_stderr_only() {
        let d = sc(Some("D-DIAG-02"));
        assert!(matches!(
            variant_verdict(&d, PLAIN, &[diff("step 1 (x) stderr")], &[]),
            Verdict::Diverged { id: Some(_), .. }
        ));
        assert!(matches!(
            variant_verdict(
                &d,
                PLAIN,
                &[diff("step 1 (x) stderr"), diff("step 1 (x) exit")],
                &[]
            ),
            Verdict::Diverged { id: None, .. }
        ));
    }

    #[test]
    fn strict_extension_variant_must_be_identical() {
        let e = sc(Some("D-EXT-02"));
        let strict = Variant {
            brand: Brand::Snp,
            strict: true,
        };
        assert!(matches!(
            variant_verdict(&e, strict, &[], &[]),
            Verdict::Same
        ));
        assert!(matches!(
            variant_verdict(&e, strict, &[diff("step 1 (x) stdout")], &[]),
            Verdict::Diverged { id: None, .. }
        ));
        assert_eq!(
            Variant {
                brand: Brand::Snp,
                strict: true
            }
            .label(),
            "snp+strict"
        );
    }

    #[test]
    fn combine_takes_the_worst() {
        let mk = |verdict| VariantReport {
            variant: PLAIN,
            differences: Vec::new(),
            expectation_failures: Vec::new(),
            stripped_diag: Vec::new(),
            verdict,
            oracle: SideReport::default(),
            snp: SideReport::default(),
        };
        let v = combine(&[
            mk(Verdict::Same),
            mk(Verdict::StaleDivergence(String::from("D-X-01"))),
            mk(Verdict::Same),
        ]);
        assert!(matches!(v, Verdict::StaleDivergence(_)));
        assert!(matches!(combine(&[]), Verdict::Same));
    }

    #[test]
    fn skip_reasons() {
        let opts = Options {
            probe: None,
            python: None,
            brand_map: BrandMap::identity(),
            allow_net: false,
            taskset: None,
            artifacts: None,
            brands: Vec::new(),
            parent: Env::capture_process(),
            git_version: Some([2, 43, 0]),
            self_check: false,
            keep: false,
        };
        let mut s = Scenario::new("x");
        assert_eq!(skip_reason(&s, &opts), None);
        s.status = Status::Stub;
        assert!(skip_reason(&s, &opts).is_some());
        s.status = Status::Active;
        s.requires.net = true;
        assert!(skip_reason(&s, &opts).unwrap().contains("network"));
        s.requires.net = false;
        s.requires.git = crate::scenario::VersionReq::parse("<2.19").unwrap();
        assert!(skip_reason(&s, &opts).is_some());
        s.requires.git = crate::scenario::VersionReq::default();
        s.requires.probe = true;
        assert!(skip_reason(&s, &opts).unwrap().contains("probe"));
        s.requires.probe = false;
        s.since = Some(String::from("5.0.0"));
        assert!(skip_reason(&s, &opts).is_some());
        assert_eq!(safe_name("divergences/D-X: y"), "divergences_D-X__y");
        for bad in ["", ".", "..", ".hidden"] {
            let n = safe_name(bad);
            assert!(n.starts_with('_') && !n.ends_with('.'), "{bad:?} -> {n:?}");
        }
    }

    #[test]
    fn artifact_dir_never_removes_foreign_directories() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("precious.txt"), b"x").unwrap();
        let out = d.path().join("out");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("keep.txt"), b"x").unwrap();
        // `..` and `.` once named --out's parent and --out itself.
        for name in ["..", "."] {
            let base = artifact_dir(&out, name).unwrap();
            assert_eq!(base.parent(), Some(out.as_path()));
            assert!(base.join(ARTIFACT_MARKER).is_file());
        }
        assert!(d.path().join("precious.txt").is_file());
        assert!(out.join("keep.txt").is_file());
        // A re-run replaces its own artifact directory…
        let base = artifact_dir(&out, "s").unwrap();
        std::fs::write(base.join("old"), b"x").unwrap();
        let base = artifact_dir(&out, "s").unwrap();
        assert!(!base.join("old").exists());
        // …but never one the harness did not write.
        std::fs::create_dir_all(out.join("mine")).unwrap();
        std::fs::write(out.join("mine/data"), b"x").unwrap();
        assert!(artifact_dir(&out, "mine").is_err());
        assert!(out.join("mine/data").is_file());
    }
}
