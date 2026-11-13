//! Scenario files: the 10 §7.4.3 schema plus `brands`, `expect_divergence`, `status`, `strict` and the extensions the
//! divergence owners use (`files_bytes`, `expected`, …), parsed into a typed model (ARCHITECTURE §13.2).
//!
//! Parsing is total: a malformed file is an `Err` naming the file and the key, never a panic. Keys the model knows
//! but the harness cannot execute on this host (or at all yet) are recorded in [`Scenario::unsupported`]; the runner
//! skips such scenarios with that reason instead of guessing.
//!
//! Owner: WP-35.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::text::{expand_byte_escapes, yaml_bytes, yaml_quote};
use crate::yamlv::Yv;

/// Whether a scenario file is filled (`status: active`, the default) or still a W0 stub (`status: stub`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Status {
    /// A runnable scenario.
    Active,
    /// A placeholder (`status: stub`): reported by the register tooling, skipped by the runner.
    Stub,
    /// Not reproducible through the CLI; pinned by the unit tests listed in `unit_tests:` (`status: unit`), which the
    /// register tooling checks for existence. Skipped by the runner.
    Unit,
}

/// The tool identity a run uses (COMPATIBILITY §3.1).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Brand {
    /// snp through a `pre-commit`-named symlink, compared with the plain oracle byte for byte.
    Compat,
    /// snp as `snp`, compared with the prog-patched oracle after `brand_map`.
    Snp,
}

impl Brand {
    /// `compat` / `snp`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Compat => "compat",
            Self::Snp => "snp",
        }
    }
}

/// How a byte stream of the two tools is compared (10 §7.4.3 `compare:` values).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum StreamMode {
    /// Byte equality after the Tier-C normalisers (durations, patch names, store roots, random suffixes).
    Exact,
    /// Byte equality without any normaliser (Tier A proper).
    Raw,
    /// The same multiset of lines (Tier-C normalised): N-1 merge-conflict sets, `autoupdate -j>1`.
    LinesUnordered,
    /// Tier B: semantic reductions (tracebacks → last line, usage blocks → error lines, YAML error bodies, …).
    TierB,
    /// Not compared.
    Ignore,
}

/// How exit codes are compared.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExitMode {
    /// Equal codes.
    Exact,
    /// Both zero or both non-zero.
    NonzeroEqual,
    /// Not compared.
    Ignore,
}

/// The `compare:` block.
#[derive(Clone, Debug, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one switch per compared effect, as in the YAML schema"
)]
pub struct Compare {
    /// stdout comparison.
    pub stdout: StreamMode,
    /// stderr comparison.
    pub stderr: StreamMode,
    /// exit-code comparison.
    pub exit: ExitMode,
    /// Worktree bytes, exec bits and symlink targets (`.git` excluded).
    pub tree: bool,
    /// `git ls-files -s`.
    pub index: bool,
    /// `git for-each-ref`, `HEAD` and the stash list.
    pub refs: bool,
    /// `.git/hooks`: compared by name and exec bit (bytes differ by design: `INSTALL_PYTHON` vs `SNP_EXE`).
    pub hooks_dir: bool,
    /// `.pre-commit-config.yaml` bytes after each step.
    pub config_file: StreamMode,
    /// `.git/config` bytes.
    pub git_config: bool,
    /// Logical store effects (the set of `(repo, rev)` the store holds).
    pub store: bool,
    /// Line endings are normalised (`\r\n` → `\n`) before comparing streams and files (14 §A.6.4 `eol: insensitive`).
    pub eol_insensitive: bool,
}

impl Default for Compare {
    fn default() -> Self {
        Self {
            stdout: StreamMode::Exact,
            stderr: StreamMode::Exact,
            exit: ExitMode::Exact,
            tree: true,
            index: true,
            refs: true,
            hooks_dir: false,
            config_file: StreamMode::Exact,
            git_config: false,
            store: false,
            eol_insensitive: false,
        }
    }
}

/// A version constraint on git (`">=2.31"`, `">=2.19, <2.31"`).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct VersionReq {
    /// `(operator, [major, minor, patch])` terms, all of which must hold.
    pub terms: Vec<(String, [u32; 3])>,
}

impl VersionReq {
    /// Parses `">=2.31, <2.43"`.
    pub fn parse(s: &str) -> Result<Self, String> {
        let mut terms = Vec::new();
        for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let op_len = part.bytes().take_while(|c| b"<>=!".contains(c)).count();
            let (op, ver) = part
                .split_at_checked(op_len)
                .ok_or_else(|| format!("bad version term {part:?}"))?;
            let op = if op.is_empty() { "==" } else { op };
            if !matches!(op, ">=" | ">" | "<=" | "<" | "==" | "!=") {
                return Err(format!("bad version operator {op:?}"));
            }
            terms.push((
                op.to_owned(),
                parse_version(ver.trim()).ok_or_else(|| format!("bad version {ver:?}"))?,
            ));
        }
        Ok(Self { terms })
    }

    /// Whether `v` satisfies every term.
    pub fn matches(&self, v: [u32; 3]) -> bool {
        self.terms.iter().all(|(op, t)| match op.as_str() {
            ">=" => v >= *t,
            ">" => v > *t,
            "<=" => v <= *t,
            "<" => v < *t,
            "!=" => v != *t,
            _ => v == *t,
        })
    }
}

/// `2.43.0` / `2.31` / `2.39.5.windows.1` → `[2, 43, 0]`.
pub fn parse_version(s: &str) -> Option<[u32; 3]> {
    let mut out = [0_u32; 3];
    let mut any = false;
    for (slot, part) in out.iter_mut().zip(s.split('.')) {
        let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            break;
        }
        *slot = digits.parse().ok()?;
        any = true;
    }
    any.then_some(out)
}

/// The `requires:` block.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Requires {
    /// git version constraint.
    pub git: VersionReq,
    /// Needs the network (skipped unless the run allows it).
    pub net: bool,
    /// Executables that must be on `PATH`.
    pub tools: Vec<String>,
    /// `linux` / `macos` / `windows`; empty = every OS.
    pub os: Vec<String>,
    /// Needs a pty (posix).
    pub pty: bool,
    /// The snp-probe binary must be available (`${probe}` is used).
    pub probe: bool,
    /// `root: true` (euid 0) / `not_root: true`.
    pub root: Option<bool>,
    /// At least this many logical CPUs.
    pub logical_cpus_min: Option<u32>,
}

/// A post-setup operation on a hook repository (`then:`) or the work repository (`setup:`).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Op {
    /// `git <args>` (dates tick for object-creating commands).
    Git(Vec<String>),
    /// Write a file (relative path) with these bytes.
    Write(String, Vec<u8>),
    /// Remove a file or directory tree (relative path).
    Remove(String),
    /// `sh -c <script>` in the repository (posix shells; Git for Windows' sh on Windows).
    Shell(String),
    /// `git commit -m <msg>` of everything staged (`git add -A` first).
    Commit(String),
    /// `git tag <name>`.
    Tag(String),
}

/// A hook repository to materialise under `<root>/repos/<name>`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RepoSpec {
    /// The repository's key in `repos:`; also the directory name unless `path` is set.
    pub name: String,
    /// A pre-commit `testing/resources` repository recreated by snp-testkit (`script_hooks_repo`, …).
    pub fixture: Option<String>,
    /// Inline files (path → bytes), written after the fixture copy.
    pub files: Vec<(String, Vec<u8>)>,
    /// Files to mark executable.
    pub exec: Vec<String>,
    /// Tags on the first commit.
    pub tags: Vec<String>,
    /// Operations after the first commit.
    pub then: Vec<Op>,
    /// Directory relative to `<root>` (default `repos/<name>`).
    pub path: Option<String>,
    /// An empty bare repository (a push target) instead of a hook repository.
    pub bare: bool,
    /// `git init --object-format=<f>`.
    pub object_format: Option<String>,
    /// `url:` + `rev:`: an externally hosted repository (not materialised; `${repo:x}` / `${rev:x}` are these
    /// values and the scenario needs the network).
    pub external: Option<(String, String)>,
}

/// The `work:` block: the repository both tools run in.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Work {
    /// Files (path → bytes); a path may carry `\xNN` escapes (non-UTF-8 names).
    pub files: Vec<(Vec<u8>, Vec<u8>)>,
    /// Files to `chmod +x` (and `git update-index --chmod=+x` when committed).
    pub exec: Vec<String>,
    /// Symlinks (posix): name → target.
    pub symlinks: Vec<(String, String)>,
    /// Explicit modes (octal) applied right after the commit (before staging): name → mode.
    pub modes: Vec<(String, u32)>,
    /// Modes applied after commit and staging (e.g. a read-only file): name → mode.
    pub chmod: Vec<(String, u32)>,
    /// FIFOs to create (posix).
    pub fifos: Vec<String>,
    /// `.pre-commit-config.yaml` text (`None`: no config file).
    pub config: Option<String>,
    /// Paths committed before the steps (empty: no commit).
    pub commit: Vec<Vec<u8>>,
    /// Paths staged after the commit.
    pub stage: Vec<Vec<u8>>,
    /// Post-staging worktree edits: path → bytes (`None`: delete the file).
    pub unstaged: Vec<(String, Option<Vec<u8>>)>,
    /// Repository-local git config.
    pub git_config: Vec<(String, String)>,
    /// Further setup operations, in order, after everything above.
    pub setup: Vec<Op>,
    /// `false`: the work directory is not a git repository (RUN-57).
    pub git: bool,
    /// `git init --object-format=<f>`.
    pub object_format: Option<String>,
    /// The work directory relative to `<root>` (default `work`).
    pub dir: Option<String>,
}

/// What a step runs.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum StepKind {
    /// The tool under test with these arguments (after the program name).
    Tool(Vec<String>),
    /// `git <args>`: installed hook scripts call back into the side's tool.
    Git(Vec<String>),
    /// `sh -c <script>` with `$PRE_COMMIT` naming the side's tool (both brands).
    Shell(String),
}

/// A signal sent to a running step (posix).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalSpec {
    /// `SIGINT`, `SIGTERM`, `SIGHUP`, `SIGKILL`.
    pub name: String,
    /// Milliseconds after the spawn (or after `after_file` appears).
    pub after_ms: u64,
    /// `after: <file>`: wait until this file (relative to the work directory) exists — a hook's "I am running"
    /// marker — then wait `after_ms` and signal.
    pub after_file: Option<String>,
}

/// One step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    /// What runs.
    pub kind: StepKind,
    /// Environment changes (`None`: unset). `PATH_PREPEND` prepends to `PATH`.
    pub env: Vec<(String, Option<String>)>,
    /// stdin bytes.
    pub stdin: Vec<u8>,
    /// Run under a pty (posix; stdout and stderr then arrive merged on stdout).
    pub tty: bool,
    /// Wall-clock limit in milliseconds (default 120 s); a timed-out step is killed and recorded as such.
    pub timeout_ms: Option<u64>,
    /// Working directory relative to the work directory.
    pub cwd: Option<String>,
    /// A signal to deliver while the step runs.
    pub signal: Option<SignalSpec>,
}

/// One side's expectation for one step (the `expected:` extension of the divergence files).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct StepExpect {
    /// `(key, value)` pairs in file order; evaluated by the runner (`exit`, `stdout`, `stdout_contains`, …).
    pub checks: Vec<(String, Yv)>,
}

/// The `expected:` block: how each side must behave for the divergence to count as reproduced.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Expected {
    /// pre-commit's side: per step (1-based step → expectation).
    pub oracle: Vec<(usize, StepExpect)>,
    /// snp's side.
    pub snp: Vec<(usize, StepExpect)>,
}

/// One differential scenario.
#[derive(Debug, Clone)]
pub struct Scenario {
    /// The file it was loaded from (empty for generated scenarios).
    pub path: PathBuf,
    /// `name:` (unique in the corpus).
    pub name: String,
    /// The oldest pre-commit version the scenario applies to.
    pub since: Option<String>,
    /// `status:`.
    pub status: Status,
    /// The register ID the scenario must keep reproducing (`D-…`).
    pub expect_divergence: Option<String>,
    /// The brands it runs in (default both).
    pub brands: Vec<Brand>,
    /// `strict: both`: also run with `SNP_COMPAT_STRICT=1` on snp's side (extension scenarios, ARCHITECTURE §13.2).
    pub strict_variants: bool,
    /// The catalog rows it ports (`covers: [T10-RUN-50]`).
    pub covers: Vec<String>,
    /// Free-form tags (`seed`, `crash-recipe`, `platform`, `migration`, `fast-path`, …).
    pub tags: Vec<String>,
    /// `requires:`.
    pub requires: Requires,
    /// Hook repositories in file order.
    pub repos: Vec<RepoSpec>,
    /// The work repository.
    pub work: Work,
    /// The steps.
    pub steps: Vec<Step>,
    /// `compare:`.
    pub compare: Compare,
    /// `expected:` (divergence files).
    pub expected: Option<Expected>,
    /// Keys the harness cannot execute; non-empty ⇒ the runner skips the scenario with these reasons.
    pub unsupported: Vec<String>,
    /// `unit_tests:` (`status: unit`): `path::test_name` entries that pin the behaviour instead.
    pub unit_tests: Vec<String>,
    /// `projects: [one/proj, two/proj]`: the work block is materialised in each of these directories (relative to the
    /// root, sharing one store) and every step runs in each, project by project. Expectations select a project with
    /// `project: <dir>`. Empty: one work directory (`work.dir`, default `work`).
    pub projects: Vec<String>,
}

impl Scenario {
    /// A minimal active scenario (used by the generator and in tests).
    pub fn new(name: &str) -> Self {
        Self {
            path: PathBuf::new(),
            name: name.to_owned(),
            since: None,
            status: Status::Active,
            expect_divergence: None,
            brands: vec![Brand::Compat, Brand::Snp],
            strict_variants: false,
            covers: Vec::new(),
            tags: Vec::new(),
            requires: Requires::default(),
            repos: Vec::new(),
            work: Work {
                git: true,
                ..Work::default()
            },
            steps: Vec::new(),
            compare: Compare::default(),
            expected: None,
            unsupported: Vec::new(),
            unit_tests: Vec::new(),
            projects: Vec::new(),
        }
    }

    /// Whether the scenario's register ID is of class DIAG (stderr-only differences).
    pub fn is_diag(&self) -> bool {
        self.expect_divergence
            .as_deref()
            .is_some_and(|id| id.starts_with("D-DIAG-"))
    }

    /// Parses one scenario file's text. `path` is used in messages and for `name` defaults.
    pub fn parse(text: &str, path: &Path) -> Result<Self, String> {
        let shown = path.display().to_string();
        let doc = Yv::parse(text).map_err(|e| format!("{shown}: invalid YAML: {e}"))?;
        parse_doc(&doc, path).map_err(|e| format!("{shown}: {e}"))
    }

    /// Renders the scenario back to YAML in the 10 §7.4.3 schema (shrinker output, generated cases). Re-parsing the
    /// result gives an equal scenario (round-trip tested).
    pub fn to_yaml(&self) -> String {
        emit(self)
    }
}

/// Loads every scenario under `dir` (`tests/differential/scenarios/**`), recursively, sorted by path; stub files
/// are included with [`Status::Stub`]. Duplicate names are an error.
pub fn load_scenarios(dir: &Path) -> Result<Vec<Scenario>, String> {
    let mut files = Vec::new();
    collect_yaml(dir, &mut files)?;
    files.sort();
    let mut out: Vec<Scenario> = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?;
        let s = Scenario::parse(&text, &f)?;
        if let Some(prev) = out.iter().find(|o| o.name == s.name) {
            return Err(format!(
                "{}: duplicate scenario name {:?} (also in {})",
                f.display(),
                s.name,
                prev.path.display()
            ));
        }
        out.push(s);
    }
    Ok(out)
}

fn collect_yaml(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        let p = entry.path();
        let ft = entry
            .file_type()
            .map_err(|e| format!("{}: {e}", p.display()))?;
        if ft.is_dir() {
            collect_yaml(&p, out)?;
        } else if p.extension().is_some_and(|e| e == "yaml" || e == "yml") {
            out.push(p);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// parsing

const ROOT_KEYS: &[&str] = &[
    "name",
    "since",
    "status",
    "expect_divergence",
    "brands",
    "strict",
    "covers",
    "tags",
    "requires",
    "repos",
    "work",
    "steps",
    "compare",
    "expected",
    "description",
    // owner extensions the harness does not execute yet (reported as unsupported)
    "unit_tests",
    "projects",
    "fixtures",
];

fn s(v: &Yv, what: &str) -> Result<String, String> {
    v.to_text()
        .ok_or_else(|| format!("{what}: expected a string, got a {}", v.kind()))
}

fn bool_of(v: &Yv, what: &str) -> Result<bool, String> {
    if v.is_null() {
        return Ok(false);
    }
    v.as_bool()
        .ok_or_else(|| format!("{what}: expected a boolean"))
}

fn str_list(v: &Yv, what: &str) -> Result<Vec<String>, String> {
    match v {
        Yv::Null => Ok(Vec::new()),
        Yv::Seq(items) => items.iter().map(|i| s(i, what)).collect(),
        Yv::Scalar { .. } => Ok(vec![s(v, what)?]),
        _ => Err(format!("{what}: expected a list of strings")),
    }
}

fn map_of<'a>(v: &'a Yv, what: &str) -> Result<&'a [(String, Yv)], String> {
    match v {
        Yv::Null => Ok(&[]),
        Yv::Map(m) => Ok(m),
        _ => Err(format!("{what}: expected a mapping, got a {}", v.kind())),
    }
}

fn bytes_of(v: &Yv, what: &str) -> Result<Vec<u8>, String> {
    v.to_bytes()
        .ok_or_else(|| format!("{what}: expected a string or !!binary"))
}

fn ms_of(v: &Yv, what: &str) -> Result<u64, String> {
    let t = s(v, what)?;
    let secs: f64 = t
        .trim_end_matches('s')
        .parse()
        .map_err(|_| format!("{what}: expected seconds"))?;
    if !(0.0..=86_400.0).contains(&secs) {
        return Err(format!("{what}: out of range"));
    }
    let d = std::time::Duration::try_from_secs_f64(secs)
        .map_err(|_| format!("{what}: out of range"))?;
    u64::try_from(d.as_millis()).map_err(|_| format!("{what}: out of range"))
}

fn mode_of(v: &Yv, what: &str) -> Result<u32, String> {
    let t = s(v, what)?;
    let t = t.trim_start_matches("0o");
    u32::from_str_radix(t, 8).map_err(|_| format!("{what}: expected an octal mode"))
}

fn parse_doc(doc: &Yv, path: &Path) -> Result<Scenario, String> {
    let root = map_of(doc, "scenario")?;
    let default_name = path
        .file_stem()
        .map_or_default(|n| n.to_string_lossy().into_owned());
    let mut sc = Scenario::new(&default_name);
    sc.path = path.to_path_buf();
    for (k, _) in root {
        if !ROOT_KEYS.contains(&k.as_str()) {
            return Err(format!("unknown key {k:?}"));
        }
    }
    if let Some(v) = doc.get("status") {
        sc.status = match s(v, "status")?.as_str() {
            "stub" => Status::Stub,
            "unit" => Status::Unit,
            "active" | "" => Status::Active,
            other => return Err(format!("status: unknown value {other:?}")),
        };
    }
    if let Some(v) = doc.get("expect_divergence") {
        let id = s(v, "expect_divergence")?;
        if !id.is_empty() {
            if !is_register_id(&id) {
                return Err(format!(
                    "expect_divergence: {id:?} is not a register ID (D-<CLASS>-nn)"
                ));
            }
            sc.expect_divergence = Some(id);
        }
    }
    if let Some(v) = doc.get("name") {
        sc.name = s(v, "name")?;
    } else if sc.status != Status::Active {
        sc.name = format!(
            "divergences/{}",
            sc.expect_divergence.clone().unwrap_or(default_name)
        );
    }
    // The name becomes an artifact directory below `--out` (runner::write_artifacts removes and rewrites it).
    if !root_relative(&sc.name) {
        return Err(format!(
            "name: {:?} must be a relative name without `.`/`..` components or backslashes",
            sc.name
        ));
    }
    if let Some(v) = doc.get("unit_tests") {
        sc.unit_tests = str_list(v, "unit_tests")?;
    }
    if sc.status == Status::Unit && sc.unit_tests.is_empty() {
        return Err(String::from(
            "status: unit needs a non-empty `unit_tests:` list",
        ));
    }
    if sc.status != Status::Active {
        return Ok(sc);
    }
    if doc.get("fixtures").is_some() {
        sc.unsupported.push(String::from(
            "`fixtures:` (network fixtures such as a MITM proxy) is not executed by the harness",
        ));
    }
    if let Some(v) = doc.get("projects") {
        sc.projects = str_list(v, "projects")?;
        if sc.projects.iter().any(|p| !root_relative(p)) {
            return Err(String::from(
                "projects: entries are relative paths below the scenario root",
            ));
        }
    }
    if let Some(v) = doc.get("since") {
        sc.since = Some(s(v, "since")?);
    }
    if let Some(v) = doc.get("brands") {
        let mut brands = Vec::new();
        for b in str_list(v, "brands")? {
            let br = match b.as_str() {
                "compat" => Brand::Compat,
                "snp" => Brand::Snp,
                other => return Err(format!("brands: unknown brand {other:?}")),
            };
            if !brands.contains(&br) {
                brands.push(br);
            }
        }
        if brands.is_empty() {
            return Err(String::from("brands: empty"));
        }
        sc.brands = brands;
    }
    if let Some(v) = doc.get("strict") {
        sc.strict_variants = match v.as_bool() {
            Some(b) => b,
            None => s(v, "strict")? == "both",
        };
    }
    if let Some(v) = doc.get("covers") {
        sc.covers = str_list(v, "covers")?;
    }
    if let Some(v) = doc.get("tags") {
        sc.tags = str_list(v, "tags")?;
    }
    if let Some(v) = doc.get("requires") {
        parse_requires(v, &mut sc)?;
    }
    if let Some(v) = doc.get("repos") {
        for (name, spec) in map_of(v, "repos")? {
            sc.repos.push(parse_repo(name, spec, &mut sc.unsupported)?);
        }
    }
    if let Some(v) = doc.get("work") {
        sc.work = parse_work(v, &mut sc.unsupported)?;
    }
    if sc.repos.iter().any(|r| r.external.is_some()) {
        sc.requires.net = true;
    }
    let steps = doc.get("steps").ok_or("missing key \"steps\"")?;
    for (i, st) in steps
        .as_seq()
        .ok_or("steps: expected a list")?
        .iter()
        .enumerate()
    {
        sc.steps
            .push(parse_step(st, i.saturating_add(1), &mut sc.unsupported)?);
    }
    if sc.steps.is_empty() {
        return Err(String::from("steps: empty"));
    }
    if let Some(v) = doc.get("compare") {
        sc.compare = parse_compare(v, &mut sc.unsupported)?;
    }
    if let Some(v) = doc.get("expected") {
        sc.expected = Some(parse_expected(v, sc.steps.len())?);
    }
    if uses_probe(&sc) {
        sc.requires.probe = true;
    }
    Ok(sc)
}

/// `D-<CLASS>-nn`.
pub(crate) fn is_register_id(id: &str) -> bool {
    let mut parts = id.split('-');
    parts.next() == Some("D")
        && parts
            .next()
            .is_some_and(|c| !c.is_empty() && c.bytes().all(|b| b.is_ascii_uppercase()))
        && parts
            .next()
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        && parts.next().is_none()
}

fn uses_probe(sc: &Scenario) -> bool {
    let mut texts: Vec<&str> = Vec::new();
    if let Some(c) = &sc.work.config {
        texts.push(c);
    }
    for st in &sc.steps {
        match &st.kind {
            StepKind::Tool(a) | StepKind::Git(a) => texts.extend(a.iter().map(String::as_str)),
            StepKind::Shell(c) => texts.push(c),
        }
    }
    texts.iter().any(|t| t.contains("${probe}"))
}

fn parse_requires(v: &Yv, sc: &mut Scenario) -> Result<(), String> {
    for (k, val) in map_of(v, "requires")? {
        let what = format!("requires.{k}");
        match k.as_str() {
            "git" => {
                sc.requires.git =
                    VersionReq::parse(&s(val, &what)?).map_err(|e| format!("{what}: {e}"))?;
            }
            "net" => sc.requires.net = bool_of(val, &what)?,
            "tools" => sc.requires.tools = str_list(val, &what)?,
            "os" => sc.requires.os = str_list(val, &what)?,
            "pty" => sc.requires.pty = bool_of(val, &what)?,
            "probe" => sc.requires.probe = bool_of(val, &what)?,
            "root" => {
                if bool_of(val, &what)? {
                    sc.requires.root = Some(true);
                }
            }
            "not_root" => {
                if bool_of(val, &what)? {
                    sc.requires.root = Some(false);
                }
            }
            "logical_cpus_min" => {
                let n = val
                    .as_i64()
                    .ok_or_else(|| format!("{what}: expected an integer"))?;
                sc.requires.logical_cpus_min =
                    Some(u32::try_from(n).map_err(|_| format!("{what}: out of range"))?);
            }
            "path_profile" => {
                let p = s(val, &what)?;
                if p != "runner" {
                    sc.unsupported
                        .push(format!("requires.path_profile {p:?} (only `runner`)"));
                }
            }
            "sandbox" => sc
                .unsupported
                .push(String::from("requires.sandbox (a sandboxed runner)")),
            // 14 §A.6.1: the console the tool runs under. `pipe` is the default; a posix `pty` is the step-level
            // `tty: true`; a Windows ConPTY needs a console host the harness does not drive yet.
            "console" => match s(val, &what)?.as_str() {
                "pipe" | "none" => {}
                "pty" => sc.requires.pty = true,
                other => sc.unsupported.push(format!(
                    "requires.console {other:?} (only `pipe` and `pty`)"
                )),
            },
            // 14 §A.6.1: how git is started on Windows (`git-cmd`, `git-pwsh`, `git-bash`); the harness spawns git
            // directly, which is the `direct` launcher.
            "launcher" => {
                let l = s(val, &what)?;
                if l != "direct" {
                    sc.unsupported
                        .push(format!("requires.launcher {l:?} (only `direct`)"));
                }
            }
            // A setup the harness cannot build (drive substitution, a held file lock, a benchmark…): the scenario is
            // recorded for the manual / platform matrix and skipped with this reason.
            "manual" => sc
                .unsupported
                .push(format!("requires.manual: {}", s(val, &what)?)),
            other => return Err(format!("requires: unknown key {other:?}")),
        }
    }
    Ok(())
}

fn parse_ops(v: &Yv, what: &str, unsupported: &mut Vec<String>) -> Result<Vec<Op>, String> {
    let mut ops = Vec::new();
    let items: Vec<Yv> = match v {
        Yv::Seq(items) => items.clone(),
        Yv::Map(_) => vec![v.clone()],
        Yv::Null => Vec::new(),
        _ => return Err(format!("{what}: expected a list of operations")),
    };
    for item in &items {
        for (k, val) in map_of(item, what)? {
            let w = format!("{what}.{k}");
            ops.push(match k.as_str() {
                "git" => Op::Git(str_list(val, &w)?),
                "shell" | "sh" => Op::Shell(s(val, &w)?),
                "commit" => Op::Commit(s(val, &w)?),
                "tag" => Op::Tag(s(val, &w)?),
                "remove" => Op::Remove(s(val, &w)?),
                "write" => {
                    let mut pairs = Vec::new();
                    for (p, c) in map_of(val, &w)? {
                        pairs.push(Op::Write(p.clone(), bytes_of(c, &w)?));
                    }
                    ops.extend(pairs);
                    continue;
                }
                other => {
                    unsupported.push(format!("{what}: operation {other:?}"));
                    continue;
                }
            });
        }
    }
    Ok(ops)
}

fn parse_repo(name: &str, v: &Yv, unsupported: &mut Vec<String>) -> Result<RepoSpec, String> {
    let mut r = RepoSpec {
        name: name.to_owned(),
        ..RepoSpec::default()
    };
    for (k, val) in map_of(v, "repos")? {
        let what = format!("repos.{name}.{k}");
        match k.as_str() {
            "fixture" => r.fixture = Some(s(val, &what)?),
            "files" => {
                for (p, c) in map_of(val, &what)? {
                    r.files.push((p.clone(), bytes_of(c, &what)?));
                }
            }
            "exec" => r.exec = str_list(val, &what)?,
            "tags" => r.tags = str_list(val, &what)?,
            "then" => r.then = parse_ops(val, &what, unsupported)?,
            "path" => r.path = Some(root_rel(s(val, &what)?, &what)?),
            "bare" => r.bare = bool_of(val, &what)?,
            "object_format" => r.object_format = Some(s(val, &what)?),
            "url" => {
                let rev = r.external.take().map_or_default(|(_, rev)| rev);
                r.external = Some((s(val, &what)?, rev));
            }
            "rev" => {
                let url = r.external.take().map_or_default(|(u, _)| u);
                r.external = Some((url, s(val, &what)?));
            }
            other => return Err(format!("{what}: unknown key {other:?}")),
        }
    }
    if let Some((url, rev)) = &r.external
        && (url.is_empty() || rev.is_empty())
    {
        return Err(format!(
            "repos.{name}: an external repository needs both `url` and `rev`"
        ));
    }
    Ok(r)
}

/// Whether `p` is a non-empty relative path that stays below the directory it is joined to: no root, drive or
/// backslash (Windows separators), no `.`/`..` components. Scenario names and root-relative directories (`work.dir`,
/// `repos.*.path`, `projects`) must be.
pub(crate) fn root_relative(p: &str) -> bool {
    !p.is_empty()
        && !p.starts_with('/')
        && !p.contains('\\')
        && !Path::new(p).is_absolute()
        && p.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
        && Path::new(p)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

fn root_rel(p: String, what: &str) -> Result<String, String> {
    if root_relative(&p) {
        Ok(p)
    } else {
        Err(format!(
            "{what}: {p:?} must be a relative path below the scenario root (no `..`, `.` or absolute parts)"
        ))
    }
}

/// Largest single `{generate}` file.
const MAX_GENERATED_FILE: usize = 64 * 1024 * 1024;

/// Largest total of generated work bytes (`{generate}` contents and `generated:` files together) one scenario may
/// materialise. Literal contents are already bounded by the YAML reader's expansion budget.
pub(crate) const MAX_GENERATED_TOTAL: usize = 256 * 1024 * 1024;

/// Charges `n` bytes against the scenario's generated-bytes budget.
fn charge_work(left: &mut usize, n: usize, what: &str) -> Result<(), String> {
    *left = left.checked_sub(n).ok_or_else(|| {
        format!("{what}: generated work files exceed {MAX_GENERATED_TOTAL} bytes in total")
    })?;
    Ok(())
}

/// `{generate: {prefix, repeat, count, suffix}}` file contents: `prefix + repeat × count + suffix`.
fn generated_content(v: &Yv, what: &str, left: &mut usize) -> Result<Vec<u8>, String> {
    let part = |k: &str| -> Result<Vec<u8>, String> {
        v.get(k)
            .map_or(Ok(Vec::new()), |x| bytes_of(x, &format!("{what}.{k}")))
    };
    let count = v
        .get("count")
        .and_then(Yv::as_i64)
        .and_then(|c| usize::try_from(c).ok())
        .filter(|c| *c <= MAX_GENERATED_FILE)
        .ok_or_else(|| format!("{what}: count must be an integer in 0..={MAX_GENERATED_FILE}"))?;
    let unit = part("repeat")?;
    let (prefix, suffix) = (part("prefix")?, part("suffix")?);
    let size = count
        .checked_mul(unit.len())
        .and_then(|n| n.checked_add(prefix.len()))
        .and_then(|n| n.checked_add(suffix.len()))
        .filter(|n| *n <= MAX_GENERATED_FILE)
        .ok_or_else(|| format!("{what}: generated contents out of range (at most 64 MiB)"))?;
    charge_work(left, size, what)?;
    let mut out = Vec::new();
    out.try_reserve_exact(size)
        .map_err(|e| format!("{what}: {e}"))?;
    out.extend_from_slice(&prefix);
    match unit.as_slice() {
        [] => {}
        [b] => out.resize(prefix.len().saturating_add(count), *b),
        _ => {
            for _ in 0..count {
                out.extend_from_slice(&unit);
            }
        }
    }
    out.extend_from_slice(&suffix);
    Ok(out)
}

fn parse_work(v: &Yv, unsupported: &mut Vec<String>) -> Result<Work, String> {
    let mut w = Work {
        git: true,
        ..Work::default()
    };
    let mut left = MAX_GENERATED_TOTAL;
    for (k, val) in map_of(v, "work")? {
        let what = format!("work.{k}");
        match k.as_str() {
            "files" => {
                for (p, c) in map_of(val, &what)? {
                    if p == "generated" && c.get("pattern").is_some() {
                        w.files.extend(generated_files(
                            c,
                            &format!("{what}.generated"),
                            &mut left,
                        )?);
                        continue;
                    }
                    if c.as_map().is_some() {
                        let g = c
                            .get("generate")
                            .ok_or_else(|| format!("{what}.{p}: expected contents"))?;
                        w.files.push((
                            expand_byte_escapes(p),
                            generated_content(g, &format!("{what}.{p}"), &mut left)?,
                        ));
                        continue;
                    }
                    w.files.push((expand_byte_escapes(p), bytes_of(c, &what)?));
                }
            }
            "files_bytes" => {
                for (p, c) in map_of(val, &what)? {
                    w.files.push((expand_byte_escapes(p), bytes_of(c, &what)?));
                }
            }
            "exec" => w.exec = str_list(val, &what)?,
            "symlinks" => {
                for (p, t) in map_of(val, &what)? {
                    w.symlinks.push((p.clone(), s(t, &what)?));
                }
            }
            "modes" => {
                for (p, m) in map_of(val, &what)? {
                    w.modes.push((p.clone(), mode_of(m, &what)?));
                }
            }
            "chmod" => {
                for (p, m) in map_of(val, &what)? {
                    w.chmod.push((p.clone(), mode_of(m, &what)?));
                }
            }
            "fifos" => w.fifos = str_list(val, &what)?,
            "config" => {
                w.config = if val.is_null() {
                    None
                } else {
                    Some(s(val, &what)?)
                }
            }
            "commit" => {
                w.commit = str_list(val, &what)?
                    .iter()
                    .map(|p| expand_byte_escapes(p))
                    .collect();
            }
            "stage" => {
                w.stage = str_list(val, &what)?
                    .iter()
                    .map(|p| expand_byte_escapes(p))
                    .collect();
            }
            "unstaged" => {
                for (p, c) in map_of(val, &what)? {
                    w.unstaged.push((
                        p.clone(),
                        if c.is_null() {
                            None
                        } else {
                            Some(bytes_of(c, &what)?)
                        },
                    ));
                }
            }
            "git_config" => {
                for (key, value) in map_of(val, &what)? {
                    w.git_config.push((key.clone(), s(value, &what)?));
                }
            }
            "setup" => w.setup = parse_ops(val, &what, unsupported)?,
            "git" => {
                w.git = val
                    .as_bool()
                    .ok_or_else(|| format!("{what}: expected a boolean"))?;
            }
            "object_format" => w.object_format = Some(s(val, &what)?),
            "dir" => w.dir = Some(root_rel(s(val, &what)?, &what)?),
            "generated" => w.files.extend(generated_files(val, &what, &mut left)?),
            "junctions" => {
                unsupported.push(String::from("work.junctions (Windows directory junctions)"));
            }
            other => return Err(format!("{what}: unknown key {other:?}")),
        }
    }
    Ok(w)
}

/// A file name and its contents, both raw bytes.
type NamedBytes = (Vec<u8>, Vec<u8>);

/// Upper bound on `generated:` file counts.
const MAX_GENERATED: i64 = 100_000;

/// `generated: {pattern: "a b{i:04}", count: 1000, content: "x\n"}`: `count` files named by `pattern`, where `{i}` is
/// the index and `{i:0N}` the index zero-padded to `N` digits.
fn generated_files(v: &Yv, what: &str, left: &mut usize) -> Result<Vec<NamedBytes>, String> {
    let pattern = s(
        v.get("pattern")
            .ok_or_else(|| format!("{what}: missing pattern"))?,
        what,
    )?;
    let count = v
        .get("count")
        .and_then(Yv::as_i64)
        .ok_or_else(|| format!("{what}: count must be an integer"))?;
    if !(0..=MAX_GENERATED).contains(&count) {
        return Err(format!("{what}: count out of range (0..={MAX_GENERATED})"));
    }
    let content = match v.get("content") {
        Some(c) => bytes_of(c, what)?,
        None => Vec::new(),
    };
    let (head, spec, tail) = {
        let start = pattern
            .find("{i")
            .ok_or_else(|| format!("{what}: pattern needs {{i}} or {{i:0N}}"))?;
        let (head, rest) = pattern.split_at_checked(start).ok_or("bad pattern")?;
        let end = rest
            .find('}')
            .ok_or_else(|| format!("{what}: unterminated {{i"))?;
        let (spec, tail) = rest.split_at_checked(end).ok_or("bad pattern")?;
        (
            head.to_owned(),
            spec.to_owned(),
            tail.get(1..).unwrap_or_default().to_owned(),
        )
    };
    let width: usize = match spec.strip_prefix("{i") {
        Some("") => 0,
        Some(f) => f
            .strip_prefix(":0")
            .and_then(|n| n.parse().ok())
            .filter(|n| *n <= 12)
            .ok_or_else(|| format!("{what}: bad index format {spec:?}"))?,
        None => return Err(format!("{what}: bad pattern")),
    };
    // Names are at most head + 20 digits + tail; charge them too so a huge pattern cannot amplify either.
    let per_file = content
        .len()
        .checked_add(head.len())
        .and_then(|n| n.checked_add(tail.len()))
        .and_then(|n| n.checked_add(width.max(20)))
        .ok_or_else(|| format!("{what}: generated files out of range"))?;
    let all = usize::try_from(count)
        .ok()
        .and_then(|c| c.checked_mul(per_file))
        .ok_or_else(|| format!("{what}: generated files out of range"))?;
    charge_work(left, all, what)?;
    Ok((0..count)
        .map(|i| {
            (
                format!("{head}{i:0width$}{tail}").into_bytes(),
                content.clone(),
            )
        })
        .collect())
}

#[expect(
    clippy::many_single_char_names,
    reason = "schema-walking locals: v(alue), m(ap), k/w (key, what), s (the string helper)"
)]
fn parse_step(v: &Yv, n: usize, unsupported: &mut Vec<String>) -> Result<Step, String> {
    let what = format!("steps[{n}]");
    let m = map_of(v, &what)?;
    let mut kind = None;
    let mut step = Step {
        kind: StepKind::Tool(Vec::new()),
        env: Vec::new(),
        stdin: Vec::new(),
        tty: false,
        timeout_ms: None,
        cwd: None,
        signal: None,
    };
    for (k, val) in m {
        let w = format!("{what}.{k}");
        match k.as_str() {
            "tool" => kind = Some(StepKind::Tool(str_list(val, &w)?)),
            "git" => kind = Some(StepKind::Git(str_list(val, &w)?)),
            "shell" => kind = Some(StepKind::Shell(s(val, &w)?)),
            "env" => {
                for (ek, ev) in map_of(val, &w)? {
                    step.env.push((
                        ek.clone(),
                        if ev.is_null() { None } else { Some(s(ev, &w)?) },
                    ));
                }
            }
            "stdin" => step.stdin = bytes_of(val, &w)?,
            "stdin_bytes" => {
                step.stdin = expand_byte_escapes(&s(val, &w)?);
            }
            "tty" => step.tty = bool_of(val, &w)?,
            "timeout" => step.timeout_ms = Some(ms_of(val, &w)?),
            "cwd" => step.cwd = Some(s(val, &w)?),
            "signal" => {
                let name = s(
                    val.get("name")
                        .ok_or_else(|| format!("{w}: missing name"))?,
                    &w,
                )?;
                let (after_ms, after_file) = match val.get("after") {
                    Some(a) => match ms_of(a, &w) {
                        Ok(ms) => (ms, None),
                        Err(_) => (0, Some(s(a, &w)?)),
                    },
                    None => (0, None),
                };
                if after_file.as_deref().is_some_and(str::is_empty) {
                    return Err(format!("{w}: empty `after`"));
                }
                step.signal = Some(SignalSpec {
                    name,
                    after_ms,
                    after_file,
                });
            }
            "compare" | "comment" => {}
            other => {
                unsupported.push(format!("{w}: unknown step key {other:?}"));
            }
        }
    }
    step.kind = kind.ok_or_else(|| format!("{what}: needs one of tool/git/shell"))?;
    Ok(step)
}

fn stream_mode(v: &Yv, what: &str) -> Result<StreamMode, String> {
    if let Some(b) = v.as_bool() {
        return Ok(if b {
            StreamMode::Exact
        } else {
            StreamMode::Ignore
        });
    }
    match s(v, what)?.as_str() {
        "exact" => Ok(StreamMode::Exact),
        "raw" => Ok(StreamMode::Raw),
        "lines-unordered" => Ok(StreamMode::LinesUnordered),
        "tierB" | "tierb" | "tier-b" => Ok(StreamMode::TierB),
        "ignore" => Ok(StreamMode::Ignore),
        other => Err(format!("{what}: unknown compare mode {other:?}")),
    }
}

fn parse_compare(v: &Yv, unsupported: &mut Vec<String>) -> Result<Compare, String> {
    let mut c = Compare::default();
    for (k, val) in map_of(v, "compare")? {
        let what = format!("compare.{k}");
        match k.as_str() {
            "stdout" => c.stdout = stream_mode(val, &what)?,
            "stderr" => c.stderr = stream_mode(val, &what)?,
            "exit" => {
                c.exit = match val.as_bool() {
                    Some(true) => ExitMode::Exact,
                    Some(false) => ExitMode::Ignore,
                    None => match s(val, &what)?.as_str() {
                        "exact" => ExitMode::Exact,
                        "nonzero-equal" => ExitMode::NonzeroEqual,
                        "ignore" => ExitMode::Ignore,
                        other => return Err(format!("{what}: unknown exit mode {other:?}")),
                    },
                };
            }
            "tree" => c.tree = bool_of(val, &what)?,
            "index" => c.index = bool_of(val, &what)?,
            "refs" => c.refs = bool_of(val, &what)?,
            "hooks_dir" => {
                c.hooks_dir = match val.as_bool() {
                    Some(b) => b,
                    None => match s(val, &what)?.as_str() {
                        "behavior" | "behaviour" => true,
                        "ignore" => false,
                        other => return Err(format!("{what}: unknown value {other:?}")),
                    },
                };
            }
            "config_file" => c.config_file = stream_mode(val, &what)?,
            "git_config" => {
                c.git_config = bool_of(val, &what)
                    .or_else(|_| Ok::<bool, String>(s(val, &what)? == "exact"))?;
            }
            "store" => {
                c.store = match val.as_bool() {
                    Some(b) => b,
                    None => match s(val, &what)?.as_str() {
                        "logical" => true,
                        "ignore" => false,
                        other => return Err(format!("{what}: unknown value {other:?}")),
                    },
                };
            }
            // `windows`: insensitive on Windows only, where pre-commit's text-mode writes end lines with `\r\n`
            // and snp keeps each line's original ending (D-DATA-09)
            "eol" => {
                c.eol_insensitive = match s(val, &what)?.as_str() {
                    "insensitive" => true,
                    "windows" => cfg!(windows),
                    _ => false,
                }
            }
            "winerror_text" => {}
            other => unsupported.push(format!("{what}: unknown compare key {other:?}")),
        }
    }
    Ok(c)
}

fn parse_side(v: &Yv, steps: usize, what: &str) -> Result<Vec<(usize, StepExpect)>, String> {
    let mut out = Vec::new();
    match v {
        Yv::Seq(items) => {
            for (i, item) in items.iter().enumerate() {
                out.push((i.saturating_add(1), side_checks(item, what)?));
            }
        }
        Yv::Map(m) => {
            let mut last = StepExpect::default();
            for (k, val) in m {
                if k == "steps" {
                    for (n, sv) in map_of(val, what)? {
                        let idx: usize = n
                            .parse()
                            .map_err(|_| format!("{what}.steps: bad step number {n:?}"))?;
                        out.push((idx, side_checks(sv, what)?));
                    }
                } else if let Some(n) = k.strip_prefix("exit_of_step_") {
                    let idx: usize = n.parse().map_err(|_| format!("{what}: bad key {k:?}"))?;
                    out.push((
                        idx,
                        StepExpect {
                            checks: vec![(String::from("exit"), val.clone())],
                        },
                    ));
                } else {
                    last.checks.push((k.clone(), val.clone()));
                }
            }
            if !last.checks.is_empty() {
                out.push((steps, last));
            }
        }
        Yv::Null => {}
        _ => return Err(format!("{what}: expected a mapping or a list")),
    }
    Ok(out)
}

fn side_checks(v: &Yv, what: &str) -> Result<StepExpect, String> {
    Ok(StepExpect {
        checks: map_of(v, what)?.to_vec(),
    })
}

fn parse_expected(v: &Yv, steps: usize) -> Result<Expected, String> {
    let mut e = Expected::default();
    for (k, val) in map_of(v, "expected")? {
        match k.as_str() {
            "oracle" => e.oracle = parse_side(val, steps, "expected.oracle")?,
            "snp" => e.snp = parse_side(val, steps, "expected.snp")?,
            other => return Err(format!("expected: unknown side {other:?}")),
        }
    }
    Ok(e)
}

// ---------------------------------------------------------------------------------------------------------------
// emitting

fn emit_list(out: &mut String, items: &[String]) {
    out.push('[');
    for (i, it) in items.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&yaml_quote(it));
    }
    out.push(']');
}

fn byte_name(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) if !s.contains('\\') => s.to_owned(),
        _ => {
            let mut out = String::new();
            for &c in b {
                if c.is_ascii_graphic() && c != b'\\' || c == b' ' {
                    out.push(char::from(c));
                } else {
                    write!(out, "\\x{c:02x}").unwrap_or_default();
                }
            }
            out
        }
    }
}

fn emit_ops(out: &mut String, indent: &str, ops: &[Op]) {
    for op in ops {
        match op {
            Op::Git(a) => {
                write!(out, "{indent}- git: ").unwrap_or_default();
                emit_list(out, a);
                out.push('\n');
            }
            Op::Write(p, b) => {
                writeln!(
                    out,
                    "{indent}- write: {{{}: {}}}",
                    yaml_quote(p),
                    yaml_bytes(b)
                )
                .unwrap_or_default();
            }
            Op::Remove(p) => {
                writeln!(out, "{indent}- remove: {}", yaml_quote(p)).unwrap_or_default();
            }
            Op::Shell(c) => {
                writeln!(out, "{indent}- shell: {}", yaml_quote(c)).unwrap_or_default();
            }
            Op::Commit(m) => {
                writeln!(out, "{indent}- commit: {}", yaml_quote(m)).unwrap_or_default();
            }
            Op::Tag(t) => {
                writeln!(out, "{indent}- tag: {}", yaml_quote(t)).unwrap_or_default();
            }
        }
    }
}

const fn mode_name(m: StreamMode) -> &'static str {
    match m {
        StreamMode::Exact => "exact",
        StreamMode::Raw => "raw",
        StreamMode::LinesUnordered => "lines-unordered",
        StreamMode::TierB => "tierB",
        StreamMode::Ignore => "ignore",
    }
}

fn emit(sc: &Scenario) -> String {
    let mut o = String::new();
    writeln!(o, "name: {}", yaml_quote(&sc.name)).unwrap_or_default();
    if let Some(v) = &sc.since {
        writeln!(o, "since: {}", yaml_quote(v)).unwrap_or_default();
    }
    match sc.status {
        Status::Stub => o.push_str("status: stub\n"),
        Status::Unit => o.push_str("status: unit\n"),
        Status::Active => {}
    }
    if let Some(id) = &sc.expect_divergence {
        writeln!(o, "expect_divergence: {id}").unwrap_or_default();
    }
    let brands: Vec<String> = sc.brands.iter().map(|b| b.name().to_owned()).collect();
    o.push_str("brands: ");
    emit_list(&mut o, &brands);
    o.push('\n');
    if sc.strict_variants {
        o.push_str("strict: both\n");
    }
    if !sc.covers.is_empty() {
        o.push_str("covers: ");
        emit_list(&mut o, &sc.covers);
        o.push('\n');
    }
    if !sc.tags.is_empty() {
        o.push_str("tags: ");
        emit_list(&mut o, &sc.tags);
        o.push('\n');
    }
    o.push_str("requires:\n");
    let git: Vec<String> = sc
        .requires
        .git
        .terms
        .iter()
        .map(|(op, v)| {
            format!(
                "{op}{}.{}.{}",
                v.first().unwrap_or(&0),
                v.get(1).unwrap_or(&0),
                v.get(2).unwrap_or(&0)
            )
        })
        .collect();
    writeln!(o, "  git: {}", yaml_quote(&git.join(", "))).unwrap_or_default();
    writeln!(o, "  net: {}", sc.requires.net).unwrap_or_default();
    o.push_str("  tools: ");
    emit_list(&mut o, &sc.requires.tools);
    o.push_str("\n  os: ");
    emit_list(&mut o, &sc.requires.os);
    writeln!(o, "\n  pty: {}", sc.requires.pty).unwrap_or_default();
    if sc.requires.probe {
        o.push_str("  probe: true\n");
    }
    match sc.requires.root {
        Some(true) => o.push_str("  root: true\n"),
        Some(false) => o.push_str("  not_root: true\n"),
        None => {}
    }
    if let Some(n) = sc.requires.logical_cpus_min {
        writeln!(o, "  logical_cpus_min: {n}").unwrap_or_default();
    }
    if sc.repos.is_empty() {
        o.push_str("repos: {}\n");
    } else {
        o.push_str("repos:\n");
    }
    for r in &sc.repos {
        writeln!(o, "  {}:", yaml_quote(&r.name)).unwrap_or_default();
        if let Some(f) = &r.fixture {
            writeln!(o, "    fixture: {}", yaml_quote(f)).unwrap_or_default();
        }
        if !r.files.is_empty() {
            o.push_str("    files:\n");
            for (p, b) in &r.files {
                writeln!(o, "      {}: {}", yaml_quote(p), yaml_bytes(b)).unwrap_or_default();
            }
        }
        if !r.exec.is_empty() {
            o.push_str("    exec: ");
            emit_list(&mut o, &r.exec);
            o.push('\n');
        }
        if !r.tags.is_empty() {
            o.push_str("    tags: ");
            emit_list(&mut o, &r.tags);
            o.push('\n');
        }
        if !r.then.is_empty() {
            o.push_str("    then:\n");
            emit_ops(&mut o, "      ", &r.then);
        }
        if let Some(p) = &r.path {
            writeln!(o, "    path: {}", yaml_quote(p)).unwrap_or_default();
        }
        if r.bare {
            o.push_str("    bare: true\n");
        }
        if let Some(f) = &r.object_format {
            writeln!(o, "    object_format: {}", yaml_quote(f)).unwrap_or_default();
        }
        if let Some((url, rev)) = &r.external {
            writeln!(
                o,
                "    url: {}\n    rev: {}",
                yaml_quote(url),
                yaml_quote(rev)
            )
            .unwrap_or_default();
            continue;
        }
        if r.fixture.is_none()
            && r.files.is_empty()
            && !r.bare
            && r.tags.is_empty()
            && r.then.is_empty()
        {
            o.push_str("    files: {}\n");
        }
    }
    if !sc.projects.is_empty() {
        o.push_str("projects: ");
        emit_list(&mut o, &sc.projects);
        o.push('\n');
    }
    let w = &sc.work;
    o.push_str("work:\n");
    if !w.git {
        o.push_str("  git: false\n");
    }
    if let Some(d) = &w.dir {
        writeln!(o, "  dir: {}", yaml_quote(d)).unwrap_or_default();
    }
    if let Some(f) = &w.object_format {
        writeln!(o, "  object_format: {}", yaml_quote(f)).unwrap_or_default();
    }
    let (plain, bytes): (Vec<_>, Vec<_>) = w
        .files
        .iter()
        .partition(|(p, _)| std::str::from_utf8(p).is_ok_and(|s| !s.contains('\\')));
    if plain.is_empty() {
        o.push_str("  files: {}\n");
    } else {
        o.push_str("  files:\n");
        for (p, b) in plain {
            writeln!(o, "    {}: {}", yaml_quote(&byte_name(p)), yaml_bytes(b)).unwrap_or_default();
        }
    }
    if !bytes.is_empty() {
        o.push_str("  files_bytes:\n");
        for (p, b) in bytes {
            writeln!(o, "    {}: {}", yaml_quote(&byte_name(p)), yaml_bytes(b)).unwrap_or_default();
        }
    }
    if !w.exec.is_empty() {
        o.push_str("  exec: ");
        emit_list(&mut o, &w.exec);
        o.push('\n');
    }
    for (key, pairs) in [("symlinks", &w.symlinks)] {
        if !pairs.is_empty() {
            writeln!(o, "  {key}:").unwrap_or_default();
            for (p, t) in pairs {
                writeln!(o, "    {}: {}", yaml_quote(p), yaml_quote(t)).unwrap_or_default();
            }
        }
    }
    for (key, pairs) in [("modes", &w.modes), ("chmod", &w.chmod)] {
        if !pairs.is_empty() {
            writeln!(o, "  {key}:").unwrap_or_default();
            for (p, m) in pairs {
                writeln!(o, "    {}: \"{m:o}\"", yaml_quote(p)).unwrap_or_default();
            }
        }
    }
    if !w.fifos.is_empty() {
        o.push_str("  fifos: ");
        emit_list(&mut o, &w.fifos);
        o.push('\n');
    }
    if let Some(c) = &w.config {
        writeln!(o, "  config: {}", yaml_quote(c)).unwrap_or_default();
    }
    let names = |v: &[Vec<u8>]| v.iter().map(|p| byte_name(p)).collect::<Vec<_>>();
    o.push_str("  commit: ");
    emit_list(&mut o, &names(&w.commit));
    o.push_str("\n  stage: ");
    emit_list(&mut o, &names(&w.stage));
    o.push('\n');
    if !w.unstaged.is_empty() {
        o.push_str("  unstaged:\n");
        for (p, b) in &w.unstaged {
            writeln!(
                o,
                "    {}: {}",
                yaml_quote(p),
                b.as_ref()
                    .map_or_else(|| String::from("null"), |b| yaml_bytes(b))
            )
            .unwrap_or_default();
        }
    }
    if !w.git_config.is_empty() {
        o.push_str("  git_config:\n");
        for (k, v) in &w.git_config {
            writeln!(o, "    {}: {}", yaml_quote(k), yaml_quote(v)).unwrap_or_default();
        }
    }
    if !w.setup.is_empty() {
        o.push_str("  setup:\n");
        emit_ops(&mut o, "    ", &w.setup);
    }
    o.push_str("steps:\n");
    for st in &sc.steps {
        match &st.kind {
            StepKind::Tool(a) => {
                o.push_str("  - tool: ");
                emit_list(&mut o, a);
            }
            StepKind::Git(a) => {
                o.push_str("  - git: ");
                emit_list(&mut o, a);
            }
            StepKind::Shell(c) => {
                o.push_str("  - shell: ");
                o.push_str(&yaml_quote(c));
            }
        }
        o.push('\n');
        if !st.env.is_empty() {
            o.push_str("    env:\n");
            for (k, v) in &st.env {
                writeln!(
                    o,
                    "      {}: {}",
                    yaml_quote(k),
                    v.as_ref()
                        .map_or_else(|| String::from("null"), |v| yaml_quote(v))
                )
                .unwrap_or_default();
            }
        }
        if !st.stdin.is_empty() {
            writeln!(o, "    stdin: {}", yaml_bytes(&st.stdin)).unwrap_or_default();
        }
        if st.tty {
            o.push_str("    tty: true\n");
        }
        if let Some(t) = st.timeout_ms {
            writeln!(o, "    timeout: \"{}.{:03}\"", t / 1000, t % 1000).unwrap_or_default();
        }
        if let Some(c) = &st.cwd {
            writeln!(o, "    cwd: {}", yaml_quote(c)).unwrap_or_default();
        }
        if let Some(sig) = &st.signal {
            match &sig.after_file {
                Some(f) => {
                    writeln!(
                        o,
                        "    signal: {{name: {}, after: {}}}",
                        yaml_quote(&sig.name),
                        yaml_quote(f)
                    )
                    .unwrap_or_default();
                }
                None => {
                    writeln!(
                        o,
                        "    signal: {{name: {}, after: \"{}.{:03}\"}}",
                        yaml_quote(&sig.name),
                        sig.after_ms / 1000,
                        sig.after_ms % 1000
                    )
                    .unwrap_or_default();
                }
            }
        }
    }
    let c = &sc.compare;
    o.push_str("compare:\n");
    writeln!(o, "  stdout: {}", mode_name(c.stdout)).unwrap_or_default();
    writeln!(o, "  stderr: {}", mode_name(c.stderr)).unwrap_or_default();
    writeln!(
        o,
        "  exit: {}",
        match c.exit {
            ExitMode::Exact => "exact",
            ExitMode::NonzeroEqual => "nonzero-equal",
            ExitMode::Ignore => "ignore",
        }
    )
    .unwrap_or_default();
    writeln!(o, "  tree: {}", c.tree).unwrap_or_default();
    writeln!(o, "  index: {}", c.index).unwrap_or_default();
    writeln!(o, "  refs: {}", c.refs).unwrap_or_default();
    writeln!(
        o,
        "  hooks_dir: {}",
        if c.hooks_dir { "behavior" } else { "ignore" }
    )
    .unwrap_or_default();
    writeln!(o, "  config_file: {}", mode_name(c.config_file)).unwrap_or_default();
    writeln!(o, "  git_config: {}", c.git_config).unwrap_or_default();
    writeln!(o, "  store: {}", if c.store { "logical" } else { "ignore" }).unwrap_or_default();
    if c.eol_insensitive {
        o.push_str("  eol: insensitive\n");
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `s` repeated `n` times (`str::repeat` is disallowed).
    fn rep(s: &str, n: usize) -> String {
        std::iter::repeat_n(s, n).collect()
    }

    const SAMPLE: &str = r#"
name: run/sample
since: "4.1.0"
requires: {git: ">=2.31", net: false, tools: [sh], os: [linux, macos], pty: false}
repos:
  hooks: {fixture: script_hooks_repo, tags: [v1]}
work:
  files: {foo.py: "", "bar.py": "x\n", bin: !!binary /w==}
  exec: [bin]
  config: |
    repos:
    -   repo: ${repo:hooks}
        rev: ${rev:hooks}
        hooks: [{id: bash_hook}]
  commit: [".pre-commit-config.yaml"]
  stage: [foo.py, bar.py]
  unstaged: {bar.py: "y\n", gone: null}
  git_config: {core.autocrlf: "false"}
  setup:
    - git: [tag, x]
    - write: {extra.txt: "e\n"}
steps:
  - tool: [run]
    env: {SKIP: bash_hook, X: null}
    stdin: ""
    tty: false
  - git: [commit, -m, msg]
    timeout: 5
compare:
  stdout: exact
  stderr: lines-unordered
  exit: nonzero-equal
  tree: true
  index: true
  refs: false
  hooks_dir: behavior
  config_file: exact
  store: logical
"#;

    #[test]
    fn parses_the_schema() {
        let sc = Scenario::parse(SAMPLE, Path::new("x.yaml")).unwrap();
        assert_eq!(sc.name, "run/sample");
        assert_eq!(sc.brands, vec![Brand::Compat, Brand::Snp]);
        assert!(sc.requires.git.matches([2, 43, 0]));
        assert!(!sc.requires.git.matches([2, 30, 9]));
        assert_eq!(sc.repos.len(), 1);
        assert_eq!(sc.repos[0].fixture.as_deref(), Some("script_hooks_repo"));
        assert_eq!(sc.work.files.len(), 3);
        assert_eq!(sc.work.files[2].1, vec![0xff]);
        assert_eq!(sc.work.unstaged[1], (String::from("gone"), None));
        assert_eq!(sc.work.setup.len(), 2);
        assert_eq!(sc.steps.len(), 2);
        assert_eq!(sc.steps[0].env[1], (String::from("X"), None));
        assert_eq!(sc.steps[1].timeout_ms, Some(5000));
        assert_eq!(sc.compare.stderr, StreamMode::LinesUnordered);
        assert_eq!(sc.compare.exit, ExitMode::NonzeroEqual);
        assert!(sc.compare.hooks_dir && sc.compare.store && !sc.compare.refs);
        assert_eq!(sc.unsupported, Vec::<String>::new());
    }

    #[test]
    fn round_trips_through_yaml() {
        let sc = Scenario::parse(SAMPLE, Path::new("x.yaml")).unwrap();
        let text = sc.to_yaml();
        let back = Scenario::parse(&text, Path::new("x.yaml")).unwrap();
        assert_eq!(back.name, sc.name);
        assert_eq!(back.requires, sc.requires);
        assert_eq!(back.repos, sc.repos);
        assert_eq!(back.work, sc.work);
        assert_eq!(back.steps, sc.steps);
        assert_eq!(back.compare, sc.compare);
    }

    #[test]
    fn stubs_and_errors() {
        let st = Scenario::parse(
            "expect_divergence: D-CRASH-05\nstatus: stub\n",
            Path::new("D-CRASH-05.yaml"),
        )
        .unwrap();
        assert_eq!(st.status, Status::Stub);
        assert_eq!(st.name, "divergences/D-CRASH-05");
        for bad in [
            "name: x\n",
            "name: x\nsteps: []\n",
            "name: x\nsteps: [{tool: [run]}]\nbogus: 1\n",
            "name: x\nexpect_divergence: CRASH-1\nsteps: [{tool: [run]}]\n",
            "name: x\nsteps: [{tool: [run]}]\ncompare: {stdout: fuzzy}\n",
            "name: x\nsteps: [{}]\n",
            "[",
        ] {
            assert!(Scenario::parse(bad, Path::new("b.yaml")).is_err(), "{bad}");
        }
    }

    #[test]
    fn hostile_names_and_root_relative_paths_are_refused() {
        for name in ["..", ".", "", "/abs", "a/../b", "a\\\\b", "a//b", "./a"] {
            let text = format!("name: {name:?}\nsteps: [{{tool: [run]}}]\n");
            assert!(
                Scenario::parse(&text, Path::new("n.yaml")).is_err(),
                "{name:?}"
            );
        }
        assert!(
            Scenario::parse(
                "name: a/b-c.d\nsteps: [{tool: [run]}]\n",
                Path::new("n.yaml")
            )
            .is_ok()
        );
        for bad in [
            "work: {dir: ../x}",
            "work: {dir: /tmp/x}",
            "repos: {h: {path: ../../h}}",
            "projects: [../p]",
        ] {
            let text = format!("name: x\n{bad}\nsteps: [{{tool: [run]}}]\n");
            assert!(
                Scenario::parse(&text, Path::new("p.yaml")).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn alias_bomb_scenario_is_refused_quickly() {
        let mut text = crate::yamlv::tests::alias_bomb(9, 10);
        text.push_str("name: b\nsteps: [{tool: [run]}]\n");
        let start = std::time::Instant::now();
        let err = Scenario::parse(&text, Path::new("b.yaml")).unwrap_err();
        assert!(err.contains("alias bomb"), "{err}");
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn generated_work_is_bounded() {
        let parse = |work: &str| {
            Scenario::parse(
                &format!("name: g\nwork: {work}\nsteps: [{{tool: [run]}}]\n"),
                Path::new("g.yaml"),
            )
        };
        let start = std::time::Instant::now();
        // An empty unit once looped `count` (9.2e18) times.
        for count in [i64::MAX, -1, i64::MIN] {
            assert!(
                parse(&format!(
                    "{{files: {{a.txt: {{generate: {{count: {count}}}}}}}}}"
                ))
                .is_err(),
                "{count}"
            );
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
        let ok =
            parse("{files: {a.txt: {generate: {count: 3, repeat: ab, prefix: '<', suffix: '>'}}}}")
                .unwrap();
        assert_eq!(ok.work.files[0].1, b"<ababab>");
        let ok = parse("{files: {a.txt: {generate: {count: 4, repeat: z}}}}").unwrap();
        assert_eq!(ok.work.files[0].1, b"zzzz");
        // Five 60 MiB files: each under the per-file cap, together over the 256 MiB total.
        let five: Vec<String> = (0..5)
            .map(|i| format!("f{i}: {{generate: {{count: 62914560, repeat: x}}}}"))
            .collect();
        let err = parse(&format!("{{files: {{{}}}}}", five.join(", "))).unwrap_err();
        assert!(err.contains("in total"), "{err}");
        // generated: count × content is charged before anything is cloned (100 000 × 4 KiB > 256 MiB).
        let big = rep("x", 4096);
        let err = parse(&format!(
            "{{generated: {{pattern: \"f{{i}}\", count: 100000, content: {big}}}}}"
        ))
        .unwrap_err();
        assert!(err.contains("in total"), "{err}");
    }

    #[test]
    fn platform_requirements() {
        let parse = |req: &str| {
            Scenario::parse(
                &format!("name: p\nrequires: {req}\nsteps: [{{tool: [run]}}]\n"),
                Path::new("p.yaml"),
            )
            .unwrap()
        };
        let ok = parse("{console: pipe, launcher: direct, path_profile: runner}");
        assert!(ok.unsupported.is_empty(), "{:?}", ok.unsupported);
        assert!(parse("{console: pty}").requires.pty);
        for (req, why) in [
            ("{console: conpty}", "requires.console \"conpty\""),
            ("{launcher: git-cmd}", "requires.launcher \"git-cmd\""),
            ("{path_profile: user-minimal}", "requires.path_profile"),
            ("{manual: needs subst}", "requires.manual: needs subst"),
        ] {
            let sc = parse(req);
            assert!(
                sc.unsupported.iter().any(|u| u.contains(why)),
                "{req}: {:?}",
                sc.unsupported
            );
        }
        assert!(
            Scenario::parse(
                "name: p\nrequires: {consol: pipe}\nsteps: [{tool: [run]}]\n", // spellchecker:disable-line
                Path::new("p.yaml")
            )
            .is_err()
        );
    }

    #[test]
    #[expect(
        clippy::literal_string_with_formatting_args,
        reason = "`{i:04}` is the generated-files pattern syntax"
    )]
    fn generated_files_and_marker_signals() {
        let text = "name: g\nwork:\n  files:\n    keep.txt: k\n    generated: {pattern: \"a b{i:04}\", count: 3, content: \"x\\n\"}\nsteps:\n  - tool: [run]\n    signal: {name: TERM, after: .hook-started}\n  - tool: [run]\n    signal: {name: INT, after: 1.5}\n";
        let sc = Scenario::parse(text, Path::new("g.yaml")).unwrap();
        let names: Vec<&[u8]> = sc.work.files.iter().map(|(p, _)| p.as_slice()).collect();
        assert_eq!(
            names,
            vec![&b"keep.txt"[..], b"a b0000", b"a b0001", b"a b0002"]
        );
        assert_eq!(sc.work.files[1].1, b"x\n");
        let sig = sc.steps[0].signal.clone().unwrap();
        assert_eq!(sig.after_file.as_deref(), Some(".hook-started"));
        assert_eq!(sc.steps[1].signal.clone().unwrap().after_ms, 1500);
        let back = Scenario::parse(&sc.to_yaml(), Path::new("g.yaml")).unwrap();
        assert_eq!(back.steps, sc.steps);
        assert!(Scenario::parse("name: g\nwork: {files: {generated: {pattern: x, count: 2}}}\nsteps: [{tool: [run]}]\n", Path::new("g.yaml")).is_err());
    }

    #[test]
    fn version_requirements() {
        let r = VersionReq::parse(">=2.19, <2.31").unwrap();
        assert!(r.matches([2, 30, 0]) && !r.matches([2, 31, 0]) && !r.matches([2, 18, 5]));
        assert_eq!(parse_version("2.39.5.windows.1"), Some([2, 39, 5]));
        assert!(VersionReq::parse("~2").is_err());
        assert!(VersionReq::parse("").unwrap().matches([1, 0, 0]));
    }

    #[test]
    fn register_ids() {
        assert!(is_register_id("D-DIAG-02"));
        assert!(!is_register_id("D-diag-02"));
        assert!(!is_register_id("D-DIAG"));
        assert!(!is_register_id("X-DIAG-01"));
    }
}
