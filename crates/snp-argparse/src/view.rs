//! A parser as argparse sees it after construction: the implicit `-h/--help` action, the option-string table in
//! insertion order, and the derived per-action facts (`nargs` of flags, `required` of positionals, names used in
//! error messages, `str()` of defaults).
//!
//! Owner: WP-12.

use std::borrow::Cow;

use snp_core::env::Env;

use crate::pystr;
use crate::{ActionSpec, Conv, DefaultValue, Kind, Nargs, ParserSpec, SubCommand};

/// `add_help=True`: `parser.add_argument('-h', '--help', action='help', default=SUPPRESS,
/// help='show this help message and exit')`, the first action of the parser.
pub(crate) static HELP_ACTION: ActionSpec = ActionSpec {
    opts: &["-h", "--help"],
    dest: "",
    nargs: Nargs::Zero,
    kind: Kind::Help,
    conv: Conv::Identity,
    choices: None,
    default: DefaultValue::None,
    required: false,
    help: Some("show this help message and exit"),
    metavar: None,
    mutex_group: None,
    hidden: false,
};

/// Maximum nesting of sub-parsers followed by the parser, the formatter and the spec walkers (cycle guard; real
/// trees are at most four levels deep).
pub(crate) const MAX_DEPTH: usize = 64;

/// One parser: its spec, viewed with the implicit help action first (`ArgumentParser.__init__` adds it before any
/// other). Allocation-free: parsing the hook-impl argv of every `git commit` is a hot path (12 §A.17 item 8).
#[derive(Debug, Clone, Copy)]
pub(crate) struct View {
    pub(crate) spec: &'static ParserSpec,
}

impl View {
    pub(crate) const fn new(spec: &'static ParserSpec) -> Self {
        Self { spec }
    }

    /// Number of actions, the implicit help included.
    pub(crate) const fn len(self) -> usize {
        self.spec
            .actions
            .len()
            .saturating_add(if self.spec.add_help { 1 } else { 0 })
    }

    pub(crate) const fn is_empty(self) -> bool {
        self.len() == 0
    }

    pub(crate) fn action(self, i: usize) -> Option<&'static ActionSpec> {
        if self.spec.add_help {
            match i.checked_sub(1) {
                None => Some(&HELP_ACTION),
                Some(j) => self.spec.actions.get(j),
            }
        } else {
            self.spec.actions.get(i)
        }
    }

    /// The actions in registration order.
    pub(crate) fn iter(self) -> impl Iterator<Item = &'static ActionSpec> + use<> {
        let help = self.spec.add_help.then_some(&HELP_ACTION);
        help.into_iter().chain(self.spec.actions.iter())
    }

    /// `_option_string_actions` in insertion order: (option string, action index). Option strings are unique in
    /// a valid spec (argparse's default `conflict_handler='error'` rejects duplicates when the parser is built).
    pub(crate) fn options(self) -> impl Iterator<Item = (&'static str, usize)> + use<> {
        self.iter()
            .enumerate()
            .flat_map(|(i, a)| a.opts.iter().map(move |&o| (o, i)))
    }

    /// The action registered for exactly the option string `s`.
    pub(crate) fn exact(self, s: &[u8]) -> Option<(usize, &'static str)> {
        self.options()
            .find(|(o, _)| o.as_bytes() == s)
            .map(|(o, i)| (i, o))
    }

    /// `_has_negative_number_optionals`.
    pub(crate) fn negative_number_options(self) -> bool {
        self.options()
            .any(|(o, _)| pystr::is_negative_number(o.as_bytes()))
    }

    /// Indices of the optionals then the positionals (usage order, `optionals + positionals`).
    pub(crate) fn usage_order(self) -> (Vec<usize>, Vec<usize>) {
        let opt = (0..self.len())
            .filter(|&i| self.action(i).is_some_and(|a| !a.opts.is_empty()))
            .collect();
        let pos = (0..self.len())
            .filter(|&i| self.action(i).is_some_and(|a| a.opts.is_empty()))
            .collect();
        (opt, pos)
    }

    /// The sub-parser named `name` of this parser's sub-parsers action.
    pub(crate) fn subcommand(self, name: &[u8]) -> Option<&'static SubCommand> {
        self.iter().find_map(|a| match a.kind {
            Kind::SubParsers(subs) => subs.iter().find(|s| s.name.as_bytes() == name),
            _ => None,
        })
    }
}

/// The `nargs` argparse gives the action: 0 for flags, `PARSER` for sub-parsers, `'?'` only for a `store_const`
/// spec that asks for it (see [`Kind::StoreConst`]).
pub(crate) const fn nargs(a: &ActionSpec) -> Nargs {
    match a.kind {
        Kind::StoreTrue | Kind::StoreFalse | Kind::Help | Kind::Version => Nargs::Zero,
        Kind::StoreConst(_) => match a.nargs {
            Nargs::Optional => Nargs::Optional,
            _ => Nargs::Zero,
        },
        Kind::SubParsers(_) => Nargs::Parser,
        Kind::Store | Kind::Append => a.nargs,
    }
}

/// `action.required`: for a positional (other than sub-parsers) argparse derives it from `nargs`
/// (`_get_positional_kwargs`); options and sub-parsers use the spec.
pub(crate) const fn required(a: &ActionSpec) -> bool {
    if a.opts.is_empty() && !matches!(a.kind, Kind::SubParsers(_)) {
        matches!(nargs(a), Nargs::One | Nargs::OneOrMore)
    } else {
        a.required
    }
}

/// The `dest` stored in the namespace; `None` for `help`/`version` (`dest=SUPPRESS`) and an empty dest.
pub(crate) const fn dest(a: &ActionSpec) -> Option<&'static str> {
    match a.kind {
        Kind::Help | Kind::Version => None,
        _ if a.dest.is_empty() => None,
        _ => Some(a.dest),
    }
}

/// The sub-commands of a sub-parsers action.
pub(crate) const fn subcommands(a: &ActionSpec) -> &'static [SubCommand] {
    match a.kind {
        Kind::SubParsers(subs) => subs,
        _ => &[],
    }
}

/// The explicit `metavar`, or for sub-parsers with unlisted commands the `{…}` of the listed ones (the
/// `metavar=` such a parser is built with).
pub(crate) fn metavar(a: &ActionSpec) -> Option<Cow<'static, str>> {
    if let Some(m) = a.metavar {
        return Some(Cow::Borrowed(m));
    }
    let subs = subcommands(a);
    if subs.iter().any(|s| !s.listed) {
        let names: Vec<&str> = subs.iter().filter(|s| s.listed).map(|s| s.name).collect();
        return Some(Cow::Owned(format!("{{{}}}", names.join(","))));
    }
    None
}

/// `action.choices` as strings: the spec's choices, or every sub-command name for sub-parsers.
pub(crate) fn choices(a: &ActionSpec) -> Option<Vec<&'static str>> {
    match a.kind {
        Kind::SubParsers(subs) => Some(subs.iter().map(|s| s.name).collect()),
        _ => a.choices.map(<[&str]>::to_vec),
    }
}

/// `_get_action_name` (`argparse.py:728-748`).
pub(crate) fn action_name(a: &ActionSpec) -> Option<Cow<'static, str>> {
    if !a.opts.is_empty() {
        Some(Cow::Owned(a.opts.join("/")))
    } else if let Some(m) = metavar(a) {
        Some(m)
    } else if let Some(d) = dest(a) {
        Some(Cow::Borrowed(d))
    } else {
        choices(a).map(|c| Cow::Owned(format!("{{{}}}", c.join(","))))
    }
}

/// `type.__name__` (or `None`) of the converter.
pub(crate) const fn conv_name(c: Conv) -> &'static str {
    match c {
        Conv::Identity | Conv::HookType => "None",
        Conv::UseColor => "use_color",
        Conv::Stage => "transform_stage",
        Conv::Int => "int",
        Conv::Custom(_, name) => name,
    }
}

/// The raw string default (`str` defaults are converted at the end of the parse when the option is absent).
pub(crate) fn raw_default(a: &ActionSpec, env: &Env) -> Option<std::ffi::OsString> {
    match a.default {
        DefaultValue::Str(s) => Some(s.into()),
        DefaultValue::Env { var, fallback } => Some(
            env.get(var)
                .map_or_else(|| fallback.into(), std::ffi::OsStr::to_os_string),
        ),
        _ => None,
    }
}

/// `str(action.default)` for `%(default)s`; `None` when the default is `SUPPRESS` (help, version).
pub(crate) fn default_str(a: &ActionSpec, env: &Env) -> Option<String> {
    if matches!(a.kind, Kind::Help | Kind::Version) {
        return None;
    }
    Some(match a.default {
        DefaultValue::None => "None".to_owned(),
        DefaultValue::Str(s) | DefaultValue::Const(s) => s.to_owned(),
        DefaultValue::Env { .. } => {
            raw_default(a, env).map_or_default(|v| pystr::display(&v).into_owned())
        }
        DefaultValue::False => "False".to_owned(),
        DefaultValue::True => "True".to_owned(),
        DefaultValue::Int(n) => n.to_string(),
        DefaultValue::EmptyList => "[]".to_owned(),
    })
}

/// `str(action.const)`.
pub(crate) fn const_str(a: &ActionSpec) -> String {
    match a.kind {
        Kind::StoreConst(c) => c.to_owned(),
        Kind::StoreTrue => "True".to_owned(),
        Kind::StoreFalse => "False".to_owned(),
        _ => "None".to_owned(),
    }
}

/// `str(action.nargs)`.
pub(crate) const fn nargs_str(a: &ActionSpec) -> &'static str {
    match nargs(a) {
        Nargs::Zero => "0",
        Nargs::One => "None",
        Nargs::Optional => "?",
        Nargs::ZeroOrMore => "*",
        Nargs::OneOrMore => "+",
        Nargs::Remainder => "...",
        Nargs::Parser => "A...",
    }
}

/// The value of one `%(name)s` / `%(name)r` key of `HelpFormatter._expand_help`.
pub(crate) fn help_param(
    a: &ActionSpec,
    prog: &str,
    env: &Env,
    key: &str,
    repr: bool,
) -> Option<String> {
    let quoted = |s: &str| {
        if repr {
            snp_pycompat::repr::repr_str(s)
        } else {
            s.to_owned()
        }
    };
    match key {
        "prog" => Some(quoted(prog)),
        "default" => {
            let s = default_str(a, env)?;
            let is_str = matches!(
                a.default,
                DefaultValue::Str(_) | DefaultValue::Env { .. } | DefaultValue::Const(_)
            );
            Some(if is_str { quoted(&s) } else { s })
        }
        "dest" => dest(a).map(quoted),
        "metavar" => Some(metavar(a).map_or_else(|| "None".to_owned(), |m| quoted(&m))),
        "choices" => Some(choices(a).map_or_else(|| "None".to_owned(), |c| c.join(", "))),
        "const" => Some(match a.kind {
            Kind::StoreConst(c) => quoted(c),
            _ => const_str(a),
        }),
        "type" => Some(conv_name(a.conv).to_owned()),
        "required" => Some(if required(a) { "True" } else { "False" }.to_owned()),
        "nargs" => Some(match nargs(a) {
            Nargs::Zero => "0".to_owned(),
            Nargs::One => "None".to_owned(),
            _ => quoted(nargs_str(a)),
        }),
        "option_strings" => Some(snp_pycompat::repr::repr_str_list(a.opts)),
        "help" => a.help.map(quoted),
        "deprecated" => Some("False".to_owned()),
        _ => None,
    }
}
