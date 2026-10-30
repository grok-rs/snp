//! `ruamel/yaml/resolver.py` (`VersionedResolver`): the YAML 1.1 and 1.2 implicit-tag rules, tried in
//! `implicit_resolvers` order among those registered for the scalar's first character, and the
//! `util.timestamp_regexp` the constructor matches. The regular expressions are matched by hand (they are
//! regular and every backtracking choice is local); `$` also matches before a final `'\n'`, as in Python.
//!
//! Owner: WP-17.

use super::{Fail, Loader, R, Rx, Ver};
use crate::pystr;

const BOOL: &str = "tag:yaml.org,2002:bool";
const FLOAT: &str = "tag:yaml.org,2002:float";
const INT: &str = "tag:yaml.org,2002:int";
const MERGE: &str = "tag:yaml.org,2002:merge";
const NULL: &str = "tag:yaml.org,2002:null";
const TIMESTAMP: &str = "tag:yaml.org,2002:timestamp";
const VALUE: &str = "tag:yaml.org,2002:value";
const YAML: &str = "tag:yaml.org,2002:yaml";
/// `BaseResolver.DEFAULT_SCALAR_TAG`.
pub(super) const STR: &str = "tag:yaml.org,2002:str";

/// The rules registered for `first` (a code point, `None` for the empty string) under version 1.1 or 1.2.
fn rules(v11: bool, first: Option<u32>) -> Vec<Rx> {
    let c = first.and_then(char::from_u32);
    let mut out = Vec::new();
    let digit = c.is_some_and(|c| c.is_ascii_digit());
    let sign = c.is_some_and(|c| c == '-' || c == '+');
    if v11 {
        if c.is_some_and(|c| "yYnNtTfFoO".contains(c)) {
            out.push(Rx::Bool11);
        }
        if digit || sign || c == Some('.') {
            out.push(Rx::Float11);
        }
        if digit || sign {
            out.push(Rx::Int11);
        }
    } else {
        if c.is_some_and(|c| "tTfF".contains(c)) {
            out.push(Rx::Bool12);
        }
        if digit || sign || c == Some('.') {
            out.push(Rx::Float12);
        }
        if digit || sign {
            out.push(Rx::Int12);
        }
    }
    if c == Some('<') {
        out.push(Rx::Merge);
    }
    if first.is_none() || c.is_some_and(|c| "~nN".contains(c)) {
        out.push(Rx::Null);
    }
    if digit {
        out.push(Rx::Timestamp);
    }
    if c == Some('=') {
        out.push(Rx::Value);
    }
    if c.is_some_and(|c| "!&*".contains(c)) {
        out.push(Rx::Yaml);
    }
    out
}

const fn tag_of(rx: Rx) -> &'static str {
    match rx {
        Rx::Bool11 | Rx::Bool12 => BOOL,
        Rx::Float11 | Rx::Float12 => FLOAT,
        Rx::Int11 | Rx::Int12 => INT,
        Rx::Merge => MERGE,
        Rx::Null => NULL,
        Rx::Timestamp | Rx::TimestampCons => TIMESTAMP,
        Rx::Value => VALUE,
        Rx::Yaml | Rx::NonPrintable => YAML,
    }
}

/// `regexp.match(value)` with Python's `$` (end, or before a final newline).
pub(super) fn rx_match(rx: Rx, value: &[u8]) -> bool {
    let full = |s: &[u8]| match rx {
        Rx::Bool12 => matches!(
            s,
            b"true" | b"True" | b"TRUE" | b"false" | b"False" | b"FALSE"
        ),
        Rx::Bool11 => matches!(
            s,
            b"y" | b"Y"
                | b"yes"
                | b"Yes"
                | b"YES"
                | b"n"
                | b"N"
                | b"no"
                | b"No"
                | b"NO"
                | b"true"
                | b"True"
                | b"TRUE"
                | b"false"
                | b"False"
                | b"FALSE"
                | b"on"
                | b"On"
                | b"ON"
                | b"off"
                | b"Off"
                | b"OFF"
        ),
        Rx::Float12 => float_full(s, false),
        Rx::Float11 => float_full(s, true),
        Rx::Int12 => int12_full(s),
        Rx::Int11 => int11_full(s),
        Rx::Merge => s == b"<<",
        Rx::Null => matches!(s, b"~" | b"null" | b"Null" | b"NULL" | b""),
        Rx::Timestamp => timestamp_implicit(s),
        Rx::Value => s == b"=",
        Rx::Yaml => matches!(s, b"!" | b"&" | b"*"),
        Rx::TimestampCons => timestamp_groups(s).is_some(),
        Rx::NonPrintable => false,
    };
    full(value)
        || value.last() == Some(&b'\n')
            && full(
                value
                    .get(..value.len().saturating_sub(1))
                    .unwrap_or_default(),
            )
}

fn run(s: &[u8], i: usize, f: impl Fn(u8) -> bool) -> usize {
    let mut j = i;
    while s.get(j).is_some_and(|&b| f(b)) {
        j = j.saturating_add(1);
    }
    j
}

const fn digit(b: u8) -> bool {
    b.is_ascii_digit()
}

const fn digit_us(b: u8) -> bool {
    b.is_ascii_digit() || b == b'_'
}

fn strip_sign(s: &[u8]) -> &[u8] {
    match s.first() {
        Some(b'-' | b'+') => s.get(1..).unwrap_or_default(),
        _ => s,
    }
}

/// `[eE][-+]?[0-9]+` (`sign_required`: `[eE][-+][0-9]+`) running to the end of `s` from `i`.
fn exponent_to_end(s: &[u8], i: usize, sign_required: bool) -> bool {
    if !matches!(s.get(i), Some(b'e' | b'E')) {
        return false;
    }
    let mut j = i.saturating_add(1);
    if matches!(s.get(j), Some(b'-' | b'+')) {
        j = j.saturating_add(1);
    } else if sign_required {
        return false;
    }
    let k = run(s, j, digit);
    k > j && k == s.len()
}

/// `(?::[0-5]?[0-9])+` from `i` (the index after it, `None` if no group matches).
#[expect(
    clippy::many_single_char_names,
    reason = "the names of the Python and C code this ports"
)]
fn sexagesimal_groups(s: &[u8], mut i: usize) -> Option<usize> {
    let mut any = false;
    while s.get(i) == Some(&b':') {
        let a = s.get(i.saturating_add(1)).copied();
        let b = s.get(i.saturating_add(2)).copied();
        match (a, b) {
            (Some(x @ b'0'..=b'5'), Some(y)) if digit(y) && digit(x) => i = i.saturating_add(3),
            (Some(x), _) if digit(x) => i = i.saturating_add(2),
            _ => break,
        }
        any = true;
    }
    any.then_some(i)
}

fn float_full(s: &[u8], v11: bool) -> bool {
    // \.(?:nan|NaN|NAN)
    if matches!(s, b".nan" | b".NaN" | b".NAN") {
        return true;
    }
    if !v11 || !matches!(s.first(), Some(b'-' | b'+')) {
        // 1.2: [-+]?\.[0-9_]+(?:[eE][-+][0-9]+)?   1.1: \.[0-9_]+(?:[eE][-+][0-9]+)?
        let t = if v11 { s } else { strip_sign(s) };
        if t.first() == Some(&b'.') {
            let j = run(t, 1, digit_us);
            if j > 1 && (j == t.len() || exponent_to_end(t, j, true)) {
                return true;
            }
        }
    }
    let t = strip_sign(s);
    // [-+]?\.(?:inf|Inf|INF)
    if matches!(t, b".inf" | b".Inf" | b".INF") {
        return true;
    }
    if !t.first().copied().is_some_and(digit) {
        return false;
    }
    let j = run(t, 1, digit_us);
    // [-+]?(?:[0-9][0-9_]*)\.[0-9_]*(?:[eE][-+]?[0-9]+)?
    if t.get(j) == Some(&b'.') {
        let k = run(t, j.saturating_add(1), digit_us);
        if k == t.len() || exponent_to_end(t, k, false) {
            return true;
        }
    }
    // [-+]?(?:[0-9][0-9_]*)(?:[eE][-+]?[0-9]+)
    if exponent_to_end(t, j, false) {
        return true;
    }
    // 1.1: [-+]?[0-9][0-9_]*(?::[0-5]?[0-9])+\.[0-9_]*
    if v11
        && let Some(k) = sexagesimal_groups(t, j)
        && t.get(k) == Some(&b'.')
    {
        return run(t, k.saturating_add(1), digit_us) == t.len();
    }
    false
}

fn int12_full(s: &[u8]) -> bool {
    let t = strip_sign(s);
    let rest = |p: &[u8], f: fn(u8) -> bool| -> bool { !p.is_empty() && p.iter().all(|&b| f(b)) };
    if let Some(p) = t.strip_prefix(b"0b")
        && rest(p, |b| matches!(b, b'0' | b'1' | b'_'))
    {
        return true;
    }
    if let Some(p) = t.strip_prefix(b"0") {
        let p = p.strip_prefix(b"o").unwrap_or(p);
        if rest(p, |b| matches!(b, b'0'..=b'7' | b'_')) {
            return true;
        }
    }
    if rest(t, digit_us) {
        return true;
    }
    if let Some(p) = t.strip_prefix(b"0x")
        && rest(p, |b| b.is_ascii_hexdigit() || b == b'_')
    {
        return true;
    }
    false
}

fn int11_full(s: &[u8]) -> bool {
    let t = strip_sign(s);
    let rest = |p: &[u8], f: fn(u8) -> bool| -> bool { !p.is_empty() && p.iter().all(|&b| f(b)) };
    if let Some(p) = t.strip_prefix(b"0b")
        && rest(p, |b| matches!(b, b'0' | b'1' | b'_'))
    {
        return true;
    }
    if rest(t, |b| matches!(b, b'0'..=b'7' | b'_')) {
        return true;
    }
    if t == b"0" {
        return true;
    }
    if t.first().is_some_and(|b| matches!(b, b'1'..=b'9')) && run(t, 1, digit_us) == t.len() {
        return true;
    }
    if let Some(p) = t.strip_prefix(b"0x")
        && rest(p, |b| b.is_ascii_hexdigit() || b == b'_')
    {
        return true;
    }
    if t.first().is_some_and(|b| matches!(b, b'1'..=b'9')) {
        let j = run(t, 1, digit_us);
        if sexagesimal_groups(t, j) == Some(t.len()) {
            return true;
        }
    }
    false
}

/// `[0-9][0-9]?` followed by a byte satisfying `next` (greedy, then one digit).
fn one_or_two(s: &[u8], i: usize, next: impl Fn(Option<u8>) -> bool) -> Option<usize> {
    if !s.get(i).copied().is_some_and(digit) {
        return None;
    }
    let i1 = i.saturating_add(1);
    let i2 = i.saturating_add(2);
    if s.get(i1).copied().is_some_and(digit) && next(s.get(i2).copied()) {
        return Some(i2);
    }
    next(s.get(i1).copied()).then_some(i1)
}

fn exact_digits(s: &[u8], i: usize, n: usize) -> bool {
    (0..n).all(|k| s.get(i.saturating_add(k)).copied().is_some_and(digit))
}

/// The resolver's timestamp rule.
fn timestamp_implicit(s: &[u8]) -> bool {
    // [0-9]{4}-[0-9]{2}-[0-9]{2}
    if s.len() == 10
        && exact_digits(s, 0, 4)
        && s.get(4) == Some(&b'-')
        && exact_digits(s, 5, 2)
        && s.get(7) == Some(&b'-')
        && exact_digits(s, 8, 2)
    {
        return true;
    }
    timestamp_groups(s).is_some_and(|g| g.hour.is_some())
}

/// The groups of `util.timestamp_regexp` (also the resolver's second alternative when `hour` is set).
#[derive(Debug, Clone, Default)]
pub(super) struct TsGroups {
    pub(super) year: String,
    pub(super) month: String,
    pub(super) day: String,
    pub(super) hour: Option<String>,
    pub(super) minute: String,
    pub(super) second: String,
    pub(super) fraction: Option<String>,
    pub(super) tz: Option<String>,
    pub(super) tz_sign: Option<char>,
    pub(super) tz_hour: String,
    pub(super) tz_minute: Option<String>,
}

fn text(s: &[u8], a: usize, b: usize) -> String {
    String::from_utf8_lossy(s.get(a..b).unwrap_or_default()).into_owned()
}

/// `timestamp_regexp.match(value)` without the `$` newline rule (the caller handles it).
pub(super) fn timestamp_groups(s: &[u8]) -> Option<TsGroups> {
    if !exact_digits(s, 0, 4) || s.get(4) != Some(&b'-') {
        return None;
    }
    let mut g = TsGroups {
        year: text(s, 0, 4),
        ..TsGroups::default()
    };
    let m_end = one_or_two(s, 5, |b| b == Some(b'-'))?;
    g.month = text(s, 5, m_end);
    let d_start = m_end.saturating_add(1);
    let time_sep = |b: Option<u8>| matches!(b, Some(b'T' | b't' | b' ' | b'\t'));
    // The day is followed by the end (date only) or by the time part.
    let try_day = |end: usize| -> Option<TsGroups> {
        let mut g = g.clone();
        g.day = text(s, d_start, end);
        if end == s.len() {
            return Some(g);
        }
        time_part(s, end, &mut g).then_some(g)
    };
    let i1 = d_start.saturating_add(1);
    let i2 = d_start.saturating_add(2);
    if !s.get(d_start).copied().is_some_and(digit) {
        return None;
    }
    if s.get(i1).copied().is_some_and(digit)
        && (i2 == s.len() || time_sep(s.get(i2).copied()))
        && let Some(r) = try_day(i2)
    {
        return Some(r);
    }
    if i1 == s.len() || time_sep(s.get(i1).copied()) {
        return try_day(i1);
    }
    None
}

/// `(?:((?P<t>[Tt])|[ \t]+)(?P<hour>…):…(?:\.(?P<fraction>[0-9]*))?(?:[ \t]*(?P<tz>…))?)` then `$`.
#[expect(
    clippy::many_single_char_names,
    reason = "the names of the Python and C code this ports"
)]
fn time_part(s: &[u8], mut i: usize, g: &mut TsGroups) -> bool {
    match s.get(i) {
        Some(b'T' | b't') => i = i.saturating_add(1),
        Some(b' ' | b'\t') => i = run(s, i, |b| b == b' ' || b == b'\t'),
        _ => return false,
    }
    let Some(h_end) = one_or_two(s, i, |b| b == Some(b':')) else {
        return false;
    };
    g.hour = Some(text(s, i, h_end));
    i = h_end.saturating_add(1);
    if !exact_digits(s, i, 2) || s.get(i.saturating_add(2)) != Some(&b':') {
        return false;
    }
    g.minute = text(s, i, i.saturating_add(2));
    i = i.saturating_add(3);
    if !exact_digits(s, i, 2) {
        return false;
    }
    g.second = text(s, i, i.saturating_add(2));
    i = i.saturating_add(2);
    if s.get(i) == Some(&b'.') {
        let j = run(s, i.saturating_add(1), digit);
        g.fraction = Some(text(s, i.saturating_add(1), j));
        i = j;
    }
    if i == s.len() {
        return true;
    }
    let j = run(s, i, |b| b == b' ' || b == b'\t');
    match s.get(j) {
        Some(b'Z') if j.saturating_add(1) == s.len() => {
            g.tz = Some("Z".to_owned());
            true
        }
        Some(&c @ (b'-' | b'+')) => {
            let k = j.saturating_add(1);
            // tz_hour [0-9][0-9]? then (:[0-9][0-9])? then the end
            for h_end in [k.saturating_add(2), k.saturating_add(1)] {
                if h_end > s.len() || !exact_digits(s, k, h_end.saturating_sub(k)) {
                    continue;
                }
                let (tz_minute, end) =
                    if s.get(h_end) == Some(&b':') && exact_digits(s, h_end.saturating_add(1), 2) {
                        (
                            Some(text(s, h_end.saturating_add(1), h_end.saturating_add(3))),
                            h_end.saturating_add(3),
                        )
                    } else {
                        (None, h_end)
                    };
                if end == s.len() {
                    g.tz = Some(text(s, j, end));
                    g.tz_sign = Some(char::from(c));
                    g.tz_hour = text(s, k, h_end);
                    g.tz_minute = tz_minute;
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the loader's methods live next to the ruamel module they port"
)]
impl Loader<'_, '_> {
    /// `VersionedResolver.versioned_resolver` (a property: one frame), building the tables on first use.
    fn versioned_resolver(&mut self) -> R<Ver> {
        self.frame(|s| {
            s.touch(1)?; // processing_version
            let v = s.processing_version();
            let known = v.is11() || v.is("1", "2");
            if !s.st.resolver_built.contains(&v) {
                if known {
                    s.touch(1)?; // add_version_implicit_resolver
                    s.st.resolver_built.push(v.clone());
                } else {
                    return Err(Fail::crash("KeyError", v.repr()));
                }
            }
            Ok(v)
        })
    }

    /// `VersionedResolver.resolve(ScalarNode, value, implicit)` (the caller pushed its frame).
    pub(super) fn resolve_scalar(&mut self, value: &[u8], implicit: bool) -> R<String> {
        if implicit {
            let v = self.versioned_resolver()?;
            let first = pystr::code_points(value).next();
            let candidates = rules(v.is11(), first);
            self.versioned_resolver()?;
            for rx in candidates {
                self.rx_call(rx)?;
                if rx_match(rx, value) {
                    self.touch(1)?; // Tag(suffix=tag)
                    return Ok(tag_of(rx).to_owned());
                }
            }
        }
        Ok(STR.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(v11: bool, s: &str) -> &'static str {
        let b = s.as_bytes();
        for rx in rules(v11, pystr::code_points(b).next()) {
            if rx_match(rx, b) {
                return tag_of(rx);
            }
        }
        STR
    }

    #[test]
    fn implicit_tags() {
        for (s, t) in [
            ("1", INT),
            ("017", INT),
            ("0o17", INT),
            ("0x_1f", INT),
            ("-_", INT),
            ("1_000", INT),
            ("1.5", FLOAT),
            ("1e5", FLOAT),
            ("-.5e+3", FLOAT),
            (".5", FLOAT),
            ("-.inf", FLOAT),
            (".NaN", FLOAT),
            ("-.nan", STR),
            ("1:20", STR),
            ("true", BOOL),
            ("yes", STR),
            ("", NULL),
            ("~", NULL),
            ("<<", MERGE),
            ("=", VALUE),
            ("2001-01-01", TIMESTAMP),
            ("2001-1-1", STR),
            ("2001-1-1 1:00:00", TIMESTAMP),
            ("2001-01-01t10:00:00.5 -05:00", TIMESTAMP),
            ("2001-01-01 10:00:00 Z", TIMESTAMP),
            ("2001-01-01 10:00:00.Z", TIMESTAMP),
            ("2001-13-45", TIMESTAMP),
        ] {
            assert_eq!(resolve(false, s), t, "1.2 {s:?}");
        }
        for (s, t) in [
            ("yes", BOOL),
            ("1:20", INT),
            ("190:20:30.15", FLOAT),
            ("017", INT),
            ("09", STR),
            (".5", FLOAT),
            ("-.5", STR),
            ("1e5", FLOAT),
        ] {
            assert_eq!(resolve(true, s), t, "1.1 {s:?}");
        }
    }
}
