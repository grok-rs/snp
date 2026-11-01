//! hazmat path H (`snp __hazmat`, `in_hook_main`) and path C (`hazmat::cli`) (WP-26 acceptance: T10-HAZ-01…05,
//! the §H.5/§H.6 rows of 03's Addendum, X13-HZ-06, SIGINT, no `/bin/sh` fallback).
//!
//! Path H is a process of its own (`raise SystemExit(main())` of `python -m pre_commit.commands.hazmat`), so this
//! harness-less test binary re-executes itself as that process: with `SNP_HOOKS_TEST_HAZMAT_ARGV` (a JSON array)
//! set, `main` is `in_hook_main` over those arguments. The goldens come from `cargo xtask pch-goldens hazmat`
//! (pre-commit 4.6.2, CPython 3.13). POSIX only (the fixtures are shell scripts).
//!
//! Owner: WP-26.
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "a harness-less test binary: its checks are test assertions, not production code"
)]

#[path = "../framework/support.rs"]
mod support;

use std::ffi::OsString;
use std::io::{Read as _, Write as _};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use snp_cli::HazmatTool;
use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_core::failure::{ExcKind, Failure};
use snp_hooks::hazmat;
use support::{latin1, show};

/// The variable that turns this binary into `snp __hazmat`.
const CHILD: &str = "SNP_HOOKS_TEST_HAZMAT_ARGV";

fn main() {
    let process = Env::capture_process();
    if let Some(json) = process.get(CHILD) {
        let argv: Vec<String> = serde_json::from_str(&json.to_string_lossy()).unwrap();
        let argv: Vec<OsString> = argv.into_iter().map(OsString::from).collect();
        let env = process.without(&[CHILD]);
        let code = hazmat::in_hook_main(&argv, &env);
        #[expect(
            clippy::disallowed_methods,
            reason = "this process stands in for `snp __hazmat`: its status is the hook's exit code"
        )]
        std::process::exit(code);
    }
    let tests: [(&str, fn()); 12] = [
        ("hazmat_goldens", hazmat_goldens),
        ("t10_haz_01_cmd_filenames", t10_haz_01_cmd_filenames),
        ("t10_haz_02_cd_prefix", t10_haz_02_cd_prefix),
        ("t10_haz_03_cd_runs_in_subdir", t10_haz_03_cd_runs_in_subdir),
        ("t10_haz_04_ignore_exit_code", t10_haz_04_ignore_exit_code),
        (
            "t10_haz_05_n1_one_call_per_file",
            t10_haz_05_n1_one_call_per_file,
        ),
        ("x13_hz_06_in_hook_failures", x13_hz_06_in_hook_failures),
        ("no_sh_fallback", no_sh_fallback),
        ("sigint_kills_after_250ms", sigint_kills_after_250ms),
        ("cli_path_exit_codes", cli_path_exit_codes),
        ("spec_and_help", spec_and_help),
        ("support_links", support::link_all),
    ];
    let args = Args::parse(std::env::args().skip(1));
    if args.list {
        // libtest's list protocol (`--list --format terse`, what `cargo nextest` reads): `name: test` per test;
        // nothing is ignored, so `--ignored` lists nothing.
        let mut out = std::io::stdout().lock();
        if !args.ignored {
            for (name, _) in &tests {
                if args.selects(name) {
                    writeln!(out, "{name}: test").unwrap();
                }
            }
        }
        return;
    }
    let mut failed = 0_u32;
    let mut ran = 0_u32;
    let mut out = std::io::stdout().lock();
    for (name, test) in tests {
        if args.ignored || !args.selects(name) {
            continue;
        }
        ran += 1;
        let ok = !cfg!(unix) || std::panic::catch_unwind(test).is_ok();
        writeln!(out, "test {name} ... {}", if ok { "ok" } else { "FAILED" }).unwrap();
        if !ok {
            failed += 1;
        }
    }
    writeln!(
        out,
        "\ntest result: {}. {ran} run; {failed} failed",
        if failed == 0 { "ok" } else { "FAILED" }
    )
    .unwrap();
    assert_eq!(failed, 0, "{failed} hazmat tests failed");
}

/// The libtest command line this harness honours (`cargo test`, `cargo nextest`): filters, `--exact`, `--skip`,
/// `--list`, `--ignored`; other flags (and their values) are accepted and ignored.
#[derive(Default)]
struct Args {
    filters: Vec<String>,
    skip: Vec<String>,
    exact: bool,
    list: bool,
    ignored: bool,
}

impl Args {
    fn parse(argv: impl Iterator<Item = String>) -> Self {
        let mut a = Self::default();
        let mut argv = argv.peekable();
        while let Some(arg) = argv.next() {
            match arg.as_str() {
                "--list" => a.list = true,
                "--exact" => a.exact = true,
                "--ignored" => a.ignored = true,
                "--skip" => a.skip.extend(argv.next()),
                // Flags of libtest that take a separate value.
                "--format" | "--logfile" | "--test-threads" | "--color" | "-Z"
                | "--report-time" => {
                    if argv.peek().is_some_and(|v| !v.starts_with('-')) {
                        argv.next();
                    }
                }
                _ if arg.starts_with('-') => {}
                _ => a.filters.push(arg),
            }
        }
        a
    }

    fn selects(&self, name: &str) -> bool {
        let hit = |f: &String| {
            if self.exact {
                name == f
            } else {
                name.contains(f.as_str())
            }
        };
        (self.filters.is_empty() || self.filters.iter().any(hit)) && !self.skip.iter().any(hit)
    }
}

/// The fixture tree of `cases.json` in a fresh directory (the work tree of the hook).
fn fixtures() -> tempfile::TempDir {
    let cases = support::json("hazmat/cases.json");
    let dir = tempfile::tempdir().unwrap();
    for (path, content) in cases["fixtures"].as_object().unwrap() {
        let p = dir.path().join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, latin1(content.as_str().unwrap())).unwrap();
    }
    #[cfg(unix)]
    for path in cases["exec"].as_array().unwrap() {
        use std::os::unix::fs::PermissionsExt as _;
        let p = dir.path().join(path.as_str().unwrap());
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    dir
}

/// Runs `snp __hazmat argv` (this binary) in `cwd` with stdout and stderr on one pipe and an empty stdin pipe
/// (the hook runner's capture); returns `(status, merged output)`.
fn hazmat_process(argv: &[&str], cwd: &Path) -> (std::process::ExitStatus, Vec<u8>) {
    let (mut reader, writer) = std::io::pipe().unwrap();
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LC_ALL", "C.UTF-8")
        .env(CHILD, serde_json::to_string(argv).unwrap())
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(writer.try_clone().unwrap())
        .stderr(writer);
    let mut child = cmd.spawn().unwrap();
    drop(child.stdin.take());
    drop(cmd);
    let mut out = Vec::new();
    reader.read_to_end(&mut out).unwrap();
    (child.wait().unwrap(), out)
}

/// The exit code a hook runner reports (`returncode`: `-N` for a signal).
fn returncode(status: std::process::ExitStatus) -> i64 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        if let Some(sig) = status.signal() {
            return -i64::from(sig);
        }
    }
    i64::from(status.code().unwrap())
}

/// `(code, output)` with the work tree replaced by `<root>` (also its physical path, which `pwd` prints: macOS
/// temp directories live under the `/var` -> `/private/var` symlink).
fn run_in(argv: &[&str], dir: &Path) -> (i64, String) {
    let (status, out) = hazmat_process(argv, dir);
    let root = dir.to_string_lossy().into_owned();
    let real = snp_os::realpath::realpath_nonstrict(dir, dir)
        .map_or_else(|_| root.clone(), |p| p.to_string_lossy().into_owned());
    (
        returncode(status),
        show(&out).replace(&real, "<root>").replace(&root, "<root>"),
    )
}

/// Whether `/bin/sh` is dash (Debian, Ubuntu).
fn sh_is_dash() -> bool {
    snp_os::realpath::realpath_nonstrict(Path::new("/bin/sh"), Path::new("/"))
        .is_ok_and(|p| p.file_name().is_some_and(|n| n == "dash"))
}

/// Every case of `cases.json` against the pre-commit 4.6.2 goldens.
fn hazmat_goldens() {
    let cases = support::json("hazmat/cases.json");
    let goldens = support::json("hazmat/goldens.json");
    let mut failures = Vec::new();
    for case in cases["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let argv: Vec<&str> = case["argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect();
        let dir = fixtures();
        let (code, out) = run_in(&argv, dir.path());
        let want = &goldens["results"][name];
        let want_out = show(&latin1(want["out"].as_str().unwrap()));
        // The goldens were recorded where /bin/sh is dash; a case whose output is the shell's own error text
        // (`/bin/sh: 0: cannot open …`) has another text and code under bash or zsh (macOS).
        if want_out.starts_with("/bin/sh: 0: ") && !sh_is_dash() {
            continue;
        }
        if code != want["code"].as_i64().unwrap() || out != want_out {
            failures.push(format!(
                "{name}: code {code} (want {})\n--- got\n{out}\n--- want\n{want_out}",
                want["code"]
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// T10-HAZ-01 (`hazmat_test.py:12-34`): no `--` → ``hazmat entry must end with `--` ``; the last `--` splits.
fn t10_haz_01_cmd_filenames() {
    let dir = fixtures();
    assert_eq!(
        run_in(&["n1", "hello", "world"], dir.path()),
        (1, "hazmat entry must end with `--`\n".to_owned())
    );
    let (code, out) = run_in(
        &[
            "n1",
            "sh",
            "-c",
            "printf '<%s>' \"$@\"; echo",
            "sh",
            "--",
            "arg",
            "--",
            "f1",
            "f2",
        ],
        dir.path(),
    );
    assert_eq!((code, out.as_str()), (0, "<--><arg><f1>\n<--><arg><f2>\n"));
}

/// T10-HAZ-02 (`:37-41`): a file outside the subdir → `unexpected file without prefix='subdir/': not-subdir/2`.
fn t10_haz_02_cd_prefix() {
    let dir = fixtures();
    std::fs::create_dir_all(dir.path().join("subdir")).unwrap();
    assert_eq!(
        run_in(
            &["cd", "subdir", "echo", "--", "subdir/1", "not-subdir/2"],
            dir.path()
        ),
        (
            1,
            "unexpected file without prefix='subdir/': not-subdir/2\n".to_owned()
        )
    );
}

/// T10-HAZ-03 (`:48-67`): the command runs in the subdir with the prefix stripped; nothing on stderr.
fn t10_haz_03_cd_runs_in_subdir() {
    let dir = fixtures();
    std::fs::create_dir_all(dir.path().join("subdir")).unwrap();
    let (code, out) = run_in(
        &[
            "cd",
            "subdir",
            "sh",
            "-c",
            "pwd; printf '<%s>' \"$@\"",
            "sh",
            "--",
            "subdir/a",
            "subdir/b",
        ],
        dir.path(),
    );
    assert_eq!((code, out.as_str()), (0, "<root>/subdir\n<a><b>"));
}

/// T10-HAZ-04 (`:70-77`): `ignore-exit-code` returns 0 whatever the child does; its stderr passes through.
fn t10_haz_04_ignore_exit_code() {
    let dir = fixtures();
    assert_eq!(
        run_in(
            &["ignore-exit-code", "sh", "-c", "echo bye >&2; exit 1"],
            dir.path()
        ),
        (0, "bye\n".to_owned())
    );
}

/// T10-HAZ-05 (`:80-99`): one invocation per file; any failure → non-zero (the OR of the codes).
fn t10_haz_05_n1_one_call_per_file() {
    let dir = fixtures();
    assert_eq!(
        run_in(
            &[
                "n1",
                "sh",
                "-c",
                "echo \"['$1']\"",
                "sh",
                "--",
                "foo",
                "bar",
                "baz"
            ],
            dir.path()
        ),
        (0, "['foo']\n['bar']\n['baz']\n".to_owned())
    );
    let (code, _) = run_in(
        &[
            "n1",
            "sh",
            "-c",
            "[ $1 != bar ]",
            "sh",
            "--",
            "foo",
            "bar",
            "baz",
        ],
        dir.path(),
    );
    assert_eq!(code, 1);
}

/// X13-HZ-06: inside a hook entry there is no error handler: never exit 3; the final traceback line with exit 1,
/// argparse texts with prog `hazmat.py` and exit 2, `SystemExit` texts with exit 1 (the prefix check before the
/// cwd check).
fn x13_hz_06_in_hook_failures() {
    let dir = fixtures();
    assert_eq!(
        run_in(&["n1", "nonexistent-exe", "--"], dir.path()),
        (
            1,
            "pre_commit.parse_shebang.ExecutableNotFoundError: Executable `nonexistent-exe` not found\n"
                .to_owned()
        )
    );
    let (code, out) = run_in(&[".pre-commit-config.yaml"], dir.path());
    assert_eq!(code, 2);
    assert_eq!(
        out,
        "usage: hazmat.py [-h] {cd,ignore-exit-code,n1} ...\nhazmat.py: error: argument tool: invalid choice: \
         '.pre-commit-config.yaml' (choose from 'cd', 'ignore-exit-code', 'n1')\n"
    );
    assert_eq!(
        run_in(
            &["cd", "nodir", "echo", "--", ".pre-commit-config.yaml"],
            dir.path()
        ),
        (
            1,
            "unexpected file without prefix='nodir/': .pre-commit-config.yaml\n".to_owned()
        )
    );
    assert_eq!(
        run_in(&[], dir.path()),
        (
            1,
            "AttributeError: 'Namespace' object has no attribute 'cmd'\n".to_owned()
        )
    );
}

/// HAZ-B13/C8: an executable without a shebang is `OSError: [Errno 8] Exec format error`, never run by `/bin/sh`.
fn no_sh_fallback() {
    let dir = fixtures();
    assert_eq!(
        run_in(&["n1", "./noshebang.sh", "--", "b.txt"], dir.path()),
        (
            1,
            "OSError: [Errno 8] Exec format error: './noshebang.sh'\n".to_owned()
        )
    );
}

/// §H.7.4: on SIGINT the child gets 250 ms, is `SIGKILL`ed (even when it ignores SIGINT) and hazmat dies by SIGINT.
fn sigint_kills_after_250ms() {
    let dir = fixtures();
    let marker = dir.path().join("marker");
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env(
            CHILD,
            serde_json::to_string(&[
                "n1",
                "sh",
                "-c",
                "trap '' INT; echo started; sleep 3; echo survived > marker",
                "sh",
                "--",
                "x",
            ])
            .unwrap(),
        )
        .current_dir(dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut first = [0_u8; 8];
    stdout.read_exact(&mut first).unwrap();
    assert_eq!(&first, b"started\n");
    let start = Instant::now();
    let kill = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(kill.success());
    let status = child.wait().unwrap();
    let elapsed = start.elapsed();
    assert!(elapsed < Duration::from_millis(1500), "took {elapsed:?}");
    assert!(elapsed >= Duration::from_millis(200), "took {elapsed:?}");
    assert_eq!(returncode(status), -2, "{status:?}");
    std::thread::sleep(Duration::from_millis(3000));
    assert!(!marker.exists(), "the child survived");
}

/// Path C (`hazmat::cli`, inside the envelope): exceptions are `Failure`s (`ExecutableNotFoundError` exit 3),
/// `SystemExit` texts exit 1, the child status otherwise.
fn cli_path_exit_codes() {
    let env = Env::from_pairs([("PATH", "/usr/bin:/bin")]);
    let cancel = CancelToken::new();
    let os = |xs: &[&str]| xs.iter().map(OsString::from).collect::<Vec<_>>();
    let n1 = |cmd: &[&str]| HazmatTool::N1 { cmd: os(cmd) };
    match hazmat::cli(&n1(&["nope-not-here", "--", "a"]), &env, &cancel) {
        Err(Failure::Exc { kind, exc }) => {
            assert_eq!(kind, ExcKind::Unexpected);
            assert_eq!(exc.ty, "ExecutableNotFoundError");
            assert_eq!(exc.text, b"Executable `nope-not-here` not found");
        }
        other => panic!("{other:?}"),
    }
    match hazmat::cli(&n1(&["echo"]), &env, &cancel) {
        Err(Failure::SystemExit { code, stderr }) => {
            assert_eq!(code, 1);
            assert_eq!(stderr, b"hazmat entry must end with `--`\n");
        }
        other => panic!("{other:?}"),
    }
    match hazmat::cli(
        &HazmatTool::IgnoreExitCode { cmd: Vec::new() },
        &env,
        &cancel,
    ) {
        Err(Failure::Exc { kind, exc }) => {
            assert_eq!(kind, ExcKind::Unexpected);
            assert_eq!(exc.ty, "IndexError");
        }
        other => panic!("{other:?}"),
    }
    let ok = hazmat::cli(
        &n1(&["sh", "-c", "exit $1", "sh", "--", "1", "2"]),
        &env,
        &cancel,
    );
    assert_eq!(ok.unwrap(), 3);
    let ok = hazmat::cli(
        &n1(&["sh", "-c", "kill -9 $$", "sh", "--", "a"]),
        &env,
        &cancel,
    );
    assert_eq!(ok.unwrap(), 247);
    let ok = hazmat::cli(
        &HazmatTool::IgnoreExitCode {
            cmd: os(&["sh", "-c", "exit 5"]),
        },
        &env,
        &cancel,
    );
    assert_eq!(ok.unwrap(), 0);
}

/// `HAZMAT_PY_SPEC` renders the path H help texts of 03 Addendum §H.3 (CPython 3.13).
fn spec_and_help() {
    let env = Env::from_pairs(Vec::<(OsString, OsString)>::new());
    let cfg = snp_argparse::ParseCfg {
        prog: "hazmat.py",
        env: &env,
        columns: 80,
    };
    assert_eq!(
        snp_argparse::format_help(hazmat::HAZMAT_PY_SPEC, &["cd"], &cfg),
        "usage: hazmat.py cd [-h] subdir ...\n\npositional arguments:\n  subdir\n  cmd\n\noptions:\n  -h, \
         --help  show this help message and exit\n"
    );
}
