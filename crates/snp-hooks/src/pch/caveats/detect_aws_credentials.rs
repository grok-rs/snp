//! `detect-aws-credentials` (`detect_aws_credentials.py`; 09 §4.D) over snp-pyparsers' `configparser` subset,
//! NATIVE-WITH-CAVEATS behind its parity gate. One epoch v3.0.0…v6.0.0.
//!
//! The credential files are `--credentials-file` (`action='append'` onto its default list, so the defaults are
//! always read) plus `AWS_CONFIG_FILE`, `AWS_CREDENTIAL_FILE`, `AWS_SHARED_CREDENTIALS_FILE` and `BOTO_CONFIG`
//! from the hook's environment; each is `expanduser`ed, skipped when missing or unreadable, and parsed as
//! `ConfigParser().read()` does (a file without a section header gives no keys; any other parse or interpolation
//! error, or an undecodable file, is a crash). The secrets of `AWS_SECRET_ACCESS_KEY`, `AWS_SECURITY_TOKEN` and
//! `AWS_SESSION_TOKEN` are added. No key: exit 2 with the hint (exit 0 with `--allow-missing-credentials`).
//! Python iterates `set`s, so the order of several findings in one file (and which of two broken credential files
//! crashes first) varies per process; snp reads the credential files sorted by path and reports findings per file
//! sorted by key. Against the pinned oracle run (`PYTHONHASHSEED=0`) that is a message-order difference: the
//! caveat corpus gate counts it as a message difference (the generated parity-gate row says how many are
//! order-only), which makes this hook's gate `Caveat` — still eligible for `auto`, since no acceptance (exit code)
//! differs. The order in which broken credential files are tried is not covered by the single-file corpus.
//! Credential files are decoded as UTF-8, not the locale encoding (09 §10 Q6: on Windows CPython 3.12/3.13 without
//! UTF-8 mode Python would decode with the ANSI code page; not yet in the COMPATIBILITY register, an
//! integration-lead row).
//!
//! Owner: WP-27.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::sync::Arc;

use snp_argparse::{ActionSpec, Nargs, ParserSpec};
use snp_core::failure::PyExc;
use snp_hooks_data::Epoch;
use snp_pyparsers::CheckError;

use crate::framework::{self, FileStep, PyOut, append, positional, store_true, with_help};
use crate::pch::p0::common::{self, epoch_in, filenames, read_all};
use crate::pch::p1::util::{bytes_of, text_read_lines};
use crate::{HookCx, NativeHook, Outcome};

/// The console script.
const PROG: &str = "detect-aws-credentials";

/// `--credentials-file`'s default.
const DEFAULT_FILES: [&str; 4] = [
    "~/.aws/config",
    "~/.aws/credentials",
    "/etc/boto.cfg",
    "~/.boto",
];

/// Environment variables naming credential files.
const FILE_VARS: [&str; 4] = [
    "AWS_CONFIG_FILE",
    "AWS_CREDENTIAL_FILE",
    "AWS_SHARED_CREDENTIALS_FILE",
    "BOTO_CONFIG",
];

/// Environment variables holding secrets.
const SECRET_VARS: [&str; 3] = [
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SECURITY_TOKEN",
    "AWS_SESSION_TOKEN",
];

static ACTIONS: [ActionSpec; 3] = [
    with_help(
        positional("filenames", Nargs::OneOrMore),
        Some("Filenames to run"),
    ),
    with_help(
        append(&["--credentials-file"], "credentials_file"),
        Some(
            "Location of additional AWS credential file from which to get secret keys. Can be passed multiple \
             times.",
        ),
    ),
    with_help(
        store_true(
            &["--allow-missing-credentials"],
            "allow_missing_credentials",
        ),
        Some("Allow hook to pass when no credentials are detected."),
    ),
];

/// `detect_aws_credentials.main`'s parser.
pub(crate) static SPEC: ParserSpec = ParserSpec {
    prog_suffix: "",
    description: None,
    epilog: None,
    actions: &ACTIONS,
    allow_abbrev: true,
    add_help: true,
};

/// The native.
#[derive(Debug)]
pub(crate) struct DetectAwsCredentials;

/// The pre-commit-hooks native.
pub(crate) static UPSTREAM: DetectAwsCredentials = DetectAwsCredentials;

/// `get_aws_secrets_from_file(credentials_file)`.
fn secrets_from_file(
    cx: &HookCx<'_>,
    file: &OsStr,
    keys: &mut BTreeSet<String>,
) -> Result<(), PyExc> {
    let path = snp_os::home::expanduser(file, cx.env);
    let full = framework::at(cx, &path);
    if std::fs::metadata(&full).is_err() {
        return Ok(());
    }
    // `ConfigParser.read` skips a file it cannot open or read (`OSError`).
    let Ok(bytes) = std::fs::read(&full) else {
        return Ok(());
    };
    let text = text_read_lines(&bytes)?;
    match snp_pyparsers::ini::aws_credentials_ex(&text, &path.to_string_lossy()) {
        Ok(pairs) => {
            keys.extend(pairs.into_iter().map(|(_, v)| v));
            Ok(())
        }
        Err(CheckError::Crash(e)) => Err(e),
        Err(other) => Err(PyExc::new("configparser.Error", other.into_message())),
    }
}

/// `key.decode()[:4].ljust(28, '*')`.
pub(crate) fn hidden(key: &str) -> String {
    let mut h: String = key.chars().take(4).collect();
    let n = h.chars().count();
    h.extend(std::iter::repeat_n('*', 28_usize.saturating_sub(n)));
    h
}

impl NativeHook for DetectAwsCredentials {
    fn id(&self) -> &'static str {
        "detect-aws-credentials"
    }

    fn prog(&self) -> &'static str {
        PROG
    }

    fn preflight(&self, epoch: Epoch, _args: &[Arc<str>]) -> Result<(), String> {
        epoch_in(epoch, 1)
    }

    fn run(&self, cx: &HookCx<'_>, argv: &[&OsStr]) -> Outcome {
        let ns = match common::parse(cx, &SPEC, PROG, argv) {
            Ok(ns) => ns,
            Err(o) => return o,
        };
        let mut out = PyOut::default();
        let mut cred_files: BTreeSet<std::ffi::OsString> =
            DEFAULT_FILES.iter().map(Into::into).collect();
        cred_files.extend(ns.list("credentials_file").iter().cloned());
        for v in FILE_VARS {
            if let Some(p) = cx.env.get(v) {
                cred_files.insert(p.to_os_string());
            }
        }
        let mut keys: BTreeSet<String> = BTreeSet::new();
        for f in &cred_files {
            if let Err(e) = secrets_from_file(cx, f, &mut keys) {
                return framework::crashed(out, &e);
            }
        }
        for v in SECRET_VARS {
            if let Some(k) = cx.env.get(v)
                && !k.is_empty()
            {
                keys.insert(k.to_string_lossy().into_owned());
            }
        }
        if keys.is_empty() {
            if ns.flag("allow_missing_credentials") {
                return Outcome {
                    output: out.finish(),
                    reads_only: true,
                    ..Outcome::default()
                };
            }
            out.print(
                "No AWS keys were found in the configured credential files and environment variables.\nPlease \
                 ensure you have the correct setting for --credentials-file",
            );
            return Outcome {
                code: 2,
                output: out.finish(),
                reads_only: true,
                ..Outcome::default()
            };
        }
        let files = filenames(&ns);
        let mut bad: Vec<(Vec<u8>, String)> = Vec::new();
        let mut outcome = {
            let mut step = |name: &OsStr, _out: &mut PyOut| {
                let body = match read_all(cx, name) {
                    Ok(b) => b,
                    Err(e) => return FileStep::Crash(e),
                };
                for k in &keys {
                    if memchr::memmem::find(&body, k.as_bytes()).is_some() {
                        bad.push((bytes_of(name).to_vec(), hidden(k)));
                    }
                }
                FileStep::Unchanged
            };
            framework::for_each_file(cx, &files, &mut out, &mut step)
        };
        if !outcome.crashed {
            for (f, h) in &bad {
                let mut line = b"AWS secret found in ".to_vec();
                line.extend_from_slice(f);
                line.extend_from_slice(b": ");
                line.extend_from_slice(h.as_bytes());
                out.print_bytes_text(&line);
            }
            outcome.code = i32::from(!bad.is_empty());
        }
        outcome.reads_only = true;
        framework::finish(outcome, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_keys() {
        assert_eq!(hidden("SECRETKEY"), "SECR************************");
        assert_eq!(hidden("ab"), "ab**************************");
        assert_eq!(hidden("ébcde").chars().count(), 28);
    }
}
