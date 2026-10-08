//! Static parser descriptions (`ArgumentParser.add_argument` calls as data) and parse results.
//!
//! Owner: WP-12.

use std::ffi::{OsStr, OsString};

use crate::Namespace;

/// `nargs=` of `add_argument` (`argparse.py` `_get_nargs_pattern`).
#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub enum Nargs {
    /// No value (flags, `store_true`, …).
    Zero,
    /// `None`: exactly one value.
    One,
    /// `'?'`.
    Optional,
    /// `'*'`.
    ZeroOrMore,
    /// `'+'`.
    OneOrMore,
    /// `argparse.REMAINDER`.
    Remainder,
    /// `argparse.PARSER` (sub-parsers).
    Parser,
}

/// `action=` of `add_argument`.
#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub enum Kind {
    /// `store`.
    Store,
    /// `store_true`.
    StoreTrue,
    /// `store_false`.
    StoreFalse,
    /// `store_const` with `const=`.
    StoreConst(&'static str),
    /// `append`.
    Append,
    /// `help`.
    Help,
    /// `version`.
    Version,
    /// `add_subparsers()` with its commands in registration order.
    SubParsers(&'static [SubCommand]),
}

/// One `subparsers.add_parser(name, help=…)` call.
#[derive(Copy, Clone, Debug)]
pub struct SubCommand {
    /// Command name.
    pub name: &'static str,
    /// `help=`; `None` hides the command from the listing (`hook-impl`).
    pub help: Option<&'static str>,
    /// Listed in `{a,b,…}` / the commands block.
    pub listed: bool,
    /// The sub-parser.
    pub spec: &'static ParserSpec,
}

/// `type=` of `add_argument`.
#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub enum Conv {
    /// No conversion.
    Identity,
    /// `color.use_color`-style choice check of `--color` (12 §A.9).
    UseColor,
    /// A stage name (legacy names transformed, 01 §1.3).
    Stage,
    /// A hook type name.
    HookType,
    /// Python `int()`.
    Int,
    /// Any other converter: the function and the `type=` name used in error texts.
    Custom(fn(&OsStr) -> Result<OsString, String>, &'static str),
}

/// `default=` of `add_argument`.
#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub enum DefaultValue {
    /// `None`.
    None,
    /// A string default (converted with `type=` like argparse does).
    Str(&'static str),
    /// `os.environ.get(var, fallback)`, converted at the end of the owning sub-parser only when the option was absent
    /// (ADR-007).
    Env {
        /// The environment variable.
        var: &'static str,
        /// The value when `var` is unset.
        fallback: &'static str,
    },
    /// `False`.
    False,
    /// `True`.
    True,
    /// An integer.
    Int(i64),
    /// `[]`.
    EmptyList,
    /// `const=` value of a `store_const` action.
    Const(&'static str),
}

/// One `add_argument` call.
#[derive(Debug)]
pub struct ActionSpec {
    /// Option strings (`["-c", "--config"]`) or `[]` for a positional.
    pub opts: &'static [&'static str],
    /// `dest=`.
    pub dest: &'static str,
    /// `nargs=`.
    pub nargs: Nargs,
    /// `action=`.
    pub kind: Kind,
    /// `type=`.
    pub conv: Conv,
    /// `choices=`.
    pub choices: Option<&'static [&'static str]>,
    /// `default=`.
    pub default: DefaultValue,
    /// `required=`.
    pub required: bool,
    /// `help=`.
    pub help: Option<&'static str>,
    /// `metavar=`.
    pub metavar: Option<&'static str>,
    /// Index of the `add_mutually_exclusive_group()` this action belongs to.
    pub mutex_group: Option<u8>,
    /// `help=argparse.SUPPRESS`.
    pub hidden: bool,
}

/// One `ArgumentParser(…)` with its actions in registration order.
#[derive(Debug)]
pub struct ParserSpec {
    /// Appended to the parent's prog (`" run"`); `""` for the top level.
    pub prog_suffix: &'static str,
    /// `description=`.
    pub description: Option<&'static str>,
    /// `epilog=`.
    pub epilog: Option<&'static str>,
    /// The actions.
    pub actions: &'static [ActionSpec],
    /// `allow_abbrev=`.
    pub allow_abbrev: bool,
    /// `add_help=`.
    pub add_help: bool,
}

/// Per-invocation inputs of [`crate::parse`].
#[derive(Debug)]
pub struct ParseCfg<'a> {
    /// `prog` (brand dependent).
    pub prog: &'a str,
    /// The captured environment (env-backed defaults).
    pub env: &'a snp_core::env::Env,
    /// [`crate::columns`] result.
    pub columns: usize,
}

/// Result of [`crate::parse`].
#[derive(Debug)]
#[non_exhaustive]
pub enum Outcome {
    /// Parsed successfully.
    Parsed(Namespace),
    /// `-h`: help text for stdout, exit 0.
    Help(String),
    /// `--version`: version text for stdout, exit 0.
    Version(String),
    /// Usage error: text for stderr, exit 2.
    Usage {
        /// `usage: …\n{prog}: error: …\n`.
        stderr: String,
    },
}

/// One parsed `dest` value.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Value {
    /// Absent with a `None` default.
    Unset,
    /// `store_true` / `store_false` / boolean defaults.
    Bool(bool),
    /// One string (argv bytes kept, non-UTF-8 round-trips on Unix).
    Str(OsString),
    /// `nargs` `*`/`+`/REMAINDER or `append`.
    List(Vec<OsString>),
    /// `type=int`.
    Int(i64),
}
