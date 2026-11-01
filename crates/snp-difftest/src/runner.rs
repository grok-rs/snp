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
use crate::snapshot::{self, State, StoreKind, Wanted};
use crate::text::{fnv64, replace_bytes, show};

/// `oracle_snp_prog` = the pc462 oracle launched with argparse prog patched to `snp` (ADR-058), used for snp-brand
/// runs.
#[derive(Debug)]
pub struct Runner {
    /// The snp binary under test.
    pub snp_bin: PathBuf,
    /// `target/oracle/pc462-py312/bin/pre-commit`.
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
            Self::Diverged { id, diff } => Self::Diverged { id: id.clone(), diff: diff.clone() },
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
        let probe = r.snp_bin.parent().map(|d| d.join(format!("snp-probe{exe}"))).filter(|p| p.is_file());
        let python = r
            .oracle_bin
            .parent()
            .map(|d| d.join(if cfg!(windows) { "python.exe" } else { "python" }))
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
    let n = std::thread::available_parallelism().map_or(1, std::num::NonZero::get).min(4);
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
        if self.strict { format!("{}+strict", self.brand.name()) } else { self.brand.name().to_owned() }
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
}

impl Outcome {
    /// Whether the scenario passes: skipped, identical, or diverging exactly as registered.
    pub fn passed(&self) -> bool {
        self.error.is_none() && (self.skipped.is_some() || self.verdict.is_pass(self.expect_divergence.as_deref()))
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
        let _ = writeln!(o, "{}: {status}", self.name);
        for v in &self.variants {
            if v.verdict.is_pass(self.expect_divergence.as_deref()) && !matches!(v.verdict, Verdict::Same) {
                continue;
            }
            if matches!(v.verdict, Verdict::Same) && self.expect_divergence.is_none() {
                continue;
            }
            let _ = writeln!(o, "  [{}] {}", v.variant.label(), v.verdict.label());
            for d in &v.differences {
                let _ = writeln!(o, "  --- {} (oracle -, snp +)", d.what);
                for line in d.diff.lines() {
                    let _ = writeln!(o, "    {line}");
                }
            }
            for f in &v.expectation_failures {
                let _ = writeln!(o, "  expectation failed: {f}");
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
            Err(e) => Verdict::Diverged { id: None, diff: format!("harness error: {e}") },
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
        };
        if out.skipped.is_some() {
            return out;
        }
        let root = match self.scenario_root(s) {
            Ok(r) => r,
            Err(e) => return errored(out, e),
        };
        let brands: Vec<Brand> =
            s.brands.iter().copied().filter(|b| opts.brands.is_empty() || opts.brands.contains(b)).collect();
        if brands.is_empty() {
            out.skipped = Some(String::from("no selected brand"));
            return out;
        }
        for brand in brands {
            let oracle = match self.run_side(s, &root, brand, Side::Oracle, false, opts) {
                Ok(r) => r,
                Err(e) => return errored(out, format!("[{}] oracle: {e}", brand.name())),
            };
            let stricts: &[bool] = if s.strict_variants && !opts.self_check { &[false, true] } else { &[false] };
            for &strict in stricts {
                let snp = match self.run_side(s, &root, brand, Side::Snp, strict, opts) {
                    Ok(r) => r,
                    Err(e) => {
                        return errored(out, format!("[{}] snp: {e}", Variant { brand, strict }.label()));
                    }
                };
                out.variants.push(judge(s, Variant { brand, strict }, &oracle, snp, &root, opts));
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
        std::fs::create_dir_all(&self.root).map_err(|e| format!("creating {}: {e}", self.root.display()))?;
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
        w.vars.push((String::from("tool"), tool.to_string_lossy().into_owned()));
        let work = w.layout.work.clone();
        let want = Wanted {
            tree: s.compare.tree || needs_tree(s),
            index: s.compare.index,
            refs: s.compare.refs,
            hooks: s.compare.hooks_dir,
            git_config: true,
            store: s.compare.store,
        };
        let store_kind = if side == Side::Snp && !opts.self_check { StoreKind::Snp } else { StoreKind::PreCommit };
        let mut report =
            SideReport { steps: Vec::new(), initial_git_config: std::fs::read(work.join(".git/config")).ok() };
        for step in &s.steps {
            let inv = step_invocation(&mut w, step, &tool, side == Side::Snp && strict, opts)?;
            let captured = exec::run(&inv)?;
            let state = snapshot::capture(&mut w, want, store_kind, opts.python.as_deref())?;
            report.steps.push(StepReport { captured, state });
        }
        Ok(SideRun { report, world: w })
    }
}

fn errored(mut out: Outcome, e: String) -> Outcome {
    out.verdict = Verdict::Diverged { id: None, diff: format!("harness error: {e}") };
    out.error = Some(e);
    out
}

/// Whether an `expected:` check needs the tree snapshot although `compare.tree` is off.
fn needs_tree(s: &Scenario) -> bool {
    s.expected.as_ref().is_some_and(|e| {
        e.oracle.iter().chain(&e.snp).any(|(_, se)| se.checks.iter().any(|(k, _)| k.starts_with("tree")))
    })
}

/// Why `s` cannot run here, if it cannot.
fn skip_reason(s: &Scenario, opts: &Options) -> Option<String> {
    match s.status {
        Status::Stub => return Some(String::from("stub divergence file")),
        Status::Unit => return Some(format!("covered by unit tests: {}", s.unit_tests.join(", "))),
        Status::Active => {}
    }
    if !s.unsupported.is_empty() {
        return Some(format!("unsupported: {}", s.unsupported.join("; ")));
    }
    if let Some(since) = &s.since
        && crate::scenario::parse_version(since).is_some_and(|v| v > [4, 6, 2])
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
    if !r.os.is_empty() && !r.os.iter().any(|o| o == os || (o == "posix" && !cfg!(windows))) {
        return Some(format!("os {os} not in {:?}", r.os));
    }
    if !r.git.terms.is_empty() {
        match opts.git_version {
            Some(v) if r.git.matches(v) => {}
            Some(v) => return Some(format!("git {}.{}.{} does not satisfy the requirement", v[0], v[1], v[2])),
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
    if let Some(t) = r.tools.iter().find(|t| exec::which(t, &opts.parent).is_none()) {
        return Some(format!("tool {t:?} not on PATH"));
    }
    if let Some(want_root) = r.root
        && is_root() != want_root
    {
        return Some(String::from(if want_root { "needs root" } else { "must not run as root" }));
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
    #[cfg(unix)]
    {
        let link = w.layout.bin.join(name);
        std::os::unix::fs::symlink(program, &link).map_err(|e| format!("linking {}: {e}", link.display()))?;
        Ok(link)
    }
    #[cfg(not(unix))]
    {
        // A pip console-script launcher only works from its own Scripts directory: the oracle runs in place.
        if program.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")) && program.ends_with("snp.exe") {
            let copy = w.layout.bin.join(format!("{name}.exe"));
            std::fs::copy(program, &copy).map_err(|e| format!("copying {}: {e}", program.display()))?;
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
            let mut a: Vec<std::ffi::OsString> = vec!["-c".into(), cpus.into(), program.into_os_string()];
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
            exec::signal_number(&sig.name).ok_or_else(|| format!("unknown signal {:?}", sig.name))?,
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
        timeout: step.timeout_ms.map_or(exec::DEFAULT_TIMEOUT, Duration::from_millis),
        signal,
    })
}

fn subst_all(w: &World, a: &[String]) -> Result<Vec<std::ffi::OsString>, String> {
    a.iter().map(|x| w.subst(x).map(Into::into)).collect()
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

/// The normalisation of one stream for one side.
fn norm_stream(mode: StreamMode, bytes: &[u8], map: &BrandMap, stores: &[&[u8]], eol: bool) -> Vec<u8> {
    let tier = match mode {
        StreamMode::Raw | StreamMode::Ignore => Tier::A,
        StreamMode::Exact | StreamMode::LinesUnordered => Tier::C,
        StreamMode::TierB => Tier::B,
    };
    let mut b = normalise::normalise(tier, bytes, map);
    if !matches!(mode, StreamMode::Raw) {
        b = normalise::mask_paths(&b, stores);
    }
    if eol {
        b = replace_bytes(&b, b"\r\n", b"\n");
    }
    b
}

fn stream_diff(
    out: &mut Vec<Difference>,
    what: String,
    mode: StreamMode,
    (a, b): (&[u8], &[u8]),
) {
    if !compare::streams_equal(mode, a, b) {
        let (x, y) = if matches!(mode, StreamMode::LinesUnordered) { (sorted_lines(a), sorted_lines(b)) } else { (a.to_vec(), b.to_vec()) };
        out.push(Difference { what, diff: compare::unified(&x, &y) });
    }
}

fn sorted_lines(b: &[u8]) -> Vec<u8> {
    let mut l = crate::text::lines_with_ends(b);
    l.sort_unstable();
    l.concat()
}

/// Compares one variant and evaluates its expectations.
fn judge(s: &Scenario, variant: Variant, oracle: &SideRun, snp: SideRun, root: &Path, opts: &Options) -> VariantReport {
    let c: &Compare = &s.compare;
    let store_a = root.join("store");
    let store_b = root.join("snp-store");
    let stores: [&[u8]; 2] = [store_a.as_os_str().as_encoded_bytes(), store_b.as_os_str().as_encoded_bytes()];
    let identity = BrandMap::identity();
    let (map_oracle, map_snp) = match (variant.brand, opts.self_check) {
        (Brand::Compat, _) => (&identity, &identity),
        (Brand::Snp, false) => (&opts.brand_map, &identity),
        (Brand::Snp, true) => (&opts.brand_map, &opts.brand_map),
    };
    let keep_diag = s.expect_divergence.as_deref().filter(|_| s.is_diag());
    let mut diffs = Vec::new();
    let mut stripped: Vec<&'static str> = Vec::new();
    for (i, step) in s.steps.iter().enumerate() {
        let n = i.saturating_add(1);
        let label = format!("step {n} ({})", describe(step));
        let (Some(a), Some(b)) = (oracle.report.steps.get(i), snp.report.steps.get(i)) else { continue };
        let (ca, cb) = (&a.captured, &b.captured);
        if ca.timed_out != cb.timed_out {
            diffs.push(Difference {
                what: format!("{label} timeout"),
                diff: format!("-timed out: {}\n+timed out: {}\n", ca.timed_out, cb.timed_out),
            });
        }
        if !compare::exits_equal(c.exit, ca.exit, cb.exit) {
            diffs.push(Difference { what: format!("{label} exit"), diff: format!("-{:?}\n+{:?}\n", ca.exit, cb.exit) });
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
        let eol = c.eol_insensitive;
        stream_diff(
            &mut diffs,
            format!("{label} stdout"),
            c.stdout,
            (&norm_stream(c.stdout, &ca.stdout, map_oracle, &stores, eol), &norm_stream(c.stdout, &snp_out, map_snp, &stores, eol)),
        );
        stream_diff(
            &mut diffs,
            format!("{label} stderr"),
            c.stderr,
            (&norm_stream(c.stderr, &ca.stderr, map_oracle, &stores, eol), &norm_stream(c.stderr, &snp_err, map_snp, &stores, eol)),
        );
        state_diffs(&mut diffs, &label, c, &a.state, &b.state);
    }
    stripped.sort_unstable();
    stripped.dedup();
    let mut failures = Vec::new();
    if let Some(e) = &s.expected {
        let snp_map = (variant.brand == Brand::Snp && !opts.self_check).then_some(&opts.brand_map);
        for (side_name, checks, run, map) in
            [("oracle", &e.oracle, oracle, None), ("snp", &e.snp, &snp, snp_map)]
        {
            for (n, se) in checks {
                let Some(st) = n.checked_sub(1).and_then(|i| run.report.steps.get(i)) else {
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
    let verdict = variant_verdict(s, variant, &diffs, &failures);
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

fn state_diffs(diffs: &mut Vec<Difference>, label: &str, c: &Compare, a: &State, b: &State) {
    let eolfix = |v: &[u8]| if c.eol_insensitive { replace_bytes(v, b"\r\n", b"\n") } else { v.to_vec() };
    if c.tree && a.tree != b.tree {
        let (x, y) = (snapshot::render_tree(&a.tree), snapshot::render_tree(&b.tree));
        if !c.eol_insensitive || eolfix(&x) != eolfix(&y) {
            diffs.push(Difference { what: format!("{label} tree"), diff: compare::unified(&x, &y) });
        }
    }
    if c.index && a.index != b.index {
        diffs.push(Difference { what: format!("{label} index"), diff: compare::unified(&a.index, &b.index) });
    }
    if c.refs && a.refs != b.refs {
        diffs.push(Difference { what: format!("{label} refs"), diff: compare::unified(&a.refs, &b.refs) });
    }
    if c.hooks_dir && a.hooks != b.hooks {
        diffs.push(Difference { what: format!("{label} hooks"), diff: format!("-{:?}\n+{:?}\n", a.hooks, b.hooks) });
    }
    if !matches!(c.config_file, StreamMode::Ignore) {
        let x = eolfix(a.config_file.as_deref().unwrap_or(b"<absent>"));
        let y = eolfix(b.config_file.as_deref().unwrap_or(b"<absent>"));
        if !compare::streams_equal(c.config_file, &x, &y) {
            diffs.push(Difference { what: format!("{label} config file"), diff: compare::unified(&x, &y) });
        }
    }
    if c.git_config && a.git_config != b.git_config {
        let (x, y) = (a.git_config.clone().unwrap_or_default(), b.git_config.clone().unwrap_or_default());
        diffs.push(Difference { what: format!("{label} .git/config"), diff: compare::unified(&x, &y) });
    }
    if c.store && a.store != b.store {
        let x = a.store.join("\n");
        let y = b.store.join("\n");
        diffs.push(Difference { what: format!("{label} store"), diff: compare::unified(x.as_bytes(), y.as_bytes()) });
    }
}

/// Whether a difference's channel is stderr (DIAG may differ there only).
fn is_stderr(d: &Difference) -> bool {
    d.what.ends_with(" stderr")
}

fn render_diffs(diffs: &[Difference], failures: &[String]) -> String {
    let mut o = String::new();
    for d in diffs {
        let _ = writeln!(o, "--- {}", d.what);
        o.push_str(&d.diff);
    }
    for f in failures {
        let _ = writeln!(o, "expectation failed: {f}");
    }
    o
}

fn variant_verdict(s: &Scenario, variant: Variant, diffs: &[Difference], failures: &[String]) -> Verdict {
    let id = s.expect_divergence.as_deref();
    // The strict variant of an extension scenario must equal pre-commit: SNP_COMPAT_STRICT turns the extension off.
    let id = if variant.strict && id.is_some_and(|i| i.starts_with("D-EXT-")) { None } else { id };
    let rendered = render_diffs(diffs, failures);
    match id {
        None if diffs.is_empty() && failures.is_empty() => Verdict::Same,
        None => Verdict::Diverged { id: None, diff: rendered },
        Some(id) => {
            if id.starts_with("D-DIAG-") && diffs.iter().any(|d| !is_stderr(d)) {
                let outside: Vec<Difference> = diffs.iter().filter(|d| !is_stderr(d)).cloned().collect();
                return Verdict::Diverged { id: None, diff: render_diffs(&outside, &[]) };
            }
            if diffs.is_empty() || !failures.is_empty() {
                Verdict::StaleDivergence(id.to_owned())
            } else {
                Verdict::Diverged { id: Some(id.to_owned()), diff: rendered }
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
    vs.iter().map(|v| &v.verdict).max_by_key(|v| rank(v)).map_or(Verdict::Same, Verdict::copy)
}

/// A file-system-safe directory name for a scenario.
pub(crate) fn safe_name(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect()
}

fn write_artifacts(dir: &Path, s: &Scenario, out: &Outcome, r: &Runner, root: &Path) -> Result<(), String> {
    let base = dir.join(safe_name(&s.name));
    if base.exists() {
        std::fs::remove_dir_all(&base).map_err(|e| format!("{}: {e}", base.display()))?;
    }
    let w = |p: PathBuf, b: &[u8]| materialise::write_file(&p, b);
    w(base.join("scenario.yaml"), s.to_yaml().as_bytes())?;
    w(base.join("report.txt"), out.report().as_bytes())?;
    for v in &out.variants {
        let vd = base.join(v.variant.label());
        for (side, rep) in [(Side::Oracle, &v.oracle), (Side::Snp, &v.snp)] {
            for (i, st) in rep.steps.iter().enumerate() {
                let p = vd.join(side.name()).join(format!("step-{}", i.saturating_add(1)));
                w(p.join("stdout"), &st.captured.stdout)?;
                w(p.join("stderr"), &st.captured.stderr)?;
                let exit = format!("{:?}{}\n", st.captured.exit, if st.captured.timed_out { " (timed out)" } else { "" });
                w(p.join("exit"), exit.as_bytes())?;
                w(p.join("tree.txt"), &snapshot::render_tree(&st.state.tree))?;
                w(p.join("index.txt"), &st.state.index)?;
                w(p.join("refs.txt"), &st.state.refs)?;
            }
        }
        w(vd.join("diff.txt"), render_diffs(&v.differences, &v.expectation_failures).as_bytes())?;
    }
    let script = crate::shrink::repro_script(s, r, root);
    w(base.join("repro.sh"), script.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(base.join("repro.sh"), std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("chmod repro.sh: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(what: &str) -> Difference {
        Difference { what: what.to_owned(), diff: String::from("-a\n+b\n") }
    }

    fn sc(id: Option<&str>) -> Scenario {
        let mut s = Scenario::new("t");
        s.expect_divergence = id.map(str::to_owned);
        s
    }

    const PLAIN: Variant = Variant { brand: Brand::Compat, strict: false };

    #[test]
    fn verdicts() {
        assert!(matches!(variant_verdict(&sc(None), PLAIN, &[], &[]), Verdict::Same));
        assert!(matches!(variant_verdict(&sc(None), PLAIN, &[diff("step 1 (x) stdout")], &[]), Verdict::Diverged { id: None, .. }));
        let listed = variant_verdict(&sc(Some("D-CRASH-04")), PLAIN, &[diff("step 1 (x) stdout")], &[]);
        assert!(matches!(&listed, Verdict::Diverged { id: Some(i), .. } if i == "D-CRASH-04"));
        assert!(listed.is_pass(Some("D-CRASH-04")));
        assert!(!listed.is_pass(None));
        assert!(matches!(variant_verdict(&sc(Some("D-CRASH-04")), PLAIN, &[], &[]), Verdict::StaleDivergence(_)));
        let failed = variant_verdict(&sc(Some("D-CRASH-04")), PLAIN, &[diff("step 1 (x) stdout")], &[String::from("x")]);
        assert!(matches!(failed, Verdict::StaleDivergence(_)));
    }

    #[test]
    fn diag_may_differ_on_stderr_only() {
        let d = sc(Some("D-DIAG-02"));
        assert!(matches!(variant_verdict(&d, PLAIN, &[diff("step 1 (x) stderr")], &[]), Verdict::Diverged { id: Some(_), .. }));
        assert!(matches!(
            variant_verdict(&d, PLAIN, &[diff("step 1 (x) stderr"), diff("step 1 (x) exit")], &[]),
            Verdict::Diverged { id: None, .. }
        ));
    }

    #[test]
    fn strict_extension_variant_must_be_identical() {
        let e = sc(Some("D-EXT-02"));
        let strict = Variant { brand: Brand::Snp, strict: true };
        assert!(matches!(variant_verdict(&e, strict, &[], &[]), Verdict::Same));
        assert!(matches!(variant_verdict(&e, strict, &[diff("step 1 (x) stdout")], &[]), Verdict::Diverged { id: None, .. }));
        assert!(Variant { brand: Brand::Snp, strict: true }.label() == "snp+strict");
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
        let v = combine(&[mk(Verdict::Same), mk(Verdict::StaleDivergence(String::from("D-X-01"))), mk(Verdict::Same)]);
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
    }
}
