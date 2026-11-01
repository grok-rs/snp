//! `cargo xtask platform-facts [--out DIR] [--python EXE] [--snp EXE] [--strict]`: record the facts of the
//! platform this runs on and check the derived Windows/macOS predictions of 14 §A.6 against them (14 §A.6.3, §A.6.5;
//! WBS WP-37).
//!
//! Every Windows and macOS claim of the research was observed on Linux, under Wine or derived from source (14 §A.0).
//! The `platform` workflow runs this task on the real runners (`windows-2025`, `windows-11-arm`, `macos-15`,
//! `macos-15-intel`, Linux as the control) and publishes its output as an artifact:
//!
//! * `facts.json`: `git --version`, the git configuration that changes observable behaviour (`core.autocrlf`,
//!   `core.filemode`, `core.symlinks`, `core.ignorecase`, `core.precomposeunicode`, `core.fsmonitor`, …, with origin
//!   and scope, plus the values `git init` probes on this file system), `PATHEXT` and the `where`-style lookups of
//!   `sh bash env python git`, the environment a git-spawned native program sees (Git for Windows' `MSYSTEM`/`TERM`/
//!   `PATH` additions, 14 A.1 #15), the Python facts pre-commit depends on (`os.cpu_count`, `process_cpu_count`,
//!   `PYTHON_CPU_COUNT`, UTF-8 mode, `isatty` of a `NUL`/`/dev/null` stdin vs an empty pipe, `os.access(X_OK)` of a
//!   plain file, `os.environ` key case, `print()` line endings into a pipe, the unsigned exit code of `cmd /c exit -1`,
//!   the `[WinError 193]` text), the Rust facts snp depends on (`available_parallelism`, `IsTerminal` of a null
//!   stdin, `ExitStatus` of the same child), file-system facts (temp-dir realpath — `/tmp` → `/private/tmp` on macOS —,
//!   case sensitivity, Unicode normalisation of names, symlink creation) and the OS version (`ver`/`systeminfo`,
//!   `sw_vers`, `/etc/os-release`), `ulimit -n` and `ARG_MAX`; with `--snp`, `snp --version` and the hidden
//!   `snp __debug platform` report;
//! * `report.md`: one row per prediction (ID, 14 § reference, claim, verdict, observed value): **confirmed**,
//!   **contradicted** or **unknown** (fact not available on this runner); also appended to `$GITHUB_STEP_SUMMARY`;
//! * `corrections.md`: only when a prediction is contradicted; the workflow files it as an issue, and 14 §A.6.5
//!   requires the matrix to be corrected in the PR that fixes it.
//!
//! The task always prints and never fails (14 §A.6.3): a missing tool is recorded as `unavailable: …`. `--strict`
//! (local use) turns a contradicted prediction into a failure. The hidden `--probe-stdin` / `--print-env` modes are the
//! child processes the task spawns of itself.
//!
//! Owner: WP-37.

use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::io::{IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, bail};
use serde_json::{Map, Value, json};
use snp_core::env::Env;

use crate::workspace;

/// Git configuration keys whose values change observable behaviour (14 §A.6.3).
const GIT_KEYS: &[&str] = &[
    "core.autocrlf",
    "core.filemode",
    "core.symlinks",
    "core.ignorecase",
    "core.precomposeunicode",
    "core.fsmonitor",
    "core.useBuiltinFSMonitor",
    "core.longpaths",
    "core.hooksPath",
    "init.defaultBranch",
    "init.templateDir",
    "safe.directory",
];

/// Keys `git init` writes after probing the file system.
const GIT_INIT_KEYS: &[&str] = &[
    "core.filemode",
    "core.symlinks",
    "core.ignorecase",
    "core.precomposeunicode",
];

/// Programs looked up on `PATH` the way `where`/`command -v -a` would.
const WHERE: &[&str] = &["git", "sh", "bash", "env", "python", "python3", "py"];

/// Environment variables reported from the git-spawned child (14 A.1 #15, W02).
const HOOK_ENV_KEYS: &[&str] = &[
    "MSYSTEM",
    "TERM",
    "COLORTERM",
    "HOME",
    "TMPDIR",
    "LC_CTYPE",
    "SHELL",
    "EXEPATH",
    "GIT_EXEC_PATH",
];

/// Runner identification variables (GitHub Actions images).
const RUNNER_KEYS: &[&str] = &[
    "RUNNER_OS",
    "RUNNER_ARCH",
    "ImageOS",
    "ImageVersion",
    "PROCESSOR_ARCHITECTURE",
    "PROCESSOR_IDENTIFIER",
];

/// The Python facts probe (run with `-I`, cwd = a scratch directory).
const PY_PROBE: &str = r#"
import json, os, subprocess, sys, tempfile
out = {}
out["version"] = "%d.%d.%d" % sys.version_info[:3]
out["executable"] = sys.executable
out["platform"] = sys.platform
out["cpu_count"] = os.cpu_count()
out["process_cpu_count"] = getattr(os, "process_cpu_count", lambda: None)()
out["utf8_mode"] = sys.flags.utf8_mode
try:
    out["stdin_isatty"] = sys.stdin.isatty()
except Exception as e:
    out["stdin_isatty"] = "error: %r" % (e,)
d = tempfile.mkdtemp()
p = os.path.join(d, "plain.txt")
open(p, "w").close()
out["access_x_ok_plain_file"] = os.access(p, os.X_OK)
out["environ_upper_cases_keys"] = "SNPFACTSMIXEDCASE" in os.environ and "SnpFactsMixedCase" not in list(os.environ)
if os.name == "nt":
    out["returncode_cmd_exit_minus_1"] = subprocess.run(["cmd", "/d", "/c", "exit -1"]).returncode
    s = os.path.join(d, "noshebang.sh")
    with open(s, "w") as f:
        f.write("echo hi\n")
    try:
        subprocess.run([s])
        out["spawn_text_file_error"] = None
    except OSError as e:
        out["spawn_text_file_error"] = str(e)
else:
    out["returncode_exit_255"] = subprocess.run(["sh", "-c", "exit 255"]).returncode
print(json.dumps(out))
"#;

/// Options of one run.
#[derive(Debug, Default)]
struct Opts {
    out: Option<PathBuf>,
    python: Option<PathBuf>,
    snp: Option<PathBuf>,
    strict: bool,
}

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    match args.first().and_then(|a| a.to_str()) {
        Some("--probe-stdin") => {
            probe_stdin();
            return Ok(());
        }
        Some("--print-env") => {
            print_env();
            return Ok(());
        }
        _ => {}
    }
    let opts = parse_args(args)?;
    let root = workspace::root()?;
    let env = Env::capture_process();
    let facts = collect(&env, &opts);
    let results = evaluate(&facts);
    let report = render_report(&facts, &results);

    let out_dir = opts.out.clone().unwrap_or_else(|| {
        root.join("target").join("platform-facts").join(format!(
            "{}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ))
    });
    std::fs::create_dir_all(&out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    let json_text = serde_json::to_string_pretty(&facts)?;
    std::fs::write(out_dir.join("facts.json"), format!("{json_text}\n"))?;
    std::fs::write(out_dir.join("report.md"), &report)?;
    let corrections = out_dir.join("corrections.md");
    let contradicted: Vec<&Outcome> = results
        .iter()
        .filter(|r| matches!(r.verdict, Verdict::Contradicted(_)))
        .collect();
    if contradicted.is_empty() {
        if corrections.exists() {
            std::fs::remove_file(&corrections)?;
        }
    } else {
        std::fs::write(&corrections, render_corrections(&facts, &contradicted))?;
    }
    if let Some(summary) = env.get_nonempty("GITHUB_STEP_SUMMARY") {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(summary)
            .context("opening $GITHUB_STEP_SUMMARY")?;
        f.write_all(report.as_bytes())?;
    }
    for line in report.lines() {
        workspace::say(line);
    }
    workspace::say(&format!("platform-facts: wrote {}", out_dir.display()));
    if opts.strict && !contradicted.is_empty() {
        bail!("{} prediction(s) contradicted", contradicted.len());
    }
    Ok(())
}

fn parse_args(args: &[OsString]) -> anyhow::Result<Opts> {
    let mut opts = Opts::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| -> anyhow::Result<PathBuf> {
            it.next()
                .map(PathBuf::from)
                .with_context(|| format!("{name} needs a value"))
        };
        match a.to_str() {
            Some("--out") => opts.out = Some(value("--out")?),
            Some("--python") => opts.python = Some(value("--python")?),
            Some("--snp") => opts.snp = Some(value("--snp")?),
            Some("--strict") => opts.strict = true,
            _ => bail!(
                "usage: cargo xtask platform-facts [--out DIR] [--python EXE] [--snp EXE] [--strict] (got {})",
                a.to_string_lossy()
            ),
        }
    }
    Ok(opts)
}

/// Child mode: is this process's stdin a terminal (Rust `IsTerminal`)?
fn probe_stdin() {
    workspace::say(if std::io::stdin().is_terminal() {
        "true"
    } else {
        "false"
    });
}

/// Child mode (spawned through a git alias): the variables of [`HOOK_ENV_KEYS`] and the first `PATH` entries.
fn print_env() {
    let env = Env::capture_process();
    let mut m = Map::new();
    for k in HOOK_ENV_KEYS {
        m.insert(
            (*k).to_owned(),
            env.get(k).map_or(Value::Null, |v| {
                Value::String(v.to_string_lossy().into_owned())
            }),
        );
    }
    let path: Vec<Value> = env
        .get("PATH")
        .map(|p| {
            std::env::split_paths(p)
                .take(6)
                .map(|e| Value::String(e.to_string_lossy().into_owned()))
                .collect()
        })
        .unwrap_or_default();
    m.insert("PATH_head".to_owned(), Value::Array(path));
    workspace::say(&Value::Object(m).to_string());
}

// ----- collection ----------------------------------------------------------------------------------------------

/// The output of a finished command.
#[derive(Debug)]
struct Ran {
    code: Option<i32>,
    display: String,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl Ran {
    fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).trim_end().to_owned()
    }
}

/// Runs `cmd` with stdin from `/dev/null` unless the caller set it; `Err` text when it cannot be spawned.
fn exec(mut cmd: Command) -> Result<Ran, String> {
    let out = cmd.output().map_err(|e| format!("unavailable: {e}"))?;
    Ok(Ran {
        code: out.status.code(),
        display: out.status.to_string(),
        stdout: out.stdout,
        stderr: out.stderr,
    })
}

fn command(program: impl AsRef<OsStr>, args: &[&str], cwd: &Path) -> Command {
    let mut c = Command::new(program);
    c.args(args).current_dir(cwd).stdin(Stdio::null());
    c
}

/// stdout of a successful command, else an `unavailable:` string.
fn text_of(r: Result<Ran, String>) -> Value {
    match r {
        Ok(r) if r.code == Some(0) => Value::String(r.stdout_text()),
        Ok(r) => Value::String(format!(
            "unavailable: {} ({})",
            r.display,
            String::from_utf8_lossy(&r.stderr).trim()
        )),
        Err(e) => Value::String(e),
    }
}

/// A scratch directory for the probes (removed at the end of [`collect`]).
fn scratch() -> PathBuf {
    std::env::temp_dir().join(format!("snp-platform-facts-{}", std::process::id()))
}

/// Gathers every fact; never fails.
fn collect(env: &Env, opts: &Opts) -> Value {
    let dir = scratch();
    let made = std::fs::create_dir_all(&dir).is_ok();
    let cwd = if made {
        dir.clone()
    } else {
        std::env::temp_dir()
    };
    let mut facts = Map::new();
    facts.insert("schema".to_owned(), json!(1));
    facts.insert("host".to_owned(), host_facts(env, &cwd));
    facts.insert("fs".to_owned(), fs_facts(&cwd));
    facts.insert("rust".to_owned(), rust_facts(&cwd));
    facts.insert("env".to_owned(), env_facts(env));
    facts.insert("git".to_owned(), git_facts(&cwd));
    facts.insert("python".to_owned(), python_facts(opts, &cwd));
    if let Some(snp) = &opts.snp {
        facts.insert("snp".to_owned(), snp_facts(snp, &cwd));
    }
    if made {
        std::fs::remove_dir_all(&dir).unwrap_or_default();
    }
    Value::Object(facts)
}

fn host_facts(env: &Env, cwd: &Path) -> Value {
    let mut m = Map::new();
    m.insert("os".to_owned(), json!(std::env::consts::OS));
    m.insert("arch".to_owned(), json!(std::env::consts::ARCH));
    m.insert("family".to_owned(), json!(std::env::consts::FAMILY));
    let mut runner = Map::new();
    for k in RUNNER_KEYS {
        if let Some(v) = env.get(k) {
            runner.insert((*k).to_owned(), json!(v.to_string_lossy()));
        }
    }
    m.insert("runner".to_owned(), Value::Object(runner));
    let version = if cfg!(windows) {
        let ver = text_of(exec(command("cmd", &["/d", "/c", "ver"], cwd)));
        let sys = match exec(command("systeminfo", &[], cwd)) {
            Ok(r) => Value::String(
                String::from_utf8_lossy(&r.stdout)
                    .lines()
                    .filter(|l| l.starts_with("OS Name") || l.starts_with("OS Version"))
                    .map(str::trim)
                    .collect::<Vec<_>>()
                    .join("; "),
            ),
            Err(e) => Value::String(e),
        };
        json!({ "ver": ver, "systeminfo": sys })
    } else if cfg!(target_os = "macos") {
        json!({ "sw_vers": text_of(exec(command("sw_vers", &[], cwd))) })
    } else {
        let pretty = std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|t| {
                t.lines()
                    .find_map(|l| l.strip_prefix("PRETTY_NAME="))
                    .map(|v| v.trim_matches('"').to_owned())
            })
            .map_or_else(|| json!("unavailable: /etc/os-release"), Value::String);
        json!({ "os_release": pretty, "uname": text_of(exec(command("uname", &["-srm"], cwd))) })
    };
    m.insert("os_version".to_owned(), version);
    if cfg!(unix) {
        m.insert(
            "ulimit_n".to_owned(),
            text_of(exec(command("sh", &["-c", "ulimit -n"], cwd))),
        );
        m.insert(
            "arg_max".to_owned(),
            text_of(exec(command("getconf", &["ARG_MAX"], cwd))),
        );
    }
    m.insert(
        "available_parallelism".to_owned(),
        std::thread::available_parallelism()
            .map_or_else(|e| json!(format!("unavailable: {e}")), |n| json!(n.get())),
    );
    Value::Object(m)
}

/// Lower-case hex of `b`.
fn hex(b: &[u8]) -> String {
    let mut s = String::new();
    for x in b {
        write!(s, "{x:02x}").unwrap_or_default();
    }
    s
}

fn fs_facts(cwd: &Path) -> Value {
    let mut m = Map::new();
    let temp = std::env::temp_dir();
    m.insert("temp_dir".to_owned(), json!(temp.to_string_lossy()));
    m.insert("temp_realpath".to_owned(), realpath(&temp));
    if cfg!(unix) {
        m.insert("tmp_realpath".to_owned(), realpath(Path::new("/tmp")));
    }
    // case sensitivity
    let case = cwd.join("case-probe");
    let ci = std::fs::create_dir_all(&case)
        .and_then(|()| std::fs::write(case.join("Ab"), b""))
        .map(|()| case.join("ab").exists());
    m.insert(
        "case_insensitive".to_owned(),
        ci.map_or_else(|e| json!(format!("unavailable: {e}")), Value::Bool),
    );
    // Unicode normalisation: create an NFC name, list it back, look it up in NFD
    let uni = cwd.join("unicode-probe");
    let nfc = "\u{e4}.txt";
    let nfd = "a\u{308}.txt";
    match std::fs::create_dir_all(&uni).and_then(|()| std::fs::write(uni.join(nfc), b"")) {
        Ok(()) => {
            let names: Vec<String> = std::fs::read_dir(&uni)
                .map(|rd| {
                    rd.filter_map(Result::ok)
                        .map(|e| hex(e.file_name().as_encoded_bytes()))
                        .collect()
                })
                .unwrap_or_default();
            m.insert("nfc_name_listed_as_hex".to_owned(), json!(names));
            m.insert("nfc_name_hex".to_owned(), json!(hex(nfc.as_bytes())));
            m.insert(
                "nfd_lookup_finds_nfc".to_owned(),
                json!(uni.join(nfd).exists()),
            );
        }
        Err(e) => {
            m.insert(
                "nfc_name_listed_as_hex".to_owned(),
                json!(format!("unavailable: {e}")),
            );
        }
    }
    // symlink creation
    let target = cwd.join("link-target");
    let link = cwd.join("link");
    let made = std::fs::write(&target, b"x").and_then(|()| symlink_file(&target, &link));
    m.insert(
        "symlink_create".to_owned(),
        made.map_or_else(|e| json!(format!("failed: {e}")), |()| json!("ok")),
    );
    Value::Object(m)
}

#[cfg(unix)]
fn symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

#[cfg(not(any(unix, windows)))]
fn symlink_file(_target: &Path, _link: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other("no symlinks on this platform"))
}

/// `realpath` of `p` as JSON (a fact, not a path snp uses).
fn realpath(p: &Path) -> Value {
    #[expect(
        clippy::disallowed_methods,
        reason = "platform-facts records the OS realpath itself (macOS /tmp -> /private/tmp), not Python's"
    )]
    let r = std::fs::canonicalize(p);
    r.map_or_else(
        |e| json!(format!("unavailable: {e}")),
        |p| json!(p.to_string_lossy()),
    )
}

fn rust_facts(cwd: &Path) -> Value {
    let mut m = Map::new();
    match self_exe() {
        Ok(me) => {
            let mut null = command(&me, &["platform-facts", "--probe-stdin"], cwd);
            null.stdin(Stdio::null());
            m.insert("stdin_null_is_terminal".to_owned(), parse_bool(exec(null)));
            let mut pipe = command(&me, &["platform-facts", "--probe-stdin"], cwd);
            pipe.stdin(Stdio::piped());
            m.insert("stdin_pipe_is_terminal".to_owned(), parse_bool(exec(pipe)));
        }
        Err(e) => {
            m.insert("stdin_null_is_terminal".to_owned(), json!(e));
        }
    }
    let status = if cfg!(windows) {
        exec(command("cmd", &["/d", "/c", "exit -1"], cwd))
    } else {
        exec(command("sh", &["-c", "exit 255"], cwd))
    };
    m.insert(
        "exit_status".to_owned(),
        match status {
            Ok(r) => json!({ "code": r.code, "display": r.display }),
            Err(e) => json!(e),
        },
    );
    m.insert(
        "stderr_is_terminal".to_owned(),
        json!(std::io::stderr().is_terminal()),
    );
    Value::Object(m)
}

/// This xtask executable, for the child modes; unit tests run inside the test harness, which has no such modes.
fn self_exe() -> Result<PathBuf, String> {
    if cfg!(test) {
        return Err("unavailable: not spawned from the unit-test harness".to_owned());
    }
    std::env::current_exe().map_err(|e| format!("unavailable: {e}"))
}

fn parse_bool(r: Result<Ran, String>) -> Value {
    match r {
        Ok(r) => match r.stdout_text().as_str() {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            other => json!(format!(
                "unavailable: unexpected output {other:?} ({})",
                r.display
            )),
        },
        Err(e) => json!(e),
    }
}

/// `PATHEXT` split as Windows does (`;`-separated, empty entries dropped).
fn pathext(env: &Env) -> Vec<String> {
    env.get("PATHEXT")
        .map(|v| {
            v.to_string_lossy()
                .split(';')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Every match of `name` on `PATH` in order (with `PATHEXT` extensions on Windows), like `where name`.
fn where_all(env: &Env, name: &str) -> Vec<String> {
    let Some(path) = env.get("PATH") else {
        return Vec::new();
    };
    let exts: Vec<String> = if cfg!(windows) {
        let mut e = vec![String::new()];
        e.extend(pathext(env));
        e
    } else {
        vec![String::new()]
    };
    let mut out = Vec::new();
    for dir in std::env::split_paths(path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for ext in &exts {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                let s = candidate.to_string_lossy().into_owned();
                if !out.contains(&s) {
                    out.push(s);
                }
            }
        }
    }
    out
}

fn env_facts(env: &Env) -> Value {
    let mut m = Map::new();
    m.insert(
        "PATHEXT".to_owned(),
        env.get("PATHEXT")
            .map_or(Value::Null, |v| json!(v.to_string_lossy())),
    );
    m.insert("pathext_parsed".to_owned(), json!(pathext(env)));
    let mut w = Map::new();
    for name in WHERE {
        w.insert((*name).to_owned(), json!(where_all(env, name)));
    }
    m.insert("where".to_owned(), Value::Object(w));
    Value::Object(m)
}

fn git_facts(cwd: &Path) -> Value {
    let mut m = Map::new();
    m.insert(
        "version".to_owned(),
        text_of(exec(command("git", &["--version"], cwd))),
    );
    // configuration outside any repository: system + global (+ worktree-less)
    let mut cfg = Map::new();
    for key in GIT_KEYS {
        let r = exec(command(
            "git",
            &["config", "--show-origin", "--show-scope", "--get-all", key],
            cwd,
        ));
        let v = match r {
            Ok(r) if r.code == Some(0) => json!(r.stdout_text().lines().collect::<Vec<_>>()),
            Ok(r) if r.code == Some(1) => Value::Null,
            Ok(r) => json!(format!("unavailable: {}", r.display)),
            Err(e) => json!(e),
        };
        cfg.insert((*key).to_owned(), v);
    }
    m.insert("config".to_owned(), Value::Object(cfg));
    // what `git init` probes on this file system
    let repo = cwd.join("init-probe");
    let mut init = Map::new();
    match exec(command(
        "git",
        &["init", "-q", &repo.to_string_lossy()],
        cwd,
    )) {
        Ok(r) if r.code == Some(0) => {
            for key in GIT_INIT_KEYS {
                let v = exec(command("git", &["config", "--local", "--get", key], &repo));
                init.insert(
                    (*key).to_owned(),
                    match v {
                        Ok(r) if r.code == Some(0) => json!(r.stdout_text()),
                        Ok(_) => Value::Null,
                        Err(e) => json!(e),
                    },
                );
            }
        }
        Ok(r) => {
            init.insert("error".to_owned(), json!(r.display));
        }
        Err(e) => {
            init.insert("error".to_owned(), json!(e));
        }
    }
    m.insert("init".to_owned(), Value::Object(init));
    m.insert("hook_env".to_owned(), git_child_env(cwd));
    Value::Object(m)
}

/// The environment of a native program that git spawns (through a `!` alias, i.e. git's shell, the same route as a
/// hook script exec'ing snp), with `MSYSTEM` and `TERM` removed from git's own environment first (cmd/PowerShell/IDE
/// launch, 14 A.1 #15).
fn git_child_env(cwd: &Path) -> Value {
    let me = match self_exe() {
        Ok(me) => me,
        Err(e) => return json!(e),
    };
    let exe = me
        .to_string_lossy()
        .replace('\\', "/")
        .replace('\'', "'\\''");
    let alias = format!("alias.snp-platform-facts-env=!'{exe}' platform-facts --print-env");
    let mut c = command("git", &["-c", &alias, "snp-platform-facts-env"], cwd);
    c.env_remove("MSYSTEM").env_remove("TERM");
    match exec(c) {
        Ok(r) if r.code == Some(0) => serde_json::from_slice(&r.stdout).unwrap_or_else(|e| {
            json!(format!(
                "unavailable: unparsable child output ({e}): {}",
                r.stdout_text()
            ))
        }),
        Ok(r) => json!(format!(
            "unavailable: {} ({})",
            r.display,
            String::from_utf8_lossy(&r.stderr).trim()
        )),
        Err(e) => json!(e),
    }
}

/// The interpreter to probe: `--python`, else the first of `python3`, `python` (and `py -3` on Windows) that runs.
fn python_cmd(opts: &Opts, cwd: &Path) -> Option<(PathBuf, Vec<&'static str>)> {
    if let Some(p) = &opts.python {
        return Some((p.clone(), Vec::new()));
    }
    let mut candidates: Vec<(PathBuf, Vec<&'static str>)> = vec![
        (PathBuf::from("python3"), Vec::new()),
        (PathBuf::from("python"), Vec::new()),
    ];
    if cfg!(windows) {
        candidates.push((PathBuf::from("py"), vec!["-3"]));
    }
    candidates.into_iter().find(|(p, pre)| {
        let mut args = pre.clone();
        args.extend([
            "-c",
            "import sys; sys.exit(0 if sys.version_info[0] == 3 else 1)",
        ]);
        exec(command(p, &args, cwd)).is_ok_and(|r| r.code == Some(0))
    })
}

fn python_facts(opts: &Opts, cwd: &Path) -> Value {
    let Some((py, pre)) = python_cmd(opts, cwd) else {
        return json!("unavailable: no python3 interpreter found");
    };
    let mut m = Map::new();
    m.insert("interpreter".to_owned(), json!(py.to_string_lossy()));
    let probe = |stdin: Stdio| {
        let mut args = pre.clone();
        args.extend(["-I", "-c", PY_PROBE]);
        let mut c = command(&py, &args, cwd);
        c.stdin(stdin).env("SnpFactsMixedCase", "1");
        match exec(c) {
            Ok(r) if r.code == Some(0) => serde_json::from_slice(&r.stdout)
                .unwrap_or_else(|e| json!(format!("unavailable: unparsable probe output: {e}"))),
            Ok(r) => json!(format!(
                "unavailable: {} ({})",
                r.display,
                String::from_utf8_lossy(&r.stderr).trim()
            )),
            Err(e) => json!(e),
        }
    };
    m.insert("probe_stdin_null".to_owned(), probe(Stdio::null()));
    m.insert("probe_stdin_pipe".to_owned(), probe(Stdio::piped()));
    // print() into a pipe: CRLF on Windows (text-mode stdout, 14 A.1 #16)
    let mut args = pre.clone();
    args.extend(["-I", "-c", "print('x')"]);
    m.insert(
        "print_into_pipe_hex".to_owned(),
        match exec(command(&py, &args, cwd)) {
            Ok(r) => json!(hex(&r.stdout)),
            Err(e) => json!(e),
        },
    );
    // PYTHON_CPU_COUNT (3.13+; ignored under -I/-E, so run with -S from the scratch directory)
    let mut args = pre.clone();
    args.extend(["-S", "-c", "import os; print(os.cpu_count())"]);
    let mut c = command(&py, &args, cwd);
    c.env("PYTHON_CPU_COUNT", "3");
    m.insert(
        "cpu_count_with_python_cpu_count_3".to_owned(),
        match exec(c) {
            Ok(r) if r.code == Some(0) => r
                .stdout_text()
                .parse::<u64>()
                .map_or_else(|_| json!(r.stdout_text()), |n| json!(n)),
            other => text_of(other),
        },
    );
    Value::Object(m)
}

fn snp_facts(snp: &Path, cwd: &Path) -> Value {
    json!({
        "version": text_of(exec(command(snp, &["--version"], cwd))),
        "debug_platform": text_of(exec(command(snp, &["__debug", "platform"], cwd))),
    })
}

// ----- predictions ---------------------------------------------------------------------------------------------

/// The verdict on one prediction.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    /// The fact matches the prediction (observed value attached).
    Confirmed(String),
    /// The fact contradicts the prediction.
    Contradicted(String),
    /// The fact is not available on this runner.
    Unknown(String),
}

/// One derived prediction of 14 §A.6 checked against the facts.
#[derive(Debug)]
struct Prediction {
    /// Stable ID (`PF-nn`).
    id: &'static str,
    /// Where the prediction is made.
    source: &'static str,
    /// Operating systems the prediction is about (`std::env::consts::OS` values; `*` = all).
    os: &'static [&'static str],
    /// The claim, in words.
    claim: &'static str,
    /// The check.
    check: fn(&Value) -> Verdict,
}

/// A prediction with its verdict.
#[derive(Debug)]
struct Outcome {
    prediction: &'static Prediction,
    verdict: Verdict,
}

const WIN: &[&str] = &["windows"];
const MAC: &[&str] = &["macos"];
const WIN_MAC: &[&str] = &["windows", "macos"];
const ALL: &[&str] = &["*"];

fn get<'a>(facts: &'a Value, pointer: &str) -> Option<&'a Value> {
    facts.pointer(pointer).filter(|v| !is_unavailable(v))
}

fn is_unavailable(v: &Value) -> bool {
    v.as_str()
        .is_some_and(|s| s.starts_with("unavailable") || s.starts_with("failed"))
}

fn os_of(facts: &Value) -> &str {
    facts
        .pointer("/host/os")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// `Confirmed`/`Contradicted` depending on `ok`, with the observed value.
fn verdict(ok: bool, observed: impl std::fmt::Display) -> Verdict {
    if ok {
        Verdict::Confirmed(observed.to_string())
    } else {
        Verdict::Contradicted(observed.to_string())
    }
}

/// Compares the fact at `pointer` with `expected`.
fn expect_eq(facts: &Value, pointer: &str, expected: &Value) -> Verdict {
    match get(facts, pointer) {
        Some(v) => verdict(v == expected, v),
        None => unknown(facts, pointer),
    }
}

fn unknown(facts: &Value, pointer: &str) -> Verdict {
    Verdict::Unknown(facts.pointer(pointer).map_or_else(
        || format!("{pointer}: not collected"),
        |v| format!("{pointer}: {v}"),
    ))
}

/// The predictions; each names the 14 section that makes it.
static PREDICTIONS: &[Prediction] = &[
    Prediction {
        id: "PF-01",
        source: "14 A.1 #8, A.4.1, W16",
        os: ALL,
        claim: "Python sees a null stdin (`NUL` / `/dev/null`) as a TTY on Windows only (`NUL` is a character device); hooks must get an empty pipe",
        check: |f| {
            let expected = os_of(f) == "windows";
            expect_eq(
                f,
                "/python/probe_stdin_null/stdin_isatty",
                &Value::Bool(expected),
            )
        },
    },
    Prediction {
        id: "PF-02",
        source: "14 A.1 #8, W16",
        os: ALL,
        claim: "Python sees an empty pipe on stdin as not a TTY on every OS",
        check: |f| {
            expect_eq(
                f,
                "/python/probe_stdin_pipe/stdin_isatty",
                &Value::Bool(false),
            )
        },
    },
    Prediction {
        id: "PF-03",
        source: "14 A.1 #9, A.7.2",
        os: ALL,
        claim: "Rust `IsTerminal` is false for a null stdin on every OS (so it cannot stand in for the CRT `isatty` parity check)",
        check: |f| expect_eq(f, "/rust/stdin_null_is_terminal", &Value::Bool(false)),
    },
    Prediction {
        id: "PF-04",
        source: "14 A.1 #2, W03",
        os: ALL,
        claim: "`os.access(path, X_OK)` is true for every existing file on Windows and false for a 0644 file elsewhere",
        check: |f| {
            let expected = os_of(f) == "windows";
            expect_eq(
                f,
                "/python/probe_stdin_null/access_x_ok_plain_file",
                &Value::Bool(expected),
            )
        },
    },
    Prediction {
        id: "PF-05",
        source: "14 A.1 #4, W10",
        os: WIN,
        claim: "Python reports `cmd /c exit -1` as 4294967295 (unsigned DWORD); Rust's `ExitStatus::code()` is -1",
        check: |f| {
            let py = get(f, "/python/probe_stdin_null/returncode_cmd_exit_minus_1");
            let rs = get(f, "/rust/exit_status/code");
            match (py, rs) {
                (Some(p), Some(r)) => verdict(
                    p == &json!(4_294_967_295_u64) && r == &json!(-1),
                    format!("python {p}, rust {r}"),
                ),
                _ => unknown(f, "/python/probe_stdin_null/returncode_cmd_exit_minus_1"),
            }
        },
    },
    Prediction {
        id: "PF-06",
        source: "14 A.1 #16, W13, D-WIN-04",
        os: ALL,
        claim: "`print()` into a pipe ends lines with CRLF on Windows and LF elsewhere",
        check: |f| {
            let expected = if os_of(f) == "windows" {
                "780d0a"
            } else {
                "780a"
            };
            expect_eq(f, "/python/print_into_pipe_hex", &json!(expected))
        },
    },
    Prediction {
        id: "PF-07",
        source: "14 A.1 #17, W17, ADR-074",
        os: ALL,
        claim: "`os.environ` upper-cases variable names on Windows only",
        check: |f| {
            let expected = os_of(f) == "windows";
            expect_eq(
                f,
                "/python/probe_stdin_null/environ_upper_cases_keys",
                &Value::Bool(expected),
            )
        },
    },
    Prediction {
        id: "PF-08",
        source: "14 A.1 #12, M01",
        os: MAC,
        claim: "`/tmp` resolves to `/private/tmp`",
        check: |f| expect_eq(f, "/fs/tmp_realpath", &json!("/private/tmp")),
    },
    Prediction {
        id: "PF-09",
        source: "14 A.1 #15, W02",
        os: WIN,
        claim: "a program git spawns with MSYSTEM and TERM unset sees MSYSTEM set by Git for Windows, TERM=xterm-256color and Git's `usr\\bin` within the first PATH entries",
        check: |f| {
            let Some(e) = get(f, "/git/hook_env").filter(|v| v.is_object()) else {
                return unknown(f, "/git/hook_env");
            };
            let msystem = e.get("MSYSTEM").and_then(Value::as_str).unwrap_or_default();
            let term = e.get("TERM").and_then(Value::as_str).unwrap_or_default();
            let usr_bin = e
                .get("PATH_head")
                .and_then(Value::as_array)
                .is_some_and(|a| {
                    a.iter().take(3).filter_map(Value::as_str).any(|p| {
                        p.to_ascii_lowercase()
                            .replace('/', "\\")
                            .trim_end_matches('\\')
                            .ends_with("\\usr\\bin")
                    })
                });
            verdict(
                !msystem.is_empty() && term == "xterm-256color" && usr_bin,
                format!("MSYSTEM={msystem:?} TERM={term:?} usr\\bin in PATH[0..3]={usr_bin}"),
            )
        },
    },
    Prediction {
        id: "PF-10",
        source: "14 A.1 #14, W18, D-PLAT-10",
        os: WIN_MAC,
        claim: "Python `os.cpu_count()` equals Rust `available_parallelism()` (hosts with at most 64 logical CPUs)",
        check: |f| {
            let py = get(f, "/python/probe_stdin_null/cpu_count");
            let rs = get(f, "/host/available_parallelism");
            match (py, rs) {
                (Some(p), Some(r)) => verdict(p == r, format!("python {p}, rust {r}")),
                _ => unknown(f, "/python/probe_stdin_null/cpu_count"),
            }
        },
    },
    Prediction {
        id: "PF-11",
        source: "14 A.1 #14, A.9 Q3, W18",
        os: WIN_MAC,
        claim: "with CPython >= 3.13, `PYTHON_CPU_COUNT=3` makes `os.cpu_count()` return 3 (snp ignores the variable)",
        check: |f| {
            let version = get(f, "/python/probe_stdin_null/version")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let minor = version
                .split('.')
                .nth(1)
                .and_then(|m| m.parse::<u32>().ok());
            match minor {
                Some(m) if m >= 13 => {
                    expect_eq(f, "/python/cpu_count_with_python_cpu_count_3", &json!(3))
                }
                Some(_) => Verdict::Unknown(format!("python {version} predates PYTHON_CPU_COUNT")),
                None => unknown(f, "/python/probe_stdin_null/version"),
            }
        },
    },
    Prediction {
        id: "PF-12",
        source: "14 A.9 Q8, A.6.1",
        os: WIN,
        claim: "the GitHub-hosted Windows image sets `core.autocrlf=true` at system scope (Git for Windows installer default)",
        check: |f| match f.pointer("/git/config/core.autocrlf") {
            Some(Value::Array(lines)) => {
                let system_true = lines
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|l| l.starts_with("system") && l.trim_end().ends_with("true"));
                verdict(system_true, Value::Array(lines.clone()))
            }
            Some(Value::Null) => Verdict::Contradicted("unset".to_owned()),
            _ => unknown(f, "/git/config/core.autocrlf"),
        },
    },
    Prediction {
        id: "PF-13",
        source: "14 A.2.5, M03",
        os: MAC,
        claim: "the default macOS volume is case-insensitive and `git init` records `core.ignorecase=true`",
        check: |f| {
            let fs = get(f, "/fs/case_insensitive");
            let git = get(f, "/git/init/core.ignorecase");
            match (fs, git) {
                (Some(a), Some(b)) => verdict(
                    a == &Value::Bool(true) && b == &json!("true"),
                    format!("fs {a}, git {b}"),
                ),
                _ => unknown(f, "/fs/case_insensitive"),
            }
        },
    },
    Prediction {
        id: "PF-14",
        source: "14 A.2.5, M02",
        os: MAC,
        claim: "`git init` records `core.precomposeunicode=true` and APFS keeps an NFC name as NFC bytes while an NFD lookup still finds it",
        check: |f| {
            let git = get(f, "/git/init/core.precomposeunicode");
            let listed = get(f, "/fs/nfc_name_listed_as_hex");
            let nfc = get(f, "/fs/nfc_name_hex");
            let lookup = get(f, "/fs/nfd_lookup_finds_nfc");
            match (git, listed, nfc, lookup) {
                (Some(g), Some(Value::Array(names)), Some(n), Some(l)) => verdict(
                    g == &json!("true") && names.contains(n) && l == &Value::Bool(true),
                    format!("precomposeunicode {g}, listed {names:?}, nfd lookup {l}"),
                ),
                _ => unknown(f, "/git/init/core.precomposeunicode"),
            }
        },
    },
    Prediction {
        id: "PF-15",
        source: "14 A.2.6, M04",
        os: MAC,
        claim: "`ARG_MAX` is 1 MiB (1048576), so pre-commit's xargs budget clamps to 131072",
        check: |f| expect_eq(f, "/host/arg_max", &json!("1048576")),
    },
    Prediction {
        id: "PF-16",
        source: "14 A.1 #5, W09",
        os: WIN,
        claim: "spawning a text file without a shebang fails with `[WinError 193] %1 is not a valid Win32 application` (English image)",
        check: |f| match get(f, "/python/probe_stdin_null/spawn_text_file_error") {
            Some(Value::String(s)) => verdict(
                s.starts_with("[WinError 193] %1 is not a valid Win32 application"),
                s,
            ),
            Some(v) => Verdict::Contradicted(v.to_string()),
            None => unknown(f, "/python/probe_stdin_null/spawn_text_file_error"),
        },
    },
    Prediction {
        id: "PF-17",
        source: "14 A.2.5, W21",
        os: WIN,
        claim: "without Developer Mode `git init` records `core.symlinks=false`",
        check: |f| expect_eq(f, "/git/init/core.symlinks", &json!("false")),
    },
    Prediction {
        id: "PF-18",
        source: "14 A.5.2 (CI row)",
        os: WIN,
        claim: "the runner's PATH holds Git for Windows' `sh` and `bash` (Git\\bin, Git\\usr\\bin), so `#!/bin/sh` hook scripts resolve",
        check: |f| {
            let sh = f
                .pointer("/env/where/sh")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let bash = f
                .pointer("/env/where/bash")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            verdict(
                sh > 0 && bash > 0,
                format!("sh: {sh} match(es), bash: {bash} match(es)"),
            )
        },
    },
    Prediction {
        id: "PF-19",
        source: "14 A.2.4, A.5.2 (CI row)",
        os: ALL,
        claim: "CI stderr is a pipe, not a terminal (no colour under `--color auto`)",
        check: |f| expect_eq(f, "/rust/stderr_is_terminal", &Value::Bool(false)),
    },
];

/// Evaluates every prediction that applies to the facts' OS.
fn evaluate(facts: &Value) -> Vec<Outcome> {
    let os = os_of(facts);
    PREDICTIONS
        .iter()
        .filter(|p| p.os.iter().any(|o| *o == "*" || *o == os))
        .map(|p| Outcome {
            prediction: p,
            verdict: (p.check)(facts),
        })
        .collect()
}

fn cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

fn runner_label(facts: &Value) -> String {
    let os = os_of(facts);
    let arch = facts
        .pointer("/host/arch")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let image = facts
        .pointer("/host/runner/ImageOS")
        .and_then(Value::as_str)
        .unwrap_or("local");
    let version = facts
        .pointer("/host/runner/ImageVersion")
        .and_then(Value::as_str)
        .unwrap_or("-");
    format!("{os}-{arch} ({image} {version})")
}

fn render_report(facts: &Value, results: &[Outcome]) -> String {
    let mut out = String::new();
    writeln!(out, "## Platform facts: {}\n", runner_label(facts)).unwrap_or_default();
    let git = facts
        .pointer("/git/version")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let py = facts
        .pointer("/python/probe_stdin_null/version")
        .and_then(Value::as_str)
        .unwrap_or("?");
    writeln!(out, "`{git}`, Python {py}\n").unwrap_or_default();
    let count = |f: fn(&Verdict) -> bool| results.iter().filter(|r| f(&r.verdict)).count();
    writeln!(
        out,
        "{} prediction(s): {} confirmed, {} contradicted, {} unknown.\n",
        results.len(),
        count(|v| matches!(v, Verdict::Confirmed(_))),
        count(|v| matches!(v, Verdict::Contradicted(_))),
        count(|v| matches!(v, Verdict::Unknown(_))),
    )
    .unwrap_or_default();
    out.push_str("| ID | Source | Claim | Verdict | Observed |\n|---|---|---|---|---|\n");
    for r in results {
        let (v, obs) = match &r.verdict {
            Verdict::Confirmed(o) => ("confirmed", o),
            Verdict::Contradicted(o) => ("**contradicted**", o),
            Verdict::Unknown(o) => ("unknown", o),
        };
        writeln!(
            out,
            "| {} | {} | {} | {v} | {} |",
            r.prediction.id,
            cell(r.prediction.source),
            cell(r.prediction.claim),
            cell(obs)
        )
        .unwrap_or_default();
    }
    out.push('\n');
    out
}

fn render_corrections(facts: &Value, contradicted: &[&Outcome]) -> String {
    let mut out = String::new();
    writeln!(
        out,
        "The platform-facts job on `{}` contradicted {} derived prediction(s) of docs/research/14-platform-matrix.md.\n\
         Per 14 §A.6.5 the matrix (and any dependent ADR or divergence entry) must be corrected in the PR that fixes\n\
         this; the full facts are in the `platform-facts` artifact of this run.\n",
        runner_label(facts),
        contradicted.len()
    )
    .unwrap_or_default();
    for r in contradicted {
        let obs = match &r.verdict {
            Verdict::Contradicted(o) | Verdict::Confirmed(o) | Verdict::Unknown(o) => o,
        };
        writeln!(
            out,
            "- **{}** ({}): {}\n  - observed: `{}`",
            r.prediction.id,
            r.prediction.source,
            r.prediction.claim,
            obs.replace('`', "'")
        )
        .unwrap_or_default();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts_for(os: &str) -> Value {
        json!({
            "schema": 1,
            "host": { "os": os, "arch": "x86_64", "available_parallelism": 4, "arg_max": "1048576",
                      "runner": { "ImageOS": "test", "ImageVersion": "1" } },
            "fs": { "tmp_realpath": "/private/tmp", "case_insensitive": true,
                    "nfc_name_hex": "c3a42e747874", "nfc_name_listed_as_hex": ["c3a42e747874"],
                    "nfd_lookup_finds_nfc": true },
            "rust": { "stdin_null_is_terminal": false, "stdin_pipe_is_terminal": false,
                      "exit_status": { "code": -1, "display": "exit code: 0xffffffff" },
                      "stderr_is_terminal": false },
            "env": { "where": { "sh": ["C:\\Program Files\\Git\\usr\\bin\\sh.exe"],
                                "bash": ["C:\\Program Files\\Git\\bin\\bash.exe"] } },
            "git": { "version": "git version 2.51.0.windows.1",
                     "config": { "core.autocrlf": ["system\tfile:C:/Program Files/Git/etc/gitconfig\ttrue"] },
                     "init": { "core.symlinks": "false", "core.ignorecase": "true", "core.precomposeunicode": "true" },
                     "hook_env": { "MSYSTEM": "MINGW64", "TERM": "xterm-256color",
                                   "PATH_head": ["C:\\Users\\r\\bin", "C:\\Program Files\\Git\\mingw64\\bin",
                                                 "C:\\Program Files\\Git\\usr\\bin"] } },
            "python": {
                "probe_stdin_null": { "version": "3.13.1", "stdin_isatty": os == "windows",
                    "access_x_ok_plain_file": os == "windows", "environ_upper_cases_keys": os == "windows",
                    "cpu_count": 4, "returncode_cmd_exit_minus_1": 4_294_967_295_u64,
                    "spawn_text_file_error": "[WinError 193] %1 is not a valid Win32 application" },
                "probe_stdin_pipe": { "stdin_isatty": false },
                "print_into_pipe_hex": if os == "windows" { "780d0a" } else { "780a" },
                "cpu_count_with_python_cpu_count_3": 3
            }
        })
    }

    #[test]
    fn predicted_facts_confirm_every_prediction() {
        for os in ["windows", "macos", "linux"] {
            let f = facts_for(os);
            let results = evaluate(&f);
            assert!(!results.is_empty());
            for r in &results {
                assert!(
                    matches!(r.verdict, Verdict::Confirmed(_)),
                    "{os} {}: {:?}",
                    r.prediction.id,
                    r.verdict
                );
            }
        }
        // OS filtering: macOS-only predictions are not evaluated on Linux
        let linux: Vec<&str> = evaluate(&facts_for("linux"))
            .iter()
            .map(|r| r.prediction.id)
            .collect();
        assert!(!linux.contains(&"PF-08"));
        assert!(linux.contains(&"PF-01"));
    }

    #[test]
    fn contradictions_and_unknowns_are_reported() {
        let mut f = facts_for("windows");
        *f.pointer_mut("/python/probe_stdin_null/stdin_isatty")
            .unwrap() = json!(false);
        *f.pointer_mut("/git/config/core.autocrlf").unwrap() = Value::Null;
        *f.pointer_mut("/python/print_into_pipe_hex").unwrap() = json!("unavailable: no python");
        let results = evaluate(&f);
        let by_id = |id: &str| {
            &results
                .iter()
                .find(|r| r.prediction.id == id)
                .unwrap()
                .verdict
        };
        assert_eq!(by_id("PF-01"), &Verdict::Contradicted("false".into()));
        assert_eq!(by_id("PF-12"), &Verdict::Contradicted("unset".into()));
        assert!(matches!(by_id("PF-06"), Verdict::Unknown(_)));
        let report = render_report(&f, &results);
        assert!(report.contains("| PF-01 |"));
        assert!(report.contains("**contradicted**"));
        let contradicted: Vec<&Outcome> = results
            .iter()
            .filter(|r| matches!(r.verdict, Verdict::Contradicted(_)))
            .collect();
        let c = render_corrections(&f, &contradicted);
        assert!(c.contains("**PF-01**"));
        assert!(c.contains("14 §A.6.5"));
    }

    #[test]
    fn python_before_313_leaves_python_cpu_count_unknown() {
        let mut f = facts_for("macos");
        *f.pointer_mut("/python/probe_stdin_null/version").unwrap() = json!("3.12.3");
        let r = evaluate(&f);
        let v = &r
            .iter()
            .find(|r| r.prediction.id == "PF-11")
            .unwrap()
            .verdict;
        assert!(matches!(v, Verdict::Unknown(_)));
    }

    #[test]
    fn where_all_follows_path_order() {
        let dir = scratch().with_extension("where");
        let a = dir.join("a");
        let b = dir.join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let name = if cfg!(windows) { "tool.EXE" } else { "tool" };
        std::fs::write(a.join(name), b"").unwrap();
        std::fs::write(b.join(name), b"").unwrap();
        let path = std::env::join_paths([&b, &a]).unwrap();
        let env = Env::from_pairs([("PATH", path), ("PATHEXT", OsString::from(".COM;.EXE;;"))]);
        let got = where_all(&env, "tool");
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(got.len(), 2);
        assert!(got[0].starts_with(&*b.to_string_lossy()));
        assert_eq!(pathext(&env), [".COM", ".EXE"]);
    }

    #[test]
    fn args_are_validated() {
        assert!(parse_args(&[OsString::from("--bogus")]).is_err());
        assert!(parse_args(&[OsString::from("--out")]).is_err());
        let o = parse_args(&[
            OsString::from("--strict"),
            OsString::from("--out"),
            OsString::from("x"),
        ])
        .unwrap();
        assert!(o.strict);
        assert_eq!(o.out.as_deref(), Some(Path::new("x")));
    }

    #[test]
    fn collection_on_this_host_never_fails_and_has_every_section() {
        let env = Env::capture_process();
        let facts = collect(&env, &Opts::default());
        for key in ["host", "fs", "rust", "env", "git", "python"] {
            assert!(facts.get(key).is_some(), "{key}");
        }
        assert_eq!(os_of(&facts), std::env::consts::OS);
        // the self-spawned stdin probe works under the test harness too (the test binary is not xtask, so the
        // probe may be unavailable; it must then be recorded, not fail)
        assert!(facts.pointer("/rust/stdin_null_is_terminal").is_some());
        let results = evaluate(&facts);
        let report = render_report(&facts, &results);
        assert!(report.starts_with("## Platform facts: "));
    }
}
