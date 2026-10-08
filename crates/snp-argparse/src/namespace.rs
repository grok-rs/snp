//! `argparse.Namespace`: dest → value in declaration order, plus the chosen sub-command (`argparse.py:1361-1379`).
//!
//! Unlike CPython, whose sub-parsers copy their attributes into the one flat namespace, a sub-command's values live
//! in a nested namespace reached through [`Namespace::command`]; the sub-parser action's own `dest` (`command`,
//! `tool`) is still stored as a string in the parent, as `setattr(namespace, self.dest, parser_name)` does.
//!
//! Owner: WP-12.

use std::ffi::{OsStr, OsString};

use crate::Value;

/// One `dest` of the namespace.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    /// The `dest` name.
    pub(crate) dest: &'static str,
    /// The current value.
    pub(crate) value: Value,
    /// Index of the action whose default initialised the slot (`action.default is getattr(namespace, dest)`).
    pub(crate) default_from: usize,
    /// An action stored into the slot (the value is no longer the default object).
    pub(crate) assigned: bool,
    /// The argument appeared on the command line: an option string or a positional with at least one string.
    pub(crate) given: bool,
}

/// The parse result (`argparse.Namespace`).
#[derive(Debug, Clone, Default)]
pub struct Namespace {
    entries: Vec<Entry>,
    sub: Option<(&'static str, Box<Self>)>,
}

impl Namespace {
    pub(crate) fn entry(&self, dest: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.dest == dest)
    }

    pub(crate) fn entry_mut(&mut self, dest: &str) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| e.dest == dest)
    }

    /// `setattr(namespace, dest, default)` unless the dest already exists (`_parse_known_args2`).
    pub(crate) fn init_default(&mut self, dest: &'static str, value: Value, action: usize) {
        if self.entry(dest).is_none() {
            self.entries.push(Entry {
                dest,
                value,
                default_from: action,
                assigned: false,
                given: false,
            });
        }
    }

    /// `setattr(namespace, dest, value)` by an action.
    pub(crate) fn assign(&mut self, dest: &'static str, value: Value, given: bool) {
        match self.entry_mut(dest) {
            Some(e) => {
                e.value = value;
                e.assigned = true;
                e.given |= given;
            }
            None => self.entries.push(Entry {
                dest,
                value,
                default_from: usize::MAX,
                assigned: true,
                given,
            }),
        }
    }

    /// `_copy_items(getattr(namespace, dest)) + values` (`_AppendAction`).
    pub(crate) fn append(&mut self, dest: &'static str, values: Vec<OsString>, given: bool) {
        let mut items = match self.entry(dest).map(|e| &e.value) {
            Some(Value::List(v)) => v.clone(),
            Some(Value::Str(s)) => vec![s.clone()],
            _ => Vec::new(),
        };
        items.extend(values);
        self.assign(dest, Value::List(items), given);
    }

    /// Replaces the default with its converted value (end of `_parse_known_args`), keeping it a default.
    pub(crate) fn convert_default(&mut self, dest: &str, value: Value) {
        if let Some(e) = self.entry_mut(dest) {
            e.value = value;
        }
    }

    pub(crate) fn set_command(&mut self, name: &'static str, ns: Self) {
        self.sub = Some((name, Box::new(ns)));
    }

    /// The chosen sub-command and its namespace (`args.command`).
    pub fn command(&self) -> Option<(&'static str, &Self)> {
        self.sub.as_ref().map(|(name, ns)| (*name, &**ns))
    }

    /// A boolean dest (`store_true` / `store_false`): `True` → `true`; anything else (absent included) → `false`.
    pub fn flag(&self, dest: &str) -> bool {
        matches!(self.entry(dest).map(|e| &e.value), Some(Value::Bool(true)))
    }

    /// A single string dest; `None` when unset (`None` in Python), absent, or not a string.
    pub fn value(&self, dest: &str) -> Option<&OsStr> {
        match self.entry(dest).map(|e| &e.value) {
            Some(Value::Str(s)) => Some(s.as_os_str()),
            _ => None,
        }
    }

    /// A list dest (`nargs='*'`/`'+'`/REMAINDER, `append`); empty when unset or absent.
    pub fn list(&self, dest: &str) -> &[OsString] {
        match self.entry(dest).map(|e| &e.value) {
            Some(Value::List(v)) => v,
            _ => &[],
        }
    }

    /// An integer dest (`type=int`, saturated to `i64`).
    pub fn int(&self, dest: &str) -> Option<i64> {
        match self.entry(dest).map(|e| &e.value) {
            Some(Value::Int(n)) => Some(*n),
            _ => None,
        }
    }

    /// Whether the argument appeared on the command line (an option string, or a positional that received at
    /// least one string), as opposed to holding its default.
    pub fn was_given(&self, dest: &str) -> bool {
        self.entry(dest).is_some_and(|e| e.given)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accessors() {
        let mut ns = Namespace::default();
        ns.init_default("color", Value::Str("auto".into()), 0);
        ns.init_default("color", Value::Str("other".into()), 1);
        ns.init_default("files", Value::List(Vec::new()), 2);
        ns.init_default("verbose", Value::Bool(false), 3);
        ns.init_default("jobs", Value::Int(1), 4);
        ns.init_default("hook", Value::Unset, 5);
        assert_eq!(ns.value("color"), Some(OsStr::new("auto")));
        assert_eq!(ns.entry("color").map(|e| e.default_from), Some(0));
        assert!(!ns.was_given("color"));
        ns.assign("verbose", Value::Bool(true), true);
        assert!(ns.flag("verbose") && ns.was_given("verbose"));
        ns.append("files", vec!["a".into()], true);
        ns.append("files", vec!["b".into()], true);
        assert_eq!(ns.list("files"), [OsString::from("a"), OsString::from("b")]);
        assert_eq!(ns.int("jobs"), Some(1));
        assert_eq!(ns.value("hook"), None);
        assert_eq!(ns.value("missing"), None);
        let none: &[OsString] = &[];
        assert_eq!(ns.list("missing"), none);
        assert!(ns.command().is_none());
        ns.set_command("run", Namespace::default());
        assert_eq!(ns.command().map(|c| c.0), Some("run"));
    }
}
