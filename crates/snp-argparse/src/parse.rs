//! `ArgumentParser.parse_args` and `HelpFormatter` of CPython 3.13.14 (12 §A.8, §A.13, §A.16): the public entry
//! points over the engine ([`crate::engine`]) and the formatter ([`crate::format`]).
//!
//! Owner: WP-12.

use std::ffi::OsString;

use snp_core::env::Env;

use crate::engine::{self, Stop};
use crate::format::Formatter;
use crate::view::{MAX_DEPTH, View};
use crate::{Outcome, ParseCfg, ParserSpec};

/// argparse 3.13.14 `_parse_known_args` + `parse_args` (12 §A.8.2): unknown args deferred to the top level, ambiguity
/// at consumption, env-string default converted at the end of the owning subparser only when the option was absent.
///
/// `argv` excludes the program name. Help (`-h` at any level) and `-V` stop the parse where they are consumed and
/// return the text for stdout; every usage error returns the reporting parser's usage plus
/// `{prog}: error: {message}\n` for stderr (exit 2). Never panics, never touches disk; linear in `argv`.
pub fn parse(spec: &'static ParserSpec, argv: &[OsString], cfg: &ParseCfg<'_>) -> Outcome {
    match engine::parse_args(spec, argv, cfg) {
        Ok(ns) => Outcome::Parsed(ns),
        Err(Stop::Help(text)) => Outcome::Help(text),
        Err(Stop::Version(text)) => Outcome::Version(text),
        Err(Stop::Usage(stderr)) => Outcome::Usage { stderr },
    }
}

/// The parser at `path` (sub-command names from the top level) and its prog, or `None` for an unknown name.
fn resolve(spec: &'static ParserSpec, path: &[&str], prog: &str) -> Option<(View, String)> {
    let mut view = View::new(spec);
    let mut prog = prog.to_owned();
    for (depth, name) in path.iter().enumerate() {
        if depth >= MAX_DEPTH {
            return None;
        }
        let sub = view.subcommand(name.as_bytes())?;
        prog.push_str(sub.spec.prog_suffix);
        view = View::new(sub.spec);
    }
    Some((view, prog))
}

/// `parser.format_help()` of the parser at `path` (sub-command names from the top level, e.g. `["hazmat", "cd"]`);
/// `prog` is `cfg.prog` plus each sub-parser's `prog_suffix`. An unknown name yields an empty string.
pub fn format_help(spec: &'static ParserSpec, path: &[&str], cfg: &ParseCfg<'_>) -> String {
    resolve(spec, path, cfg.prog)
        .map_or_default(|(view, prog)| Formatter::new(&view, &prog, cfg).help())
}

/// `parser.format_usage()` of the parser at `path` (`usage: …\n`, wrapped like the usage block of `error()`). An
/// unknown name yields an empty string.
pub fn format_usage(spec: &'static ParserSpec, path: &[&str], cfg: &ParseCfg<'_>) -> String {
    resolve(spec, path, cfg.prog)
        .map_or_default(|(view, prog)| Formatter::new(&view, &prog, cfg).usage())
}

/// `COLUMNS` (Python `int()`), else the stdout terminal width, else 80 (the formatter subtracts 2).
///
/// `shutil.get_terminal_size().columns` as `HelpFormatter.__init__` reads it: `int(os.environ['COLUMNS'])` when that
/// parses (surrounding whitespace, `_` separators and Unicode digits allowed) and is positive; otherwise the width
/// of the terminal on **stdout** (`stdout_tty_cols`, `None` when stdout is not a terminal), with a reported 0 and
/// the no-terminal case both falling back to 80.
pub fn columns(env: &Env, stdout_tty_cols: Option<usize>) -> usize {
    if let Some(n) = env
        .get("COLUMNS")
        .and_then(std::ffi::OsStr::to_str)
        .and_then(crate::pystr::py_int)
        .filter(|&n| n > 0)
    {
        return usize::try_from(n).unwrap_or(usize::MAX);
    }
    match stdout_tty_cols {
        Some(c) if c > 0 => c,
        _ => 80,
    }
}

/// Pure; every unique-prefix → option mapping of `base` must be unchanged in `extended` (12 §A.17 item 6).
///
/// For every parser of `base` (walking sub-commands by name), the parser of the same path in `extended` must exist
/// and, for every long-option abbreviation (`--x…`) that `base` resolves to exactly one option string, resolve it to
/// the same option string with the same meaning (`dest`, action kind, `nargs`); every exact option string of `base`
/// must keep its meaning, no sub-command may disappear, and `extended` must not add negative-number-like option
/// strings (they would turn `-1` arguments into options). The error names the first broken abbreviation.
pub fn prefix_stable(
    base: &'static ParserSpec,
    extended: &'static ParserSpec,
) -> Result<(), String> {
    crate::stable::check(base, extended)
}

/// JSON description of a spec (prog included) from which `xtask cli-goldens` builds a real CPython argparse parser to
/// cross-check the snp-brand goldens (ADR-058).
///
/// Schema (`xtask/oracle/argparse_from_json.py` reads it): `{"prog": …, "parser": P}` where `P` is
/// `{"prog_suffix", "description", "epilog", "allow_abbrev", "add_help", "actions": [A…]}` and every action `A` is
/// `{"opts", "dest", "action", "nargs", "const", "type", "choices", "default", "required", "help", "hidden",
/// "metavar", "mutex_group", "subcommands"}`; `default` is `{"kind": "none"|"str"|"env"|"false"|"true"|"int"|"list"|
/// "const", …}`, `subcommands` is `[{"name", "help", "listed", "parser": P}]` for sub-parser actions.
pub fn export_json(spec: &'static ParserSpec, prog: &str) -> String {
    crate::json::export(spec, prog)
}
