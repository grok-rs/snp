//! `check-vcs-permalinks` (`check_vcs_permalinks.py`; 09 §4.D).
//!
//! Epochs: 1 = v3.0.0…v3.3.0 (one fixed `github.com/…/blob/master/…` pattern, no option), 2 = v3.4.0
//! (`--additional-github-domain`, `blob/master` only), 3 = v4.0.0+ (any ref that is not a 4–64 digit hex id). The
//! option appends to its default `['github.com']`; every domain is inserted into the pattern unescaped (`.` matches
//! any byte) and compiled as a CPython bytes pattern by snp-pyre, once, before the first file (an invalid domain
//! is the `re.PatternError` crash; one beyond snp's engines makes the caller delegate). Each binary line is
//! searched with every pattern in domain order, so a duplicated domain reports the line twice. The location is
//! written as text and flushed, the line itself through `sys.stdout.buffer`.
//!
//! Owner: WP-27.

use std::ffi::OsStr;
use std::sync::Arc;

use snp_argparse::{ActionSpec, ParserSpec};
use snp_hooks_data::Epoch;
use snp_pyre::{Haystack, Mode, Options, PyReError, PyRegex};

use super::kernels;
use crate::framework::{self, FileStep, PyOut, append, files_positional};
use crate::pch::p0::common::{self, epoch_in, filenames, parse_static, read_all};
use crate::pch::p1::kernels::binary_lines;
use crate::pch::p1::util::bytes_of;
use crate::{HookCx, NativeHook, Outcome};

/// The console script.
const PROG: &str = "check-vcs-permalinks";

static E1_ACTIONS: [ActionSpec; 1] = [files_positional(None)];
static E2_ACTIONS: [ActionSpec; 2] = [
    files_positional(None),
    append(&["--additional-github-domain"], "additional_github_domains"),
];

/// The parser of v3.0.0…v3.3.0.
pub(crate) static SPEC_E1: ParserSpec = ParserSpec {
    prog_suffix: "",
    description: None,
    epilog: None,
    actions: &E1_ACTIONS,
    allow_abbrev: true,
    add_help: true,
};
/// v3.4.0+.
pub(crate) static SPEC_E2: ParserSpec = ParserSpec {
    actions: &E2_ACTIONS,
    ..SPEC_E1
};

/// The parser at `epoch`.
pub(crate) fn spec(epoch: u16) -> &'static ParserSpec {
    if epoch <= 1 { &SPEC_E1 } else { &SPEC_E2 }
}

/// Bytes patterns over lines.
const OPTS: Options = Options {
    mode: Mode::Bytes,
    haystack: Haystack::Line,
    ignore_case: false,
    multiline_dotall: false,
};

/// The native.
#[derive(Debug)]
pub(crate) struct CheckVcsPermalinks;

/// The pre-commit-hooks native.
pub(crate) static UPSTREAM: CheckVcsPermalinks = CheckVcsPermalinks;

/// `args.additional_github_domains`: the default `['github.com']` with every given domain appended.
fn domains(ns: &snp_argparse::Namespace) -> Vec<String> {
    let mut d = vec!["github.com".to_owned()];
    d.extend(
        ns.list("additional_github_domains")
            .iter()
            .map(|x| x.to_string_lossy().into_owned()),
    );
    d
}

impl NativeHook for CheckVcsPermalinks {
    fn id(&self) -> &'static str {
        "check-vcs-permalinks"
    }

    fn prog(&self) -> &'static str {
        PROG
    }

    /// Delegates when a domain makes a pattern that is valid Python but beyond snp's regex engines.
    fn preflight(&self, epoch: Epoch, args: &[Arc<str>]) -> Result<(), String> {
        epoch_in(epoch, 3)?;
        let Some(ns) = parse_static(spec(epoch.0), PROG, args) else {
            return Ok(());
        };
        for d in domains(&ns) {
            if let Err(PyReError::Unsupported(why)) =
                PyRegex::compile(&kernels::permalink_pattern(&d, epoch.0), OPTS)
            {
                return Err(format!("--additional-github-domain {d:?}: {why}"));
            }
        }
        Ok(())
    }

    fn run(&self, cx: &HookCx<'_>, argv: &[&OsStr]) -> Outcome {
        let epoch = cx.epoch.0;
        let ns = match common::parse(cx, spec(epoch), PROG, argv) {
            Ok(ns) => ns,
            Err(o) => return o,
        };
        let mut out = PyOut::default();
        let doms = if epoch <= 1 {
            vec!["github.com".to_owned()]
        } else {
            domains(&ns)
        };
        let mut patterns = Vec::new();
        for d in &doms {
            match PyRegex::compile(&kernels::permalink_pattern(d, epoch), OPTS) {
                Ok(re) => {
                    if framework::python_warnings(cx.env) {
                        out.eprint_bytes(&framework::regex_warnings(&re));
                    }
                    patterns.push(re);
                }
                Err(e) => return framework::crashed(out, &framework::re_exc(&e)),
            }
        }
        let files = filenames(&ns);
        let mut stopped = false;
        let mut step = |name: &OsStr, out: &mut PyOut| {
            if stopped {
                return FileStep::Unchanged;
            }
            let content = match read_all(cx, name) {
                Ok(c) => c,
                Err(e) => return FileStep::Crash(e),
            };
            let mut found = false;
            for (i, line) in binary_lines(&content).into_iter().enumerate() {
                for re in &patterns {
                    match re.search_bytes(line) {
                        Ok(None) => {}
                        Ok(Some(_)) => {
                            let mut loc = bytes_of(name).to_vec();
                            loc.extend_from_slice(format!(":{}:", i.saturating_add(1)).as_bytes());
                            out.write_text(&loc);
                            out.flush();
                            out.write_bytes(line);
                            found = true;
                        }
                        Err(_) => {
                            // D-SEC-10: CPython would hang here; fail closed.
                            out.eprint_bytes(&framework::backtrack_line(
                                re.source(),
                                OsStr::new(&*String::from_utf8_lossy(line)),
                            ));
                            stopped = true;
                            return FileStep::Failed;
                        }
                    }
                }
            }
            if found {
                FileStep::Failed
            } else {
                FileStep::Unchanged
            }
        };
        let mut outcome = framework::for_each_file(cx, &files, &mut out, &mut step);
        outcome.reads_only = true;
        if stopped {
            outcome.crashed = true;
        } else if outcome.code != 0 && !outcome.crashed {
            out.print("");
            out.print("Non-permanent github link detected.");
            out.print("On any page on github press [y] to load a permalink.");
        }
        framework::finish(outcome, out)
    }
}
