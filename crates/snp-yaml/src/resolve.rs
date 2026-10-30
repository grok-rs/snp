//! PyYAML's implicit resolver table (`resolver.py`) and the YAML 1.2 core switch.
//!
//! The PyYAML regexes are matched by hand-written matchers (faster than a regex engine, and free of the `re.X`
//! whitespace pitfall of porting them, 07 §3.1.6). Each matcher is the full-match of one regex; the `^…$` anchors
//! are applied by [`py_full_match`], where Python's `$` also matches before a final `\n`.
//!
//! Owner: WP-14.

use crate::Schema;

/// `tag:yaml.org,2002:null`.
pub(crate) const NULL_TAG: &str = "tag:yaml.org,2002:null";
/// `tag:yaml.org,2002:bool`.
pub(crate) const BOOL_TAG: &str = "tag:yaml.org,2002:bool";
/// `tag:yaml.org,2002:int`.
pub(crate) const INT_TAG: &str = "tag:yaml.org,2002:int";
/// `tag:yaml.org,2002:float`.
pub(crate) const FLOAT_TAG: &str = "tag:yaml.org,2002:float";
/// `tag:yaml.org,2002:timestamp`.
pub(crate) const TIMESTAMP_TAG: &str = "tag:yaml.org,2002:timestamp";
/// `tag:yaml.org,2002:merge`.
pub(crate) const MERGE_TAG: &str = "tag:yaml.org,2002:merge";
/// `tag:yaml.org,2002:value`.
pub(crate) const VALUE_TAG: &str = "tag:yaml.org,2002:value";
/// `tag:yaml.org,2002:yaml`.
pub(crate) const YAML_TAG: &str = "tag:yaml.org,2002:yaml";
/// `tag:yaml.org,2002:str` (`DEFAULT_SCALAR_TAG`).
pub(crate) const STR_TAG: &str = "tag:yaml.org,2002:str";
/// `tag:yaml.org,2002:seq` (`DEFAULT_SEQUENCE_TAG`).
pub(crate) const SEQ_TAG: &str = "tag:yaml.org,2002:seq";
/// `tag:yaml.org,2002:map` (`DEFAULT_MAPPING_TAG`).
pub(crate) const MAP_TAG: &str = "tag:yaml.org,2002:map";
/// `tag:yaml.org,2002:binary`.
pub(crate) const BINARY_TAG: &str = "tag:yaml.org,2002:binary";
/// `tag:yaml.org,2002:set`.
pub(crate) const SET_TAG: &str = "tag:yaml.org,2002:set";
/// `tag:yaml.org,2002:omap`.
pub(crate) const OMAP_TAG: &str = "tag:yaml.org,2002:omap";
/// `tag:yaml.org,2002:pairs`.
pub(crate) const PAIRS_TAG: &str = "tag:yaml.org,2002:pairs";

/// The implicit tag of a plain scalar.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ImplicitTag {
    /// `tag:yaml.org,2002:null`.
    Null,
    /// `tag:yaml.org,2002:bool`.
    Bool,
    /// `tag:yaml.org,2002:int`.
    Int,
    /// `tag:yaml.org,2002:float`.
    Float,
    /// `tag:yaml.org,2002:timestamp`.
    Timestamp,
    /// `tag:yaml.org,2002:merge`.
    Merge,
    /// `tag:yaml.org,2002:value`.
    Value,
    /// `tag:yaml.org,2002:str`.
    Str,
    /// `tag:yaml.org,2002:yaml` — PyYAML's "documentation only" resolver for `!`, `&` and `*` (`resolver.py:225`).
    /// A plain scalar cannot have these values, but a scalar with the non-specific tag `!` can (`! '*'`), and it
    /// then fails to construct (`could not determine a constructor for the tag 'tag:yaml.org,2002:yaml'`).
    Yaml,
}

impl ImplicitTag {
    /// The full tag string.
    #[must_use]
    pub const fn as_tag(self) -> &'static str {
        match self {
            Self::Null => NULL_TAG,
            Self::Bool => BOOL_TAG,
            Self::Int => INT_TAG,
            Self::Float => FLOAT_TAG,
            Self::Timestamp => TIMESTAMP_TAG,
            Self::Merge => MERGE_TAG,
            Self::Value => VALUE_TAG,
            Self::Str => STR_TAG,
            Self::Yaml => YAML_TAG,
        }
    }
}

/// `Resolver.resolve(ScalarNode, s, (True, False))`: bool/int/float/null/timestamp/merge/value/str.
///
/// * [`Schema::PyYaml11Safe`] is PyYAML 6.0.3's YAML 1.1 table (`resolver.py:170-228`): the resolvers registered
///   for the first character of `s` (the empty string uses the key `''`) are tried in registration order — bool,
///   float, int, merge, null, timestamp, value, yaml — and the first regex that matches wins; Python's `$` also
///   matches before a trailing `\n`. Quirks kept: `y`/`n` and mixed case (`yEs`) are strings, floats need a `.`
///   and a *signed* exponent (`1e3`, `1.0e3` are strings), `0o17` is a string, `09` is a string, `-.nan` is a
///   string, and `=` resolves to the `value` tag (which has no constructor).
/// * [`Schema::Yaml12Core`] is what lefthook's YAML parser (go-yaml v3 through koanf) makes of a plain scalar —
///   its `resolve()`, not the YAML 1.2 spec: only `true`/`True`/`TRUE`/`false`/… are booleans (`yes`/`on` are
///   strings), `null`/`Null`/`NULL`/`~`/empty is null, `<<` is a merge key; a text starting with a sign or digit
///   loses its `_` and is a Go base-0 int (`0777` is octal 511, `0b1`, `+0x1F`, `1_000` are ints; above `int64`
///   a `uint64`), else a float when it matches `[-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?` and is in
///   range (`08` is the float 8, `1e400` a string); a text starting with `.` is a float when `ParseFloat` takes it.
///   Everything else is a string — timestamps too: go-yaml returns a `time.Time` there, which lefthook's
///   weakly typed mapstructure decode rejects for its string fields; snp keeps the text instead of failing.
pub fn resolve_plain(s: &str, schema: Schema) -> ImplicitTag {
    match schema {
        Schema::PyYaml11Safe => resolve_py11(s),
        Schema::Yaml12Core => resolve_core12(s),
    }
}

/// Python `re.match(r'^(?:…)$', s)`: `$` matches at the end or before a final `\n`.
fn py_full_match(s: &str, f: impl Fn(&str) -> bool) -> bool {
    f(s) || s.strip_suffix('\n').is_some_and(f)
}

/// PyYAML's YAML 1.1 implicit resolvers, in registration order per first character.
fn resolve_py11(s: &str) -> ImplicitTag {
    let Some(first) = s.chars().next() else {
        // `yaml_implicit_resolvers['']` holds only the null resolver, whose regex matches ''.
        return ImplicitTag::Null;
    };
    if matches!(
        first,
        'y' | 'Y' | 'n' | 'N' | 't' | 'T' | 'f' | 'F' | 'o' | 'O'
    ) && py_full_match(s, is_bool11)
    {
        return ImplicitTag::Bool;
    }
    if matches!(first, '-' | '+' | '.' | '0'..='9') && py_full_match(s, is_float11) {
        return ImplicitTag::Float;
    }
    if matches!(first, '-' | '+' | '0'..='9') && py_full_match(s, is_int11) {
        return ImplicitTag::Int;
    }
    if first == '<' && py_full_match(s, |t| t == "<<") {
        return ImplicitTag::Merge;
    }
    if matches!(first, '~' | 'n' | 'N') && py_full_match(s, is_null11) {
        return ImplicitTag::Null;
    }
    if first.is_ascii_digit() && py_full_match(s, is_timestamp11) {
        return ImplicitTag::Timestamp;
    }
    if first == '=' && py_full_match(s, |t| t == "=") {
        return ImplicitTag::Value;
    }
    if matches!(first, '!' | '&' | '*') && py_full_match(s, |t| matches!(t, "!" | "&" | "*")) {
        return ImplicitTag::Yaml;
    }
    ImplicitTag::Str
}

/// `^(?:yes|Yes|YES|no|No|NO|true|True|TRUE|false|False|FALSE|on|On|ON|off|Off|OFF)$`.
fn is_bool11(t: &str) -> bool {
    matches!(
        t,
        "yes"
            | "Yes"
            | "YES"
            | "no"
            | "No"
            | "NO"
            | "true"
            | "True"
            | "TRUE"
            | "false"
            | "False"
            | "FALSE"
            | "on"
            | "On"
            | "ON"
            | "off"
            | "Off"
            | "OFF"
    )
}

/// `^(?: ~ |null|Null|NULL | )$` under `re.X`.
fn is_null11(t: &str) -> bool {
    matches!(t, "~" | "null" | "Null" | "NULL" | "")
}

/// Strips one optional leading `[-+]`.
fn strip_sign(t: &str) -> &str {
    t.strip_prefix(['-', '+']).unwrap_or(t)
}

/// `[0-9]`.
const fn digit(c: char) -> bool {
    c.is_ascii_digit()
}

/// `[0-9_]`.
const fn digit_(c: char) -> bool {
    c.is_ascii_digit() || c == '_'
}

/// One or more characters, all satisfying `f`.
fn plus(t: &str, f: impl Fn(char) -> bool) -> bool {
    !t.is_empty() && t.chars().all(f)
}

/// `[0-9][0-9_]*`.
fn head_digits(t: &str) -> bool {
    let mut cs = t.chars();
    cs.next().is_some_and(digit) && cs.all(digit_)
}

/// `[1-9][0-9_]*`.
fn head_nonzero(t: &str) -> bool {
    let mut cs = t.chars();
    cs.next().is_some_and(|c| matches!(c, '1'..='9')) && cs.all(digit_)
}

/// One sexagesimal part after a `:`: `[0-5]?[0-9]`.
fn sexagesimal_part(p: &str) -> bool {
    let b = p.as_bytes();
    match b {
        [d] => d.is_ascii_digit(),
        [h, d] => matches!(h, b'0'..=b'5') && d.is_ascii_digit(),
        _ => false,
    }
}

/// `<head>(?::[0-5]?[0-9])+` where `head` checks the part before the first `:`.
fn sexagesimal(t: &str, head: impl Fn(&str) -> bool) -> bool {
    let mut parts = t.split(':');
    let Some(first) = parts.next() else {
        return false;
    };
    let mut any = false;
    for p in parts {
        if !sexagesimal_part(p) {
            return false;
        }
        any = true;
    }
    any && head(first)
}

/// `(?:[eE][-+][0-9]+)?` — empty, or a *signed* exponent.
fn opt_signed_exponent(t: &str) -> bool {
    if t.is_empty() {
        return true;
    }
    let Some(rest) = t.strip_prefix(['e', 'E']) else {
        return false;
    };
    let Some(digits) = rest.strip_prefix(['-', '+']) else {
        return false;
    };
    plus(digits, digit)
}

/// The float regex of `resolver.py:181-188`:
/// `[-+]?(?:[0-9][0-9_]*)\.[0-9_]*(?:[eE][-+][0-9]+)?` | `\.[0-9][0-9_]*(?:[eE][-+][0-9]+)?` |
/// `[-+]?[0-9][0-9_]*(?::[0-5]?[0-9])+\.[0-9_]*` | `[-+]?\.(?:inf|Inf|INF)` | `\.(?:nan|NaN|NAN)`.
fn is_float11(t: &str) -> bool {
    if matches!(t, ".nan" | ".NaN" | ".NAN") {
        return true;
    }
    let unsigned = strip_sign(t);
    if matches!(unsigned, ".inf" | ".Inf" | ".INF") {
        return true;
    }
    // Alternative 2: no sign, starts with '.'.
    if let Some(rest) = t.strip_prefix('.') {
        let end = rest.find(['e', 'E']).unwrap_or(rest.len());
        let (mantissa, exp) = rest.split_at_checked(end).unwrap_or((rest, ""));
        if head_digits(mantissa) && opt_signed_exponent(exp) {
            return true;
        }
    }
    // Alternatives 1 and 3: an integral head, a '.', then `[0-9_]*` (+ exponent for alternative 1).
    let Some((int_part, frac)) = unsigned.split_once('.') else {
        return false;
    };
    if int_part.contains(':') {
        return sexagesimal(int_part, head_digits) && frac.chars().all(digit_);
    }
    let end = frac.find(['e', 'E']).unwrap_or(frac.len());
    let (digits, exp) = frac.split_at_checked(end).unwrap_or((frac, ""));
    head_digits(int_part) && digits.chars().all(digit_) && opt_signed_exponent(exp)
}

/// The int regex of `resolver.py:190-196`: `[-+]?0b[0-1_]+` | `[-+]?0[0-7_]+` | `[-+]?(?:0|[1-9][0-9_]*)` |
/// `[-+]?0x[0-9a-fA-F_]+` | `[-+]?[1-9][0-9_]*(?::[0-5]?[0-9])+`.
fn is_int11(t: &str) -> bool {
    let u = strip_sign(t);
    if let Some(bits) = u.strip_prefix("0b")
        && plus(bits, |c| matches!(c, '0' | '1' | '_'))
    {
        return true;
    }
    if let Some(oct) = u.strip_prefix('0')
        && plus(oct, |c| matches!(c, '0'..='7' | '_'))
    {
        return true;
    }
    if u == "0" || head_nonzero(u) {
        return true;
    }
    if let Some(hex) = u.strip_prefix("0x")
        && plus(hex, |c| c.is_ascii_hexdigit() || c == '_')
    {
        return true;
    }
    sexagesimal(u, head_nonzero)
}

/// Exactly `n` ASCII digits.
fn digits_n(t: &str, n: usize) -> bool {
    t.len() == n && t.bytes().all(|b| b.is_ascii_digit())
}

/// Splits 1 or 2 leading ASCII digits off `t` (greedy; the regexes' `[0-9][0-9]?` is always followed by a
/// non-digit or the end, so greedy matching never needs to backtrack).
fn split_1_or_2_digits(t: &str) -> Option<(&str, &str)> {
    let n = t.bytes().take(2).take_while(u8::is_ascii_digit).count();
    if n == 0 { None } else { t.split_at_checked(n) }
}

/// The timestamp regex of `resolver.py:212-219`:
/// `[0-9]{4}-[0-9]{2}-[0-9]{2}` | `[0-9]{4}-[0-9][0-9]?-[0-9][0-9]?(?:[Tt]|[ \t]+)[0-9][0-9]?:[0-9]{2}:[0-9]{2}`
/// `(?:\.[0-9]*)?(?:[ \t]*(?:Z|[-+][0-9][0-9]?(?::[0-9][0-9])?))?` (written with `{n}` for brevity).
pub(crate) fn is_timestamp11(t: &str) -> bool {
    let b = t.as_bytes();
    if b.len() == 10
        && b.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
    {
        return true;
    }
    timestamp_parts(t, true).is_some()
}

/// The groups of a timestamp (resolver or constructor regex; `require_time` selects the resolver's second
/// alternative, which needs the time part).
#[derive(Debug, Clone, Copy)]
pub(crate) struct TimestampParts<'a> {
    pub(crate) year: &'a str,
    pub(crate) month: &'a str,
    pub(crate) day: &'a str,
    pub(crate) time: Option<TimeParts<'a>>,
}

/// The time groups of a timestamp.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TimeParts<'a> {
    pub(crate) hour: &'a str,
    pub(crate) minute: &'a str,
    pub(crate) second: &'a str,
    /// `(?P<fraction>[0-9]*)`, `None` when there is no `.`.
    pub(crate) fraction: Option<&'a str>,
    pub(crate) tz: Option<Tz<'a>>,
}

/// The `tz` group.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Tz<'a> {
    /// `Z`.
    Utc,
    /// `[-+]HH(:MM)?`.
    Offset {
        negative: bool,
        hour: &'a str,
        minute: Option<&'a str>,
    },
}

/// Matches `[0-9]{4}-[0-9][0-9]?-[0-9][0-9]?` + (time part when present) + end, without the trailing-newline
/// allowance (the callers apply it). With `require_time`, the time part is mandatory.
fn timestamp_parts(t: &str, require_time: bool) -> Option<TimestampParts<'_>> {
    let (year, rest) = t.split_at_checked(4)?;
    if !digits_n(year, 4) {
        return None;
    }
    let rest = rest.strip_prefix('-')?;
    let (month, rest) = split_1_or_2_digits(rest)?;
    let rest = rest.strip_prefix('-')?;
    let (day, rest) = split_1_or_2_digits(rest)?;
    if rest.is_empty() {
        return if require_time {
            None
        } else {
            Some(TimestampParts {
                year,
                month,
                day,
                time: None,
            })
        };
    }
    // (?:[Tt]|[ \t]+)
    let rest = if let Some(r) = rest.strip_prefix(['T', 't']) {
        r
    } else {
        let r = rest.trim_start_matches([' ', '\t']);
        if r.len() == rest.len() {
            return None;
        }
        r
    };
    let (hour, rest) = split_1_or_2_digits(rest)?;
    let rest = rest.strip_prefix(':')?;
    let (minute, rest) = rest.split_at_checked(2)?;
    if !digits_n(minute, 2) {
        return None;
    }
    let rest = rest.strip_prefix(':')?;
    let (second, rest) = rest.split_at_checked(2)?;
    if !digits_n(second, 2) {
        return None;
    }
    let (fraction, rest) = match rest.strip_prefix('.') {
        Some(r) => {
            let n = r.bytes().take_while(u8::is_ascii_digit).count();
            let (f, r) = r.split_at_checked(n)?;
            (Some(f), r)
        }
        None => (None, rest),
    };
    let tz = if rest.is_empty() {
        None
    } else {
        let r = rest.trim_start_matches([' ', '\t']);
        if r == "Z" {
            Some(Tz::Utc)
        } else {
            let (negative, r) = if let Some(r) = r.strip_prefix('-') {
                (true, r)
            } else {
                (false, r.strip_prefix('+')?)
            };
            let (hour, r) = split_1_or_2_digits(r)?;
            let minute = if r.is_empty() {
                None
            } else {
                let m = r.strip_prefix(':')?;
                if !digits_n(m, 2) {
                    return None;
                }
                Some(m)
            };
            Some(Tz::Offset {
                negative,
                hour,
                minute,
            })
        }
    };
    Some(TimestampParts {
        year,
        month,
        day,
        time: Some(TimeParts {
            hour,
            minute,
            second,
            fraction,
            tz,
        }),
    })
}

/// `SafeConstructor.timestamp_regexp.match(value)` (`constructor.py:306-316`, `re.X`): like the resolver's second
/// alternative with an optional time part, and with Python's `$` (a final `\n` is allowed).
pub(crate) fn match_timestamp(value: &str) -> Option<TimestampParts<'_>> {
    timestamp_parts(value, false).or_else(|| {
        value
            .strip_suffix('\n')
            .and_then(|v| timestamp_parts(v, false))
    })
}

/// The YAML 1.2 core schema (+ the `<<` merge key).
fn resolve_core12(s: &str) -> ImplicitTag {
    // `resolve()` makes `<<` a string (`<` has no resolver hint); go-yaml's decoder treats a plain `<<` key as a
    // merge key (`isMerge`), and the constructor reads `<<` elsewhere as the string.
    if s == "<<" {
        return ImplicitTag::Merge;
    }
    match go_resolve(s) {
        GoScalar::Null => ImplicitTag::Null,
        GoScalar::Bool(_) => ImplicitTag::Bool,
        GoScalar::Int(_) | GoScalar::Uint(_) => ImplicitTag::Int,
        GoScalar::Float(_) => ImplicitTag::Float,
        GoScalar::Str => ImplicitTag::Str,
    }
}

/// What go-yaml v3's `resolve(tag, in)` (`resolve.go:113-265`) makes of a scalar's text.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) enum GoScalar {
    /// `!!null`.
    Null,
    /// `!!bool`.
    Bool(bool),
    /// `!!int` that fits an `int64`.
    Int(i64),
    /// `!!int` above `int64` (a Go `uint64`).
    Uint(u64),
    /// `!!float`.
    Float(f64),
    /// `!!str` (timestamps included, see [`resolve_plain`]).
    Str,
}

impl GoScalar {
    /// go-yaml's short tag of the result (`!!int`, …) for its `cannot decode` message.
    pub(crate) const fn short_tag(self) -> &'static str {
        match self {
            Self::Null => "!!null",
            Self::Bool(_) => "!!bool",
            Self::Int(_) | Self::Uint(_) => "!!int",
            Self::Float(_) => "!!float",
            Self::Str => "!!str",
        }
    }
}

/// go-yaml v3's `resolve("", in)` minus timestamps: the exact-match table (`true`/`True`/`TRUE`, `false`/…,
/// `""`/`~`/`null`/`Null`/`NULL`, `±.inf` in three spellings, `.nan`/`.NaN`/`.NAN`; its `<<` entry is unreachable
/// because `<` has no hint), then by the first
/// character: `.` → `strconv.ParseFloat`; a sign or digit → `_` removed, `strconv.ParseInt(_, 0, 64)` (sign,
/// `0x`/`0o`/`0b` in any case or a leading `0` for octal), `ParseUint(_, 0, 64)`, the `yamlStyleFloat` regex with
/// `ParseFloat` (out of range stays a string), then `0b…`/`-0b…`/`0o…`/`-0o…` with a sign after the prefix; any
/// other first character is a string.
pub(crate) fn go_resolve(s: &str) -> GoScalar {
    match s {
        "true" | "True" | "TRUE" => return GoScalar::Bool(true),
        "false" | "False" | "FALSE" => return GoScalar::Bool(false),
        "" | "~" | "null" | "Null" | "NULL" => return GoScalar::Null,
        ".nan" | ".NaN" | ".NAN" => return GoScalar::Float(f64::NAN),
        ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" => {
            return GoScalar::Float(f64::INFINITY);
        }
        "-.inf" | "-.Inf" | "-.INF" => return GoScalar::Float(f64::NEG_INFINITY),
        _ => {}
    }
    match s.as_bytes().first() {
        Some(b'.') => go_parse_float(s).map_or(GoScalar::Str, GoScalar::Float),
        Some(b'+' | b'-' | b'0'..=b'9') => go_resolve_number(s),
        _ => GoScalar::Str,
    }
}

/// The sign-or-digit branch of go-yaml's `resolve` (after the timestamp attempt, which never matches a text that
/// one of these parses accepts).
fn go_resolve_number(s: &str) -> GoScalar {
    let plain: String = s.chars().filter(|c| *c != '_').collect();
    if let Some(i) = go_parse_int(&plain, 0) {
        return GoScalar::Int(i);
    }
    if let Some(u) = go_parse_uint(&plain, 0) {
        return GoScalar::Uint(u);
    }
    if yaml_style_float(&plain)
        && let Some(f) = go_parse_float(&plain)
    {
        return GoScalar::Float(f);
    }
    if let Some(rest) = plain.strip_prefix("0b") {
        if let Some(i) = go_parse_int(rest, 2) {
            return GoScalar::Int(i);
        }
        if let Some(u) = go_parse_uint(rest, 2) {
            return GoScalar::Uint(u);
        }
    } else if let Some(rest) = plain.strip_prefix("-0b")
        && let Some(i) = go_parse_int(&format!("-{rest}"), 2)
    {
        return GoScalar::Int(i);
    }
    if let Some(rest) = plain.strip_prefix("0o") {
        if let Some(i) = go_parse_int(rest, 8) {
            return GoScalar::Int(i);
        }
        if let Some(u) = go_parse_uint(rest, 8) {
            return GoScalar::Uint(u);
        }
    } else if let Some(rest) = plain.strip_prefix("-0o")
        && let Some(i) = go_parse_int(&format!("-{rest}"), 8)
    {
        return GoScalar::Int(i);
    }
    GoScalar::Str
}

/// `strconv.ParseUint(s, base, 64)` for a text without `_` (`base` 0 detects `0b`/`0o`/`0x` when at least one
/// character follows, else a leading `0` means octal).
fn go_parse_uint(s: &str, base: u32) -> Option<u64> {
    if s.is_empty() {
        return None;
    }
    let b = s.as_bytes();
    let (radix, digits) = if base != 0 {
        (base, b)
    } else if let [b'0', x, rest @ ..] = b {
        match (x.to_ascii_lowercase(), rest.is_empty()) {
            (b'b', false) => (2, rest),
            (b'o', false) => (8, rest),
            (b'x', false) => (16, rest),
            _ => (8, b.get(1..).unwrap_or_default()),
        }
    } else {
        (10, b)
    };
    let mut n: u64 = 0;
    for c in digits {
        let d = char::from(*c).to_digit(36)?;
        if d >= radix {
            return None;
        }
        n = n.checked_mul(u64::from(radix))?.checked_add(u64::from(d))?;
    }
    Some(n)
}

/// `strconv.ParseInt(s, base, 64)` for a text without `_`: one optional sign, then [`go_parse_uint`] in range.
fn go_parse_int(s: &str, base: u32) -> Option<i64> {
    let (neg, rest) = match s.as_bytes().first() {
        Some(b'+') => (false, s.get(1..)?),
        Some(b'-') => (true, s.get(1..)?),
        _ => (false, s),
    };
    let un = go_parse_uint(rest, base)?;
    if neg {
        0i64.checked_sub_unsigned(un)
    } else {
        i64::try_from(un).ok()
    }
}

/// `^[-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?$` (go-yaml's `yamlStyleFloat`).
fn yaml_style_float(s: &str) -> bool {
    let u = strip_sign(s);
    let end = u.find(['e', 'E']).unwrap_or(u.len());
    let (mantissa, exp) = u.split_at_checked(end).unwrap_or((u, ""));
    let mantissa_ok = match mantissa.split_once('.') {
        Some(("", frac)) => plus(frac, digit),
        Some((int, frac)) => plus(int, digit) && frac.chars().all(digit),
        None => plus(mantissa, digit),
    };
    let exp_ok = exp.is_empty()
        || exp
            .get(1..)
            .is_some_and(|e| plus(e.strip_prefix(['-', '+']).unwrap_or(e), digit));
    mantissa_ok && exp_ok
}

/// `strconv.ParseFloat(s, 64)` on a decimal text (the only kind go-yaml hands it here: the `.` branch and texts
/// that match `yamlStyleFloat`): `readFloat`'s grammar — sign, digits and `_` with at most one `.`, at least one
/// digit, an optional `e`/`E` exponent with a sign and digits or `_` — with `underscoreOK`'s rule that every `_`
/// sits between digits; an out-of-range result (`ErrRange`) is `None`.
fn go_parse_float(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    let mut i = usize::from(matches!(b.first(), Some(b'+' | b'-')));
    let mut saw_dot = false;
    let mut saw_digits = false;
    while let Some(c) = b.get(i) {
        match c {
            b'_' => {}
            b'.' if !saw_dot => saw_dot = true,
            b'0'..=b'9' => saw_digits = true,
            _ => break,
        }
        i = i.saturating_add(1);
    }
    if !saw_digits {
        return None;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i = i.saturating_add(1);
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i = i.saturating_add(1);
        }
        if !b.get(i).is_some_and(u8::is_ascii_digit) {
            return None;
        }
        while b.get(i).is_some_and(|c| c.is_ascii_digit() || *c == b'_') {
            i = i.saturating_add(1);
        }
    }
    if i != b.len() || !go_underscore_ok(s) {
        return None;
    }
    let cleaned: String = s.chars().filter(|c| *c != '_').collect();
    let x = cleaned.parse::<f64>().ok()?;
    x.is_finite().then_some(x)
}

/// `strconv.underscoreOK` for a decimal text: each `_` follows a digit and is followed by a digit.
fn go_underscore_ok(s: &str) -> bool {
    // ^ start, 0 digit, _ underscore, ! anything else.
    let mut saw = b'^';
    for c in strip_sign(s).bytes() {
        if c.is_ascii_digit() {
            saw = b'0';
        } else if c == b'_' {
            if saw != b'0' {
                return false;
            }
            saw = b'_';
        } else {
            if saw == b'_' {
                return false;
            }
            saw = b'!';
        }
    }
    saw != b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn py(s: &str) -> ImplicitTag {
        resolve_plain(s, Schema::PyYaml11Safe)
    }

    #[test]
    fn verified_table_01_12_2() {
        for s in [
            "yes", "Yes", "YES", "on", "On", "ON", "true", "True", "TRUE",
        ] {
            assert_eq!(py(s), ImplicitTag::Bool, "{s}");
        }
        for s in [
            "no", "No", "NO", "off", "Off", "OFF", "false", "False", "FALSE",
        ] {
            assert_eq!(py(s), ImplicitTag::Bool, "{s}");
        }
        for s in ["y", "n", "Y", "N", "yEs", "tRue"] {
            assert_eq!(py(s), ImplicitTag::Str, "{s}");
        }
        for s in ["~", "null", "Null", "NULL", ""] {
            assert_eq!(py(s), ImplicitTag::Null, "{s:?}");
        }
        for s in [
            "010",
            "0700",
            "0_1",
            "0x1F",
            "0b101",
            "1_000",
            "+12",
            "-0",
            "1__0",
            "1:30",
            "190:20:30",
            "-1:30",
        ] {
            assert_eq!(py(s), ImplicitTag::Int, "{s}");
        }
        for s in [
            "0800", "09", "0o10", "1:60", "1e3", "1.0e3", "1e3456", "123e4567", "-.NaN",
        ] {
            assert_eq!(py(s), ImplicitTag::Str, "{s}");
        }
        for s in [
            "1.10",
            "1.0",
            ".5",
            "0.",
            "1.e+3",
            "1_0.5",
            "6.8523015e+5",
            "12:30:45.5",
            ".inf",
            "+.inf",
            "-.inf",
            ".Inf",
            ".INF",
            ".nan",
            ".NaN",
            ".NAN",
        ] {
            assert_eq!(py(s), ImplicitTag::Float, "{s}");
        }
        for s in ["1.2.3", "v1.10", "3.2.0", "2002-1-1", "-.5"] {
            assert_eq!(py(s), ImplicitTag::Str, "{s}");
        }
        for s in [
            "2020-01-01",
            "2001-12-14t21:59:43.10-05:00",
            "2001-12-14 21:59:43.10 -5",
            "2002-01-01T00:00:00Z",
            "2020-13-45",
        ] {
            assert_eq!(py(s), ImplicitTag::Timestamp, "{s}");
        }
        assert_eq!(py("="), ImplicitTag::Value);
        assert_eq!(py("<<"), ImplicitTag::Merge);
        for s in ["!", "&", "*"] {
            assert_eq!(py(s), ImplicitTag::Yaml);
        }
        // Python's `$` before a final newline.
        assert_eq!(py("1\n"), ImplicitTag::Int);
        assert_eq!(py("1\n\n"), ImplicitTag::Str);
        assert_eq!(py("\n"), ImplicitTag::Str);
    }

    /// go-yaml v3 `resolve()` vectors (`tests/vectors/core12.json` has the full oracle table).
    #[test]
    fn core_schema() {
        let core = |s| resolve_plain(s, Schema::Yaml12Core);
        for s in [
            "yes",
            "on",
            "off",
            "y",
            "2020-01-01",
            "1:30",
            ".5e",
            "1e400",
            "_1",
            "+.nan",
            "0b2",
            "0x",
            "=",
        ] {
            assert_eq!(core(s), ImplicitTag::Str, "{s}");
        }
        for s in ["true", "False", "TRUE"] {
            assert_eq!(core(s), ImplicitTag::Bool, "{s}");
        }
        for s in ["", "~", "null"] {
            assert_eq!(core(s), ImplicitTag::Null, "{s:?}");
        }
        for s in [
            "0777",
            "-12",
            "0o17",
            "0x1F",
            "+0",
            "1_000",
            "0b1",
            "-0b1",
            "+0x1F",
            "1__0",
            "1_",
            "0x_1",
            "0b-1",
            "18446744073709551615",
        ] {
            assert_eq!(core(s), ImplicitTag::Int, "{s}");
        }
        for s in [
            "1e3",
            "1.5",
            ".5",
            "-.5",
            "1.",
            "+.inf",
            ".NaN",
            "1E+3",
            "08",
            "09",
            "1_0.5",
            "18446744073709551616",
            ".5_0",
        ] {
            assert_eq!(core(s), ImplicitTag::Float, "{s}");
        }
        assert_eq!(core("<<"), ImplicitTag::Merge);
        assert_eq!(go_resolve("0777"), GoScalar::Int(511));
        assert_eq!(go_resolve("-0x1F"), GoScalar::Int(-31));
        assert_eq!(go_resolve("08"), GoScalar::Float(8.0));
        assert_eq!(go_resolve("9223372036854775808"), GoScalar::Uint(1 << 63));
        assert_eq!(
            go_resolve("-9223372036854775809"),
            GoScalar::Float(-9.223_372_036_854_776e18)
        );
        assert_eq!(go_resolve("<<"), GoScalar::Str);
    }
}
