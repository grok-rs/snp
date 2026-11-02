//! CPython 3.13 `argparse`-compatible parser and `HelpFormatter` driven by static [`ParserSpec`] tables
//! (ARCHITECTURE §1.4.3, §9.1, 12 §A.13, ADR-010).
//!
//! A port of `Lib/argparse.py` and `Lib/textwrap.py` of CPython 3.13.14 for the subset pre-commit 4.7.0
//! (`pre_commit/main.py`), `pygrep.py`, `hazmat.py` and the pre-commit-hooks console scripts use: optionals and
//! positionals, `nargs` (`None`, `?`, `*`, `+`, `REMAINDER`, `PARSER`), the `store`/`store_true`/`store_false`/
//! `store_const`/`append`/`help`/`version` actions, `choices`, mutually exclusive groups, sub-parsers (listed and
//! hidden), `allow_abbrev` prefix matching with ambiguity raised at consumption, `--x=y`, short-option clusters,
//! the negative-number heuristic, `--` handling, unknown arguments deferred to the top-level parser (ADR-008) and
//! string defaults (including `os.environ.get(…)`) converted at the end of the owning parser only when the
//! option was absent (ADR-007). Error texts use Python `repr` (`snp_pycompat::repr`); help and usage wrap at
//! [`columns`] − 2 exactly as `HelpFormatter` does.
//!
//! The crate never prints and never panics: [`parse`] returns the help/version text for stdout or the usage error
//! for stderr, and the caller writes it (exit 0 / exit 2).
//!
//! Owner: WP-12.

mod engine;
mod format;
mod json;
mod matcher;
mod namespace;
mod parse;
mod pystr;
mod spec;
mod stable;
pub mod textwrap;
mod view;

pub use namespace::Namespace;
pub use parse::{columns, export_json, format_help, format_usage, parse, prefix_stable};
pub use spec::{
    ActionSpec, Conv, DefaultValue, Kind, Nargs, Outcome, ParseCfg, ParserSpec, SubCommand, Value,
};
