//! Static parser descriptions (`ArgumentParser(…)` / `add_argument(…)` / `add_subparsers(…)` calls as data,
//! `argparse.py:780-1300`) and parse results.
//!
//! A [`ParserSpec`] lists the actions in registration order; `add_help` adds the implicit first action
//! `-h, --help` exactly as `ArgumentParser.__init__` does, so specs never list it. The order of `actions` is the
//! order argparse sees: it drives usage, the help sections, ambiguity listings, `required` lists and default
//! conversion.
//!
//! Owner: WP-12.

use std::ffi::{OsStr, OsString};

use crate::Namespace;

/// `nargs=` of `add_argument` (`argparse.py` `_get_nargs_pattern`). Flags (`store_true`, `store_false`, `help`,
/// `version`, plain `store_const`) always behave as `nargs=0` and sub-parsers as `PARSER`, whatever the spec says.
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
    /// `store_const` with `const=` (`nargs=0`). With [`Nargs::Optional`] it is instead `action='store',
    /// nargs='?', const=…`: the option given without a value stores the constant (converted and checked against
    /// `choices` like any value).
    StoreConst(&'static str),
    /// `append`.
    Append,
    /// `help`.
    Help,
    /// `version`.
    Version,
    /// `add_subparsers(dest=…, required=…)` with its commands in registration order. The action's `dest` receives
    /// the command name; its `required` is the `required=` of `add_subparsers`; `choices` are all command names.
    SubParsers(&'static [SubCommand]),
}

/// One `subparsers.add_parser(name, help=…)` call.
#[derive(Copy, Clone, Debug)]
pub struct SubCommand {
    /// Command name.
    pub name: &'static str,
    /// `help=`; `None` hides the command from the commands block (`hook-impl`: `add_parser('hook-impl')` without
    /// `help=` creates no `_ChoicesPseudoAction`) while it stays in `{a,b,…}` and in `invalid choice` lists.
    pub help: Option<&'static str>,
    /// Named in the `{a,b,…}` of usage and help. An unlisted command is modelled as
    /// `add_subparsers(metavar='{<listed commands>}')`: it is still accepted, named in `invalid choice` lists and,
    /// when it has a `help=`, shown in the commands block (argparse lists every command created with `help=`); the
    /// metavar also becomes the action's name in error messages (`_get_action_name`). Hide a command completely
    /// with `listed: false` and `help: None`.
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
    /// `color.use_color` (`color.py:84-100`): valid iff `auto`, `always` or `never` (case-sensitive); the value
    /// stays the validated string; failures read `invalid use_color value: %r` (12 §A.9).
    UseColor,
    /// `clientlib.transform_stage`: `commit` → `pre-commit`, `merge-commit` → `pre-merge-commit`, `push` →
    /// `pre-push`, anything else unchanged (never fails; `choices` then check the transformed value, 01 §1.3).
    Stage,
    /// A hook type name: no `type=` in pre-commit (`-t/--hook-type` relies on `choices=HOOK_TYPES`); identity.
    HookType,
    /// Python `int()` (12 §A.8.3), saturated to `i64`; failures read `invalid int value: %r`.
    Int,
    /// Any other converter: the function and the `type=` name used in error texts. `Err("")` is a
    /// `ValueError` (`invalid <name> value: %r`); `Err(message)` is an `ArgumentTypeError` whose message is used
    /// verbatim (`argument X: <message>`).
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
    /// A string default that is never passed through `type=` (argparse: a non-`str` default object rendered by
    /// `str()`). For a [`Kind::Version`] action it is the `version=` template (`%(prog)s` is expanded).
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
    /// `required=` of an option or of a sub-parsers action. Ignored for other positionals: argparse derives it from
    /// `nargs` (required unless `?`, `*` or `REMAINDER`).
    pub required: bool,
    /// `help=`.
    pub help: Option<&'static str>,
    /// `metavar=`.
    pub metavar: Option<&'static str>,
    /// Index of the `add_mutually_exclusive_group()` this action belongs to (groups are never `required`; members
    /// should be adjacent in `actions`, as argparse only brackets contiguous groups in usage).
    pub mutex_group: Option<u8>,
    /// `help=argparse.SUPPRESS`.
    pub hidden: bool,
}

/// One `ArgumentParser(…)` with its actions in registration order.
#[derive(Debug)]
pub struct ParserSpec {
    /// Appended to the parent's prog (`" run"`, as `add_parser` does with `'%s %s' % (prog_prefix, name)`); `""` for
    /// the top level.
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
    /// `prog` of the top-level parser (brand dependent: `pre-commit` | `snp`).
    pub prog: &'a str,
    /// The captured environment: [`DefaultValue::Env`] defaults are read from it.
    pub env: &'a snp_core::env::Env,
    /// [`crate::columns`] result; help and usage wrap at `columns - 2`.
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
    /// `None`: absent with a `None` default.
    Unset,
    /// `store_true` / `store_false` / boolean defaults.
    Bool(bool),
    /// One string (argv bytes kept, non-UTF-8 round-trips on Unix).
    Str(OsString),
    /// `nargs` `*`/`+`/REMAINDER or `append`.
    List(Vec<OsString>),
    /// `type=int` (saturated to `i64`).
    Int(i64),
}
