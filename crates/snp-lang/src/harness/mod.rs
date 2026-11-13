//! 04b §5.12 shim harness: golden argv/cwd/env logs per language (feature `testing`).
//!
//! Backends run setup and probe commands only through [`CmdRunner`]; in tests the runner is a [`ShimRunner`], which
//! records every call ([`Recorded`]: argv as given, cwd, environment delta against a base env) and replays scripted
//! [`Reply`]s (optionally with a side effect, e.g. creating the files the next step expects). WP-24/WP-25 compare
//! these records with logs recorded from pre-commit 4.7.0 running the same scenario with fake tools on `PATH`
//! ([`install_shims`]), both rendered in the **golden log format v1**:
//!
//! * UTF-8 text, one JSON object per line and per call, in call order, compact separators (`,` `:`), no ASCII
//!   escaping (`json.dumps(…, ensure_ascii=False, separators=(',', ':'))` on the Python side, `serde_json` here);
//! * `{"argv": [tool, arg1, …], "cwd": "…", "env": {"NAME": "value" | null, …}}`: `tool` is the basename of
//!   `argv[0]` without a Windows `.exe`/`.cmd`/`.bat` suffix (pre-commit resolves `argv[0]` to an absolute path and
//!   may prepend a shebang interpreter; snp's runner sees it unresolved, so only the tool name is comparable); `env`
//!   holds the variables whose value differs from the base env (`null`: removed), keys sorted;
//! * paths that differ between the two runs (temporary directories, the store, the home) are replaced by
//!   placeholders with [`Substitutions`] on both sides before comparing (longest literal first);
//! * non-UTF-8 bytes are replaced with U+FFFD (record on Linux with UTF-8 paths).
//!
//! The Python side ([`SHIM_LOGGER_PY`], installed under each tool's name by [`install_shims`]) appends one line to
//! `$SNP_SHIM_LOG`, computing the delta against the JSON object in `$SNP_SHIM_BASE` (the environment the oracle was
//! started with plus `PRE_COMMIT=1`, which `pre-commit run` sets before installing), and answers from the JSON list in
//! `$SNP_SHIM_REPLIES` (`[{"tool": …, "when": [args prefix], "status": 0, "stdout": "…", "stderr": "…"}]`, first
//! match, every reply sticky); variables named `SNP_SHIM_*` are never logged.
//!
//! Recording and replay are deterministic: replies are matched in insertion order, the delta is sorted, and nothing
//! depends on time or randomness (`tests/framework` self-test).
//!
//! Owner: WP-16.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use snp_core::brand::Brand;
use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_core::failure::Failure;
use snp_core::output::{Printer, PrinterOpts};
use snp_core::sync::lock;
use snp_net::Fetch;
use snp_os::lock::{FileLock, LockMode};
use snp_proc::spawn::{Captured, PyStatus};

use crate::{CmdRunner, Host, LangError, ToolDirs, ToolProbe};

/// A side effect of a scripted call (create the env layout, write a file the next step reads, …).
pub type Effect = Arc<dyn Fn(&Recorded) -> io::Result<()> + Send + Sync>;

/// One scripted answer of a [`ShimRunner`].
#[derive(Clone)]
pub struct Reply {
    /// The reply matches calls whose arguments (after the tool) start with these; empty: any call of the tool.
    pub when: Vec<OsString>,
    /// Exit status.
    pub status: i64,
    /// stdout bytes.
    pub stdout: Vec<u8>,
    /// stderr bytes.
    pub stderr: Vec<u8>,
    /// Answer every matching call (else: consumed by the first one).
    pub sticky: bool,
    /// Run before answering (an error fails the call with an I/O error).
    pub effect: Option<Effect>,
}

impl fmt::Debug for Reply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reply")
            .field("when", &self.when)
            .field("status", &self.status)
            .field("stdout", &String::from_utf8_lossy(&self.stdout))
            .field("stderr", &String::from_utf8_lossy(&self.stderr))
            .field("sticky", &self.sticky)
            .field("effect", &self.effect.is_some())
            .finish()
    }
}

impl Reply {
    /// Exit 0 with `stdout`.
    pub fn ok(stdout: impl Into<Vec<u8>>) -> Self {
        Self {
            when: Vec::new(),
            status: 0,
            stdout: stdout.into(),
            stderr: Vec::new(),
            sticky: false,
            effect: None,
        }
    }

    /// Exit `status` with `stderr`.
    pub fn fail(status: i64, stderr: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            stderr: stderr.into(),
            ..Self::ok(Vec::new())
        }
    }

    /// Only for calls whose arguments start with `args`.
    #[must_use]
    pub fn when<I: IntoIterator<Item = S>, S: Into<OsString>>(mut self, args: I) -> Self {
        self.when = args.into_iter().map(Into::into).collect();
        self
    }

    /// Answers every matching call.
    #[must_use]
    pub const fn sticky(mut self) -> Self {
        self.sticky = true;
        self
    }

    /// Runs `f` with the recorded call before answering.
    #[must_use]
    pub fn with_effect(
        mut self,
        f: impl Fn(&Recorded) -> io::Result<()> + Send + Sync + 'static,
    ) -> Self {
        self.effect = Some(Arc::new(f));
        self
    }
}

/// Scripted responses keyed by `argv[0]` basename; records calls.
///
/// Unscripted calls succeed with empty output (exit 0).
#[derive(Debug, Default)]
pub struct ShimRunner {
    /// The env deltas are computed against this (empty by default).
    base: Option<Env>,
    /// (tool, reply) in insertion order.
    replies: Mutex<Vec<(String, Reply)>>,
    /// Every call, in order.
    calls: Mutex<Vec<Recorded>>,
}

impl ShimRunner {
    /// A runner recording env deltas against `base` (usually the host's base env).
    pub fn new(base: Env) -> Self {
        Self {
            base: Some(base),
            ..Self::default()
        }
    }

    /// Scripts `reply` for calls of `tool` (the basename of `argv[0]`, see [`tool_name`]).
    pub fn reply(&self, tool: &str, reply: Reply) {
        lock(&self.replies).push((tool.to_owned(), reply));
    }

    /// The calls so far.
    pub fn calls(&self) -> Vec<Recorded> {
        lock(&self.calls).clone()
    }

    /// The calls so far as golden-log entries.
    pub fn golden(&self, subst: &Substitutions) -> Vec<GoldenCall> {
        lock(&self.calls)
            .iter()
            .map(|r| GoldenCall::from_recorded(r, subst))
            .collect()
    }

    /// The calls so far in the golden log format v1 (one JSON line per call).
    pub fn golden_log(&self, subst: &Substitutions) -> String {
        render_log(&self.golden(subst))
    }

    /// The first reply for `tool` matching `args`; consumed unless sticky.
    fn answer(&self, tool: &str, args: &[OsString]) -> Option<Reply> {
        let mut replies = lock(&self.replies);
        let idx = replies
            .iter()
            .position(|(t, r)| t == tool && args.starts_with(&r.when))?;
        let reply = replies.get(idx)?.1.clone();
        if !reply.sticky {
            // Order-preserving removal without the panicking `Vec::remove` (ADR-066).
            let mut i = 0_usize;
            replies.retain(|_| {
                let keep = i != idx;
                i = i.saturating_add(1);
                keep
            });
        }
        Some(reply)
    }
}

impl CmdRunner for ShimRunner {
    fn run(&self, argv: &[OsString], env: &Env, cwd: &Path) -> Result<Captured, LangError> {
        let recorded = Recorded {
            argv: argv.to_vec(),
            cwd: cwd.to_path_buf(),
            env_delta: env_delta(self.base.as_ref(), env),
        };
        let tool = argv.first().map_or_default(|a| tool_name(a));
        let args = argv.get(1..).unwrap_or_default();
        lock(&self.calls).push(recorded.clone());
        let reply = self
            .answer(&tool, args)
            .unwrap_or_else(|| Reply::ok(Vec::new()));
        if let Some(effect) = &reply.effect {
            effect(&recorded).map_err(|err| LangError::Io {
                op: "shim effect",
                path: cwd.to_path_buf(),
                err,
            })?;
        }
        Ok(Captured {
            status: PyStatus(reply.status),
            stdout: reply.stdout,
            stderr: reply.stderr,
            stopped_by_tty: false,
        })
    }
}

/// One recorded call: argv, cwd and the environment delta against the host env.
#[derive(Debug, Clone)]
pub struct Recorded {
    /// The argv as passed to the runner.
    pub argv: Vec<OsString>,
    /// The cwd.
    pub cwd: PathBuf,
    /// (name, Some(value) | None for removed) relative to the base env.
    pub env_delta: Vec<(OsString, Option<OsString>)>,
}

/// The variables of `env` that differ from `base` (`None`: removed), sorted by name.
fn env_delta(base: Option<&Env>, env: &Env) -> Vec<(OsString, Option<OsString>)> {
    let mut out: Vec<(OsString, Option<OsString>)> = Vec::new();
    for (k, v) in env.iter() {
        let before = base.and_then(|b| b.get(&k.to_string_lossy()));
        if before != Some(v) {
            out.push((k.to_os_string(), Some(v.to_os_string())));
        }
    }
    if let Some(b) = base {
        for (k, _) in b.iter() {
            if env.get(&k.to_string_lossy()).is_none() {
                out.push((k.to_os_string(), None));
            }
        }
    }
    out.sort();
    out
}

/// The tool name of `argv0`: its basename without a `.exe`/`.cmd`/`.bat` suffix (ASCII case-insensitive).
pub fn tool_name(argv0: &OsStr) -> String {
    let base = Path::new(argv0)
        .file_name()
        .map_or_else(|| argv0.to_string_lossy(), OsStr::to_string_lossy)
        .into_owned();
    for ext in [".exe", ".cmd", ".bat"] {
        if base.len() > ext.len()
            && base
                .get(base.len().saturating_sub(ext.len())..)
                .is_some_and(|e| e.eq_ignore_ascii_case(ext))
        {
            return base
                .get(..base.len().saturating_sub(ext.len()))
                .unwrap_or_default()
                .to_owned();
        }
    }
    base
}

/// Placeholders for the paths that differ between two runs (applied to argv, cwd and env values, longest first).
#[derive(Debug, Clone, Default)]
pub struct Substitutions {
    /// (literal, placeholder).
    pairs: Vec<(String, String)>,
}

impl Substitutions {
    /// No substitution.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces `literal` (e.g. a temporary path) with `placeholder` (e.g. `{PREFIX}`).
    #[must_use]
    pub fn with(mut self, literal: impl AsRef<OsStr>, placeholder: &str) -> Self {
        let lit = literal.as_ref().to_string_lossy().into_owned();
        if !lit.is_empty() {
            self.pairs.push((lit, placeholder.to_owned()));
            self.pairs.sort_by_key(|a| std::cmp::Reverse(a.0.len()));
        }
        self
    }

    /// `s` with every literal replaced.
    pub fn apply(&self, s: &str) -> String {
        let mut out = s.to_owned();
        for (lit, ph) in &self.pairs {
            out = out.replace(lit.as_str(), ph);
        }
        out
    }
}

/// One call in the golden log format v1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoldenCall {
    /// Tool name, then the arguments.
    pub argv: Vec<String>,
    /// The cwd.
    pub cwd: String,
    /// Changed (`Some`) and removed (`None`) variables.
    pub env: BTreeMap<String, Option<String>>,
}

impl GoldenCall {
    /// The golden form of a recorded call.
    pub fn from_recorded(r: &Recorded, subst: &Substitutions) -> Self {
        let lossy = |s: &OsStr| subst.apply(&s.to_string_lossy());
        let mut argv: Vec<String> = Vec::new();
        if let Some((first, rest)) = r.argv.split_first() {
            argv.push(tool_name(first));
            argv.extend(rest.iter().map(|a| lossy(a)));
        }
        Self {
            argv,
            cwd: lossy(r.cwd.as_os_str()),
            env: r
                .env_delta
                .iter()
                .map(|(k, v)| (k.to_string_lossy().into_owned(), v.as_deref().map(lossy)))
                .collect(),
        }
    }

    /// The same call keeping only the variables in `names` (scenarios compare the variables a backend sets).
    #[must_use]
    pub fn only_env(mut self, names: &[&str]) -> Self {
        self.env.retain(|k, _| names.contains(&k.as_str()));
        self
    }
}

/// Renders calls in the golden log format v1.
pub fn render_log(calls: &[GoldenCall]) -> String {
    let mut out = String::new();
    for c in calls {
        if let Ok(line) = serde_json::to_string(c) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

/// Parses a golden log (snp's or one recorded from pre-commit by [`SHIM_LOGGER_PY`]) and applies `subst` to it.
pub fn parse_log(text: &str, subst: &Substitutions) -> Result<Vec<GoldenCall>, String> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .enumerate()
        .map(|(i, line)| {
            let c: GoldenCall = serde_json::from_str(line)
                .map_err(|e| format!("line {}: {e}", i.saturating_add(1)))?;
            Ok(GoldenCall {
                argv: c.argv.iter().map(|a| subst.apply(a)).collect(),
                cwd: subst.apply(&c.cwd),
                env: c
                    .env
                    .into_iter()
                    .map(|(k, v)| (k, v.map(|v| subst.apply(&v))))
                    .collect(),
            })
        })
        .collect()
}

/// The fake-tool logger of the pre-commit side (golden log format v1, see the module docs).
pub const SHIM_LOGGER_PY: &str = r"# snp shim logger, golden log format v1 (crates/snp-lang/src/harness). Records one JSON line per call into
# $SNP_SHIM_LOG and answers from $SNP_SHIM_REPLIES. Generated by snp_lang::harness::install_shims; do not edit.
import json
import os
import sys


def main():
    tool = os.path.basename(sys.argv[0])
    for ext in ('.exe', '.cmd', '.bat'):
        if len(tool) > len(ext) and tool.lower().endswith(ext):
            tool = tool[:-len(ext)]
    base = {}
    if os.environ.get('SNP_SHIM_BASE'):
        with open(os.environ['SNP_SHIM_BASE'], encoding='utf-8') as f:
            base = json.load(f)
    env = {k: v for k, v in os.environ.items() if not k.startswith('SNP_SHIM_')}
    delta = {}
    for k in sorted(set(env) | set(base)):
        if k.startswith('SNP_SHIM_'):
            continue
        if k not in env:
            delta[k] = None
        elif base.get(k) != env[k]:
            delta[k] = env[k]
    record = {'argv': [tool, *sys.argv[1:]], 'cwd': os.getcwd(), 'env': delta}
    line = json.dumps(record, ensure_ascii=False, separators=(',', ':'))
    with open(os.environ['SNP_SHIM_LOG'], 'a', encoding='utf-8', errors='replace') as f:
        f.write(line + '\n')
    replies = []
    if os.environ.get('SNP_SHIM_REPLIES'):
        with open(os.environ['SNP_SHIM_REPLIES'], encoding='utf-8') as f:
            replies = json.load(f)
    for r in replies:
        when = r.get('when', [])
        if r.get('tool') == tool and sys.argv[1:1 + len(when)] == when:
            sys.stdout.write(r.get('stdout', ''))
            sys.stdout.flush()
            sys.stderr.write(r.get('stderr', ''))
            sys.stderr.flush()
            return r.get('status', 0)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
";

/// Installs the [`SHIM_LOGGER_PY`] fake tools named `tools` into `dir` (POSIX: one executable copy per name with a
/// `#!<python> -I` line; Windows: unsupported, record on Linux).
pub fn install_shims(dir: &Path, tools: &[&str], python: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir)?;
        for tool in tools {
            let p = dir.join(tool);
            let mut f = std::fs::File::create(&p)?;
            writeln!(f, "#!{} -I", python.display())?;
            f.write_all(SHIM_LOGGER_PY.as_bytes())?;
            drop(f);
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755))?;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _: (&Path, &[&str], &Path) = (dir, tools, python);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "shim recording needs a POSIX host",
        ))
    }
}

/// Writes `env` as the JSON object `$SNP_SHIM_BASE` points to (lossy UTF-8).
pub fn write_base_env(path: &Path, env: &Env) -> io::Result<()> {
    let map: BTreeMap<String, String> = env
        .iter()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.to_string_lossy().into_owned(),
            )
        })
        .collect();
    let bytes = serde_json::to_vec(&map).map_err(io::Error::other)?;
    std::fs::write(path, bytes)
}

/// A temp-dir [`ToolDirs`]: `<root>/tools/<tool>/<version>`, `<root>/tools/<tool>.lock`, `<root>/cache/<name>`.
#[derive(Debug, Clone)]
pub struct TempToolDirs {
    /// The root.
    root: PathBuf,
}

impl TempToolDirs {
    /// Tool directories below `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl ToolDirs for TempToolDirs {
    fn tool_dir(&self, tool: &str, version: &str) -> PathBuf {
        self.root.join("tools").join(tool).join(version)
    }

    fn lock_tool(&self, tool: &str, c: &CancelToken) -> Result<FileLock, Failure> {
        let dir = self.root.join("tools");
        std::fs::create_dir_all(&dir)
            .map_err(|e| Failure::from_io(&e, Some(dir.as_os_str()), None))?;
        let path = dir.join(format!("{tool}.lock"));
        FileLock::acquire(&path, LockMode::Exclusive, &mut || {}, c, None)
            .map_err(|e| Failure::from_io(&e, Some(path.as_os_str()), None))
    }

    fn cache_dir(&self, name: &str) -> PathBuf {
        self.root.join("cache").join(name)
    }
}

/// A `Write` into a shared buffer (captured printer output).
#[derive(Debug, Clone, Default)]
pub struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl SharedBuf {
    /// The bytes written so far.
    pub fn bytes(&self) -> Vec<u8> {
        lock(&self.0).clone()
    }
}

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        lock(&self.0).extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Everything a [`Host`] borrows, owned (tests of the framework and of the language groups).
#[derive(Debug)]
pub struct TestHost {
    /// The base environment.
    pub env: Env,
    /// The run's cwd (the repository root).
    pub cwd: PathBuf,
    /// snp's executable as hooks see it.
    pub self_exe: PathBuf,
    /// The probe cache.
    pub probe: ToolProbe,
    /// Tool directories.
    pub tools: TempToolDirs,
    /// The recording runner.
    pub runner: ShimRunner,
    /// Downloads (None: offline).
    pub fetch: Option<Box<dyn Fetch>>,
    /// The printer (output captured in `stdout`/`stderr`).
    pub printer: Printer,
    /// Captured stdout.
    pub stdout: SharedBuf,
    /// Captured stderr.
    pub stderr: SharedBuf,
    /// The cancel token.
    pub cancel: CancelToken,
    /// The brand.
    pub brand: Brand,
}

impl TestHost {
    /// A host with `env` as base env, `root` as cwd, tool dirs below `root/.snp-tools`, a [`ShimRunner`] recording
    /// against `env`, no download and the snp brand.
    pub fn new(root: &Path, env: Env) -> Self {
        let cancel = CancelToken::new();
        let (stdout, stderr) = (SharedBuf::default(), SharedBuf::default());
        let printer = Printer::new(
            Box::new(stdout.clone()),
            Box::new(stderr.clone()),
            PrinterOpts {
                brand: Brand::Snp,
                color: false,
                stdout_is_tty: false,
                windows_text_mode: false,
                cancel: cancel.clone(),
            },
        );
        Self {
            runner: ShimRunner::new(env.clone()),
            env,
            cwd: root.to_path_buf(),
            self_exe: PathBuf::from(if cfg!(windows) {
                "C:\\snp\\snp.exe"
            } else {
                "/opt/snp/bin/snp"
            }),
            probe: ToolProbe::new(None),
            tools: TempToolDirs::new(root.join(".snp-tools")),
            fetch: None,
            printer,
            stdout,
            stderr,
            cancel,
            brand: Brand::Snp,
        }
    }

    /// The borrowed [`Host`].
    pub fn host(&self) -> Host<'_> {
        Host {
            base_env: &self.env,
            cwd: &self.cwd,
            self_exe: &self.self_exe,
            probe: &self.probe,
            fetch: self.fetch.as_deref(),
            tools: &self.tools,
            runner: &self.runner,
            printer: &self.printer,
            cancel: &self.cancel,
            brand: self.brand,
        }
    }
}
