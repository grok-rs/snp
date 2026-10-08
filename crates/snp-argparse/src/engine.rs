//! `ArgumentParser.parse_args` / `parse_known_args` / `_parse_known_args` of CPython 3.13.14
//! (`argparse.py:1925-2280`, 12 §A.8.2), one parser at a time, recursing through sub-parser actions.
//!
//! The observable rules this reproduces (12 §A.1, ADR-007, ADR-008):
//! * every parser classifies its own slice of argv (`O`/`A`/`-`) against its own option table, including long
//!   option abbreviation (`allow_abbrev`), `--opt=value`, single-dash clusters and the negative-number and
//!   "contains a space" heuristics;
//! * options and positionals are consumed alternately; the first error raised in consumption order wins
//!   (ambiguity, `expected one argument`, conversions, choices, mutual exclusion); `-h`/`-V` exit at the point
//!   where they are consumed;
//! * unknown options and surplus positionals are only collected; the top-level parser reports them after the
//!   whole parse succeeded (`{prog}: error: unrecognized arguments: …` with the top-level usage);
//! * at the end of each parser, defaults that are strings are converted (`PRE_COMMIT_COLOR` through
//!   `use_color`) only for actions that were never taken, then required arguments are checked.
//!
//! The hook-impl argv of every `git commit` goes through here, so the parse borrows argv slices, builds progs only
//! for messages, and allocates little beyond the namespace values (12 §A.17 item 8).
//!
//! Owner: WP-12.

use std::borrow::Cow;
use std::ffi::{OsStr, OsString};

use snp_pycompat::repr::repr_os;

use crate::format::Formatter;
use crate::matcher::{self, match_option};
use crate::namespace::Namespace;
use crate::pystr::{self, bytes};
use crate::view::{self, MAX_DEPTH, View};
use crate::{ActionSpec, Conv, DefaultValue, Kind, Nargs, ParseCfg, ParserSpec, Value};

/// Why a parse stopped early: `print_help` + exit 0, the version + exit 0, or `error()` (usage + message, exit 2).
#[derive(Debug)]
pub(crate) enum Stop {
    Help(String),
    Version(String),
    Usage(String),
}

/// `parser.prog` without building the string until a message needs it: the top-level prog and the
/// `prog_suffix` of every sub-parser on the way down.
#[derive(Copy, Clone, Debug)]
struct Prog<'p> {
    parent: Option<&'p Self>,
    part: &'p str,
}

impl Prog<'_> {
    fn render(&self) -> String {
        let mut parts = vec![self.part];
        let mut cur = self.parent;
        while let Some(p) = cur {
            parts.push(p.part);
            cur = p.parent;
        }
        parts.iter().rev().copied().collect()
    }
}

/// `sep` of an option tuple: `None`, `''` (short option with an attached value) or `'='`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Sep {
    None,
    Empty,
    Eq,
}

/// One `(action, option_string, sep, explicit_arg)` tuple of `_parse_optional`; the explicit argument is the tail
/// of the token from byte `explicit`.
#[derive(Copy, Clone, Debug)]
struct OptTuple {
    action: Option<usize>,
    option: &'static str,
    sep: Sep,
    explicit: Option<usize>,
}

/// The option tuples of one token: `None` for a positional (`A`/`-`).
#[derive(Clone, Debug)]
enum Tuples {
    None,
    One(OptTuple),
    Many(Vec<OptTuple>),
}

impl Tuples {
    const fn is_option(&self) -> bool {
        !matches!(self, Self::None)
    }
}

/// A converted value (`type(arg_string)`).
#[derive(Clone, Debug)]
enum Converted {
    Str(OsString),
    Int(i64),
}

impl Converted {
    fn into_os(self) -> OsString {
        match self {
            Self::Str(s) => s,
            Self::Int(n) => n.to_string().into(),
        }
    }

    fn into_value(self) -> Value {
        match self {
            Self::Str(s) => Value::Str(s),
            Self::Int(n) => Value::Int(n),
        }
    }
}

/// `_get_values` results.
#[derive(Debug)]
enum Values {
    None,
    One(Converted),
    Many(Vec<OsString>),
}

/// Bits of the per-action state.
const SEEN: u8 = 1;
const SEEN_NON_DEFAULT: u8 = 2;

/// One parser's `_parse_known_args` state.
struct ParserRun<'a, 'p> {
    cfg: &'a ParseCfg<'a>,
    view: View,
    prog: &'p Prog<'p>,
    args: &'a [OsString],
    depth: usize,
    ns: Namespace,
    state: Vec<u8>,
    extras: Vec<OsString>,
    unrecognized: Vec<OsString>,
    /// Positionals already consumed (`positionals[:] = positionals[len(arg_counts):]`).
    positionals_done: usize,
    pattern: Vec<u8>,
    tuples: Vec<Tuples>,
}

/// An `ArgumentError` raised inside this parser (reported with its usage) or a stop from a sub-parser.
#[derive(Debug)]
enum Failure {
    Error(String),
    Stop(Stop),
}

impl From<Stop> for Failure {
    fn from(s: Stop) -> Self {
        Self::Stop(s)
    }
}

/// `ArgumentError(action, message)`: `argument {name}: {message}` (or the message alone without a name).
fn arg_error(a: Option<&ActionSpec>, message: &str) -> Failure {
    match a.and_then(view::action_name) {
        Some(name) => Failure::Error(format!("argument {name}: {message}")),
        None => Failure::Error(message.to_owned()),
    }
}

/// `parser.error(message)`: the parser's usage, then `{prog}: error: {message}`.
fn usage_error(view: View, prog: &str, cfg: &ParseCfg<'_>, message: &str) -> Stop {
    let usage = Formatter::new(&view, prog, cfg).usage();
    Stop::Usage(format!("{usage}{prog}: error: {message}\n"))
}

/// `parser.parse_args(argv)` for the top-level `spec` with `prog = cfg.prog`.
pub(crate) fn parse_args(
    spec: &'static ParserSpec,
    argv: &[OsString],
    cfg: &ParseCfg<'_>,
) -> Result<Namespace, Stop> {
    let prog = Prog {
        parent: None,
        part: cfg.prog,
    };
    let (ns, extras) = parse_known(spec, &prog, argv, cfg, 0)?;
    if extras.is_empty() {
        return Ok(ns);
    }
    let joined = extras
        .iter()
        .map(|e| pystr::display(e).into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    Err(usage_error(
        View::new(spec),
        cfg.prog,
        cfg,
        &format!("unrecognized arguments: {joined}"),
    ))
}

/// `parse_known_args`: the namespace and the unrecognized strings (this parser's extras, then those of
/// sub-parsers).
fn parse_known(
    spec: &'static ParserSpec,
    prog: &Prog<'_>,
    args: &[OsString],
    cfg: &ParseCfg<'_>,
    depth: usize,
) -> Result<(Namespace, Vec<OsString>), Stop> {
    let view = View::new(spec);
    let mut run = ParserRun {
        cfg,
        view,
        prog,
        args,
        depth,
        ns: Namespace::default(),
        state: vec![0; view.len()],
        extras: Vec::new(),
        unrecognized: Vec::new(),
        positionals_done: 0,
        pattern: Vec::new(),
        tuples: Vec::new(),
    };
    run.init_defaults();
    match run.parse() {
        Ok(()) => {
            let mut extras = std::mem::take(&mut run.extras);
            extras.append(&mut run.unrecognized);
            Ok((run.ns, extras))
        }
        Err(Failure::Stop(stop)) => Err(stop),
        Err(Failure::Error(msg)) => Err(usage_error(view, &prog.render(), cfg, &msg)),
    }
}

/// `_get_value`: `type(arg_string)`, mapping failures to `invalid <type> value: %r` (or the converter's own
/// message, `ArgumentTypeError`).
fn convert(a: &ActionSpec, raw: &OsStr) -> Result<Converted, Failure> {
    let invalid =
        |name: &str| arg_error(Some(a), &format!("invalid {name} value: {}", repr_os(raw)));
    match a.conv {
        Conv::Identity | Conv::HookType => Ok(Converted::Str(raw.to_os_string())),
        Conv::UseColor => match bytes(raw) {
            b"auto" | b"always" | b"never" => Ok(Converted::Str(raw.to_os_string())),
            _ => Err(invalid("use_color")),
        },
        Conv::Stage => Ok(Converted::Str(match bytes(raw) {
            b"commit" => "pre-commit".into(),
            b"merge-commit" => "pre-merge-commit".into(),
            b"push" => "pre-push".into(),
            _ => raw.to_os_string(),
        })),
        Conv::Int => raw
            .to_str()
            .and_then(pystr::py_int)
            .map(Converted::Int)
            .ok_or_else(|| invalid("int")),
        Conv::Custom(f, name) => match f(raw) {
            Ok(v) => Ok(Converted::Str(v)),
            Err(msg) if msg.is_empty() => Err(invalid(name)),
            Err(msg) => Err(arg_error(Some(a), &msg)),
        },
    }
}

/// `_check_value`: the converted value must be one of the choices.
fn check(a: &ActionSpec, v: &Converted) -> Result<(), Failure> {
    let ok = match a.kind {
        Kind::SubParsers(subs) => match v {
            Converted::Str(s) => subs.iter().any(|c| c.name.as_bytes() == bytes(s)),
            Converted::Int(_) => false,
        },
        _ => match a.choices {
            None => true,
            Some(choices) => match v {
                Converted::Str(s) => choices.iter().any(|c| c.as_bytes() == bytes(s)),
                Converted::Int(n) => {
                    let t = n.to_string();
                    choices.contains(&t.as_str())
                }
            },
        },
    };
    if ok {
        return Ok(());
    }
    let listed: Vec<String> = view::choices(a)
        .unwrap_or_default()
        .iter()
        .map(|c| snp_pycompat::repr::repr_str(c))
        .collect();
    let value = match v {
        Converted::Str(s) => repr_os(s),
        Converted::Int(n) => snp_pycompat::repr::repr_str(&n.to_string()),
    };
    Err(arg_error(
        Some(a),
        &format!(
            "invalid choice: {value} (choose from {})",
            listed.join(", ")
        ),
    ))
}

impl<'a> ParserRun<'a, '_> {
    fn action(&self, i: usize) -> Result<&'static ActionSpec, Failure> {
        self.view
            .action(i)
            .ok_or_else(|| Failure::Error("internal error: unknown action".to_owned()))
    }

    fn has(&self, i: usize, bit: u8) -> bool {
        self.state.get(i).is_some_and(|s| s & bit != 0)
    }

    fn mark(&mut self, i: usize, bit: u8) {
        if let Some(s) = self.state.get_mut(i) {
            *s |= bit;
        }
    }

    /// `_parse_known_args2`: every action's default, the first one of a shared dest wins.
    fn init_defaults(&mut self) {
        for (i, a) in self.view.iter().enumerate() {
            let Some(dest) = view::dest(a) else { continue };
            if self.ns.entry(dest).is_some() {
                continue;
            }
            let value = match a.default {
                DefaultValue::None => Value::Unset,
                DefaultValue::Str(s) | DefaultValue::Const(s) => Value::Str(s.into()),
                DefaultValue::Env { .. } => {
                    view::raw_default(a, self.cfg.env).map_or(Value::Unset, Value::Str)
                }
                DefaultValue::False => Value::Bool(false),
                DefaultValue::True => Value::Bool(true),
                DefaultValue::Int(n) => Value::Int(n),
                DefaultValue::EmptyList => Value::List(Vec::new()),
            };
            self.ns.init_default(dest, value, i);
        }
    }

    fn is_option(&self, i: usize) -> bool {
        self.tuples.get(i).is_some_and(Tuples::is_option)
    }

    fn parse(&mut self) -> Result<(), Failure> {
        self.classify();
        let max_option = (0..self.args.len()).rev().find(|&i| self.is_option(i));
        let mut start = 0usize;
        while let Some(max) = max_option.filter(|&m| start <= m) {
            let next_option = (start..=max)
                .find(|&i| self.is_option(i))
                .unwrap_or_else(|| max.saturating_add(1));
            if start != next_option {
                let end = self.consume_positionals(start)?;
                if end > start {
                    start = end;
                    continue;
                }
                start = end;
            }
            if !self.is_option(start) {
                let upto = next_option.min(self.args.len());
                self.extras
                    .extend_from_slice(self.args.get(start..upto).unwrap_or_default());
                start = next_option;
            }
            start = self.consume_optional(start)?;
        }
        let stop = self.consume_positionals(start)?;
        self.extras
            .extend_from_slice(self.args.get(stop..).unwrap_or_default());
        self.finish()
    }

    /// The `O`/`A`/`-` pattern and the option tuples of every token (`_parse_known_args`, 1991-2017).
    fn classify(&mut self) {
        let mut after_dashdash = false;
        let mut pattern = Vec::new();
        let mut tuples = Vec::new();
        for arg in self.args {
            if after_dashdash {
                pattern.push(b'A');
                tuples.push(Tuples::None);
            } else if bytes(arg) == b"--" {
                after_dashdash = true;
                pattern.push(b'-');
                tuples.push(Tuples::None);
            } else {
                let t = self.parse_optional(bytes(arg));
                pattern.push(if t.is_option() { b'O' } else { b'A' });
                tuples.push(t);
            }
        }
        self.pattern = pattern;
        self.tuples = tuples;
    }

    /// `_parse_optional` (2352-2396).
    fn parse_optional(&self, b: &[u8]) -> Tuples {
        if b.first() != Some(&b'-') {
            return Tuples::None;
        }
        if let Some((i, option)) = self.view.exact(b) {
            return Tuples::One(OptTuple {
                action: Some(i),
                option,
                sep: Sep::None,
                explicit: None,
            });
        }
        if b.len() == 1 {
            return Tuples::None;
        }
        if let Some(eq) = b.iter().position(|&c| c == b'=')
            && let Some((i, option)) = self.view.exact(b.get(..eq).unwrap_or_default())
        {
            return Tuples::One(OptTuple {
                action: Some(i),
                option,
                sep: Sep::Eq,
                explicit: Some(eq.saturating_add(1)),
            });
        }
        let tuples = self.option_tuples(b);
        if !matches!(tuples, Tuples::None) {
            return tuples;
        }
        if pystr::is_negative_number(b) && !self.view.negative_number_options() {
            return Tuples::None;
        }
        if b.contains(&b' ') {
            return Tuples::None;
        }
        Tuples::One(OptTuple {
            action: None,
            option: "",
            sep: Sep::None,
            explicit: None,
        })
    }

    /// `_get_option_tuples` (2398-2440).
    fn option_tuples(&self, b: &[u8]) -> Tuples {
        let mut found = Tuples::None;
        let mut add = |t: OptTuple| {
            found = match std::mem::replace(&mut found, Tuples::None) {
                Tuples::None => Tuples::One(t),
                Tuples::One(first) => Tuples::Many(vec![first, t]),
                Tuples::Many(mut v) => {
                    v.push(t);
                    Tuples::Many(v)
                }
            };
        };
        let (prefix, sep, explicit) = match b.iter().position(|&c| c == b'=') {
            Some(eq) => (
                b.get(..eq).unwrap_or_default(),
                Sep::Eq,
                Some(eq.saturating_add(1)),
            ),
            None => (b, Sep::None, None),
        };
        // A prefix that is not valid text (surrogate escapes) is never a prefix of an option string.
        let prefix_ok = std::str::from_utf8(prefix).is_ok();
        let abbrev = self.view.spec.allow_abbrev && prefix_ok;
        if b.get(1) == Some(&b'-') {
            if abbrev {
                for (option, i) in self.view.options() {
                    if option.as_bytes().starts_with(prefix) {
                        add(OptTuple {
                            action: Some(i),
                            option,
                            sep,
                            explicit,
                        });
                    }
                }
            }
        } else {
            let short_len = 1usize.saturating_add(pystr::py_char_len(b, 1));
            let short = b.get(..short_len).unwrap_or_default();
            for (option, i) in self.view.options() {
                if option.as_bytes() == short {
                    add(OptTuple {
                        action: Some(i),
                        option,
                        sep: Sep::Empty,
                        explicit: Some(short_len),
                    });
                } else if abbrev && option.as_bytes().starts_with(prefix) {
                    add(OptTuple {
                        action: Some(i),
                        option,
                        sep,
                        explicit,
                    });
                }
            }
        }
        found
    }

    /// `consume_optional` (2044-2141).
    fn consume_optional(&mut self, start: usize) -> Result<usize, Failure> {
        let args = self.args;
        let token: &'a OsString = match args.get(start) {
            Some(t) => t,
            None => return Ok(start.saturating_add(1)),
        };
        let first = match self.tuples.get(start) {
            Some(Tuples::One(t)) => *t,
            Some(Tuples::Many(v)) => {
                let options: Vec<&str> = v.iter().map(|t| t.option).collect();
                return Err(arg_error(
                    None,
                    &format!(
                        "ambiguous option: {} could match {}",
                        pystr::display(token),
                        options.join(", ")
                    ),
                ));
            }
            _ => {
                self.extras.push(token.clone());
                return Ok(start.saturating_add(1));
            }
        };
        let OptTuple {
            mut action,
            mut option,
            mut sep,
            mut explicit,
        } = first;
        let tb = bytes(token);
        let mut taken: Vec<(usize, Cow<'a, [OsString]>)> = Vec::new();
        let stop;
        loop {
            let Some(ai) = action else {
                self.extras.push(token.clone());
                return Ok(start.saturating_add(1));
            };
            let a = self.action(ai)?;
            if let Some(off) = explicit {
                let arg_count = usize::from(!matches!(view::nargs(a), Nargs::Zero));
                let rest = tb.get(off..).unwrap_or_default();
                let single_dash = option.as_bytes().get(1).is_some_and(|&c| c != b'-');
                if arg_count == 0 && single_dash && !rest.is_empty() {
                    if sep == Sep::Eq || rest.first() == Some(&b'-') {
                        return Err(arg_error(
                            Some(a),
                            &format!(
                                "ignored explicit argument {}",
                                repr_os(&pystr::os_tail(token, off))
                            ),
                        ));
                    }
                    taken.push((ai, Cow::Borrowed(&[])));
                    let char_len = pystr::py_char_len(tb, off);
                    let ch = tb
                        .get(off..off.saturating_add(char_len))
                        .unwrap_or_default();
                    let next = self.view.options().find(|(o, _)| {
                        let ob = o.as_bytes();
                        ob.first() == Some(&b'-') && ob.get(1..) == Some(ch)
                    });
                    if let Some((next_option, next)) = next {
                        action = Some(next);
                        option = next_option;
                        let after = off.saturating_add(char_len);
                        match tb.get(after) {
                            None => {
                                sep = Sep::None;
                                explicit = None;
                            }
                            Some(b'=') => {
                                sep = Sep::Eq;
                                explicit = Some(after.saturating_add(1));
                            }
                            Some(_) => {
                                sep = Sep::Empty;
                                explicit = Some(after);
                            }
                        }
                    } else {
                        self.extras.push(pystr::os_prefixed_tail("-", token, off));
                        stop = start.saturating_add(1);
                        break;
                    }
                } else if arg_count == 1 {
                    stop = start.saturating_add(1);
                    taken.push((ai, Cow::Owned(vec![pystr::os_tail(token, off)])));
                    break;
                } else {
                    return Err(arg_error(
                        Some(a),
                        &format!(
                            "ignored explicit argument {}",
                            repr_os(&pystr::os_tail(token, off))
                        ),
                    ));
                }
            } else {
                let from = start.saturating_add(1);
                let count =
                    match_option(view::nargs(a), self.pattern.get(from..).unwrap_or_default())
                        .map_err(|e| arg_error(Some(a), e.0))?;
                stop = from.saturating_add(count);
                taken.push((ai, Cow::Borrowed(args.get(from..stop).unwrap_or_default())));
                break;
            }
        }
        for (ai, values) in taken {
            self.take_action(ai, &values, true)?;
        }
        Ok(stop)
    }

    /// `consume_positionals` (2148-2177).
    fn consume_positionals(&mut self, start: usize) -> Result<usize, Failure> {
        let remaining: Vec<usize> = (0..self.view.len())
            .filter(|&i| self.view.action(i).is_some_and(|a| a.opts.is_empty()))
            .skip(self.positionals_done)
            .collect();
        let nargs: Vec<Nargs> = remaining
            .iter()
            .filter_map(|&i| self.view.action(i).map(view::nargs))
            .collect();
        let counts = matcher::match_partial(&nargs, self.pattern.get(start..).unwrap_or_default());
        let args = self.args;
        let mut index = start;
        self.positionals_done = self.positionals_done.saturating_add(counts.len());
        for (&ai, count) in remaining.iter().zip(counts) {
            let a = self.action(ai)?;
            let end = index.saturating_add(count);
            let slice = args.get(index..end).unwrap_or_default();
            let pat = self.pattern.get(index..end).unwrap_or_default();
            let values: Cow<'a, [OsString]> = match view::nargs(a) {
                Nargs::Parser if self.pattern.get(index) == Some(&b'-') => {
                    Cow::Borrowed(slice.get(1..).unwrap_or_default())
                }
                Nargs::Parser | Nargs::Remainder => Cow::Borrowed(slice),
                _ => match slice.iter().position(|v| bytes(v) == b"--") {
                    Some(k) if pat.contains(&b'-') => Cow::Owned(
                        slice
                            .iter()
                            .enumerate()
                            .filter(|&(j, _)| j != k)
                            .map(|(_, v)| v.clone())
                            .collect(),
                    ),
                    _ => Cow::Borrowed(slice),
                },
            };
            index = end;
            self.take_action(ai, &values, false)?;
        }
        Ok(index)
    }

    /// `_get_values` (2520-2569).
    fn get_values(&self, a: &ActionSpec, strings: &[OsString]) -> Result<Values, Failure> {
        let nargs = view::nargs(a);
        if strings.is_empty() && matches!(nargs, Nargs::Optional) {
            let value = if a.opts.is_empty() {
                match a.default {
                    DefaultValue::Str(_) | DefaultValue::Env { .. } => {
                        view::raw_default(a, self.cfg.env)
                    }
                    DefaultValue::Const(s) => Some(s.into()),
                    _ => None,
                }
            } else {
                match a.kind {
                    Kind::StoreConst(c) => Some(OsString::from(c)),
                    _ => None,
                }
            };
            return match value {
                Some(v) => {
                    let c = convert(a, &v)?;
                    check(a, &c)?;
                    Ok(Values::One(c))
                }
                None => Ok(Values::None),
            };
        }
        if strings.is_empty() && matches!(nargs, Nargs::ZeroOrMore) && a.opts.is_empty() {
            return Ok(match a.default {
                DefaultValue::Str(s) | DefaultValue::Const(s) => {
                    let c = Converted::Str(s.into());
                    check(a, &c)?;
                    Values::One(c)
                }
                _ => Values::Many(Vec::new()),
            });
        }
        if let [one] = strings
            && matches!(nargs, Nargs::One | Nargs::Optional)
        {
            let c = convert(a, one)?;
            check(a, &c)?;
            return Ok(Values::One(c));
        }
        let mut out = Vec::new();
        for s in strings {
            let c = convert(a, s)?;
            if !matches!(nargs, Nargs::Remainder) {
                check(a, &c)?;
            }
            out.push(c.into_os());
        }
        Ok(Values::Many(out))
    }

    /// `take_action` (2024-2041) and the action's `__call__`.
    fn take_action(
        &mut self,
        ai: usize,
        strings: &[OsString],
        is_option: bool,
    ) -> Result<(), Failure> {
        let a = self.action(ai)?;
        self.mark(ai, SEEN);
        let given = is_option || !strings.is_empty();
        if let Kind::SubParsers(_) = a.kind {
            // PARSER: convert (identity) and check the command name only, then parse the rest in place.
            let Some((name, rest)) = strings.split_first() else {
                return Ok(());
            };
            check(a, &Converted::Str(name.clone()))?;
            self.conflicts(ai, a, given)?;
            return self.subparser(a, name, rest);
        }
        let values = self.get_values(a, strings)?;
        self.conflicts(ai, a, given)?;
        let dest = view::dest(a);
        match a.kind {
            Kind::Help => {
                let prog = self.prog.render();
                let help = Formatter::new(&self.view, &prog, self.cfg).help();
                return Err(Failure::Stop(Stop::Help(help)));
            }
            Kind::Version => {
                let template = match a.default {
                    DefaultValue::Const(t) | DefaultValue::Str(t) => t,
                    _ => "",
                };
                let prog = self.prog.render();
                let text = Formatter::new(&self.view, &prog, self.cfg).version(template);
                return Err(Failure::Stop(Stop::Version(text)));
            }
            Kind::StoreTrue | Kind::StoreFalse => {
                if let Some(d) = dest {
                    self.ns
                        .assign(d, Value::Bool(matches!(a.kind, Kind::StoreTrue)), given);
                }
            }
            Kind::StoreConst(c) if matches!(view::nargs(a), Nargs::Zero) => {
                if let Some(d) = dest {
                    self.ns.assign(d, Value::Str(c.into()), given);
                }
            }
            Kind::Store | Kind::StoreConst(_) => {
                if let Some(d) = dest {
                    let value = match values {
                        Values::None => Value::Unset,
                        Values::One(c) => c.into_value(),
                        Values::Many(v) => Value::List(v),
                    };
                    self.ns.assign(d, value, given);
                }
            }
            Kind::Append => {
                if let Some(d) = dest {
                    let items = match values {
                        Values::None => vec![],
                        Values::One(c) => vec![c.into_os()],
                        Values::Many(v) => v,
                    };
                    self.ns.append(d, items, given);
                }
            }
            Kind::SubParsers(_) => {}
        }
        Ok(())
    }

    /// The mutual-exclusion check of `take_action`: an option, or a positional given strings, may not follow a
    /// present member of its group.
    fn conflicts(&mut self, ai: usize, a: &ActionSpec, given: bool) -> Result<(), Failure> {
        if !given {
            return Ok(());
        }
        self.mark(ai, SEEN_NON_DEFAULT);
        let Some(g) = a.mutex_group else {
            return Ok(());
        };
        let conflict = self.view.iter().enumerate().find(|&(j, other)| {
            j != ai && other.mutex_group == Some(g) && self.has(j, SEEN_NON_DEFAULT)
        });
        match conflict {
            Some((_, other)) => {
                let name = view::action_name(other).unwrap_or_default();
                Err(arg_error(
                    Some(a),
                    &format!("not allowed with argument {name}"),
                ))
            }
            None => Ok(()),
        }
    }

    /// `_SubParsersAction.__call__`: store the command name, parse the remaining strings with the sub-parser,
    /// keep its unrecognized strings for the top level.
    fn subparser(
        &mut self,
        a: &ActionSpec,
        name: &OsString,
        rest: &[OsString],
    ) -> Result<(), Failure> {
        let Some(sub) = self.view.subcommand(bytes(name)) else {
            return Ok(());
        };
        if let Some(d) = view::dest(a) {
            self.ns.assign(d, Value::Str(name.clone()), true);
        }
        if self.depth >= MAX_DEPTH {
            return Err(Failure::Error("sub-commands nested too deeply".to_owned()));
        }
        let prog = Prog {
            parent: Some(self.prog),
            part: sub.spec.prog_suffix,
        };
        let (sub_ns, sub_extras) = parse_known(
            sub.spec,
            &prog,
            rest,
            self.cfg,
            self.depth.saturating_add(1),
        )?;
        self.ns.set_command(sub.name, sub_ns);
        self.unrecognized.extend(sub_extras);
        Ok(())
    }

    /// End of `_parse_known_args` (2242-2263): convert untouched string defaults, then required arguments.
    fn finish(&mut self) -> Result<(), Failure> {
        let mut required = Vec::new();
        for i in 0..self.view.len() {
            if self.has(i, SEEN) {
                continue;
            }
            let a = self.action(i)?;
            if view::required(a) {
                required.push(view::action_name(a).unwrap_or_default().into_owned());
                continue;
            }
            if !matches!(a.default, DefaultValue::Str(_) | DefaultValue::Env { .. }) {
                continue;
            }
            let Some(dest) = view::dest(a) else { continue };
            let raw = match self.ns.entry(dest) {
                Some(e) if e.default_from == i && !e.assigned => match &e.value {
                    Value::Str(s) => s.clone(),
                    _ => continue,
                },
                _ => continue,
            };
            // `self._get_value(action, action.default)`; identity-like converters leave the value unchanged.
            match a.conv {
                Conv::Identity | Conv::HookType => {}
                Conv::UseColor | Conv::Stage | Conv::Int | Conv::Custom(..) => {
                    let converted = convert(a, &raw)?;
                    self.ns.convert_default(dest, converted.into_value());
                }
            }
        }
        if !required.is_empty() {
            return Err(arg_error(
                None,
                &format!(
                    "the following arguments are required: {}",
                    required.join(", ")
                ),
            ));
        }
        Ok(())
    }
}
