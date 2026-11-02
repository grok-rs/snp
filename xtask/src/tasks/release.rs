//! `cargo xtask release <gates|notes|version>`: the release gates of ADR-061 and release metadata (WBS WP-37).
//!
//! ```text
//! cargo xtask release gates [--tag vX.Y.Z] [--skip-registry] [--allow-dirty] [--allow-unreleased]
//!                           [--index URL] [--dl URL] [--root DIR] [--lock FILE]
//! cargo xtask release notes [VERSION]      # the CHANGELOG.md section of VERSION (default: the workspace version)
//! cargo xtask release version              # the workspace version
//! ```
//!
//! `gates` checks, and reports every failure before failing:
//!
//! 1. **lock**: `deps.lock` parses and pins exactly the workspace's out-of-tree path dependencies (the user crates
//!    `cfgv` and `file-identify`), each with a 40-hex `rev`, and each `[workspace.dependencies]` requirement
//!    (`version = "0.3"`) accepts the pinned `version` (caret semantics);
//! 2. **siblings**: each sibling checkout (`../cfgv-rs`, `../file-identify`) is a git work tree whose `HEAD` is the
//!    pinned `rev`, has no modified or untracked files, and whose `Cargo.toml` declares the pinned name and version;
//! 3. **clean checkout**: the snp work tree has no modified or untracked files (`--allow-dirty` skips this for local
//!    dry runs only);
//! 4. **version**: `--tag` (the pushed tag) is `v` + the workspace version, exists as a tag whose commit is the
//!    checked-out `HEAD` (a `workflow_dispatch` run on another ref cannot publish under the tag), and `CHANGELOG.md`
//!    has a section for that version (`--allow-unreleased` also accepts `## [Unreleased]`, for pull-request dry runs);
//! 5. **registry** (ADR-061): each user crate is published on crates.io in exactly the pinned version, not yanked,
//!    the downloaded `.crate` matches the index checksum, and its `.cargo_vcs_info.json` records the pinned commit
//!    (`git.sha1 == rev`), a clean tree (`dirty` absent or false) and `path_in_vcs == ""`. `--skip-registry` skips
//!    this check (pull requests before the user crates are published). `--index`/`--dl` replace the sparse index
//!    (`https://index.crates.io`) and download (`https://static.crates.io/crates`) base URLs; `file://` URLs are read
//!    from disk (tests, mirrors).
//!
//! Downloads use `curl` (`--proto =https`), checksums `sha256sum` or `shasum -a 256`, and `.crate` extraction
//! `tar`; all three exist on every GitHub-hosted runner.
//!
//! Owner: WP-37.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context as _, bail};

use crate::workspace;

/// The default sparse index.
const INDEX: &str = "https://index.crates.io";
/// The default download base.
const DL: &str = "https://static.crates.io/crates";
/// `User-Agent` for crates.io (required by its crawler policy).
const USER_AGENT: &str = "snp-xtask-release (https://github.com/grok-rs/snp)";

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    let strs: Vec<String> = args
        .iter()
        .map(|a| {
            a.to_str()
                .map(str::to_owned)
                .with_context(|| format!("non-UTF-8 argument {}", a.to_string_lossy()))
        })
        .collect::<anyhow::Result<_>>()?;
    let (cmd, rest) = strs
        .split_first()
        .map_or(("", &[][..]), |(c, r)| (c.as_str(), r));
    match cmd {
        "gates" => {
            let opts = GateOpts::parse(rest)?;
            let report = gates(&opts);
            for line in report.lines() {
                workspace::say(&line);
            }
            let failed = report.failures();
            if failed > 0 {
                bail!("{failed} release gate(s) failed (ADR-061)");
            }
            Ok(())
        }
        "notes" => {
            let root = workspace::root()?;
            let version = match rest {
                [] => workspace_version(&root)?,
                [v] => v.trim_start_matches('v').to_owned(),
                _ => bail!("usage: cargo xtask release notes [VERSION]"),
            };
            let changelog = std::fs::read_to_string(root.join("CHANGELOG.md"))
                .context("reading CHANGELOG.md")?;
            let notes = changelog_section(&changelog, &version)
                .with_context(|| format!("CHANGELOG.md has no section for {version}"))?;
            workspace::say(notes.trim_end());
            Ok(())
        }
        "version" => {
            workspace::say(&workspace_version(&workspace::root()?)?);
            Ok(())
        }
        _ => bail!(
            "usage: cargo xtask release gates|notes|version [args...] (see xtask/src/tasks/release.rs)"
        ),
    }
}

// ----- options -------------------------------------------------------------------------------------------------

/// Options of `release gates`.
#[derive(Debug, Clone)]
struct GateOpts {
    root: Option<PathBuf>,
    lock: Option<PathBuf>,
    tag: Option<String>,
    skip_registry: bool,
    allow_dirty: bool,
    allow_unreleased: bool,
    index: String,
    dl: String,
}

impl Default for GateOpts {
    fn default() -> Self {
        Self {
            root: None,
            lock: None,
            tag: None,
            skip_registry: false,
            allow_dirty: false,
            allow_unreleased: false,
            index: INDEX.to_owned(),
            dl: DL.to_owned(),
        }
    }
}

impl GateOpts {
    fn parse(args: &[String]) -> anyhow::Result<Self> {
        let mut o = Self::default();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            let mut value = || {
                it.next()
                    .cloned()
                    .with_context(|| format!("{a} needs a value"))
            };
            match a.as_str() {
                "--tag" => o.tag = Some(value()?),
                "--root" => o.root = Some(PathBuf::from(value()?)),
                "--lock" => o.lock = Some(PathBuf::from(value()?)),
                "--index" => value()?.trim_end_matches('/').clone_into(&mut o.index),
                "--dl" => value()?.trim_end_matches('/').clone_into(&mut o.dl),
                "--skip-registry" => o.skip_registry = true,
                "--allow-dirty" => o.allow_dirty = true,
                "--allow-unreleased" => o.allow_unreleased = true,
                _ => bail!("release gates: unknown argument {a}"),
            }
        }
        Ok(o)
    }
}

// ----- report --------------------------------------------------------------------------------------------------

/// The outcome of every gate check.
#[derive(Debug, Default)]
struct Report {
    rows: Vec<(String, Result<String, String>)>,
}

impl Report {
    fn pass(&mut self, gate: &str, detail: impl Into<String>) {
        self.rows.push((gate.to_owned(), Ok(detail.into())));
    }

    fn fail(&mut self, gate: &str, detail: impl Into<String>) {
        self.rows.push((gate.to_owned(), Err(detail.into())));
    }

    fn record(&mut self, gate: &str, r: Result<String, String>) {
        self.rows.push((gate.to_owned(), r));
    }

    fn failures(&self) -> usize {
        self.rows.iter().filter(|(_, r)| r.is_err()).count()
    }

    fn lines(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|(g, r)| match r {
                Ok(d) => format!("ok   {g}: {d}"),
                Err(d) => format!("FAIL {g}: {d}"),
            })
            .collect()
    }

    /// Whether a gate whose name starts with `gate_prefix` failed (used by the Unix-only gate tests).
    #[cfg(all(test, unix))]
    fn failed(&self, gate_prefix: &str) -> bool {
        self.rows
            .iter()
            .any(|(g, r)| g.starts_with(gate_prefix) && r.is_err())
    }
}

// ----- deps.lock -----------------------------------------------------------------------------------------------

/// One `[[crate]]` of `deps.lock`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pin {
    name: String,
    version: String,
    path: String,
    repository: String,
    branch: String,
    rev: String,
}

fn parse_lock(text: &str) -> Result<Vec<Pin>, String> {
    let table: toml::Table = text.parse().map_err(|e| format!("deps.lock: {e}"))?;
    let crates = table
        .get("crate")
        .and_then(toml::Value::as_array)
        .ok_or("deps.lock: no [[crate]] tables")?;
    let mut out = Vec::new();
    for c in crates {
        let field = |k: &str| -> Result<String, String> {
            c.get(k)
                .and_then(toml::Value::as_str)
                .map(str::to_owned)
                .filter(|v| !v.is_empty())
                .ok_or_else(|| format!("deps.lock: a [[crate]] lacks `{k}`"))
        };
        let pin = Pin {
            name: field("name")?,
            version: field("version")?,
            path: field("path")?,
            repository: field("repository")?,
            branch: field("branch")?,
            rev: field("rev")?,
        };
        if pin.rev.len() != 40
            || !pin
                .rev
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(format!(
                "deps.lock: {}: rev {:?} is not a 40-hex commit",
                pin.name, pin.rev
            ));
        }
        if parse_version(&pin.version).is_none() {
            return Err(format!(
                "deps.lock: {}: version {:?} is not X.Y.Z",
                pin.name, pin.version
            ));
        }
        out.push(pin);
    }
    Ok(out)
}

/// `X.Y.Z` (no pre-release or build metadata).
fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let mut it = v.split('.');
    let a = it.next()?.parse().ok()?;
    let b = it.next()?.parse().ok()?;
    let c = it.next()?.parse().ok()?;
    it.next().is_none().then_some((a, b, c))
}

/// Cargo's default (caret) requirement `req` (`0.3`, `0.3.1`, `^1`, `1.2.3`) accepts `version`.
fn caret_matches(req: &str, version: &str) -> bool {
    let req = req.trim().trim_start_matches('^');
    let Some(v) = parse_version(version) else {
        return false;
    };
    let parts: Vec<Option<u64>> = req.split('.').map(|p| p.parse().ok()).collect();
    let (major, minor, patch) = match parts.as_slice() {
        [Some(a)] => (*a, None, None),
        [Some(a), Some(b)] => (*a, Some(*b), None),
        [Some(a), Some(b), Some(c)] => (*a, Some(*b), Some(*c)),
        _ => return false,
    };
    let lower = (major, minor.unwrap_or(0), patch.unwrap_or(0));
    if v < lower {
        return false;
    }
    match (major, minor, patch) {
        (0, None, _) => v.0 == 0,
        (0, Some(0), Some(p)) => v.0 == 0 && v.1 == 0 && v.2 == p,
        (0, Some(m), _) => v.0 == 0 && v.1 == m,
        (a, _, _) => v.0 == a,
    }
}

/// `[workspace.package] version` of the root manifest.
fn workspace_version(root: &Path) -> anyhow::Result<String> {
    let manifest: toml::Table = std::fs::read_to_string(root.join("Cargo.toml"))
        .context("reading Cargo.toml")?
        .parse()
        .context("parsing Cargo.toml")?;
    manifest
        .get("workspace")
        .and_then(|w| w.get("package"))
        .and_then(|p| p.get("version"))
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
        .context("Cargo.toml has no [workspace.package] version")
}

/// The out-of-tree path dependencies of `[workspace.dependencies]`: (name, path, version requirement).
fn user_crate_deps(root: &Path) -> Result<Vec<(String, String, Option<String>)>, String> {
    let manifest: toml::Table = std::fs::read_to_string(root.join("Cargo.toml"))
        .map_err(|e| format!("Cargo.toml: {e}"))?
        .parse()
        .map_err(|e| format!("Cargo.toml: {e}"))?;
    let deps = manifest
        .get("workspace")
        .and_then(|w| w.get("dependencies"))
        .and_then(toml::Value::as_table)
        .ok_or("Cargo.toml has no [workspace.dependencies]")?;
    let mut out = Vec::new();
    for (name, spec) in deps {
        let Some(path) = spec.get("path").and_then(toml::Value::as_str) else {
            continue;
        };
        if path.starts_with("../") || path.starts_with("..\\") {
            let package = spec
                .get("package")
                .and_then(toml::Value::as_str)
                .unwrap_or(name);
            out.push((
                package.to_owned(),
                path.to_owned(),
                spec.get("version")
                    .and_then(toml::Value::as_str)
                    .map(str::to_owned),
            ));
        }
    }
    Ok(out)
}

// ----- subprocess helpers --------------------------------------------------------------------------------------

/// stdout of a successful command, trimmed; `Err` with stderr otherwise.
fn output(cmd: &mut Command) -> Result<String, String> {
    let out = cmd
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run {}: {e}", cmd.get_program().display()))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_owned())
    } else {
        Err(format!(
            "{} failed ({}): {}",
            cmd.get_program().display(),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    output(Command::new("git").arg("-C").arg(dir).args(args))
}

/// `git status --porcelain` is empty (modified, staged and untracked files all count; ignored files do not).
fn clean_tree(dir: &Path) -> Result<(), String> {
    let status = git(dir, &["status", "--porcelain=v1", "--untracked-files=all"])?;
    if status.is_empty() {
        Ok(())
    } else {
        let first: Vec<&str> = status.lines().take(5).collect();
        Err(format!("work tree is not clean: {}", first.join("; ")))
    }
}

/// Downloads `url` to `dest` (`file://` URLs are copied from disk).
fn fetch(url: &str, dest: &Path) -> Result<(), String> {
    if let Some(path) = url.strip_prefix("file://") {
        // file:///C:/x on Windows
        let path = if cfg!(windows) {
            path.strip_prefix('/').unwrap_or(path)
        } else {
            path
        };
        return std::fs::copy(path, dest)
            .map(|_| ())
            .map_err(|e| format!("{url}: {e}"));
    }
    output(
        Command::new("curl")
            .args([
                "--silent",
                "--show-error",
                "--fail",
                "--location",
                "--proto",
                "=https",
                "--tlsv1.2",
                "--retry",
                "3",
                "--user-agent",
                USER_AGENT,
                "--output",
            ])
            .arg(dest)
            .arg(url),
    )
    .map(|_| ())
}

/// SHA-256 of `file` (lower-case hex) via `sha256sum` or `shasum -a 256`.
fn sha256(file: &Path) -> Result<String, String> {
    let attempts: [(&str, &[&str]); 2] = [("sha256sum", &[]), ("shasum", &["-a", "256"])];
    let mut errors = Vec::new();
    for (tool, pre) in attempts {
        match output(Command::new(tool).args(pre).arg(file)) {
            Ok(out) => {
                let digest = out
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if digest.len() == 64 {
                    return Ok(digest);
                }
                errors.push(format!("{tool}: unexpected output {out:?}"));
            }
            Err(e) => errors.push(e),
        }
    }
    Err(format!("cannot compute SHA-256: {}", errors.join("; ")))
}

/// The sparse-index path of a crate (`cf/gv/cfgv`, `3/f/foo`, …).
fn index_path(name: &str) -> String {
    let n = name.to_ascii_lowercase();
    match n.len() {
        0 => n,
        1 => format!("1/{n}"),
        2 => format!("2/{n}"),
        3 => format!("3/{}/{n}", n.get(..1).unwrap_or_default()),
        _ => format!(
            "{}/{}/{n}",
            n.get(..2).unwrap_or_default(),
            n.get(2..4).unwrap_or_default()
        ),
    }
}

// ----- the gates -----------------------------------------------------------------------------------------------

fn gates(opts: &GateOpts) -> Report {
    let mut report = Report::default();
    let root = match opts.root.clone().map_or_else(workspace::root, Ok) {
        Ok(r) => r,
        Err(e) => {
            report.fail("setup", e.to_string());
            return report;
        }
    };
    let lock_path = opts.lock.clone().unwrap_or_else(|| root.join("deps.lock"));
    let pins = match std::fs::read_to_string(&lock_path)
        .map_err(|e| format!("{}: {e}", lock_path.display()))
        .and_then(|t| parse_lock(&t))
    {
        Ok(p) => {
            report.pass(
                "lock",
                format!(
                    "{} pinned: {}",
                    p.len(),
                    p.iter()
                        .map(|p| format!("{} {} @ {}", p.name, p.version, p.rev))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
            p
        }
        Err(e) => {
            report.fail("lock", e);
            return report;
        }
    };
    let lock_dir = lock_path
        .parent()
        .map_or_else(|| root.clone(), Path::to_path_buf);

    // 1. the lock covers exactly the out-of-tree path dependencies, with matching requirements
    match user_crate_deps(&root) {
        Ok(deps) => {
            for (name, path, req) in &deps {
                let gate = format!("lock/{name}");
                match pins.iter().find(|p| &p.name == name) {
                    None => report.fail(&gate, format!("path dependency {path} is not pinned in deps.lock")),
                    Some(pin) if &pin.path != path => {
                        report.fail(&gate, format!("Cargo.toml path {path} != deps.lock path {}", pin.path));
                    }
                    Some(pin) => match req {
                        Some(r) if caret_matches(r, &pin.version) => {
                            report.pass(&gate, format!("requirement {r:?} accepts {}", pin.version));
                        }
                        Some(r) => report.fail(
                            &gate,
                            format!("requirement {r:?} does not accept the pinned {}", pin.version),
                        ),
                        None => report.fail(
                            &gate,
                            "no `version` requirement: the published snp crate would not resolve it",
                        ),
                    },
                }
            }
            for pin in &pins {
                if !deps.iter().any(|(n, _, _)| n == &pin.name) {
                    report.fail(
                        &format!("lock/{}", pin.name),
                        "pinned in deps.lock but not a path dependency of the workspace",
                    );
                }
            }
        }
        Err(e) => report.fail("lock/workspace", e),
    }

    // 2. sibling checkouts at the pinned commits, clean, with the pinned version
    for pin in &pins {
        let gate = format!("sibling/{}", pin.name);
        let dir = lock_dir.join(&pin.path);
        report.record(&gate, check_sibling(&dir, pin));
    }

    // 3. a clean snp checkout
    if opts.allow_dirty {
        report.pass("checkout", "skipped (--allow-dirty)");
    } else {
        report.record(
            "checkout",
            clean_tree(&root).map(|()| format!("{} is clean", root.display())),
        );
    }

    // 4. version, tag and changelog
    match workspace_version(&root) {
        Ok(version) => {
            if let Some(tag) = &opts.tag {
                let want = format!("v{version}");
                if tag == &want {
                    report.record("tag", tag_at_head(&root, tag));
                } else {
                    report.fail("tag", format!("tag {tag} != {want} (workspace version)"));
                }
            }
            match std::fs::read_to_string(root.join("CHANGELOG.md")) {
                Ok(text) => {
                    if changelog_section(&text, &version).is_some() {
                        report.pass("changelog", format!("CHANGELOG.md has a {version} section"));
                    } else if opts.allow_unreleased
                        && changelog_section(&text, "Unreleased").is_some()
                    {
                        report.pass(
                            "changelog",
                            "CHANGELOG.md has an Unreleased section (--allow-unreleased)",
                        );
                    } else {
                        report.fail(
                            "changelog",
                            format!("CHANGELOG.md has no `## [{version}]` section"),
                        );
                    }
                }
                Err(e) => report.fail("changelog", format!("CHANGELOG.md: {e}")),
            }
        }
        Err(e) => report.fail("version", e.to_string()),
    }

    // 5. crates.io: published from exactly the pinned commits
    if opts.skip_registry {
        report.pass("registry", "skipped (--skip-registry)");
    } else {
        match private_scratch("snp-release-gates") {
            Err(e) => report.fail("registry", e),
            Ok(scratch) => {
                for pin in &pins {
                    report.record(
                        &format!("registry/{}", pin.name),
                        check_published(opts, pin, &scratch),
                    );
                }
                std::fs::remove_dir_all(&scratch).unwrap_or_default();
            }
        }
    }
    report
}

/// `tag` exists and names the commit checked out at `HEAD` of `root`.
fn tag_at_head(root: &Path, tag: &str) -> Result<String, String> {
    let tagged = git(
        root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/tags/{tag}^{{commit}}"),
        ],
    )
    .map_err(|_| format!("tag {tag} does not exist in {}", root.display()))?;
    let head = git(root, &["rev-parse", "--verify", "HEAD^{commit}"])?;
    if tagged == head {
        Ok(format!(
            "{tag} matches the workspace version and points at HEAD ({head})"
        ))
    } else {
        Err(format!(
            "tag {tag} does not point at HEAD (tag {tagged}, HEAD {head})"
        ))
    }
}

/// Creates a fresh directory under the temporary directory that no other process (or user) created first: the
/// name carries the PID, a per-process counter and the clock, creation is exclusive (`create_dir`, never
/// `create_dir_all`, so a planted directory or symlink is refused) and, on Unix, the mode is 0700.
pub(crate) fn private_scratch(prefix: &str) -> Result<PathBuf, String> {
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    let base = std::env::temp_dir();
    let mut last = String::new();
    for _ in 0..64 {
        let n = CALLS.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let dir = base.join(format!("{prefix}-{}-{n}-{nanos:08x}", std::process::id()));
        #[cfg_attr(not(unix), expect(unused_mut, reason = "the mode is Unix-only"))]
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        match builder.create(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                last = format!("{}: {e}", dir.display());
            }
            Err(e) => return Err(format!("{}: {e}", dir.display())),
        }
    }
    Err(format!(
        "no fresh scratch directory under {}: {last}",
        base.display()
    ))
}

fn check_sibling(dir: &Path, pin: &Pin) -> Result<String, String> {
    if !dir.is_dir() {
        return Err(format!(
            "{} is missing (run .github/scripts/deps-lock.sh checkout)",
            dir.display()
        ));
    }
    let head = git(dir, &["rev-parse", "HEAD"])?;
    if head != pin.rev {
        return Err(format!("HEAD is {head}, deps.lock pins {}", pin.rev));
    }
    clean_tree(dir)?;
    let manifest: toml::Table = std::fs::read_to_string(dir.join("Cargo.toml"))
        .map_err(|e| format!("Cargo.toml: {e}"))?
        .parse()
        .map_err(|e| format!("Cargo.toml: {e}"))?;
    let package = manifest.get("package");
    let name = package
        .and_then(|p| p.get("name"))
        .and_then(toml::Value::as_str);
    let version = package
        .and_then(|p| p.get("version"))
        .and_then(toml::Value::as_str);
    if name != Some(pin.name.as_str()) {
        return Err(format!("Cargo.toml names {name:?}, deps.lock {}", pin.name));
    }
    if version != Some(pin.version.as_str()) {
        return Err(format!(
            "Cargo.toml version {version:?}, deps.lock {}",
            pin.version
        ));
    }
    Ok(format!(
        "{} at {} (clean, version {})",
        dir.display(),
        pin.rev,
        pin.version
    ))
}

/// The registry gate for one pin.
fn check_published(opts: &GateOpts, pin: &Pin, scratch: &Path) -> Result<String, String> {
    // the index entry: published, not yanked, checksum
    let index_file = scratch.join(format!("{}.index", pin.name));
    let index_url = format!("{}/{}", opts.index, index_path(&pin.name));
    fetch(&index_url, &index_file)
        .map_err(|e| format!("{} is not published (index {index_url}: {e})", pin.name))?;
    let index = std::fs::read_to_string(&index_file).map_err(|e| e.to_string())?;
    let mut published = Vec::new();
    let mut entry = None;
    for line in index.lines().filter(|l| !l.trim().is_empty()) {
        let v: serde_json::Value =
            serde_json::from_str(line).map_err(|e| format!("index line: {e}"))?;
        let vers = v
            .get("vers")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if vers == pin.version {
            entry = Some(v);
        }
        published.push(vers);
    }
    let Some(entry) = entry else {
        return Err(format!(
            "{} {} is not published on crates.io (published: {})",
            pin.name,
            pin.version,
            published.join(", ")
        ));
    };
    if entry.get("yanked").and_then(serde_json::Value::as_bool) == Some(true) {
        return Err(format!("{} {} is yanked", pin.name, pin.version));
    }
    let cksum = entry
        .get("cksum")
        .and_then(serde_json::Value::as_str)
        .ok_or("index entry has no cksum")?
        .to_ascii_lowercase();

    // the .crate: checksum and VCS info
    let crate_file = scratch.join(format!("{}-{}.crate", pin.name, pin.version));
    let crate_url = format!(
        "{}/{}/{}-{}.crate",
        opts.dl, pin.name, pin.name, pin.version
    );
    fetch(&crate_url, &crate_file)?;
    let digest = sha256(&crate_file)?;
    if digest != cksum {
        return Err(format!(
            "{crate_url}: SHA-256 {digest} != index cksum {cksum}"
        ));
    }
    let member = format!("{}-{}/.cargo_vcs_info.json", pin.name, pin.version);
    let vcs = output(
        Command::new("tar")
            .arg("-xzOf")
            .arg(&crate_file)
            .arg(&member),
    )
    .map_err(|e| {
        format!(
            "{} {} has no {member} (published without git info?): {e}",
            pin.name, pin.version
        )
    })?;
    check_vcs_info(&vcs, pin)?;
    Ok(format!(
        "{} {} published from {} (clean), checksum {}",
        pin.name, pin.version, pin.rev, digest
    ))
}

/// `.cargo_vcs_info.json` records `pin.rev`, a clean tree and the repository root.
fn check_vcs_info(json: &str, pin: &Pin) -> Result<(), String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!(".cargo_vcs_info.json: {e}"))?;
    let sha1 = v
        .pointer("/git/sha1")
        .and_then(serde_json::Value::as_str)
        .ok_or(".cargo_vcs_info.json has no git.sha1")?;
    if sha1 != pin.rev {
        return Err(format!(
            "{} {} was published from {sha1}, deps.lock pins {}",
            pin.name, pin.version, pin.rev
        ));
    }
    if v.pointer("/git/dirty").and_then(serde_json::Value::as_bool) == Some(true) {
        return Err(format!(
            "{} {} was published from a dirty work tree",
            pin.name, pin.version
        ));
    }
    let path = v
        .get("path_in_vcs")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if !path.is_empty() {
        return Err(format!(
            "{} {} was published from the sub-directory {path:?} of its repository",
            pin.name, pin.version
        ));
    }
    Ok(())
}

/// The fence a line opens or closes (Markdown: up to three spaces, then at least three backticks or tildes): the
/// fence character and the length of the run.
fn fence_marker(line: &str) -> Option<(char, usize)> {
    let indent = line
        .len()
        .saturating_sub(line.trim_start_matches(' ').len());
    if indent > 3 {
        return None;
    }
    let rest = line.get(indent..)?;
    let c = rest.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let n = rest.chars().take_while(|&x| x == c).count();
    (n >= 3).then_some((c, n))
}

/// The body of the `## [version]` (or `## version`, `## vversion`) section of a Keep-a-Changelog file. Headings inside
/// fenced code blocks (a quoted Markdown snippet) neither start nor end a section.
fn changelog_section<'a>(text: &'a str, version: &str) -> Option<&'a str> {
    let mut start = None;
    let mut offset = 0_usize;
    // The open fence: its character and length (a closing fence uses the same character, at least as many times,
    // and nothing else but spaces).
    let mut fence: Option<(char, usize)> = None;
    for line in text.split_inclusive('\n') {
        let next = offset.saturating_add(line.len());
        let marker = fence_marker(line.trim_end_matches(['\n', '\r']));
        match (fence, marker) {
            (None, Some(m)) => {
                fence = Some(m);
                offset = next;
                continue;
            }
            (Some((c, n)), Some((mc, mn))) => {
                let after = line.trim().trim_start_matches(mc);
                if mc == c && mn >= n && after.is_empty() {
                    fence = None;
                }
                offset = next;
                continue;
            }
            (Some(_), None) => {
                offset = next;
                continue;
            }
            (None, None) => {}
        }
        if let Some(h) = line.strip_prefix("## ") {
            if start.is_some() {
                return text.get(start.unwrap_or(0)..offset);
            }
            let h = h.trim();
            let title = h
                .strip_prefix('[')
                .and_then(|r| r.split_once(']'))
                .map_or_else(
                    || h.split_whitespace().next().unwrap_or_default(),
                    |(t, _)| t,
                );
            let title = title.strip_prefix('v').unwrap_or(title);
            if title.eq_ignore_ascii_case(version) {
                start = Some(next);
            }
        }
        offset = next;
    }
    start.and_then(|s| text.get(s..))
}

/// Renders a report for humans (used by the Unix-only gate tests).
#[cfg(all(test, unix))]
fn render(report: &Report) -> String {
    let mut s = report.lines().join("\n");
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const REV_A: &str = "6a04a30c3f11b9582570cf4cf0639699efe3dc9f";

    fn pin(name: &str, version: &str, rev: &str) -> Pin {
        Pin {
            name: name.into(),
            version: version.into(),
            path: format!("../{name}"),
            repository: format!("https://example.invalid/{name}"),
            branch: "snp-support".into(),
            rev: rev.into(),
        }
    }

    #[test]
    fn caret_requirements() {
        assert!(caret_matches("0.3", "0.3.0"));
        assert!(caret_matches("0.3", "0.3.7"));
        assert!(!caret_matches("0.3", "0.4.0"));
        assert!(!caret_matches("0.3.2", "0.3.1"));
        assert!(caret_matches("^0.5", "0.5.0"));
        assert!(caret_matches("1", "1.9.0"));
        assert!(!caret_matches("1.2", "2.0.0"));
        assert!(caret_matches("0.0.3", "0.0.3"));
        assert!(!caret_matches("0.0.3", "0.0.4"));
        assert!(!caret_matches("0.3", "0.3"));
        assert!(!caret_matches("*", "0.3.0"));
    }

    #[test]
    fn index_paths() {
        assert_eq!(index_path("a"), "1/a");
        assert_eq!(index_path("ab"), "2/ab");
        assert_eq!(index_path("abc"), "3/a/abc");
        assert_eq!(index_path("cfgv"), "cf/gv/cfgv");
        assert_eq!(index_path("File-Identify"), "fi/le/file-identify");
    }

    #[test]
    fn lock_parsing() {
        let root = workspace::root().unwrap();
        let pins = parse_lock(&std::fs::read_to_string(root.join("deps.lock")).unwrap()).unwrap();
        let names: Vec<&str> = pins.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["cfgv", "file-identify"]);
        assert!(parse_lock("[[crate]]\nname = \"x\"\n").is_err());
        let bad_rev = "[[crate]]\nname=\"x\"\nversion=\"1.0.0\"\npath=\"../x\"\nrepository=\"r\"\nbranch=\"b\"\nrev=\"abc\"\n";
        assert!(parse_lock(bad_rev).unwrap_err().contains("40-hex"));
    }

    #[test]
    fn the_workspace_requirements_accept_the_pins() {
        let root = workspace::root().unwrap();
        let pins = parse_lock(&std::fs::read_to_string(root.join("deps.lock")).unwrap()).unwrap();
        let deps = user_crate_deps(&root).unwrap();
        assert_eq!(deps.len(), pins.len());
        for (name, path, req) in deps {
            let p = pins.iter().find(|p| p.name == name).unwrap();
            assert_eq!(p.path, path);
            assert!(caret_matches(req.as_deref().unwrap(), &p.version), "{name}");
        }
    }

    #[test]
    fn vcs_info_checks() {
        let p = pin("cfgv", "0.3.0", REV_A);
        let ok = format!("{{\"git\":{{\"sha1\":\"{REV_A}\"}},\"path_in_vcs\":\"\"}}");
        assert!(check_vcs_info(&ok, &p).is_ok());
        let other = "{\"git\":{\"sha1\":\"1324f403ba9b519b466fd27d186ae4e75cfa8d0b\"},\"path_in_vcs\":\"\"}";
        assert!(
            check_vcs_info(other, &p)
                .unwrap_err()
                .contains("deps.lock pins")
        );
        let dirty =
            format!("{{\"git\":{{\"sha1\":\"{REV_A}\",\"dirty\":true}},\"path_in_vcs\":\"\"}}");
        assert!(check_vcs_info(&dirty, &p).unwrap_err().contains("dirty"));
        let sub = format!("{{\"git\":{{\"sha1\":\"{REV_A}\"}},\"path_in_vcs\":\"crates/x\"}}");
        assert!(
            check_vcs_info(&sub, &p)
                .unwrap_err()
                .contains("sub-directory")
        );
        assert!(check_vcs_info("{}", &p).is_err());
    }

    #[test]
    fn changelog_sections() {
        let text = "# Changelog\n\n## [Unreleased]\n\nnext\n\n## [2.0.0] - 2026-11-01\n\n### Added\n\n- all\n\n## v1.0.1\n\nold\n";
        assert_eq!(
            changelog_section(text, "2.0.0"),
            Some("\n### Added\n\n- all\n\n")
        );
        assert_eq!(changelog_section(text, "Unreleased"), Some("\nnext\n\n"));
        assert_eq!(changelog_section(text, "1.0.1"), Some("\nold\n"));
        assert_eq!(changelog_section(text, "3.0.0"), None);
        // A heading inside a fenced block (a quoted Markdown snippet) neither ends nor starts a section, whichever
        // fence character and length is used; an indented (4 spaces) fence is no fence.
        let fenced = "## [2.0.0]\n\nMigration:\n\n```markdown\n## [Unreleased]\n```\n\n~~~~\n## 2.0.0\n~~~\n## [1.0.0]\n~~~~\n\ntail\n\n## [1.0.0]\n\nold\n";
        assert_eq!(
            changelog_section(fenced, "2.0.0"),
            Some(
                "\nMigration:\n\n```markdown\n## [Unreleased]\n```\n\n~~~~\n## 2.0.0\n~~~\n## [1.0.0]\n~~~~\n\ntail\n\n"
            )
        );
        assert_eq!(changelog_section(fenced, "1.0.0"), Some("\nold\n"));
        assert_eq!(changelog_section(fenced, "Unreleased"), None);
        assert_eq!(fence_marker("    ```"), None);
        assert_eq!(fence_marker("   ```rust"), Some(('`', 3)));
        assert_eq!(fence_marker("``"), None);
        let root = workspace::root().unwrap();
        let ours = std::fs::read_to_string(root.join("CHANGELOG.md")).unwrap();
        let v = workspace_version(&root).unwrap();
        assert!(
            changelog_section(&ours, &v).is_some()
                || changelog_section(&ours, "Unreleased").is_some()
        );
    }

    #[test]
    fn options() {
        let o = GateOpts::parse(&[
            "--tag".into(),
            "v2.0.0".into(),
            "--skip-registry".into(),
            "--index".into(),
            "file:///x/".into(),
        ])
        .unwrap();
        assert_eq!(o.tag.as_deref(), Some("v2.0.0"));
        assert!(o.skip_registry);
        assert_eq!(o.index, "file:///x");
        assert!(GateOpts::parse(&["--bogus".into()]).is_err());
        assert!(GateOpts::parse(&["--tag".into()]).is_err());
    }

    #[test]
    fn scratch_directories_are_fresh_private_and_distinct() {
        let a = private_scratch("snp-release-scratch-test").unwrap();
        let b = private_scratch("snp-release-scratch-test").unwrap();
        assert_ne!(a, b);
        assert!(a.is_dir() && b.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&a).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
        }
        std::fs::remove_dir_all(&a).unwrap();
        std::fs::remove_dir_all(&b).unwrap();
    }

    /// A self-contained release layout under a temporary directory: a workspace (git repo) with deps.lock,
    /// two sibling user crates (git repos) and a `file://` registry (sparse index + `.crate` files).
    #[cfg(unix)]
    mod e2e {
        use std::fmt::Write as _;

        use super::*;

        struct Fixture {
            base: PathBuf,
            root: PathBuf,
            revs: Vec<(String, String)>,
        }

        impl Drop for Fixture {
            fn drop(&mut self) {
                std::fs::remove_dir_all(&self.base).unwrap_or_default();
            }
        }

        fn sh(dir: &Path, args: &[&str]) -> String {
            let out = Command::new(args[0])
                .args(&args[1..])
                .current_dir(dir)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        }

        fn repo(dir: &Path, files: &[(&str, &str)]) -> String {
            std::fs::create_dir_all(dir).unwrap();
            sh(dir, &["git", "init", "-q", "-b", "main"]);
            for (f, body) in files {
                std::fs::write(dir.join(f), body).unwrap();
            }
            sh(dir, &["git", "add", "-A"]);
            sh(dir, &["git", "commit", "-q", "-m", "init"]);
            sh(dir, &["git", "rev-parse", "HEAD"])
        }

        /// Publishes `name-version.crate` with the given VCS sha into the file registry.
        fn publish(base: &Path, name: &str, version: &str, sha1: &str) {
            let stage = base.join("stage").join(format!("{name}-{version}"));
            std::fs::create_dir_all(&stage).unwrap();
            std::fs::write(
                stage.join(".cargo_vcs_info.json"),
                format!("{{\n  \"git\": {{\n    \"sha1\": \"{sha1}\"\n  }},\n  \"path_in_vcs\": \"\"\n}}"),
            )
            .unwrap();
            std::fs::write(
                stage.join("Cargo.toml"),
                format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\n"),
            )
            .unwrap();
            let dl = base.join("dl").join(name);
            std::fs::create_dir_all(&dl).unwrap();
            let crate_file = dl.join(format!("{name}-{version}.crate"));
            sh(
                &base.join("stage"),
                &[
                    "tar",
                    "-czf",
                    crate_file.to_str().unwrap(),
                    &format!("{name}-{version}"),
                ],
            );
            let digest = sha256(&crate_file).unwrap();
            let index = base.join("index").join(index_path(name));
            std::fs::create_dir_all(index.parent().unwrap()).unwrap();
            let mut lines = std::fs::read_to_string(&index).unwrap_or_default();
            writeln!(lines, "{{\"name\":\"{name}\",\"vers\":\"{version}\",\"cksum\":\"{digest}\",\"yanked\":false}}").unwrap();
            std::fs::write(&index, lines).unwrap();
        }

        fn fixture(tag: &str) -> Fixture {
            let base =
                std::env::temp_dir().join(format!("snp-release-e2e-{}-{tag}", std::process::id()));
            std::fs::remove_dir_all(&base).unwrap_or_default();
            let cfgv = repo(
                &base.join("cfgv-rs"),
                &[(
                    "Cargo.toml",
                    "[package]\nname = \"cfgv\"\nversion = \"0.3.0\"\n",
                )],
            );
            let fi = repo(
                &base.join("file-identify"),
                &[(
                    "Cargo.toml",
                    "[package]\nname = \"file-identify\"\nversion = \"0.5.0\"\n",
                )],
            );
            let lock = format!(
                "[[crate]]\nname = \"cfgv\"\nversion = \"0.3.0\"\npath = \"../cfgv-rs\"\nrepository = \"r\"\nbranch = \"snp-support\"\nrev = \"{cfgv}\"\n\n\
                 [[crate]]\nname = \"file-identify\"\nversion = \"0.5.0\"\npath = \"../file-identify\"\nrepository = \"r\"\nbranch = \"snp-support\"\nrev = \"{fi}\"\n"
            );
            let manifest = "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"2.0.0\"\n\n[workspace.dependencies]\n\
                            cfgv = { path = \"../cfgv-rs\", version = \"0.3\", default-features = false }\n\
                            file-identify = { path = \"../file-identify\", version = \"0.5\" }\n\
                            serde = \"1\"\nsnp-core = { path = \"crates/snp-core\", version = \"2.0.0\" }\n";
            let root = base.join("snp");
            let tagged = repo(
                &root,
                &[
                    ("Cargo.toml", manifest),
                    ("deps.lock", &lock),
                    (
                        "CHANGELOG.md",
                        "# Changelog\n\n## [2.0.0] - 2026-11-01\n\n- first\n",
                    ),
                ],
            );
            sh(&root, &["git", "tag", "v2.0.0", &tagged]);
            Fixture {
                base,
                root,
                revs: vec![("cfgv".into(), cfgv), ("file-identify".into(), fi)],
            }
        }

        fn opts(f: &Fixture) -> GateOpts {
            GateOpts {
                root: Some(f.root.clone()),
                tag: Some("v2.0.0".into()),
                index: format!("file://{}", f.base.join("index").display()),
                dl: format!("file://{}", f.base.join("dl").display()),
                ..GateOpts::default()
            }
        }

        #[test]
        fn release_gates_pass_from_a_clean_checkout_with_matching_publications() {
            let f = fixture("pass");
            for (name, rev) in &f.revs {
                let version = if name == "cfgv" { "0.3.0" } else { "0.5.0" };
                publish(
                    &f.base,
                    name,
                    "0.0.1",
                    "0000000000000000000000000000000000000000",
                );
                publish(&f.base, name, version, rev);
            }
            let r = gates(&opts(&f));
            assert_eq!(r.failures(), 0, "{}", render(&r));
        }

        #[test]
        fn release_gates_fail_when_a_published_user_crate_does_not_match_deps_lock() {
            let f = fixture("mismatch");
            let (cfgv_name, cfgv_rev) = &f.revs[0];
            publish(&f.base, cfgv_name, "0.3.0", cfgv_rev);
            // file-identify 0.5.0 was published from another commit
            publish(
                &f.base,
                "file-identify",
                "0.5.0",
                "1111111111111111111111111111111111111111",
            );
            let r = gates(&opts(&f));
            assert_eq!(r.failures(), 1, "{}", render(&r));
            assert!(r.failed("registry/file-identify"));
            assert!(render(&r).contains(
                "was published from 1111111111111111111111111111111111111111, deps.lock pins"
            ));
        }

        #[test]
        fn release_gates_fail_when_a_user_crate_is_not_published() {
            let f = fixture("unpublished");
            publish(&f.base, "cfgv", "0.2.0", &f.revs[0].1);
            let r = gates(&opts(&f));
            assert!(r.failed("registry/cfgv"), "{}", render(&r));
            assert!(
                render(&r).contains("cfgv 0.3.0 is not published on crates.io (published: 0.2.0)")
            );
            assert!(r.failed("registry/file-identify"));
            // --skip-registry: the local gates alone pass
            let mut o = opts(&f);
            o.skip_registry = true;
            assert_eq!(gates(&o).failures(), 0);
        }

        #[test]
        fn release_gates_fail_on_a_tampered_crate_or_a_yank() {
            let f = fixture("tamper");
            publish(&f.base, "cfgv", "0.3.0", &f.revs[0].1);
            publish(&f.base, "file-identify", "0.5.0", &f.revs[1].1);
            // tamper with the cfgv download after indexing
            let crate_file = f.base.join("dl/cfgv/cfgv-0.3.0.crate");
            let mut bytes = std::fs::read(&crate_file).unwrap();
            bytes.push(0);
            std::fs::write(&crate_file, bytes).unwrap();
            // yank file-identify 0.5.0
            let index = f.base.join("index").join(index_path("file-identify"));
            let text = std::fs::read_to_string(&index)
                .unwrap()
                .replace("\"yanked\":false", "\"yanked\":true");
            std::fs::write(&index, text).unwrap();
            let r = gates(&opts(&f));
            assert!(render(&r).contains("!= index cksum"), "{}", render(&r));
            assert!(render(&r).contains("file-identify 0.5.0 is yanked"));
        }

        #[test]
        fn release_gates_fail_on_dirty_trees_wrong_sibling_commits_tags_and_changelog() {
            let f = fixture("local");
            let mut o = opts(&f);
            o.skip_registry = true;
            o.tag = Some("v2.0.1".into());
            // a dirty snp checkout and an untracked file in a sibling
            std::fs::write(f.root.join("stray.txt"), "x").unwrap();
            std::fs::write(f.base.join("cfgv-rs/untracked.rs"), "x").unwrap();
            // file-identify moved past the pinned commit
            std::fs::write(f.base.join("file-identify/new.rs"), "x").unwrap();
            sh(&f.base.join("file-identify"), &["git", "add", "-A"]);
            sh(
                &f.base.join("file-identify"),
                &["git", "commit", "-q", "-m", "next"],
            );
            let r = gates(&o);
            let text = render(&r);
            assert!(r.failed("checkout"), "{text}");
            assert!(r.failed("sibling/cfgv"), "{text}");
            assert!(r.failed("sibling/file-identify"), "{text}");
            assert!(text.contains("deps.lock pins"), "{text}");
            assert!(r.failed("tag"), "{text}");
            // the right tag name, but HEAD moved past the tagged commit (a workflow_dispatch run on another ref)
            let mut moved = opts(&f);
            moved.skip_registry = true;
            moved.allow_dirty = true;
            sh(
                &f.root,
                &[
                    "git",
                    "commit",
                    "-q",
                    "--allow-empty",
                    "-m",
                    "after the release",
                ],
            );
            let r = gates(&moved);
            assert!(r.failed("tag"), "{}", render(&r));
            assert!(
                render(&r).contains("tag v2.0.0 does not point at HEAD"),
                "{}",
                render(&r)
            );
            // a tag that does not exist at all
            moved.tag = Some("v2.0.0".into());
            sh(&f.root, &["git", "tag", "-d", "v2.0.0"]);
            let r = gates(&moved);
            assert!(
                render(&r).contains("tag v2.0.0 does not exist"),
                "{}",
                render(&r)
            );
            // --allow-dirty skips only the snp checkout gate
            o.allow_dirty = true;
            o.tag = None;
            let r = gates(&o);
            assert!(!r.failed("checkout"));
            assert!(r.failed("sibling/cfgv"));
            // a changelog without the version section
            std::fs::remove_file(f.root.join("stray.txt")).unwrap();
            std::fs::write(
                f.root.join("CHANGELOG.md"),
                "# Changelog\n\n## [Unreleased]\n\n- wip\n",
            )
            .unwrap();
            let r = gates(&o);
            assert!(r.failed("changelog"), "{}", render(&r));
            o.allow_unreleased = true;
            assert!(!gates(&o).failed("changelog"));
        }

        #[test]
        fn release_gates_require_every_path_dependency_to_be_pinned() {
            let f = fixture("unpinned");
            let lock = std::fs::read_to_string(f.root.join("deps.lock")).unwrap();
            let only_cfgv = lock.split("\n\n").next().unwrap().to_owned() + "\n";
            std::fs::write(f.root.join("deps.lock"), only_cfgv).unwrap();
            let mut o = opts(&f);
            o.skip_registry = true;
            o.allow_dirty = true;
            let r = gates(&o);
            assert!(r.failed("lock/file-identify"), "{}", render(&r));
        }
    }

    /// The packaging scripts and the workflows WP-37 owns: the scripts are run (bash), the workflows read as text.
    mod packaging {
        use super::*;

        fn read(rel: &str) -> String {
            std::fs::read_to_string(workspace::root().unwrap().join(rel)).unwrap()
        }

        fn indent(line: &str) -> usize {
            line.len().saturating_sub(line.trim_start().len())
        }

        /// Every `run:` script of a workflow or composite action: the one-line value, or the block scalar that
        /// follows (`|`, `>-`, …) at a deeper indentation.
        fn run_scripts(text: &str) -> Vec<String> {
            let lines: Vec<&str> = text.lines().collect();
            let mut out = Vec::new();
            let mut i = 0;
            while i < lines.len() {
                let line = lines[i];
                let t = line.trim_start();
                let t = t.strip_prefix("- ").unwrap_or(t);
                i += 1;
                let Some(value) = t.strip_prefix("run:") else {
                    continue;
                };
                let value = value.trim();
                if value.starts_with('|') || value.starts_with('>') {
                    let key = indent(line);
                    let mut block = String::new();
                    while i < lines.len() && (lines[i].trim().is_empty() || indent(lines[i]) > key)
                    {
                        block.push_str(lines[i].trim());
                        block.push('\n');
                        i += 1;
                    }
                    out.push(block);
                } else {
                    out.push(value.to_owned());
                }
            }
            out
        }

        const WORKFLOWS: [&str; 4] = [
            ".github/workflows/platform.yml",
            ".github/workflows/release.yml",
            ".github/workflows/nightly.yml",
            "packaging/snp-action/action.yml",
        ];

        #[test]
        fn run_scripts_are_found() {
            let y = "steps:\n  - run: echo a\n  - name: b\n    run: |\n      one\n\n      two\n    env: {}\n  - run: >-\n      x\n      y\n";
            assert_eq!(run_scripts(y), ["echo a", "one\n\ntwo\n", "x\ny\n"]);
            for w in WORKFLOWS {
                assert!(!run_scripts(&read(w)).is_empty(), "{w}");
            }
        }

        /// Values an attacker can choose (a tag name, a dispatch input, an event payload) reach scripts only
        /// through the environment, never as `${{ … }}` pasted into the script text.
        #[test]
        fn scripts_never_paste_untrusted_expressions() {
            for w in WORKFLOWS {
                for script in run_scripts(&read(w)) {
                    for bad in [
                        "${{ steps.",
                        "${{ inputs.",
                        "${{ github.event",
                        "${{ github.head_ref",
                        "${{ github.ref",
                    ] {
                        assert!(
                            !script.contains(bad),
                            "{w}: `{bad}` in the script {script:?}"
                        );
                    }
                }
            }
        }

        /// The flags of `snp-difftest` (crates/snp-difftest/src/main.rs, WP-35) that the workflows may use.
        #[test]
        fn difftest_calls_use_the_harness_interface() {
            let mut calls = 0;
            for w in WORKFLOWS {
                for script in run_scripts(&read(w)) {
                    for line in script
                        .lines()
                        .filter(|l| l.trim_start().starts_with("cargo xtask difftest"))
                    {
                        calls += 1;
                        let rest = line.split("cargo xtask difftest").nth(1).unwrap();
                        let words: Vec<&str> = rest
                            .split_whitespace()
                            .take_while(|w| !matches!(*w, "|" | ">" | "&&" | ";"))
                            .collect();
                        assert!(
                            matches!(words.first(), Some(&("run" | "list"))),
                            "{w}: {line}"
                        );
                        let mut it = words.iter().skip(1);
                        while let Some(flag) = it.next() {
                            match *flag {
                                "--suite" => assert!(
                                    matches!(
                                        it.next(),
                                        Some(
                                            &("seed"
                                                | "curated"
                                                | "divergences"
                                                | "all"
                                                | "generated")
                                        )
                                    ),
                                    "{w}: {line}"
                                ),
                                "--file" | "--tag" | "--cases" | "--seed" | "--scenario"
                                | "--out" => {
                                    assert!(it.next().is_some(), "{w}: {line}");
                                }
                                "--keep" | "--no-shrink" => {}
                                other => panic!("{w}: unknown difftest flag {other:?} in {line}"),
                            }
                        }
                    }
                }
            }
            assert!(calls >= 4, "{calls}");
        }

        #[test]
        fn nightly_leaves_the_performance_gates_to_bench_yml() {
            let nightly = read(".github/workflows/nightly.yml");
            assert!(!nightly.contains("cargo xtask bench"));
            assert!(!nightly.contains("\n  bench:"));
            assert!(read(".github/workflows/bench.yml").contains("schedule:"));
        }

        #[test]
        fn a_manual_release_runs_from_its_tag() {
            let release = read(".github/workflows/release.yml");
            assert!(release.contains(r#"if [ "$REF" != "refs/tags/$tag" ]; then"#));
            assert!(release.contains(r#"cargo xtask release notes "$TAG""#));
            assert!(release.contains("rust-audit-info \"$bin\""));
            assert!(!release.contains("grep -aqF '.dep-v0'"));
        }

        #[test]
        fn the_git_matrix_is_blocking() {
            let platform = read(".github/workflows/platform.yml");
            assert!(
                !platform.contains("continue-on-error"),
                "the git axis must not hide regressions"
            );
            assert!(platform.contains("packaging/ci/git-tests.sh"));
            // every git of the matrix is pinned in build-git.sh, and each known-failure list names single tests
            let build = read("packaging/ci/build-git.sh");
            let root = workspace::root().unwrap();
            for line in platform
                .lines()
                .filter(|l| l.trim_start().starts_with("- { git: "))
            {
                let v = line
                    .split("git: ")
                    .nth(1)
                    .unwrap()
                    .split(',')
                    .next()
                    .unwrap()
                    .trim();
                assert!(
                    build.contains(&format!("\n  {v}) sha256=")),
                    "{v} is not pinned"
                );
                let known = root.join(format!("packaging/ci/git-known-failures/{v}.txt"));
                if let Ok(text) = std::fs::read_to_string(&known) {
                    for l in text
                        .lines()
                        .filter(|l| !l.is_empty() && !l.starts_with('#'))
                    {
                        assert!(
                            (l.starts_with("package(snp-") || l.starts_with("package(snp) "))
                                && l.contains(" & test(=")
                                && l.ends_with(')'),
                            "{}: {l}",
                            known.display()
                        );
                    }
                }
            }
        }

        #[test]
        fn build_git_defaults_to_a_bounded_job_count() {
            let script = read("packaging/ci/build-git.sh");
            assert!(script.contains("jobs=${BUILD_GIT_JOBS:-$((cores < 4 ? cores : 4))}"));
            assert!(script.contains("nice -n 19 make"));
            assert!(!script.contains("$(nproc)}"));
        }

        #[cfg(unix)]
        mod scripts {
            use super::*;

            struct Scratch(PathBuf);
            impl Drop for Scratch {
                fn drop(&mut self) {
                    std::fs::remove_dir_all(&self.0).unwrap_or_default();
                }
            }

            fn scratch() -> Scratch {
                Scratch(private_scratch("snp-xtask-packaging").unwrap())
            }

            fn bash() -> Command {
                let mut c = Command::new("bash");
                c.current_dir(workspace::root().unwrap())
                    .env_remove("SNP_HOME")
                    .env_remove("XDG_CACHE_HOME")
                    .env_remove("SNP_ACTION_EXTRA_ARGS")
                    .env_remove("SNP_ACTION_SNP");
                c
            }

            /// A fake `snp` that prints each argument as `[arg]` and exits with `$FAKE_SNP_EXIT`.
            fn fake_snp(dir: &Path) -> PathBuf {
                use std::os::unix::fs::PermissionsExt as _;
                let p = dir.join("snp");
                std::fs::write(
                    &p,
                    concat!(
                        "#!/usr/bin/env bash\n",
                        "printf '[%s]' \"$@\"\n",
                        "exit \"$",
                        "{FAKE_SNP_EXIT:-0}\"\n",
                    ),
                )
                .unwrap();
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
                p
            }

            fn run_action(snp: &Path, extra: Option<&str>) -> (String, Option<i32>) {
                let mut c = bash();
                c.arg("packaging/snp-action/run.sh")
                    .env("SNP_ACTION_SNP", snp)
                    .env("BASE", "abc def");
                if let Some(e) = extra {
                    c.env("SNP_ACTION_EXTRA_ARGS", e);
                }
                let o = c.output().unwrap();
                (String::from_utf8(o.stdout).unwrap(), o.status.code())
            }

            /// pre-commit/action's step: the input pasted into the bash script.
            fn run_pre_commit_action(snp: &Path, extra: &str) -> String {
                let o = bash()
                    .arg("-c")
                    .arg(format!(
                        "\"$SNP\" run --show-diff-on-failure --color=always {extra}"
                    ))
                    .env("SNP", snp)
                    .env("BASE", "abc def")
                    .output()
                    .unwrap();
                String::from_utf8(o.stdout).unwrap()
            }

            #[test]
            fn snp_action_parses_extra_args_like_pre_commit_action() {
                let d = scratch();
                let snp = fake_snp(&d.0);
                let head = "[run][--show-diff-on-failure][--color=always]";
                for (extra, args) in [
                    ("--all-files", "[--all-files]"),
                    (r#"--files "a b.py" c.py"#, "[--files][a b.py][c.py]"),
                    ("--files 'it''s.py'", "[--files][its.py]"),
                    (
                        r#"--from-ref "$BASE" --to-ref HEAD"#,
                        "[--from-ref][abc def][--to-ref][HEAD]",
                    ),
                    ("--files $(printf 'x.py y.py')", "[--files][x.py][y.py]"),
                    ("  --hook-stage   manual  ", "[--hook-stage][manual]"),
                    ("", ""),
                ] {
                    let (out, code) = run_action(&snp, Some(extra));
                    assert_eq!(code, Some(0), "{extra:?}");
                    assert_eq!(out, format!("{head}{args}"), "{extra:?}");
                    assert_eq!(out, run_pre_commit_action(&snp, extra), "{extra:?}");
                }
                // no input at all: no extra arguments (the action's default `--all-files` comes from action.yml)
                assert_eq!(run_action(&snp, None).0, head);
                // snp's exit status is the step's
                let o = bash()
                    .arg("packaging/snp-action/run.sh")
                    .env("SNP_ACTION_SNP", &snp)
                    .env("SNP_ACTION_EXTRA_ARGS", "--all-files")
                    .env("FAKE_SNP_EXIT", "3")
                    .output()
                    .unwrap();
                assert_eq!(o.status.code(), Some(3));
                let action = read("packaging/snp-action/action.yml");
                assert!(action.contains(r#"run: '"$GITHUB_ACTION_PATH/run.sh"'"#));
                assert!(!action.contains("$SNP_ACTION_EXTRA_ARGS\n"));
            }

            #[test]
            fn snp_action_caches_the_directory_snp_uses() {
                let dir = |vars: &[(&str, &str)]| {
                    let mut c = bash();
                    c.arg("packaging/snp-action/cache-dir.sh");
                    for (k, v) in vars {
                        c.env(k, v);
                    }
                    let o = c.output().unwrap();
                    assert!(o.status.success());
                    String::from_utf8(o.stdout).unwrap()
                };
                assert_eq!(dir(&[]), "~/.cache/snp\n");
                assert_eq!(
                    dir(&[("SNP_HOME", ""), ("XDG_CACHE_HOME", "")]),
                    "~/.cache/snp\n"
                );
                assert_eq!(dir(&[("XDG_CACHE_HOME", "/x/cache")]), "/x/cache/snp\n");
                assert_eq!(dir(&[("XDG_CACHE_HOME", "/x/cache/")]), "/x/cache/snp\n");
                assert_eq!(dir(&[("SNP_HOME", "/s"), ("XDG_CACHE_HOME", "/x")]), "/s\n");
                assert_eq!(
                    dir(&[("SNP_HOME", ""), ("XDG_CACHE_HOME", "/x")]),
                    "/x/snp\n"
                );
                let action = read("packaging/snp-action/action.yml");
                assert!(action.contains("path: ${{ steps.cache-dir.outputs.path }}"));
                assert!(!action.contains("path: ~/.cache/snp"));
            }

            #[test]
            fn build_git_refuses_a_bad_job_count_before_downloading() {
                for jobs in ["x", "0", "-1", "2 3"] {
                    let o = bash()
                        .arg("packaging/ci/build-git.sh")
                        .args(["2.18.5", "/nonexistent/snp-xtask-git"])
                        .env("BUILD_GIT_JOBS", jobs)
                        .output()
                        .unwrap();
                    assert_eq!(o.status.code(), Some(2), "{jobs:?}");
                    assert!(
                        String::from_utf8_lossy(&o.stderr).contains("BUILD_GIT_JOBS"),
                        "{jobs:?}"
                    );
                }
            }
        }
    }
}
