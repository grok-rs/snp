//! `argparse.HelpFormatter` of CPython 3.13.14 (`argparse.py:155-661`): usage (with the gh-75949 mutually
//! exclusive group handling), the `positional arguments:` / `options:` sections, help-text expansion and wrapping
//! (12 §A.11).
//!
//! Widths follow `HelpFormatter.__init__`: `width = columns - 2`, `max_help_position = min(24, max(width - 20,
//! 4))`. Lengths are Python `len()` (code points); signed arithmetic keeps CPython's behaviour for `COLUMNS=1`
//! (`width = -1`).
//!
//! Owner: WP-12.

use crate::pystr::{self, ilen};
use crate::textwrap;
use crate::view::{self, View};
use crate::{ActionSpec, Nargs, ParseCfg};

/// Formatter state for one parser (`HelpFormatter(prog=parser.prog)`).
#[derive(Debug)]
pub(crate) struct Formatter<'a> {
    view: &'a View,
    prog: &'a str,
    cfg: &'a ParseCfg<'a>,
    width: i64,
    max_help_position: i64,
}

/// Largest column count honoured (CPython has unbounded `int`; any width beyond the longest text renders alike).
const MAX_COLUMNS: usize = 1 << 31;

/// One item of a help section: a real action or a sub-command pseudo-action (`_ChoicesPseudoAction`).
#[derive(Copy, Clone, Debug)]
enum Item {
    Action(&'static ActionSpec),
    Choice {
        name: &'static str,
        help: &'static str,
    },
}

impl<'a> Formatter<'a> {
    pub(crate) fn new(view: &'a View, prog: &'a str, cfg: &'a ParseCfg<'a>) -> Self {
        let columns = i64::try_from(cfg.columns.min(MAX_COLUMNS)).unwrap_or(i64::MAX);
        let width = columns.saturating_sub(2);
        let max_help_position = 24.min(width.saturating_sub(20).max(4));
        Self {
            view,
            prog,
            cfg,
            width,
            max_help_position,
        }
    }

    // ----- usage -------------------------------------------------------------------------------------------

    /// `_metavar_formatter(action, default)(1)[0]`.
    fn metavar(a: &ActionSpec, default: &str) -> String {
        if let Some(m) = view::metavar(a) {
            m.into_owned()
        } else if let Some(c) = view::choices(a) {
            format!("{{{}}}", c.join(","))
        } else {
            default.to_owned()
        }
    }

    /// `_format_args(action, default_metavar)`.
    fn format_args(a: &ActionSpec, default: &str) -> String {
        let m = Self::metavar(a, default);
        match view::nargs(a) {
            Nargs::One => m,
            Nargs::Optional => format!("[{m}]"),
            Nargs::ZeroOrMore => format!("[{m} ...]"),
            Nargs::OneOrMore => format!("{m} [{m} ...]"),
            Nargs::Remainder => "...".to_owned(),
            Nargs::Parser => format!("{m} ..."),
            Nargs::Zero => String::new(),
        }
    }

    /// `dest.upper()` (`_get_default_metavar_for_optional`).
    fn optional_metavar(a: &ActionSpec) -> String {
        a.dest.to_uppercase()
    }

    /// `_get_actions_usage_parts_with_split(optionals + positionals, groups, len(optionals))`.
    fn usage_parts(&self) -> (Vec<String>, usize) {
        let (opt, pos) = self.view.usage_order();
        let order: Vec<usize> = opt.iter().chain(pos.iter()).copied().collect();
        let acts: Vec<&ActionSpec> = order.iter().filter_map(|&i| self.view.action(i)).collect();
        // Mutually exclusive groups, in order of creation (first member).
        let mut group_ids: Vec<u8> = Vec::new();
        for a in self.view.iter() {
            if let Some(g) = a.mutex_group
                && !group_ids.contains(&g)
            {
                group_ids.push(g);
            }
        }
        let mut in_group = vec![false; acts.len()];
        let mut inserts: Vec<(usize, usize)> = Vec::new();
        for g in group_ids {
            let members: Vec<&ActionSpec> = self
                .view
                .iter()
                .filter(|a| a.mutex_group == Some(g))
                .collect();
            if members.iter().all(|a| a.hidden) {
                continue;
            }
            let Some(first) = members.first() else {
                continue;
            };
            let Some(start) = acts.iter().position(|a| std::ptr::eq(*a, *first)) else {
                continue;
            };
            let end = start.saturating_add(members.len());
            let contiguous = acts.get(start..end).is_some_and(|slice| {
                slice.len() == members.len()
                    && slice
                        .iter()
                        .zip(&members)
                        .all(|(x, y)| std::ptr::eq(*x, *y))
            });
            if contiguous {
                for slot in in_group.get_mut(start..end).unwrap_or_default() {
                    *slot = true;
                }
                inserts.push((start, end));
            }
        }
        let mut parts: Vec<Option<String>> = acts
            .iter()
            .zip(&in_group)
            .map(|(a, &grouped)| {
                if a.hidden {
                    None
                } else if a.opts.is_empty() {
                    let mut part = Self::format_args(a, a.dest);
                    if grouped && part.starts_with('[') && part.ends_with(']') && part.len() >= 2 {
                        part = part
                            .get(1..part.len().saturating_sub(1))
                            .unwrap_or_default()
                            .to_owned();
                    }
                    Some(part)
                } else {
                    let first = a.opts.first().copied().unwrap_or_default();
                    let mut part = if matches!(view::nargs(a), Nargs::Zero) {
                        first.to_owned()
                    } else {
                        format!(
                            "{first} {}",
                            Self::format_args(a, &Self::optional_metavar(a))
                        )
                    };
                    if !view::required(a) && !grouped {
                        part = format!("[{part}]");
                    }
                    Some(part)
                }
            })
            .collect();
        inserts.sort_unstable();
        let mut separated = vec![false; parts.len()];
        for &(start, end) in inserts.iter().rev() {
            let mut group_parts: Vec<String> = parts
                .get(start..end)
                .unwrap_or_default()
                .iter()
                .flatten()
                .cloned()
                .collect();
            let size = group_parts.len();
            if let Some(f) = group_parts.first_mut() {
                *f = format!("[{f}");
            }
            if let Some(l) = group_parts.last_mut() {
                l.push(']');
            }
            for (k, part) in group_parts.iter().take(size.saturating_sub(1)).enumerate() {
                let i = start.saturating_add(k);
                if separated.get(i) == Some(&false) {
                    if let Some(slot) = parts.get_mut(i) {
                        *slot = Some(format!("{part} |"));
                    }
                    if let Some(s) = separated.get_mut(i) {
                        *s = true;
                    }
                }
            }
            if let (Some(last), Some(slot)) = (
                group_parts.last(),
                parts.get_mut(start.saturating_add(size).saturating_sub(1)),
            ) && size > 0
            {
                *slot = Some(last.clone());
            }
            for i in start.saturating_add(size)..end {
                if let Some(slot) = parts.get_mut(i) {
                    *slot = None;
                }
            }
        }
        let pos_start = parts.iter().take(opt.len()).flatten().count();
        (parts.into_iter().flatten().collect(), pos_start)
    }

    /// `_format_usage` (with `prefix='usage: '`) followed by `format_help`'s trimming: `usage: …\n`.
    pub(crate) fn usage(&self) -> String {
        let prefix = "usage: ";
        let prog = self.prog;
        let usage = if self.view.is_empty() {
            prog.to_owned()
        } else {
            let (parts, pos_start) = self.usage_parts();
            let action_usage = parts.join(" ");
            let full = [prog, action_usage.as_str()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            let text_width = self.width;
            if ilen(prefix).saturating_add(ilen(&full)) > text_width {
                let opt_parts = parts.get(..pos_start).unwrap_or_default();
                let pos_parts = parts.get(pos_start..).unwrap_or_default();
                let lines = if ilen(prefix).saturating_add(ilen(prog)).saturating_mul(4)
                    <= text_width.saturating_mul(3)
                {
                    let indent = " ".repeat_n_safe(
                        pylen_usize(prefix)
                            .saturating_add(pylen_usize(prog))
                            .saturating_add(1),
                    );
                    let with_prog = |rest: &[String]| -> Vec<String> {
                        let mut v = vec![prog.to_owned()];
                        v.extend(rest.iter().cloned());
                        v
                    };
                    if !opt_parts.is_empty() {
                        let mut lines =
                            get_lines(&with_prog(opt_parts), &indent, Some(prefix), text_width);
                        lines.extend(get_lines(pos_parts, &indent, None, text_width));
                        lines
                    } else if !pos_parts.is_empty() {
                        get_lines(&with_prog(pos_parts), &indent, Some(prefix), text_width)
                    } else {
                        vec![prog.to_owned()]
                    }
                } else {
                    let indent = " ".repeat_n_safe(pylen_usize(prefix));
                    let mut lines = get_lines(&parts, &indent, None, text_width);
                    if lines.len() > 1 {
                        lines = get_lines(opt_parts, &indent, None, text_width);
                        lines.extend(get_lines(pos_parts, &indent, None, text_width));
                    }
                    let mut out = vec![prog.to_owned()];
                    out.extend(lines);
                    out
                };
                lines.join("\n")
            } else {
                full
            }
        };
        format!("{prefix}{usage}\n")
    }

    // ----- help --------------------------------------------------------------------------------------------

    /// `_format_action_invocation`.
    fn invocation(item: Item) -> String {
        match item {
            Item::Choice { name, .. } => name.to_owned(),
            Item::Action(a) if a.opts.is_empty() => Self::metavar(a, a.dest),
            Item::Action(a) if matches!(view::nargs(a), Nargs::Zero) => a.opts.join(", "),
            Item::Action(a) => format!(
                "{} {}",
                a.opts.join(", "),
                Self::format_args(a, &Self::optional_metavar(a))
            ),
        }
    }

    /// `_iter_indented_subactions`: the sub-commands that have a `help=` (`_ChoicesPseudoAction`s), listed or not.
    fn subactions(a: &ActionSpec) -> Vec<Item> {
        view::subcommands(a)
            .iter()
            .filter_map(|s| s.help.map(|help| Item::Choice { name: s.name, help }))
            .collect()
    }

    /// `_expand_help`.
    fn expand_help(&self, item: Item) -> String {
        match item {
            Item::Choice { help, .. } => pystr::percent_format(help, &|key, repr| match key {
                "prog" => Some(if repr {
                    snp_pycompat::repr::repr_str(self.prog)
                } else {
                    self.prog.to_owned()
                }),
                "dest" | "metavar" => Some(String::new()),
                _ => None,
            }),
            Item::Action(a) => {
                let help = a.help.unwrap_or_default();
                pystr::percent_format(help, &|key, repr| {
                    view::help_param(a, self.prog, self.cfg.env, key, repr)
                })
            }
        }
    }

    /// `_split_lines`: ASCII whitespace runs collapsed, stripped, `textwrap.wrap`.
    fn split_lines(text: &str, width: i64) -> Vec<String> {
        let t = pystr::collapse_ascii_ws(text);
        textwrap::wrap(
            pystr::py_strip(&t),
            usize::try_from(width).unwrap_or(1).max(1),
        )
    }

    /// `_format_action` at `indent` with the final `help_position`.
    fn format_item(&self, item: Item, indent: i64, max_len: i64, out: &mut String) {
        let help_position = max_len.saturating_add(2).min(self.max_help_position);
        let help_width = self.width.saturating_sub(help_position).max(11);
        let action_width = help_position.saturating_sub(indent).saturating_sub(2);
        let header = Self::invocation(item);
        let help = match item {
            Item::Action(a) => a.help,
            Item::Choice { help, .. } => Some(help),
        };
        let pad = |n: i64| " ".repeat_n_safe(usize::try_from(n).unwrap_or(0));
        let has_help = help.is_some_and(|h| !h.is_empty());
        out.push_str(&pad(indent));
        out.push_str(&header);
        let indent_first = if has_help && ilen(&header) <= action_width {
            out.push_str(&pad(action_width.saturating_sub(ilen(&header))));
            out.push_str("  ");
            0
        } else {
            out.push('\n');
            if has_help { help_position } else { 0 }
        };
        if help.is_some_and(|h| !pystr::py_strip(h).is_empty()) {
            let text = self.expand_help(item);
            if !text.is_empty() {
                let lines = Self::split_lines(&text, help_width);
                if lines.is_empty() {
                    // CPython raises IndexError (`help_lines[0]`) for help that expands to whitespace only.
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                }
                for (k, line) in lines.iter().enumerate() {
                    out.push_str(&pad(if k == 0 { indent_first } else { help_position }));
                    out.push_str(line);
                    out.push('\n');
                }
            }
        } else if !out.ends_with('\n') {
            out.push('\n');
        }
        if let Item::Action(a) = item {
            for sub in Self::subactions(a) {
                self.format_item(sub, indent.saturating_add(2), max_len, out);
            }
        }
    }

    /// `_format_text`: `%(prog)` expansion, then `textwrap.fill` at the current indent, plus a blank line.
    fn format_text(&self, text: &str, indent: i64) -> String {
        let text = if text.contains("%(prog)") {
            pystr::percent_format(text, &|key, repr| {
                (key == "prog").then(|| {
                    if repr {
                        snp_pycompat::repr::repr_str(self.prog)
                    } else {
                        self.prog.to_owned()
                    }
                })
            })
        } else {
            text.to_owned()
        };
        let width = self.width.saturating_sub(indent).max(11);
        let pad = " ".repeat_n_safe(usize::try_from(indent).unwrap_or(0));
        let collapsed = pystr::collapse_ascii_ws(&text);
        let filled = textwrap::fill(
            pystr::py_strip(&collapsed),
            usize::try_from(width).unwrap_or(11),
            &pad,
            &pad,
        );
        format!("{filled}\n\n")
    }

    /// `format_help()` of the parser: usage, description, the two sections, epilog; runs of three or more
    /// newlines collapsed, one final newline.
    pub(crate) fn help(&self) -> String {
        let mut text = String::new();
        let usage = self.usage();
        text.push_str(&usage);
        text.push('\n');
        if let Some(d) = self.view.spec.description {
            text.push_str(&self.format_text(d, 0));
        }
        let (opt, pos) = self.view.usage_order();
        let visible = |ids: &[usize]| -> Vec<&'static ActionSpec> {
            ids.iter()
                .filter_map(|&i| self.view.action(i))
                .filter(|a| !a.hidden)
                .collect()
        };
        let sections = [
            ("positional arguments", visible(&pos)),
            ("options", visible(&opt)),
        ];
        // `add_argument` runs at indent 2 (inside `start_section`); sub-actions at 4.
        let mut max_len: i64 = 0;
        for (_, acts) in &sections {
            for &a in acts {
                max_len = max_len.max(ilen(&Self::invocation(Item::Action(a))).saturating_add(2));
                for sub in Self::subactions(a) {
                    max_len = max_len.max(ilen(&Self::invocation(sub)).saturating_add(4));
                }
            }
        }
        for (heading, acts) in &sections {
            let mut items = String::new();
            for &a in acts {
                self.format_item(Item::Action(a), 2, max_len, &mut items);
            }
            if !items.is_empty() {
                text.push('\n');
                text.push_str(heading);
                text.push_str(":\n");
                text.push_str(&items);
                text.push('\n');
            }
        }
        if let Some(e) = self.view.spec.epilog {
            text.push_str(&self.format_text(e, 0));
        }
        finish(&text)
    }

    /// The `version` action's output: `formatter.add_text(version)` → `format_help()`.
    pub(crate) fn version(&self, template: &str) -> String {
        finish(&self.format_text(template, 0))
    }
}

/// `format_help`'s tail: `\n\n\n+` → `\n\n`, `strip('\n')`, one `\n`; empty stays empty.
fn finish(text: &str) -> String {
    let mut out = String::new();
    let mut newlines = 0usize;
    for c in text.chars() {
        if c == '\n' {
            newlines = newlines.saturating_add(1);
            if newlines <= 2 {
                out.push('\n');
            }
        } else {
            newlines = 0;
            out.push(c);
        }
    }
    let trimmed = out.trim_matches('\n');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}\n")
    }
}

/// The nested `get_lines(parts, indent, prefix)` of `_format_usage`.
fn get_lines(parts: &[String], indent: &str, prefix: Option<&str>, text_width: i64) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line: Vec<&str> = Vec::new();
    let indent_len = ilen(indent);
    let mut line_len = match prefix {
        Some(p) => ilen(p).saturating_sub(1),
        None => indent_len.saturating_sub(1),
    };
    for part in parts {
        if line_len.saturating_add(1).saturating_add(ilen(part)) > text_width && !line.is_empty() {
            lines.push(format!("{indent}{}", line.join(" ")));
            line.clear();
            line_len = indent_len.saturating_sub(1);
        }
        line.push(part);
        line_len = line_len.saturating_add(ilen(part)).saturating_add(1);
    }
    if !line.is_empty() {
        lines.push(format!("{indent}{}", line.join(" ")));
    }
    if prefix.is_some()
        && let Some(first) = lines.first_mut()
    {
        let skip = indent.chars().count();
        *first = first.chars().skip(skip).collect();
    }
    lines
}

fn pylen_usize(s: &str) -> usize {
    pystr::pylen(s)
}

/// `' ' * n` without the panicking `str::repeat`.
trait RepeatSafe {
    fn repeat_n_safe(&self, n: usize) -> String;
}

impl RepeatSafe for str {
    fn repeat_n_safe(&self, n: usize) -> String {
        std::iter::repeat_n(self, n).collect()
    }
}
