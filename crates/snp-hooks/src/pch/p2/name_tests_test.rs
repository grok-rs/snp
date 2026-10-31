//! `name-tests-test` (`tests_should_end_in_test.py`; 09 §4.E).
//!
//! Epochs: 1 = v3.0.0…v4.2.0 (`--django` only; `re.match`, a prefix match), 2 = v4.3.0+ (`--pytest` /
//! `--pytest-test-first` / `--django`/`--unittest` in a mutually exclusive group storing the pattern;
//! `re.fullmatch`). The pattern is compiled and matched by snp-pyre against `os.path.basename(f)`; `__init__.py`
//! and `conftest.py` always pass.
//!
//! Owner: WP-27.

use std::ffi::OsStr;
use std::sync::Arc;

use snp_argparse::{ActionSpec, DefaultValue, Kind, Nargs, ParserSpec};
use snp_hooks_data::Epoch;
use snp_pyre::{Options, PyRegex};

use super::kernels;
use crate::framework::{
    self, FileStep, PyOut, files_positional, store, store_true, with_default, with_help,
};
use crate::pch::p0::common::{self, epoch_in, filenames};
use crate::pch::p1::util::bytes_of;
use crate::{HookCx, NativeHook, Outcome};

/// The console script.
const PROG: &str = "name-tests-test";

/// `add_argument(*opts, dest='pattern', action='store_const', const=…)` in the mutually exclusive group.
const fn pattern_opt(
    opts: &'static [&'static str],
    konst: &'static str,
    help: &'static str,
) -> ActionSpec {
    with_help(
        ActionSpec {
            kind: Kind::StoreConst(konst),
            nargs: Nargs::Zero,
            mutex_group: Some(0),
            ..store(opts, "pattern")
        },
        Some(help),
    )
}

static E1_ACTIONS: [ActionSpec; 2] = [
    files_positional(None),
    with_help(
        store_true(&["--django"], "django"),
        Some("Use Django-style test naming pattern (test*.py)"),
    ),
];
static E2_ACTIONS: [ActionSpec; 4] = [
    files_positional(None),
    with_default(
        pattern_opt(
            &["--pytest"],
            r".*_test\.py",
            r"(the default) ensure tests match .*_test\.py",
        ),
        DefaultValue::Const(r".*_test\.py"),
    ),
    pattern_opt(
        &["--pytest-test-first"],
        r"test_.*\.py",
        r"ensure tests match test_.*\.py",
    ),
    pattern_opt(
        &["--django", "--unittest"],
        r"test.*\.py",
        r"ensure tests match test.*\.py",
    ),
];

/// The parser of v3.0.0…v4.2.0.
pub(crate) static SPEC_E1: ParserSpec = ParserSpec {
    prog_suffix: "",
    description: None,
    epilog: None,
    actions: &E1_ACTIONS,
    allow_abbrev: true,
    add_help: true,
};
/// v4.3.0+.
pub(crate) static SPEC_E2: ParserSpec = ParserSpec {
    actions: &E2_ACTIONS,
    ..SPEC_E1
};

/// The parser at `epoch`.
pub(crate) fn spec(epoch: u16) -> &'static ParserSpec {
    if epoch <= 1 { &SPEC_E1 } else { &SPEC_E2 }
}

/// The native.
#[derive(Debug)]
pub(crate) struct NameTestsTest;

/// The pre-commit-hooks native.
pub(crate) static UPSTREAM: NameTestsTest = NameTestsTest;

/// `os.path.basename(f)` (`ntpath` on Windows: both separators and the drive).
fn basename(f: &[u8], windows: bool) -> &[u8] {
    if windows {
        snp_pycompat::ntpath::basename(f)
    } else {
        snp_pycompat::posixpath::basename(f)
    }
}

impl NativeHook for NameTestsTest {
    fn id(&self) -> &'static str {
        "name-tests-test"
    }

    fn prog(&self) -> &'static str {
        PROG
    }

    fn preflight(&self, epoch: Epoch, _args: &[Arc<str>]) -> Result<(), String> {
        epoch_in(epoch, 2)
    }

    fn run(&self, cx: &HookCx<'_>, argv: &[&OsStr]) -> Outcome {
        let epoch = cx.epoch.0;
        let ns = match common::parse(cx, spec(epoch), PROG, argv) {
            Ok(ns) => ns,
            Err(o) => return o,
        };
        let pattern = if epoch <= 1 {
            kernels::tests_pattern(ns.flag("django")).to_owned()
        } else {
            ns.value("pattern").map_or_else(
                || kernels::tests_pattern(false).to_owned(),
                |p| p.to_string_lossy().into_owned(),
            )
        };
        let out = PyOut::default();
        let re = match PyRegex::compile(&pattern, Options::CONFIG) {
            Ok(re) => re,
            Err(e) => return framework::crashed(out, &framework::re_exc(&e)),
        };
        let mut out = out;
        let files = filenames(&ns);
        let mut step = |name: &OsStr, out: &mut PyOut| {
            let base = basename(bytes_of(name), cx.windows);
            if base == b"__init__.py" || base == b"conftest.py" {
                return FileStep::Unchanged;
            }
            let hay = String::from_utf8_lossy(base);
            let ok = if epoch <= 1 {
                re.match_prefix(&hay)
            } else {
                re.fullmatch(&hay)
            };
            // The three patterns are linear-time: a backtracking failure cannot happen; count it as no match.
            if ok.unwrap_or(false) {
                return FileStep::Unchanged;
            }
            let mut line = bytes_of(name).to_vec();
            line.extend_from_slice(format!(" does not match pattern \"{pattern}\"").as_bytes());
            out.print_bytes_text(&line);
            FileStep::Failed
        };
        let mut outcome = framework::for_each_file(cx, &files, &mut out, &mut step);
        outcome.reads_only = true;
        framework::finish(outcome, out)
    }
}
