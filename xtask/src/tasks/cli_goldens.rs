//! `cargo xtask cli-goldens [--check]`: argparse outcome and help goldens from the oracles (12 §A.16, ADR-058).
//!
//! Writes `crates/snp-cli/tests/goldens/{compat,snp}.json`: for every case (argv + env) the exit code and the
//! stdout/stderr bytes, or for a successful parse the namespace (`vars(args)` as JSON). Sources:
//!
//! * **compat brand**: pre-commit 4.7.0 on `target/oracle/pc470-py313` (CPython ≥ 3.13.14). Help and error cases
//!   come from the real `bin/pre-commit`; successful parses from `xtask/oracle/argparse_from_json.py pre-commit`,
//!   which dumps pre-commit's own namespace before any command runs. Every case is also run through a real CPython
//!   argparse built from `snp_argparse::export_json(PRE_COMMIT_SPEC, "pre-commit")` and must agree: this proves the
//!   transcription of `main.py` and the JSON builder.
//! * **snp brand**: a real CPython argparse built from `export_json(SNP_SPEC, "snp")` (reference (b) of ADR-058).
//!   For every case reported by a parser whose snp spec equals pre-commit's, the output must also equal the
//!   prog-patched oracle `bin/pre-commit-snp-prog` (reference (a)).
//!
//! `--check` regenerates in memory and fails when the committed files differ. Needs the oracle venvs
//! (`cargo xtask oracle install`); CI's hermetic jobs only read the committed goldens.
//!
//! Owner: WP-12.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context as _, bail, ensure};
use serde_json::{Map, Value, json};

use crate::workspace;

/// Widths of the help matrix (12 §A.16) plus edge cases of `shutil.get_terminal_size` and `HelpFormatter`.
const WIDTHS: &[&str] = &["10", "40", "60", "80", "100", "120", "200"];
const EXTRA_WIDTHS: &[&str] = &["1", "3", "24", " 100 ", "abc", "0", "-5"];
const GOLDEN_DIR: &str = "crates/snp-cli/tests/goldens";
const NS_MARKER: &str = "\0ns\0";

/// One `(argv, env)` case.
#[derive(Debug, Clone)]
struct Case {
    argv: Vec<OsString>,
    env: Vec<(&'static str, OsString)>,
}

impl Case {
    fn new(argv: &[&str], env: &[(&'static str, &str)]) -> Self {
        let default_columns =
            (!env.iter().any(|&(k, _)| k == "COLUMNS")).then(|| ("COLUMNS", OsString::from("80")));
        let env: Vec<(&'static str, OsString)> = default_columns
            .into_iter()
            .chain(env.iter().map(|&(k, v)| (k, OsString::from(v))))
            .collect();
        Self {
            argv: argv.iter().map(OsString::from).collect(),
            env,
        }
    }

    /// A readable, unique identifier: `K=V … argv…` with shell-like quoting.
    fn id(&self) -> String {
        let mut parts: Vec<String> = self
            .env
            .iter()
            .filter(|(k, v)| !(*k == "COLUMNS" && v.as_os_str() == "80"))
            .map(|(k, v)| format!("{k}={}", quote(v)))
            .collect();
        parts.extend(self.argv.iter().map(|a| quote(a)));
        if parts.is_empty() {
            "(no arguments)".to_owned()
        } else {
            parts.join(" ")
        }
    }
}

fn quote(s: &OsStr) -> String {
    match s.to_str() {
        Some(t)
            if !t.is_empty()
                && t.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_=./:@,+".contains(c)) =>
        {
            t.to_owned()
        }
        Some(t) => format!("{t:?}"),
        None => format!("$'{}'", hex_escaped(s)),
    }
}

fn hex_escaped(s: &OsStr) -> String {
    s.as_encoded_bytes()
        .iter()
        .map(|&b| {
            if b.is_ascii_graphic() {
                char::from(b).to_string()
            } else {
                format!("\\x{b:02x}")
            }
        })
        .collect()
}

/// An argv item or env value as JSON: a string, or `{"hex": …}` when it is not valid UTF-8.
fn os_json(s: &OsStr) -> Value {
    match s.to_str() {
        Some(t) => Value::String(t.to_owned()),
        None => json!({ "hex": hex(s.as_encoded_bytes()) }),
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().fold(String::new(), |mut out, x| {
        write!(out, "{x:02x}").unwrap_or_default();
        out
    })
}

/// A byte string from `\xNN` escapes inside an otherwise literal ASCII string.
fn bytes_arg(s: &str) -> OsString {
    let mut out: Vec<u8> = Vec::new();
    let b = s.as_bytes();
    let mut i = 0usize;
    while let Some(&c) = b.get(i) {
        if c == b'\\'
            && b.get(i.saturating_add(1)) == Some(&b'x')
            && let Some(h) = s.get(i.saturating_add(2)..i.saturating_add(4))
            && let Ok(v) = u8::from_str_radix(h, 16)
        {
            out.push(v);
            i = i.saturating_add(4);
        } else {
            out.push(c);
            i = i.saturating_add(1);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt as _;
        OsString::from_vec(out)
    }
    #[cfg(not(unix))]
    {
        OsString::from(String::from_utf8_lossy_owned(out))
    }
}

// ----- the case tables ------------------------------------------------------------------------------------------

/// Parsers whose help is captured at every width: (path, argv prefix).
const PC_HELP_PATHS: &[&[&str]] = &[
    &[],
    &["autoupdate"],
    &["clean"],
    &["gc"],
    &["hazmat"],
    &["hazmat", "cd"],
    &["hazmat", "ignore-exit-code"],
    &["hazmat", "n1"],
    &["init-templatedir"],
    &["install"],
    &["install-hooks"],
    &["migrate-config"],
    &["run"],
    &["sample-config"],
    &["try-repo"],
    &["uninstall"],
    &["validate-config"],
    &["validate-manifest"],
    &["help"],
    &["hook-impl"],
];

const SNP_ONLY_HELP_PATHS: &[&[&str]] = &[
    &["list"],
    &["config"],
    &["config", "dump"],
    &["config", "schema"],
    &["cache"],
    &["cache", "dir"],
    &["cache", "size"],
    &["cache", "gc"],
    &["cache", "clean"],
    &["migrate"],
    &["migrate", "lefthook"],
    &["migrate", "builtin"],
    &["migrate", "upstream"],
    &["doctor"],
    &["self"],
    &["self", "alias"],
    &["self", "alias", "install"],
    &["self", "alias", "uninstall"],
    &["self", "alias", "status"],
    &["self", "check"],
    &["completions"],
];

/// One row: (env, argv).
type Row = (
    &'static [(&'static str, &'static str)],
    &'static [&'static str],
);

/// Every row of 12 §A.5, §A.6, §A.8.2, §A.8.3 and §A.9 (and the corner cases next to them), as (env, argv).
const ROWS: &[Row] = &[
    // §A.5 help and help-like argv.
    (&[], &["help"]),
    (&[], &["help", ""]),
    (&[], &["help", "run"]),
    (&[], &["help", "--", "run"]),
    (&[], &["help", "hook-impl"]),
    (&[], &["help", "hazmat"]),
    (&[], &["help", "help"]),
    (&[], &["help", "bogus"]),
    (&[], &["help", "ru"]),
    (&[], &["help", "run", "extra"]),
    (&[], &["help", "hazmat", "cd"]),
    (&[], &["help", "--color", "never"]),
    (&[], &["help", "--bogus"]),
    (&[], &["help", "--", "--bogus"]),
    (&[], &["help", "--", "-V"]),
    (&[], &["help", "--", "--"]),
    (&[], &["help", "--", "--help=x"]),
    (&[("PRE_COMMIT_COLOR", "bogus")], &["help", "run"]),
    (&[], &["help", "-h"]),
    (&[], &["help", "--help"]),
    (&[], &["--help"]),
    (&[], &["-h", "run"]),
    (&[], &["-h", "--bogus"]),
    (&[], &["--he"]),
    (&[], &["--help", "--bogus"]),
    (&[], &["--v"]),
    (&[], &["--vers"]),
    (&[], &["-V"]),
    (&[], &["--version"]),
    (&[], &["--version", "run", "--bogus"]),
    (&[], &["-Vh"]),
    (&[], &["run", "-h", "--bogus"]),
    (&[], &["run", "--bogus", "-h"]),
    (&[], &["run", "--hook-stage", "bogus", "-h"]),
    (&[], &["run", "--he"]),
    (&[], &["run", "--ver", "-a"]),
    (&[], &["run", "-V"]),
    (&[], &["run", "--version"]),
    (&[], &["run", "--vers"]),
    (&[], &["run", "-he"]),
    (&[], &[]),
    // §A.6.1 reported by the top-level parser.
    (&[], &["--bogus"]),
    (&[], &["-v"]),
    (&[], &["---"]),
    (&[], &["--=x"]),
    (&[], &["run", "--bogus"]),
    (&[], &["run", "--bogus=1"]),
    (&[], &["run", "-x"]),
    (&[], &["validate-config", "--colour", "never"]),
    (&[], &["hazmat", "cd", "--foo", "sub"]),
    (&[], &["hazmat", "n1", "-n", "echo", "--", "a"]),
    (
        &[],
        &["hazmat", "n1", "--color", "never", "echo", "--", "a"],
    ),
    (&[], &["run", "--bogus", "-y", "zz"]),
    (&[], &["run", "a", "b"]),
    (&[], &["sample-config", "extra"]),
    (&[], &["clean", "extra"]),
    (&[], &["validate-config", "a", "--color", "never", "b"]),
    (&[], &["run", "-v1"]),
    (&[], &["run", "-all-files"]),
    (&[], &["run", "-c=x.yaml", "-v1"]),
    (&[], &["--"]),
    (&[], &["bogus"]),
    (&[], &["ru"]),
    (&[], &["sample"]),
    (&[], &[""]),
    (&[], &["--color", "never", "run"]),
    (&[], &["-c", "foo.yaml", "run"]),
    (&[], &["--bogus", "run", "--bogus2"]),
    (&[], &["__hazmat", "n1"]),
    // §A.6.2 reported by a sub-parser.
    (&[], &["run", "--hook-stage", "bogus"]),
    (&[], &["run", "--hook-stage", "Manual"]),
    (&[], &["run", "--hook-stage="]),
    (&[], &["run", "--hook-stage", "it's"]),
    (&[], &["run", "--hook-stage", "a\tb\u{1}"]),
    (&[], &["run", "--hook-stage", "it's \"q\""]),
    (&[], &["run", "--hook-stage", "é\u{a0}\u{200b}"]),
    (&[], &["run", "--hook-stage", "commit"]),
    (&[], &["run", "--hook-stage", "push"]),
    (&[], &["run", "--hook-stage", "merge-commit"]),
    (&[], &["run", "--hook-stage", "manual"]),
    (&[], &["run", "--hook-st=commit"]),
    (&[], &["install", "-t", "bogus"]),
    (&[], &["install", "-t", "manual"]),
    (&[], &["install", "-tpre-push", "-t", "commit-msg"]),
    (&[], &["install", "-ftpre-push"]),
    (&[], &["install", "-ftbogus"]),
    (&[], &["run", "--color", "bogus"]),
    (&[], &["run", "--color", "Never"]),
    (&[], &["run", "--color="]),
    (&[], &["autoupdate", "-j", "x"]),
    (&[], &["autoupdate", "--jobs=1.5"]),
    (&[], &["autoupdate", "-j", " 2"]),
    (&[], &["autoupdate", "-j", "1_0"]),
    (&[], &["autoupdate", "-j", "-1"]),
    (&[], &["autoupdate", "-j", "0x10"]),
    (&[], &["autoupdate", "-j", "\u{663}"]),
    (
        &[],
        &[
            "autoupdate",
            "-j2",
            "--repo",
            "a",
            "--repo",
            "b",
            "--repo",
            "a",
        ],
    ),
    (&[], &["run", "--hook-stage"]),
    (&[], &["run", "-c"]),
    (&[], &["run", "-s"]),
    (&[], &["install", "-t"]),
    (&[], &["install", "-ft"]),
    (&[], &["autoupdate", "-j"]),
    (&[], &["autoupdate", "--repo"]),
    (&[], &["hook-impl", "--hook-type"]),
    (&[], &["run", "--color"]),
    (&[("PRE_COMMIT_COLOR", "bogus")], &["run", "--color"]),
    (&[], &["run", "--co", "x"]),
    (&[], &["run", "--co=never"]),
    (&[], &["run", "--c", "never"]),
    (&[], &["run", "--s", "x"]),
    (&[], &["try-repo", "r", "--re", "x"]),
    (&[], &["install", "--co", "x"]),
    (&[], &["install", "--con", "x.yaml"]),
    (&[], &["install", "--conf", "x.yaml"]),
    (&[], &["uninstall", "--conf", "x.yaml"]),
    (&[], &["run", "--all"]),
    (&[], &["run", "--fi", "a"]),
    (&[], &["run", "--file", "a"]),
    (&[], &["run", "--show-diff"]),
    (&[], &["run", "--verb"]),
    (&[], &["install", "--over"]),
    (&[], &["sample-config", "--c", "never"]),
    (&[], &["run", "--verbose=1"]),
    (&[], &["run", "--all-files="]),
    (&[], &["run", "-a=1"]),
    (&[], &["run", "-a-v"]),
    (&[], &["run", "-av="]),
    (&[], &["run", "-av"]),
    (&[], &["run", "-va"]),
    (&[], &["run", "-vac", "x.yaml"]),
    (&[], &["run", "-vacx.yaml"]),
    (&[], &["run", "-cfoo.yaml"]),
    (&[], &["hook-impl", "--hook-type", "x", "--skip=1"]),
    (&[], &["run", "-a", "--files", "x"]),
    (&[], &["run", "--files", "x", "-a"]),
    (&[], &["run", "--files", "-a"]),
    (&[], &["run", "--files", "a", "--files", "b", "-a"]),
    (&[], &["try-repo", "r", "-a", "--files", "x"]),
    (&[], &["init-templatedir"]),
    (&[], &["try-repo"]),
    (&[], &["hook-impl"]),
    (&[], &["hook-impl", "--", "--hook-type=post-commit"]),
    (&[("PRE_COMMIT_COLOR", "bogus")], &["sample-config"]),
    (&[("PRE_COMMIT_COLOR", "")], &["sample-config"]),
    (&[("PRE_COMMIT_COLOR", "ALWAYS")], &["sample-config"]),
    (&[("PRE_COMMIT_COLOR", "never ")], &["sample-config"]),
    (&[("PRE_COMMIT_COLOR", "bogus")], &[]),
    // §A.6.3 hazmat.
    (&[], &["hazmat", "bogus"]),
    (&[], &["hazmat", "ignore"]),
    (&[], &["hazmat", "cd"]),
    (
        &[("PRE_COMMIT_COLOR", "bogus")],
        &["hazmat", "n1", "echo", "--", "a"],
    ),
    (&[("PRE_COMMIT_COLOR", "bogus")], &["hazmat", "cd"]),
    (
        &[("PRE_COMMIT_COLOR", "bogus")],
        &["hazmat", "cd", "--help"],
    ),
    (&[], &["hazmat", "ignore-exit-code", "-h"]),
    (
        &[],
        &["hazmat", "--color", "never", "n1", "echo", "--", "a"],
    ),
    (&[], &["hazmat"]),
    (&[], &["hazmat", "--color", "always"]),
    (&[], &["hazmat", "cd", "sub"]),
    (&[], &["hazmat", "cd", "sub", "-h"]),
    (&[], &["hazmat", "n1", "--", "echo", "--", "a"]),
    (&[], &["hazmat", "ignore-exit-code", "--", "false"]),
    (
        &[],
        &["hazmat", "cd", ".", "definitely-not-a-cmd-xyz", "--"],
    ),
    (&[], &["hazmat", "cd", "--", "sub", "echo", "--"]),
    (&[], &["hazmat", "n1", "echo", "--", "a", "b"]),
    (&[], &["hazmat", "n1", "echo", "-n", "--", "a", "b"]),
    (&[], &["hazmat", "cd", "sub", "--foo", "x", "--"]),
    (&[], &["hazmat", "-h", "cd"]),
    // §A.8.2 precedence.
    (&[], &["run", "--bogus", "--hook-stage", "bogus"]),
    (&[], &["--bogus", "run", "--hook-stage", "bogus"]),
    (&[], &["run", "-a", "--files", "x", "--hook-stage", "bogus"]),
    (&[], &["run", "--hook-stage", "bogus", "-a", "--files", "x"]),
    (&[], &["run", "--hook-stage", "bogus", "--co", "x"]),
    (&[], &["run", "-h", "--co", "x"]),
    (&[], &["install", "-t", "bogus", "-t"]),
    (&[], &["autoupdate", "-j", "x", "-j", "2"]),
    (&[], &["run", "--color", "bogus", "--color", "never"]),
    (&[], &["try-repo", "--hook-stage", "bogus"]),
    (&[("PRE_COMMIT_COLOR", "bogus")], &["run", "--bogus"]),
    (
        &[("PRE_COMMIT_COLOR", "bogus")],
        &["run", "-a", "--files", "x"],
    ),
    (
        &[("PRE_COMMIT_COLOR", "bogus")],
        &["run", "--hook-stage", "bogus"],
    ),
    (&[("PRE_COMMIT_COLOR", "bogus")], &["try-repo"]),
    (&[], &["run", "-a", "--files", "x", "--hook-stage"]),
    (&[], &["run", "--files", "x", "-a", "--bogus"]),
    // §A.8.3 values.
    (&[], &["run", "--color", "always", "--color", "never"]),
    (&[], &["run", "-c", "a.yaml", "--config=b.yaml"]),
    (
        &[],
        &["run", "--hook-stage", "push", "--hook-stage", "manual"],
    ),
    (&[], &["run", "--files", "a", "--files", "b"]),
    (&[], &["install", "-t", "pre-push", "-t", "pre-push"]),
    (&[], &["run", "--files", "a.txt", "hook"]),
    (&[], &["run", "--files=a.txt", "hook"]),
    (&[], &["run", "--files", "a.txt", "--", "hook"]),
    (&[], &["run", "--files", "--", "hook"]),
    (&[], &["run", "--files", "-1", "-a"]),
    (&[], &["run", "--files", "-1", "-.5", "-1x"]),
    (&[], &["run", "--", "echo", "--files", "a.txt"]),
    (&[], &["run", "--files", "a.txt", "--", "echo", "extra"]),
    (&[], &["run", "--", "-a"]),
    (&[], &["run", "--all files"]),
    (&[], &["run", "-"]),
    (&[], &["run", "myhook", "-a"]),
    (&[], &["run", "-a", "myhook"]),
    (&[], &["run", ""]),
    (&[], &["run", "lint:js"]),
    (&[], &["run", "--files"]),
    (&[], &["run", "--files", "--"]),
    (&[], &["run", "--", "--"]),
    (&[], &["run", "--source", "a", "--origin", "b", "-s", "c"]),
    (&[], &["run", "--from-ref=", "--to-ref", ""]),
    (&[], &["run", "--commit-msg-filename", ""]),
    (
        &[],
        &[
            "run",
            "--hook-stage",
            "pre-push",
            "--remote-branch",
            "rb",
            "--local-branch",
            "lb",
            "--remote-name",
            "origin",
            "--remote-url",
            "https://x",
            "--from-ref",
            "f",
            "--to-ref",
            "t",
        ],
    ),
    (
        &[],
        &[
            "run",
            "--pre-rebase-upstream",
            "u",
            "--pre-rebase-branch",
            "b",
            "--prepare-commit-message-source",
            "s",
            "--commit-object-name",
            "o",
            "--checkout-type",
            "1",
            "--is-squash-merge",
            "0",
            "--rewrite-command",
            "amend",
            "--show-diff-on-failure",
            "--fail-fast",
            "-v",
        ],
    ),
    (&[], &["try-repo", "r", "-a", "myhook"]),
    (&[], &["try-repo", "r", "--ref", "X", "myhook"]),
    (&[], &["try-repo", "r", "--rev=", "myhook"]),
    (&[], &["try-repo", "r", "myhook", "--files", "a", "b"]),
    (&[], &["try-repo", "--", "r"]),
    (
        &[],
        &["hook-impl", "--hook-type=post-commit", "--", "a", "b"],
    ),
    (&[], &["hook-impl", "--hook-type=post-commit", "a", "b"]),
    (&[], &["hook-impl", "--hook-type=post-commit", "--", "-h"]),
    (
        &[],
        &[
            "hook-impl",
            "--hook-type=post-commit",
            "--",
            "--color",
            "never",
        ],
    ),
    (
        &[],
        &["hook-impl", "--hook-type=post-commit", "--", "--", "x"],
    ),
    (
        &[],
        &[
            "hook-impl",
            "--hook-type=post-commit",
            "x",
            "--hook-dir",
            "y",
        ],
    ),
    (&[], &["hook-impl", "--hook-type=post-commit", "-h"]),
    (&[], &["hook-impl", "--hook-type=post-commit", "--"]),
    (
        &[],
        &[
            "hook-impl",
            "--hook-type=pre-commit",
            "--hook-dir",
            "/r/.git/hooks",
            "--",
        ],
    ),
    (
        &[],
        &[
            "hook-impl",
            "--config=.pre-commit-config.yaml",
            "--hook-type=pre-push",
            "--hook-dir",
            "/r/.git/hooks",
            "--skip-on-missing-config",
            "--",
            "origin",
            "https://x",
        ],
    ),
    (
        &[],
        &[
            "hook-impl",
            "--hook-type",
            "commit-msg",
            "--hook-dir",
            ".git/hooks",
            ".git/COMMIT_EDITMSG",
        ],
    ),
    (&[], &["hook-impl", "--hook-type=bogus", "--"]),
    (&[], &["hazmat", "cd", "sub", "-h"]),
    (&[], &["hazmat", "n1", "--", "echo", "--", "a"]),
    (
        &[],
        &[
            "init-templatedir",
            "d",
            "--no-allow-missing-config",
            "-t",
            "pre-push",
        ],
    ),
    (&[], &["init-templatedir", "-c", "x", "d"]),
    (
        &[],
        &[
            "install",
            "-f",
            "--install-hooks",
            "--allow-missing-config",
            "-t",
            "pre-commit",
            "-t",
            "commit-msg",
        ],
    ),
    (&[], &["uninstall", "-t", "pre-push"]),
    (&[], &["install-hooks", "-c", "x.yaml"]),
    (&[], &["migrate-config", "--config", "x.yaml"]),
    (
        &[],
        &["autoupdate", "--bleeding-edge", "--freeze", "--repo", "r"],
    ),
    (&[], &["validate-config"]),
    (&[], &["validate-config", "a", "b"]),
    (&[], &["validate-manifest", "--", "-a"]),
    (&[], &["sample-config"]),
    (&[], &["clean"]),
    (&[], &["gc"]),
    // §A.9 PRE_COMMIT_COLOR.
    (
        &[("PRE_COMMIT_COLOR", "bogus")],
        &["sample-config", "--color", "never"],
    ),
    (
        &[("PRE_COMMIT_COLOR", "bogus")],
        &["sample-config", "--color=always"],
    ),
    (&[("PRE_COMMIT_COLOR", "bogus")], &["run"]),
    (
        &[("PRE_COMMIT_COLOR", "bogus")],
        &["hook-impl", "--hook-type=pre-commit", "--"],
    ),
    (&[("PRE_COMMIT_COLOR", "bogus")], &["-V"]),
    (&[("PRE_COMMIT_COLOR", "bogus")], &["-h"]),
    (&[("PRE_COMMIT_COLOR", "bogus")], &["help"]),
    (&[("PRE_COMMIT_COLOR", "bogus")], &["run", "--help"]),
    (&[("PRE_COMMIT_COLOR", "bogus")], &["validate-config", "-h"]),
    (&[("PRE_COMMIT_COLOR", "always")], &["sample-config"]),
    (&[("PRE_COMMIT_COLOR", "never")], &["run"]),
    (&[("PRE_COMMIT_COLOR", "")], &["run", "-h"]),
    (
        &[("PRE_COMMIT_COLOR", "a long value with spaces that wraps")],
        &["sample-config", "-h"],
    ),
    // Widths of usage errors (stdout's width, 12 §A.3).
    (&[("COLUMNS", "40")], &["run", "--bogus"]),
    (&[("COLUMNS", "40")], &["install", "-t", "bogus"]),
    (&[("COLUMNS", "10")], &["init-templatedir"]),
    (&[("COLUMNS", "200")], &["try-repo"]),
    (&[("COLUMNS", "10")], &["-V"]),
    (&[("COLUMNS", "1")], &["--version"]),
];

/// Rows that use snp-only commands and flags (both brands: the compat brand rejects them with pre-commit's texts).
const EXTENSION_ROWS: &[Row] = &[
    (&[], &["list"]),
    (
        &[],
        &[
            "list",
            "--json",
            "--hook-stage",
            "push",
            "--group",
            "g",
            "--require-group",
            "r",
            "--no-group",
            "@ungrouped",
            "-c",
            "x.yaml",
        ],
    ),
    (&[], &["list", "--hook-stage", "bogus"]),
    (&[], &["config"]),
    (&[], &["config", "dump"]),
    (
        &[],
        &[
            "config",
            "dump",
            "--json",
            "--no-local-config",
            "-c",
            "x.yaml",
        ],
    ),
    (&[], &["config", "schema"]),
    (&[], &["config", "bogus"]),
    (&[], &["cache"]),
    (&[], &["cache", "dir"]),
    (&[], &["cache", "size"]),
    (&[], &["cache", "gc"]),
    (&[], &["cache", "clean"]),
    (&[], &["cache", "bogus"]),
    (&[], &["migrate"]),
    (&[], &["migrate", "lefthook"]),
    (
        &[],
        &[
            "migrate",
            "lefthook",
            "--root",
            "r",
            "-c",
            "lefthook.yml",
            "-o",
            "-",
            "--portable",
            "--local",
            "--strict",
            "--install",
            "-f",
        ],
    ),
    (&[], &["migrate", "builtin"]),
    (&[], &["migrate", "upstream", "-c", "x.yaml"]),
    (&[], &["doctor"]),
    (
        &[],
        &["doctor", "--fix", "--fast-path", "--stash", "-c", "x.yaml"],
    ),
    (&[], &["self"]),
    (&[], &["self", "check"]),
    (&[], &["self", "alias"]),
    (&[], &["self", "alias", "install"]),
    (
        &[],
        &["self", "alias", "install", "--dir", "/usr/local/bin", "-f"],
    ),
    (&[], &["self", "alias", "uninstall"]),
    (&[], &["self", "alias", "status"]),
    (&[], &["completions"]),
    (&[], &["completions", "zsh"]),
    (&[], &["completions", "bogus"]),
    (&[], &["run", "a", "b", "lint:js"]),
    (&[], &["run", "hook1", "-a", "hook2"]),
    (&[], &["run", "-j", "4"]),
    (&[], &["run", "--jobs=0"]),
    (&[], &["run", "-j", "x"]),
    (
        &[],
        &[
            "run",
            "--group",
            "a",
            "--group",
            "b",
            "--require-group",
            "c",
            "--no-group",
            "d",
        ],
    ),
    (&[], &["run", "--gr", "a"]),
    (&[], &["run", "--req", "a"]),
    (&[], &["run", "--no-g", "a"]),
    (&[], &["run", "--no", "a"]),
    (
        &[],
        &["run", "--no-fast-path", "--summary", "--no-local-config"],
    ),
    (&[], &["run", "--stdin-files"]),
    (&[], &["run", "--stdin-files=nul"]),
    (&[], &["run", "--stdin-files=lines", "hook"]),
    (&[], &["run", "--stdin-files", "hook"]),
    (&[], &["run", "--stdin-files", "nul", "hook"]),
    (&[], &["run", "--stdin-files", "-a"]),
    (&[], &["run", "--files", "x", "--stdin-files"]),
    (&[], &["run", "--st"]),
    (&[], &["run", "--su"]),
    (
        &[],
        &["try-repo", "r", "h1", "h2", "-j", "2", "--stdin-files=nul"],
    ),
    (&[], &["install", "--git-config-hooks"]),
    (&[], &["install", "--git"]),
    (&[], &["uninstall", "--git-config-hooks", "-t", "pre-push"]),
    (&[], &["install-hooks", "--with-fallback"]),
    (&[], &["validate-config", "--strict", "--portable", "a"]),
    (&[], &["validate-manifest", "--strict"]),
    (&[], &["help", "list"]),
    (&[], &["help", "self"]),
];

fn compat_cases() -> Vec<Case> {
    let mut cases = Vec::new();
    help_cases(PC_HELP_PATHS, &mut cases);
    rows(ROWS, &mut cases);
    rows(EXTENSION_ROWS, &mut cases);
    non_utf8_cases(&mut cases);
    dedup(cases)
}

/// Drops later cases with the id of an earlier one (help rows also appear in the width matrix).
fn dedup(cases: Vec<Case>) -> Vec<Case> {
    let mut seen = BTreeSet::new();
    cases.into_iter().filter(|c| seen.insert(c.id())).collect()
}

fn snp_cases() -> Vec<Case> {
    let mut cases = Vec::new();
    help_cases(PC_HELP_PATHS, &mut cases);
    help_cases(SNP_ONLY_HELP_PATHS, &mut cases);
    rows(ROWS, &mut cases);
    rows(EXTENSION_ROWS, &mut cases);
    non_utf8_cases(&mut cases);
    dedup(cases)
}

fn help_cases(paths: &[&[&str]], out: &mut Vec<Case>) {
    for path in paths {
        let mut argv: Vec<&str> = path.to_vec();
        argv.push("-h");
        for w in WIDTHS.iter().chain(EXTRA_WIDTHS) {
            if EXTRA_WIDTHS.contains(w)
                && !(path.is_empty() || *path == ["run"] || *path == ["sample-config"])
            {
                continue;
            }
            out.push(Case::new(&argv, &[("COLUMNS", w)]));
        }
    }
}

fn rows(table: &[Row], out: &mut Vec<Case>) {
    for (env, argv) in table {
        out.push(Case::new(argv, env));
    }
}

/// Non-UTF-8 argv and env values (POSIX surrogateescape, 12 §A.3).
fn non_utf8_cases(out: &mut Vec<Case>) {
    if !cfg!(unix) {
        return;
    }
    let mk = |argv: &[&str], env: &[(&'static str, &str)]| Case {
        argv: argv.iter().map(|a| bytes_arg(a)).collect(),
        env: std::iter::once(("COLUMNS", OsString::from("80")))
            .chain(env.iter().map(|&(k, v)| (k, bytes_arg(v))))
            .collect(),
    };
    out.push(mk(&["run", "--hook-stage", "\\xff"], &[]));
    out.push(mk(&["run", "--bogus\\xff", "x\\xfe"], &[]));
    out.push(mk(&["run", "--files", "\\xff.py", "a\\xc3\\xa9"], &[]));
    out.push(mk(&["run", "h\\xff"], &[]));
    out.push(mk(
        &["hook-impl", "--hook-type=pre-push", "--", "\\xff"],
        &[],
    ));
    out.push(mk(&["run", "--co\\xff"], &[]));
    out.push(mk(&["run", "-v\\xff"], &[]));
    out.push(mk(&["sample-config"], &[("PRE_COMMIT_COLOR", "\\xff")]));
    out.push(mk(&["\\xff"], &[]));
    out.push(mk(&["autoupdate", "-j", "\\xff"], &[]));
}

// ----- running the oracles --------------------------------------------------------------------------------------

/// Paths of the oracle venv and the scratch directory.
#[derive(Debug)]
struct Oracle {
    bin: PathBuf,
    script: PathBuf,
    scratch: PathBuf,
}

/// One captured run: exit code, stdout, stderr, and the namespace when the parse succeeded.
#[derive(Debug, Clone, PartialEq)]
struct Outcome {
    exit: i32,
    stdout: String,
    stderr: String,
    ns: Option<Value>,
}

impl Oracle {
    fn command(&self, program: &Path, case: &Case) -> Command {
        let mut cmd = Command::new(program);
        cmd.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.scratch.join("home"))
            .env("PRE_COMMIT_HOME", self.scratch.join("store"))
            .env("LANG", "C.UTF-8")
            .current_dir(self.scratch.join("home"))
            .stdin(Stdio::null());
        for (k, v) in &case.env {
            cmd.env(k, v);
        }
        cmd
    }

    /// `python -I argparse_from_json.py <mode…> -- argv…`.
    fn script(&self, mode: &[&OsStr], case: &Case) -> anyhow::Result<Outcome> {
        let mut cmd = self.command(&self.bin.join("python"), case);
        cmd.arg("-I")
            .arg(&self.script)
            .args(mode)
            .arg("--")
            .args(&case.argv);
        capture(cmd)
    }

    fn exe(&self, name: &str, case: &Case) -> anyhow::Result<Outcome> {
        let mut cmd = self.command(&self.bin.join(name), case);
        cmd.args(&case.argv);
        capture(cmd)
    }
}

fn capture(mut cmd: Command) -> anyhow::Result<Outcome> {
    let out = cmd.output().with_context(|| format!("running {cmd:?}"))?;
    let stdout = String::from_utf8(out.stdout).context("oracle stdout is not UTF-8")?;
    let stderr = String::from_utf8(out.stderr).context("oracle stderr is not UTF-8")?;
    let exit = out.status.code().unwrap_or(-1);
    if let Some(rest) = stdout.strip_prefix(NS_MARKER) {
        let ns: Value = serde_json::from_str(rest.trim_end()).context("namespace JSON")?;
        return Ok(Outcome {
            exit,
            stdout: String::new(),
            stderr,
            ns: Some(ns),
        });
    }
    Ok(Outcome {
        exit,
        stdout,
        stderr,
        ns: None,
    })
}

/// Runs `f` over `cases` on a few threads, keeping the order.
fn par_map<T: Send>(
    cases: &[Case],
    f: &(dyn Fn(&Case) -> anyhow::Result<T> + Sync),
) -> anyhow::Result<Vec<T>> {
    let results: Mutex<Vec<Option<anyhow::Result<T>>>> =
        Mutex::new(cases.iter().map(|_| None).collect());
    let next = AtomicUsize::new(0);
    std::thread::scope(|s| -> anyhow::Result<()> {
        let mut handles = Vec::new();
        for _ in 0..8 {
            let h = std::thread::Builder::new().spawn_scoped(s, || {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(case) = cases.get(i) else { return };
                    let r = f(case);
                    if let Ok(mut res) = results.lock()
                        && let Some(slot) = res.get_mut(i)
                    {
                        *slot = Some(r);
                    }
                }
            })?;
            handles.push(h);
        }
        for h in handles {
            if h.join().is_err() {
                bail!("a worker thread panicked");
            }
        }
        Ok(())
    })?;
    let results = results
        .into_inner()
        .map_err(|_| anyhow::anyhow!("poisoned"))?;
    results
        .into_iter()
        .map(|r| r.unwrap_or_else(|| Err(anyhow::anyhow!("case not run"))))
        .collect()
}

fn outcome_json(case: &Case, o: &Outcome) -> Value {
    let mut m = Map::new();
    m.insert("id".to_owned(), Value::String(case.id()));
    m.insert(
        "argv".to_owned(),
        Value::Array(case.argv.iter().map(|a| os_json(a)).collect()),
    );
    let mut env = Map::new();
    for (k, v) in &case.env {
        env.insert((*k).to_owned(), os_json(v));
    }
    m.insert("env".to_owned(), Value::Object(env));
    m.insert("exit".to_owned(), json!(o.exit));
    m.insert("stdout".to_owned(), Value::String(o.stdout.clone()));
    m.insert("stderr".to_owned(), Value::String(o.stderr.clone()));
    m.insert("ns".to_owned(), o.ns.clone().unwrap_or(Value::Null));
    Value::Object(m)
}

/// The parser at `path` in an `export_json` tree.
fn parser_at<'v>(tree: &'v Value, path: &[String]) -> Option<&'v Value> {
    let mut node = tree.get("parser")?;
    for name in path {
        let actions = node.get("actions")?.as_array()?;
        node = actions.iter().find_map(|a| {
            a.get("subcommands")?
                .as_array()?
                .iter()
                .find(|s| s.get("name").and_then(Value::as_str) == Some(name))?
                .get("parser")
        })?;
    }
    Some(node)
}

/// Every parser path of an `export_json` tree with its prog.
fn all_paths(tree: &Value, prog: &str) -> Vec<(Vec<String>, String)> {
    let mut out = Vec::new();
    let mut stack: Vec<(Vec<String>, String)> = vec![(Vec::new(), prog.to_owned())];
    while let Some((path, p)) = stack.pop() {
        if let Some(node) = parser_at(tree, &path)
            && let Some(actions) = node.get("actions").and_then(Value::as_array)
        {
            for a in actions {
                for s in a
                    .get("subcommands")
                    .and_then(Value::as_array)
                    .map_or_default(Vec::as_slice)
                {
                    let (Some(name), Some(suffix)) = (
                        s.get("name").and_then(Value::as_str),
                        s.pointer("/parser/prog_suffix").and_then(Value::as_str),
                    ) else {
                        continue;
                    };
                    let mut sub = path.clone();
                    sub.push(name.to_owned());
                    stack.push((sub, format!("{p}{suffix}")));
                }
            }
        }
        out.push((path, p));
    }
    out
}

/// The parser that produced an output: the prog of the `error:` line, or the longest prog the usage line of a
/// help text starts with.
fn reporter(o: &Outcome, paths: &[(Vec<String>, String)]) -> Option<Vec<String>> {
    if o.exit == 2 {
        let last = o.stderr.lines().last()?;
        let prog = last.split(": error: ").next()?;
        return paths
            .iter()
            .find(|(_, p)| p == prog)
            .map(|(path, _)| path.clone());
    }
    let first = o.stdout.lines().next()?.strip_prefix("usage: ")?;
    paths
        .iter()
        .filter(|(_, p)| {
            first
                .strip_prefix(p.as_str())
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(' '))
        })
        .max_by_key(|(_, p)| p.len())
        .map(|(path, _)| path.clone())
}

fn golden_doc(brand: &str, source: &str, cases: &[Case], outcomes: &[Outcome]) -> Value {
    json!({
        "generator": "cargo xtask cli-goldens",
        "brand": brand,
        "source": source,
        "cases": cases.iter().zip(outcomes).map(|(c, o)| outcome_json(c, o)).collect::<Vec<_>>(),
    })
}

fn check_unique(cases: &[Case]) -> anyhow::Result<()> {
    let mut seen = BTreeSet::new();
    for c in cases {
        ensure!(seen.insert(c.id()), "duplicate case {}", c.id());
    }
    Ok(())
}

fn generate(oracle: &Oracle) -> anyhow::Result<(Value, Value)> {
    std::fs::create_dir_all(oracle.scratch.join("home"))?;
    let pc_json = oracle.scratch.join("pre-commit-spec.json");
    let snp_json = oracle.scratch.join("snp-spec.json");
    let pc_tree_text = snp_argparse::export_json(snp_cli::PRE_COMMIT_SPEC, "pre-commit");
    let snp_tree_text = snp_argparse::export_json(snp_cli::SNP_SPEC, "snp");
    std::fs::write(&pc_json, &pc_tree_text)?;
    std::fs::write(&snp_json, &snp_tree_text)?;
    let pc_tree: Value = serde_json::from_str(&pc_tree_text)?;
    let snp_tree: Value = serde_json::from_str(&snp_tree_text)?;

    // Compat brand.
    let cases = compat_cases();
    check_unique(&cases)?;
    workspace::say(&format!("cli-goldens: {} compat cases", cases.len()));
    let compat = par_map(&cases, &|case| -> anyhow::Result<Outcome> {
        let shim = oracle.script(&[OsStr::new("pre-commit")], case)?;
        let golden = if shim.ns.is_some() {
            shim
        } else {
            let real = oracle.exe("pre-commit", case)?;
            ensure!(
                real == shim,
                "{}: the pre-commit shim differs from bin/pre-commit:\n{real:#?}\n{shim:#?}",
                case.id()
            );
            real
        };
        let built = oracle.script(&[OsStr::new("json"), pc_json.as_os_str()], case)?;
        ensure!(
            built == golden,
            "{}: argparse built from export_json(PRE_COMMIT_SPEC) differs from pre-commit:\n{golden:#?}\n{built:#?}",
            case.id()
        );
        Ok(golden)
    })?;
    let compat_doc = golden_doc(
        "compat",
        "pre-commit 4.7.0 (target/oracle/pc470-py313)",
        &cases,
        &compat,
    );

    // snp brand.
    let cases = snp_cases();
    check_unique(&cases)?;
    workspace::say(&format!("cli-goldens: {} snp cases", cases.len()));
    let paths = all_paths(&snp_tree, "snp");
    let patched_checks = AtomicUsize::new(0);
    let snp = par_map(&cases, &|case| -> anyhow::Result<Outcome> {
        let built = oracle.script(&[OsStr::new("json"), snp_json.as_os_str()], case)?;
        if built.ns.is_none()
            && let Some(path) = reporter(&built, &paths)
            && let (Some(a), Some(b)) = (parser_at(&pc_tree, &path), parser_at(&snp_tree, &path))
            && a == b
            && !path.is_empty()
        {
            let patched = oracle.exe("pre-commit-snp-prog", case)?;
            ensure!(
                patched == built,
                "{}: `{}` is identical in both specs but differs from pre-commit-snp-prog:\n{patched:#?}\n{built:#?}",
                case.id(),
                path.join(" ")
            );
            patched_checks.fetch_add(1, Ordering::Relaxed);
        }
        Ok(built)
    })?;
    workspace::say(&format!(
        "cli-goldens: {} snp cases of parsers identical to pre-commit's matched pre-commit-snp-prog",
        patched_checks.load(Ordering::Relaxed)
    ));
    let snp_doc = golden_doc(
        "snp",
        "CPython 3.13 argparse built from export_json(SNP_SPEC, \"snp\"); parsers identical to pre-commit's checked against pre-commit-snp-prog",
        &cases,
        &snp,
    );
    Ok((compat_doc, snp_doc))
}

// ----- the zoo specs of snp-argparse (generic argparse features) ---------------------------------------------

mod zoo {
    include!("../../../crates/snp-argparse/tests/zoo/spec.rs");
}

const ZOO_GOLDEN: &str = "crates/snp-argparse/tests/goldens/zoo.json";

/// The prog each zoo spec is exported with.
fn zoo_prog(name: &str) -> &'static str {
    match name {
        "main" => "zoo",
        "sub" => "zoo-sub",
        _ => "zoo-files",
    }
}

/// One zoo row: (spec, env, argv).
type ZooRow = (
    &'static str,
    &'static [(&'static str, &'static str)],
    &'static [&'static str],
);

/// (spec, env, argv) rows of the zoo.
const ZOO_ROWS: &[ZooRow] = &[
    ("main", &[], &[]),
    ("main", &[], &["src"]),
    ("main", &[], &["-r", "x", "src"]),
    ("main", &[], &["-r", "x", "src", "f1", "f2"]),
    (
        "main",
        &[],
        &[
            "-r", "x", "src", "-n", "5", "--name", "ABC", "--label", "y", "--level", "high", "-q",
            "-Q", "-t", "a", "-t", "b", "--opt", "--many", "1", "2", "--some", "a", "b", "--fast",
        ],
    ),
    ("main", &[], &["-r", "x", "src", "--opt", "val"]),
    ("main", &[], &["-r", "x", "src", "--opt=val"]),
    ("main", &[], &["-r", "x", "--opt", "src"]),
    ("main", &[], &["-r", "x", "src", "--name", "abc"]),
    ("main", &[], &["-r", "x", "src", "--name="]),
    ("main", &[], &["-r", "x", "src", "--label="]),
    ("main", &[], &["-r", "x", "src", "--level", "bogus"]),
    ("main", &[], &["-r", "x", "src", "--some", "a", "c"]),
    ("main", &[], &["-r", "x", "src", "--some"]),
    ("main", &[], &["-r", "x", "src", "--many"]),
    ("main", &[], &["-r", "x", "src", "--many=1"]),
    ("main", &[], &["-r", "x", "src", "--fast", "--slow"]),
    ("main", &[], &["-r", "x", "src", "--slow"]),
    ("main", &[], &["-r", "x", "src", "--secret", "--fast"]),
    ("main", &[], &["-r", "x", "src", "--fast", "--secret"]),
    ("main", &[], &["-r", "x", "src", "-n", "x"]),
    ("main", &[], &["-r", "x", "src", "--rem", "a", "-b", "--c"]),
    ("main", &[], &["-r", "x", "--rem"]),
    ("main", &[], &["-r", "x", "src", "--rem"]),
    ("main", &[], &["-r", "x", "src", "-qQt", "tag"]),
    ("main", &[], &["-r", "x", "src", "-Qq"]),
    ("main", &[], &["-r", "x", "src", "-n=5"]),
    ("main", &[], &["-r", "x", "src", "-n5"]),
    ("main", &[], &["-r", "x", "src", "-qn5"]),
    ("main", &[], &["-r", "x", "src", "-qn"]),
    ("main", &[], &["-r", "x", "src", "--cou", "5"]),
    ("main", &[], &["-r", "x", "src", "--no-v"]),
    ("main", &[], &["-r", "x", "src", "--", "-n", "5"]),
    ("main", &[], &["-r", "x", "src", "-t"]),
    ("main", &[], &["-r", "x", "src", "--hidden", "h"]),
    ("main", &[], &["-r", "x", "src", "--le=low"]),
    ("main", &[], &["-r", "-x", "src"]),
    ("main", &[], &["-r", "x", "-1", "src"]),
    ("main", &[], &["-rx", "src", "-t1", "-t", "-2"]),
    ("main", &[], &["--req=", "src"]),
    ("main", &[], &["-r", "x", "src", "--n", "1"]),
    ("main", &[], &["-r", "x", "src", "--s", "a"]),
    ("main", &[], &["-r", "x", "src", "--tag"]),
    ("main", &[], &["-r", "x", "src", "-q=1"]),
    (
        "main",
        &[],
        &["-r", "x", "src", "--count", "1_000", "--count", "+7"],
    ),
    ("main", &[], &["-r", "x", "src", "f", "--level", "low", "g"]),
    (
        "main",
        &[("COLUMNS", "40")],
        &["-r", "x", "src", "--level", "bogus"],
    ),
    ("sub", &[], &[]),
    ("sub", &[], &["bogus"]),
    ("sub", &[], &["lis"]),
    ("sub", &[], &["--version"]),
    ("sub", &[("COLUMNS", "20")], &["--version"]),
    ("sub", &[], &["--ver"]),
    ("sub", &[], &["listed"]),
    ("sub", &[], &["listed", "a", "b", "-1"]),
    ("sub", &[], &["listed", "a", "-1", "b"]),
    ("sub", &[], &["listed", "a", "--num", "5"]),
    ("sub", &[], &["listed", "a", "--nu", "5"]),
    ("sub", &[], &["listed", "-5"]),
    ("sub", &[], &["listed", "a", "--num", "-5"]),
    ("sub", &[], &["listed", "a", "--num", "x"]),
    ("sub", &[], &["strict", "--verbose"]),
    ("sub", &[], &["strict", "--verb"]),
    ("sub", &[], &["strict", "-h"]),
    ("sub", &[], &["strict", "x"]),
    ("sub", &[], &["strict", "x", "y"]),
    ("sub", &[], &["strict", "--version-info", "--verbose"]),
    ("sub", &[], &["hidden-cmd", "v"]),
    ("sub", &[], &["hidden-cmd"]),
    ("sub", &[], &["quiet"]),
    ("sub", &[], &["quiet", "extra"]),
    ("sub", &[], &["nested"]),
    ("sub", &[], &["nested", "leaf", "--deep"]),
    ("sub", &[], &["nested", "leaf", "--de"]),
    ("sub", &[], &["nested", "bogus"]),
    ("sub", &[], &["--top", "listed", "a"]),
    ("sub", &[], &["listed", "a", "--top"]),
    ("sub", &[], &["--", "listed", "a"]),
    ("sub", &[], &["--top=1", "listed", "a"]),
    ("files", &[], &[]),
    ("files", &[], &["a", "b"]),
    ("files", &[], &["--maxkb", "10", "a"]),
    ("files", &[], &["--maxkb=x", "a"]),
    ("files", &[], &["--fix", "crlf", "a"]),
    ("files", &[], &["--fix", "bogus", "a"]),
    ("files", &[], &["-f", "no", "a"]),
    ("files", &[], &["-fno", "a"]),
    ("files", &[("ZOO_MODE", "bogus")], &["a"]),
    ("files", &[("ZOO_MODE", "crlf")], &["a"]),
    ("files", &[], &["--enforce-all", "a"]),
    ("files", &[], &["--enf", "a"]),
    ("files", &[], &["a", "--", "-b"]),
    ("files", &[], &["--", "--maxkb"]),
];

/// Help and version of every zoo parser at a few widths.
const ZOO_HELP: &[(&str, &[&str])] = &[
    ("main", &["-h"]),
    ("sub", &["-h"]),
    ("sub", &["listed", "-h"]),
    ("sub", &["nested", "-h"]),
    ("sub", &["nested", "leaf", "-h"]),
    ("sub", &["hidden-cmd", "-h"]),
    ("sub", &["quiet", "--help"]),
    ("files", &["-h"]),
];

fn zoo_cases() -> Vec<(&'static str, Case)> {
    let mut out = Vec::new();
    for &(spec, argv) in ZOO_HELP {
        for w in ["10", "40", "60", "80", "120"] {
            out.push((spec, Case::new(argv, &[("COLUMNS", w)])));
        }
    }
    for &(spec, env, argv) in ZOO_ROWS {
        out.push((spec, Case::new(argv, env)));
    }
    let mut seen = BTreeSet::new();
    out.into_iter()
        .filter(|(spec, c)| seen.insert(format!("{spec}: {}", c.id())))
        .collect()
}

fn generate_zoo(oracle: &Oracle) -> anyhow::Result<Value> {
    std::fs::create_dir_all(oracle.scratch.join("home"))?;
    let mut files: Vec<(&str, PathBuf)> = Vec::new();
    for (name, spec) in &zoo::ZOO {
        let path = oracle.scratch.join(format!("zoo-{name}.json"));
        std::fs::write(&path, snp_argparse::export_json(spec, zoo_prog(name)))?;
        files.push((name, path));
    }
    let cases = zoo_cases();
    workspace::say(&format!("cli-goldens: {} zoo cases", cases.len()));
    let only: Vec<Case> = cases.iter().map(|(_, c)| c.clone()).collect();
    let specs: Vec<&str> = cases.iter().map(|(s, _)| *s).collect();
    let outcomes = par_map(&only, &|case| -> anyhow::Result<Outcome> {
        let i = only
            .iter()
            .position(|c| std::ptr::eq(c, case))
            .context("case index")?;
        let spec = specs.get(i).copied().context("case spec")?;
        let path = files
            .iter()
            .find(|(n, _)| *n == spec)
            .map(|(_, p)| p.clone())
            .context("zoo spec file")?;
        oracle.script(&[OsStr::new("json-plain"), path.as_os_str()], case)
    })?;
    let docs: Vec<Value> = cases
        .iter()
        .zip(&outcomes)
        .map(|((spec, case), o)| {
            let mut v = outcome_json(case, o);
            if let Value::Object(m) = &mut v {
                m.insert("spec".to_owned(), Value::String((*spec).to_owned()));
                m.insert("prog".to_owned(), Value::String(zoo_prog(spec).to_owned()));
                m.insert(
                    "id".to_owned(),
                    Value::String(format!("{}: {}", zoo_prog(spec), case.id())),
                );
            }
            v
        })
        .collect();
    Ok(json!({
        "generator": "cargo xtask cli-goldens",
        "source": "CPython 3.13 argparse (target/oracle/pc470-py313) built from export_json of crates/snp-argparse/tests/zoo/spec.rs",
        "cases": docs,
    }))
}

// ----- textwrap vectors (12 §A.16: "textwrap port ≡ Python on random ASCII and Unicode text") -------------------

const TEXTWRAP_VECTORS: &str = "crates/snp-argparse/tests/vectors/textwrap.json";

/// A small deterministic generator (xorshift64*), so the vectors are reproducible without a dependency.
#[derive(Debug)]
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        let n = u64::try_from(n.max(1)).unwrap_or(u64::MAX);
        usize::try_from(self.next().checked_rem(n).unwrap_or(0)).unwrap_or(0)
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        let i = self.below(items.len());
        items.get(i).copied().unwrap_or_default()
    }
}

/// Random texts made of words (ASCII and non-ASCII letters, Unicode digits, `_`), hyphens and em-dashes,
/// punctuation and every whitespace `textwrap` treats specially, with widths and indents.
fn textwrap_cases() -> Vec<Value> {
    const LETTERS: &[&str] = &[
        "a", "b", "e", "k", "o", "z", "A", "Q", "é", "ß", "Ж", "中", "_", "1", "7", "\u{663}",
    ];
    const PIECES: &[&str] = &[
        " ", " ", " ", "  ", "\t", "\n", "\r\n", "\u{a0}", "\u{b}", "\u{c}", "-", "-", "--", "---",
        ".", ",", "!", "?", "'", "\"", "&", ";", ":", "(", ")", "`",
    ];
    const INDENTS: &[&str] = &["", "", "  ", "> ", "\u{b7} "];
    let mut rng = Rng(0x5eed_2026_1008_0012);
    let mut out = Vec::new();
    for _ in 0..3000 {
        let mut text = String::new();
        for _ in 0..rng.below(40) {
            if rng.below(3) == 0 {
                text.push_str(rng.pick(PIECES));
            } else {
                for _ in 0..rng.below(12).saturating_add(1) {
                    text.push_str(rng.pick(LETTERS));
                }
            }
        }
        let initial = rng.pick(INDENTS);
        let subsequent = rng.pick(INDENTS);
        // CPython loops forever when an indent leaves no room on a line and the text starts with whitespace
        // (`_handle_long_word` keeps appending an empty chunk), so every line gets at least one column.
        let room = initial.chars().count().max(subsequent.chars().count());
        let width = rng.below(30).saturating_add(1).saturating_add(room);
        out.push(json!({
            "text": text,
            "width": width,
            "initial": initial,
            "subsequent": subsequent,
        }));
    }
    out
}

fn generate_textwrap(oracle: &Oracle) -> anyhow::Result<Value> {
    std::fs::create_dir_all(oracle.scratch.join("home"))?;
    let cases = textwrap_cases();
    let input = oracle.scratch.join("textwrap-cases.json");
    std::fs::write(&input, serde_json::to_string(&cases)?)?;
    let case = Case::new(&[], &[]);
    let mut cmd = oracle.command(&oracle.bin.join("python"), &case);
    cmd.arg("-I")
        .arg(&oracle.script)
        .arg("textwrap")
        .arg(&input)
        .arg("--");
    let out = cmd.output().context("running the textwrap oracle")?;
    ensure!(
        out.status.success(),
        "textwrap oracle failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let lines: Vec<Value> =
        serde_json::from_slice(&out.stdout).context("textwrap oracle output")?;
    ensure!(
        lines.len() == cases.len(),
        "textwrap oracle: wrong number of results"
    );
    let docs: Vec<Value> = cases
        .into_iter()
        .zip(lines)
        .map(|(mut c, l)| {
            if let Value::Object(m) = &mut c {
                m.insert("lines".to_owned(), l);
            }
            c
        })
        .collect();
    workspace::say(&format!("cli-goldens: {} textwrap vectors", docs.len()));
    Ok(json!({
        "generator": "cargo xtask cli-goldens",
        "source": "CPython 3.13 textwrap.wrap (target/oracle/pc470-py313)",
        "cases": docs,
    }))
}

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    let check = match args {
        [] => false,
        [a] if a == "--check" => true,
        _ => bail!("usage: cargo xtask cli-goldens [--check]"),
    };
    let root = workspace::root()?;
    let bin = root.join("target/oracle/pc470-py313/bin");
    ensure!(
        bin.join("python").exists(),
        "missing oracle {} (run `cargo xtask oracle install`)",
        bin.display()
    );
    let oracle = Oracle {
        bin,
        script: root.join("xtask/oracle/argparse_from_json.py"),
        scratch: root.join("target/cli-goldens"),
    };
    let zoo = generate_zoo(&oracle)?;
    let textwrap = generate_textwrap(&oracle)?;
    let (compat, snp) = generate(&oracle)?;
    let dir = root.join(GOLDEN_DIR);
    std::fs::create_dir_all(&dir)?;
    if let Some(parent) = root.join(ZOO_GOLDEN).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut stale = Vec::new();
    for (path, doc) in [
        (dir.join("compat.json"), compat),
        (dir.join("snp.json"), snp),
        (root.join(ZOO_GOLDEN), zoo),
        (root.join(TEXTWRAP_VECTORS), textwrap),
    ] {
        let text = format!("{}\n", serde_json::to_string_pretty(&doc)?);
        if check {
            let old = std::fs::read_to_string(&path).unwrap_or_default();
            if old != text {
                stale.push(workspace::rel(&root, &path));
            }
        } else {
            std::fs::write(&path, text)?;
            workspace::say(&format!(
                "cli-goldens: wrote {}",
                workspace::rel(&root, &path)
            ));
        }
    }
    ensure!(
        stale.is_empty(),
        "stale goldens (run `cargo xtask cli-goldens`): {}",
        stale.join(", ")
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_ids_are_unique() {
        check_unique(&compat_cases()).unwrap();
        check_unique(&snp_cases()).unwrap();
    }

    #[test]
    fn quoting() {
        assert_eq!(Case::new(&["run", ""], &[]).id(), "run \"\"");
        assert_eq!(
            Case::new(&["-h"], &[("COLUMNS", "40")]).id(),
            "COLUMNS=40 -h"
        );
        // Arbitrary bytes are POSIX argv only (a Windows argument is UTF-16).
        if cfg!(unix) {
            assert_eq!(quote(&bytes_arg("a\\xff")), "$'a\\xff'");
        }
    }
}
