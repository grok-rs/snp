//! lefthook glob patterns → Python `re` / POSIX ERE (05 §19.6).
//!
//! lefthook matches `glob` and `exclude` with gobwas/glob (`glob_matcher: gobwas`, the default: compiled without
//! separators, so `*`, `**` and `?` also match `/`) or with bmatcuk/doublestar (`glob_matcher: doublestar`:
//! path-aware). `glob` lowercases both the pattern and the path (`filter.go:77-133`); `exclude` is case-sensitive.
//! The translation parses a pattern once into a small tree and renders it for the requested regex flavour: the
//! Python form becomes a pre-commit `files` / `exclude` regex (validated by `snp_pyre::validate`), the ERE form a
//! `grep -E` filter inside a shell wrapper (rule R5).
//!
//! Owner: WP-34.

use std::fmt::Write as _;

/// The matcher of lefthook's `glob_matcher` option.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Matcher {
    /// gobwas/glob without separators (lefthook's default).
    Gobwas,
    /// bmatcuk/doublestar `Match`.
    Doublestar,
}

/// The regex dialect to render.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Flavor {
    /// Python `re` (also valid for the Rust `regex` crate): `(?:…)` groups, `re.escape` escaping.
    Python,
    /// POSIX extended regular expressions (`grep -E`): `(…)` groups, bracket expressions without escapes.
    Ere,
}

/// Why a pattern cannot be translated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GlobError(pub(crate) String);

/// The deepest `{…}` nesting accepted (gobwas has no limit; this bounds the recursion).
const MAX_NESTING: usize = 32;

/// One parsed token.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    /// A literal character.
    Lit(char),
    /// A run of `n` stars.
    Star(usize),
    /// `?`.
    Any,
    /// `[…]`: negated, ranges (single characters are `(c, c)`).
    Class(bool, Vec<(char, char)>),
    /// `{a,b,…}`: the alternatives.
    Alt(Vec<Vec<Self>>),
}

/// A recursive-descent parser over the pattern's characters (doublestar).
struct Parser<'a> {
    chars: &'a [char],
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos = self.pos.saturating_add(1);
        Some(c)
    }

    /// A sequence up to the end, or (inside braces) up to an unconsumed `,` or `}`.
    fn seq(&mut self, depth: usize) -> Result<Vec<Tok>, GlobError> {
        let mut out = Vec::new();
        while let Some(c) = self.peek() {
            match c {
                ',' | '}' if depth > 0 => break,
                '\\' => {
                    self.bump();
                    match self.bump() {
                        Some(e) => out.push(Tok::Lit(e)),
                        None => return Err(GlobError("unclosed escape at the end".to_owned())),
                    }
                }
                '*' => {
                    let mut n = 0_usize;
                    while self.peek() == Some('*') {
                        self.bump();
                        n = n.saturating_add(1);
                    }
                    out.push(Tok::Star(n));
                }
                '?' => {
                    self.bump();
                    out.push(Tok::Any);
                }
                '[' => {
                    self.bump();
                    out.push(self.class()?);
                }
                '{' => {
                    self.bump();
                    if depth >= MAX_NESTING {
                        return Err(GlobError("braces are nested too deeply".to_owned()));
                    }
                    let mut alts = vec![self.seq(depth.saturating_add(1))?];
                    loop {
                        match self.bump() {
                            Some(',') => alts.push(self.seq(depth.saturating_add(1))?),
                            Some('}') => break,
                            _ => return Err(GlobError("unclosed '{'".to_owned())),
                        }
                    }
                    out.push(Tok::Alt(alts));
                }
                _ => {
                    self.bump();
                    out.push(Tok::Lit(c));
                }
            }
        }
        Ok(out)
    }

    /// After `[` (doublestar): `!` or `^` negates; `a-z` ranges; `\x` escapes; `]` closes.
    fn class(&mut self) -> Result<Tok, GlobError> {
        let unclosed = || GlobError("unclosed '['".to_owned());
        let negated = self.peek() == Some('!') || self.peek() == Some('^');
        if negated {
            self.bump();
        }
        let mut items = Vec::new();
        loop {
            let c = self.bump().ok_or_else(unclosed)?;
            let lo = match c {
                ']' if !items.is_empty() => break,
                ']' => return Err(GlobError("empty character class".to_owned())),
                '\\' => self.bump().ok_or_else(unclosed)?,
                c => c,
            };
            let mut hi = lo;
            if self.peek() == Some('-') && self.chars.get(self.pos.saturating_add(1)) != Some(&']')
            {
                self.bump();
                hi = match self.bump().ok_or_else(unclosed)? {
                    '\\' => self.bump().ok_or_else(unclosed)?,
                    c => c,
                };
                if hi < lo {
                    return Err(GlobError(format!("invalid range {lo}-{hi}")));
                }
            }
            items.push((lo, hi));
        }
        Ok(Tok::Class(negated, items))
    }
}

/// Parses `pattern` (already lowercased by the caller for `glob`).
fn parse(pattern: &str, matcher: Matcher) -> Result<Vec<Tok>, GlobError> {
    let chars: Vec<char> = pattern.chars().collect();
    if chars.contains(&'\0') {
        // gobwas's lexer reads NUL as the end of input; never guess what such a pattern means.
        return Err(GlobError("NUL character in the pattern".to_owned()));
    }
    if matcher == Matcher::Gobwas {
        return Gobwas {
            chars: &chars,
            pos: 0,
        }
        .pattern();
    }
    let mut p = Parser {
        chars: &chars,
        pos: 0,
    };
    let seq = p.seq(0)?;
    if p.pos < chars.len() {
        // A `,` or `}` outside braces is literal text; `seq(0)` never stops on them.
        return Err(GlobError("unbalanced '}'".to_owned()));
    }
    Ok(seq)
}

/// gobwas/glob v0.2.3's lexer and parser (`syntax/lexer/lexer.go`, `syntax/ast/parser.go`), followed exactly:
///
/// * text runs stop at `?`, `*`, `[`, `{` (and, inside braces, `,` and `}`); `\x` is a literal `x`, a trailing `\`
///   is dropped; `}` and `,` outside braces are literal;
/// * `[` starts either exactly one range `[lo-hi]` / `[!lo-hi]` (`lo` and `hi` taken raw, then `]` is required) or a
///   literal character list `[abc]` / `[!abc]` (escapes allowed, `-` literal): `[ab-z]` is {a, b, -, z} and
///   `[a-z0-9]`, `[a-]`, `[]` are compile errors (lefthook's `glob.MustCompile` panics on them);
/// * an unclosed `{` is closed at the end of the pattern.
struct Gobwas<'a> {
    chars: &'a [char],
    pos: usize,
}

impl Gobwas<'_> {
    fn peek_at(&self, k: usize) -> Option<char> {
        self.chars.get(self.pos.saturating_add(k)).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek_at(0)?;
        self.pos = self.pos.saturating_add(1);
        Some(c)
    }

    /// `fetchText`: escaped text up to an unescaped breaker (not consumed) or the end.
    fn text(&mut self, breakers: &[char]) -> Vec<char> {
        let mut data = Vec::new();
        while let Some(c) = self.peek_at(0) {
            if c == '\\' {
                self.bump();
                if let Some(e) = self.bump() {
                    data.push(e);
                }
                continue;
            }
            if breakers.contains(&c) {
                break;
            }
            self.bump();
            data.push(c);
        }
        data
    }

    /// `fetchRange` + `parserRange`, after the `[`.
    fn range(&mut self) -> Result<Tok, GlobError> {
        let eof = || GlobError("unclosed '['".to_owned());
        let not = self.peek_at(0) == Some('!');
        if not {
            self.bump();
        }
        let first = self.peek_at(0).ok_or_else(eof)?;
        if self.peek_at(1) == Some('-') {
            self.pos = self.pos.saturating_add(2);
            let hi = self.bump().ok_or_else(eof)?;
            match self.bump() {
                Some(']') => {}
                Some(_) => return Err(GlobError("expected ']' after a range".to_owned())),
                None => return Err(eof()),
            }
            if hi < first {
                return Err(GlobError(format!("invalid range {first}-{hi}")));
            }
            return Ok(Tok::Class(not, vec![(first, hi)]));
        }
        let list = self.text(&[']']);
        if self.bump() != Some(']') {
            return Err(eof());
        }
        if list.is_empty() {
            return Err(GlobError("empty character class".to_owned()));
        }
        Ok(Tok::Class(not, list.into_iter().map(|c| (c, c)).collect()))
    }

    /// The whole pattern: a stack of open brace groups (`{` … `,` … `}`).
    fn pattern(&mut self) -> Result<Vec<Tok>, GlobError> {
        const TEXT: [char; 4] = ['?', '*', '[', '{'];
        const TERMS: [char; 6] = ['?', '*', '[', '{', '}', ','];
        // Each open group: the alternatives finished so far; `cur` is the sequence being built.
        let mut groups: Vec<(Vec<Tok>, Vec<Vec<Tok>>)> = Vec::new();
        let mut cur: Vec<Tok> = Vec::new();
        while let Some(c) = self.peek_at(0) {
            match c {
                '{' => {
                    self.bump();
                    if groups.len() >= MAX_NESTING {
                        return Err(GlobError("braces are nested too deeply".to_owned()));
                    }
                    groups.push((std::mem::take(&mut cur), Vec::new()));
                }
                ',' if !groups.is_empty() => {
                    self.bump();
                    if let Some((_, alts)) = groups.last_mut() {
                        alts.push(std::mem::take(&mut cur));
                    }
                }
                '}' if !groups.is_empty() => {
                    self.bump();
                    if let Some((outer, mut alts)) = groups.pop() {
                        alts.push(std::mem::take(&mut cur));
                        cur = outer;
                        cur.push(alt(alts)?);
                    }
                }
                '[' => {
                    self.bump();
                    cur.push(self.range()?);
                }
                '?' => {
                    self.bump();
                    cur.push(Tok::Any);
                }
                '*' => {
                    self.bump();
                    let n = if self.peek_at(0) == Some('*') {
                        self.bump();
                        2
                    } else {
                        1
                    };
                    cur.push(Tok::Star(n));
                }
                _ => {
                    let breakers: &[char] = if groups.is_empty() { &TEXT } else { &TERMS };
                    cur.extend(self.text(breakers).into_iter().map(Tok::Lit));
                }
            }
        }
        while let Some((outer, mut alts)) = groups.pop() {
            alts.push(std::mem::take(&mut cur));
            cur = outer;
            cur.push(alt(alts)?);
        }
        Ok(cur)
    }
}

/// A finished gobwas brace group. One whose alternatives are all empty (`{}`, `{,}`) is refused: gobwas v0.2.3's
/// compiler handles it inconsistently (`x{}` panics, `{}x` matches nothing, `{}*` matches everything), so no regex
/// can stand for it.
fn alt(alts: Vec<Vec<Tok>>) -> Result<Tok, GlobError> {
    if alts.iter().all(Vec::is_empty) {
        return Err(GlobError(
            "a brace group with only empty alternatives ('{}', '{,}') has no consistent gobwas meaning".to_owned(),
        ));
    }
    Ok(Tok::Alt(alts))
}

/// Python's `re.escape` of one character.
fn py_escape(c: char, out: &mut String) {
    if "()[]{}?*+-|^$\\.&~# \t\n\r\u{b}\u{c}".contains(c) {
        out.push('\\');
    }
    out.push(c);
}

/// `s` as a POSIX ERE literal.
pub(crate) fn ere_literal(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        ere_escape(c, &mut out);
    }
    out
}

/// A POSIX ERE escape of one character outside brackets.
fn ere_escape(c: char, out: &mut String) {
    if ".[]()*+?{}|^$\\".contains(c) {
        out.push('\\');
    }
    out.push(c);
}

/// A class member for Python: ASCII punctuation escaped (never an ASCII letter or digit).
fn py_class_char(c: char, out: &mut String) {
    if c.is_ascii_punctuation() || c.is_ascii_whitespace() {
        out.push('\\');
    }
    out.push(c);
}

/// Renders a bracket expression.
fn class(
    neg: bool,
    items: &[(char, char)],
    flavor: Flavor,
    no_sep: bool,
    out: &mut String,
) -> Result<(), GlobError> {
    match flavor {
        Flavor::Python => {
            out.push('[');
            if neg {
                out.push('^');
                if no_sep {
                    out.push('/');
                }
            }
            for &(lo, hi) in items {
                py_class_char(lo, out);
                if hi != lo {
                    out.push('-');
                    py_class_char(hi, out);
                }
            }
            out.push(']');
        }
        Flavor::Ere => {
            // POSIX brackets: no escapes; `]` first, `-` last, `^` never first, and `[` only right before `^`, `-` or
            // the closing `]` (so it never opens a `[:`, `[=` or `[.` class).
            let mut close = false;
            let mut dash = false;
            let mut caret = false;
            let mut open = false;
            let mut body = String::new();
            for &(lo, hi) in items {
                if lo == hi {
                    match lo {
                        ']' => close = true,
                        '-' => dash = true,
                        '^' => caret = true,
                        '[' => open = true,
                        c => body.push(c),
                    }
                } else if lo.is_ascii_alphanumeric() && hi.is_ascii_alphanumeric() {
                    body.push(lo);
                    body.push('-');
                    body.push(hi);
                } else {
                    return Err(GlobError(format!(
                        "the range {lo}-{hi} has no shell (grep -E) equivalent"
                    )));
                }
            }
            if !neg && !close && !dash && !open && body.is_empty() && caret {
                out.push_str("\\^");
                return Ok(());
            }
            out.push('[');
            if neg {
                out.push('^');
                if no_sep {
                    out.push('/');
                }
            }
            if close {
                out.push(']');
            }
            out.push_str(&body);
            if open {
                out.push('[');
            }
            if caret {
                out.push('^');
            }
            if dash {
                out.push('-');
            }
            out.push(']');
        }
    }
    Ok(())
}

/// Renders a sequence. `top` = the sequence is the whole pattern (a trailing `/**` may only end the pattern).
fn render(
    toks: &[Tok],
    m: Matcher,
    f: Flavor,
    top: bool,
    out: &mut String,
) -> Result<(), GlobError> {
    let group_open = match f {
        Flavor::Python => "(?:",
        Flavor::Ere => "(",
    };
    let mut i = 0_usize;
    while let Some(t) = toks.get(i) {
        let next = toks.get(i.saturating_add(1));
        let at_segment_start = i == 0 || matches!(toks.get(i.wrapping_sub(1)), Some(Tok::Lit('/')));
        match t {
            Tok::Lit('/')
                if m == Matcher::Doublestar
                    && top
                    && matches!(next, Some(Tok::Star(n)) if *n >= 2)
                    && i.saturating_add(2) == toks.len() =>
            {
                // A trailing `/**`: the directory itself and everything below it.
                out.push_str(group_open);
                out.push_str("/.*)?");
                i = i.saturating_add(2);
                continue;
            }
            Tok::Lit(c) => match f {
                Flavor::Python => py_escape(*c, out),
                Flavor::Ere => ere_escape(*c, out),
            },
            Tok::Star(n) => match m {
                Matcher::Gobwas => out.push_str(".*"),
                Matcher::Doublestar
                    if *n >= 2 && at_segment_start && matches!(next, Some(Tok::Lit('/'))) =>
                {
                    // `**/`: zero or more leading directories.
                    out.push_str(group_open);
                    out.push_str(".*/)?");
                    i = i.saturating_add(2);
                    continue;
                }
                Matcher::Doublestar if *n >= 2 && at_segment_start && next.is_none() => {
                    out.push_str(".*");
                }
                Matcher::Doublestar => out.push_str("[^/]*"),
            },
            Tok::Any => match m {
                Matcher::Gobwas => out.push('.'),
                Matcher::Doublestar => out.push_str("[^/]"),
            },
            Tok::Class(neg, items) => class(*neg, items, f, m == Matcher::Doublestar, out)?,
            Tok::Alt(alts) => {
                out.push_str(group_open);
                for (k, a) in alts.iter().enumerate() {
                    if k > 0 {
                        out.push('|');
                    }
                    render(a, m, f, false, out)?;
                }
                out.push(')');
            }
        }
        i = i.saturating_add(1);
    }
    Ok(())
}

/// Translates one pattern into a regex body (unanchored). `glob` patterns are lowercased first (lefthook lowercases
/// both sides; the caller wraps the result in `(?i:…)` / uses `grep -i`).
pub(crate) fn translate(pattern: &str, m: Matcher, f: Flavor) -> Result<String, GlobError> {
    let toks = parse(pattern, m)?;
    let mut out = String::new();
    render(&toks, m, f, true, &mut out)?;
    Ok(out)
}

/// The `files` regex of a `glob` list: `^(?i:(?:<g1>)|(?:<g2>))$` (05 §19.6), `None` when every pattern is empty.
/// Patterns are lowercased like lefthook's (`strings.ToLower`). With `root_prefix`, the portable form
/// `^(?=<root>/)(?i:…)$` restricts matches to the root directory.
pub(crate) fn files_regex(
    globs: &[String],
    m: Matcher,
    root_prefix: Option<&str>,
) -> Result<Option<String>, GlobError> {
    let pats: Vec<&String> = globs.iter().filter(|g| !g.is_empty()).collect();
    if pats.is_empty() {
        return Ok(root_prefix.map(|r| {
            let mut s = String::from("^");
            for c in r.chars() {
                py_escape(c, &mut s);
            }
            s.push('/');
            s
        }));
    }
    let mut out = String::from("^");
    if let Some(r) = root_prefix {
        out.push_str("(?=");
        for c in r.chars() {
            py_escape(c, &mut out);
        }
        out.push_str("/)");
    }
    out.push_str("(?i:");
    for (k, g) in pats.iter().enumerate() {
        if k > 0 {
            out.push('|');
        }
        let body = translate(&go_lower(g), m, Flavor::Python)
            .map_err(|e| GlobError(format!("glob {}: {}", quote(g), e.0)))?;
        write!(out, "(?:{body})").unwrap_or_default();
    }
    out.push_str(")$");
    check_python(&out)?;
    Ok(Some(out))
}

/// The `exclude` regex of an `exclude` list: `^(?:<e1>|<e2>)$` (case-sensitive), `None` when empty.
pub(crate) fn exclude_regex(excludes: &[String], m: Matcher) -> Result<Option<String>, GlobError> {
    let pats: Vec<&String> = excludes.iter().filter(|g| !g.is_empty()).collect();
    if pats.is_empty() {
        return Ok(None);
    }
    let mut out = String::from("^(?:");
    for (k, g) in pats.iter().enumerate() {
        if k > 0 {
            out.push('|');
        }
        let body = translate(g, m, Flavor::Python)
            .map_err(|e| GlobError(format!("exclude {}: {}", quote(g), e.0)))?;
        out.push_str(&body);
    }
    out.push_str(")$");
    check_python(&out)?;
    Ok(Some(out))
}

/// The ERE of a glob list for `grep -E` (`^(<g1>|<g2>)$`), `None` when empty.
pub(crate) fn ere(globs: &[String], m: Matcher, lower: bool) -> Result<Option<String>, GlobError> {
    let pats: Vec<&String> = globs.iter().filter(|g| !g.is_empty()).collect();
    if pats.is_empty() {
        return Ok(None);
    }
    let mut out = String::from("^(");
    for (k, g) in pats.iter().enumerate() {
        if k > 0 {
            out.push('|');
        }
        let src = if lower { go_lower(g) } else { (*g).clone() };
        out.push_str(
            &translate(&src, m, Flavor::Ere)
                .map_err(|e| GlobError(format!("glob {}: {}", quote(g), e.0)))?,
        );
    }
    out.push_str(")$");
    Ok(Some(out))
}

/// Go's `strings.ToLower`: the simple (one-to-one) lowercase mapping of every character. Rust's full mapping turns
/// `İ` (U+0130) into two characters; its first one is the simple mapping (`i`).
pub(crate) fn go_lower(s: &str) -> String {
    s.chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect()
}

/// Rejects a regex Python's `re` would not compile (the translation is meant to always produce valid regexes).
fn check_python(src: &str) -> Result<(), GlobError> {
    snp_pyre::validate(src, snp_pyre::Mode::Str)
        .map(|_| ())
        .map_err(|e| GlobError(format!("the regex {src} is invalid: {e}")))
}

/// `'pattern'` for messages.
pub(crate) fn quote(s: &str) -> String {
    format!("'{s}'")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn py(p: &str) -> String {
        translate(p, Matcher::Gobwas, Flavor::Python).unwrap()
    }

    fn matches(re: &str, path: &str) -> bool {
        snp_pyre::PyRegex::compile(re, snp_pyre::Options::CONFIG)
            .unwrap()
            .search(path)
            .unwrap()
    }

    fn files(globs: &[&str], m: Matcher) -> String {
        let g: Vec<String> = globs.iter().map(|s| (*s).to_owned()).collect();
        files_regex(&g, m, None).unwrap().unwrap()
    }

    #[test]
    fn tokens() {
        assert_eq!(py("*.rb"), ".*\\.rb");
        assert_eq!(py("**/*.rb"), ".*/.*\\.rb");
        assert_eq!(py("a?c"), "a.c");
        assert_eq!(py("[abc]"), "[abc]");
        assert_eq!(py("[a-z]"), "[a-z]");
        assert_eq!(py("[!abc]"), "[^abc]");
        assert_eq!(py("*.{a,b,cn}"), ".*\\.(?:a|b|cn)");
        assert_eq!(py("{a,{b,c}}"), "(?:a|(?:b|c))");
        assert_eq!(py("\\*x"), "\\*x");
        assert_eq!(py("a b-c"), "a\\ b\\-c");
        assert_eq!(py("x,y}"), "x,y\\}");
        // gobwas v0.2.3: one range or a literal list per bracket; an unclosed `{` ends with the pattern; a trailing
        // `\` is dropped.
        assert_eq!(py("[ab-z]"), "[ab\\-z]");
        assert_eq!(py("[\\-a]"), "[\\\\-a]");
        assert_eq!(py("[]-a]"), "[\\]-a]");
        assert_eq!(py("[!a-c]"), "[^a-c]");
        assert_eq!(py("[!!]"), "[^\\!]");
        assert_eq!(py("{a"), "(?:a)");
        assert_eq!(py("x{a,{b"), "x(?:a|(?:b))");
        assert_eq!(py("a\\"), "a");
        assert_eq!(py("x{a,}"), "x(?:a|)");
        assert_eq!(py("[xa-c]"), "[xa\\-c]");
        for bad in [
            "{}", "x{,}y", "{a,{}}", "[", "[]", "[!]", "[a-z0-9]", "[a-]", "[c-a]", "[a\\", "a\0b",
        ] {
            assert!(
                translate(bad, Matcher::Gobwas, Flavor::Python).is_err(),
                "{bad:?}"
            );
        }
        // doublestar keeps POSIX classes.
        assert_eq!(
            translate("[a-z0-9]", Matcher::Doublestar, Flavor::Python).unwrap(),
            "[a-z0-9]"
        );
    }

    #[test]
    fn worked_example_regexes() {
        assert_eq!(
            files(&["*.{js,ts}"], Matcher::Gobwas),
            "^(?i:(?:.*\\.(?:js|ts)))$"
        );
        assert_eq!(files(&["*.rb"], Matcher::Gobwas), "^(?i:(?:.*\\.rb))$");
        assert_eq!(
            exclude_regex(&["config/routes.rb".to_owned()], Matcher::Gobwas)
                .unwrap()
                .unwrap(),
            "^(?:config/routes\\.rb)$"
        );
    }

    /// The vectors of 05 §19.6 (lefthook `filter/filter_test.go`).
    #[test]
    fn filter_test_vectors() {
        let g = files(&["*.rb"], Matcher::Gobwas);
        assert!(matches(&g, "folder/subfolder/0.rb"));
        assert!(matches(&g, "2.RB"));
        assert!(!matches(&g, "1.txt"));
        let g = files(&["**/*.rb"], Matcher::Gobwas);
        assert!(!matches(&g, "0.rb"));
        assert!(matches(&g, "folder/1.rb"));
        let g = files(&["*.rb?"], Matcher::Gobwas);
        assert!(matches(&g, "1.rBs"));
        assert!(matches(&g, "2.rbv"));
        assert!(!matches(&g, "folder/0.rb"));
        let g = files(&["*.{a,b,cn}"], Matcher::Gobwas);
        for p in ["f.a", "f.b", "f.cn"] {
            assert!(matches(&g, p), "{p}");
        }
        assert!(!matches(&g, "f.c"));
        let g = files(&["**/*.rb"], Matcher::Doublestar);
        for p in ["0.rb", "folder/1.rb", "folder/subfolder/2.rb"] {
            assert!(matches(&g, p), "{p}");
        }
        let g = files(&["src/*.js"], Matcher::Gobwas);
        assert!(matches(&g, "src/lib/util.js"));
        let g = files(&["src/*.js"], Matcher::Doublestar);
        assert!(!matches(&g, "src/lib/util.js"));
        assert!(matches(&g, "src/util.js"));
        let e = exclude_regex(&["*.txt".to_owned()], Matcher::Gobwas)
            .unwrap()
            .unwrap();
        assert!(matches(&e, "dir/a.txt"));
        let e = exclude_regex(&["a.txt".to_owned()], Matcher::Gobwas)
            .unwrap()
            .unwrap();
        assert!(!matches(&e, "dir/a.txt"));
        let e = exclude_regex(&["*.TXT".to_owned()], Matcher::Gobwas)
            .unwrap()
            .unwrap();
        assert!(!matches(&e, "a.txt"), "exclude is case-sensitive");
    }

    #[test]
    fn doublestar_forms() {
        let ds = |p: &str| translate(p, Matcher::Doublestar, Flavor::Python).unwrap();
        assert_eq!(ds("**/*.rb"), "(?:.*/)?[^/]*\\.rb");
        assert_eq!(ds("dir/**"), "dir(?:/.*)?");
        assert_eq!(ds("**"), ".*");
        assert_eq!(ds("a/**/b"), "a/(?:.*/)?b");
        assert_eq!(ds("a?"), "a[^/]");
        assert_eq!(ds("[^a]"), "[^/a]");
        assert_eq!(ds("[!a]"), "[^/a]");
        let g = files(&["dir/**"], Matcher::Doublestar);
        assert!(matches(&g, "dir"));
        assert!(matches(&g, "dir/a/b"));
        assert!(!matches(&g, "dirx"));
    }

    #[test]
    fn root_lookahead() {
        let g = files_regex(&["*.rb".to_owned()], Matcher::Gobwas, Some("backend"))
            .unwrap()
            .unwrap();
        assert_eq!(g, "^(?=backend/)(?i:(?:.*\\.rb))$");
        assert!(matches(&g, "backend/a.rb"));
        assert!(!matches(&g, "backend-old/a.rb"));
        assert_eq!(
            files_regex(&[], Matcher::Gobwas, Some("a.b"))
                .unwrap()
                .unwrap(),
            "^a\\.b/"
        );
        assert_eq!(
            files_regex(&[String::new()], Matcher::Gobwas, None).unwrap(),
            None
        );
    }

    #[test]
    fn ere_forms() {
        let e = |p: &str| translate(p, Matcher::Gobwas, Flavor::Ere).unwrap();
        assert_eq!(e("*.{js,ts}"), ".*\\.(js|ts)");
        assert_eq!(e("[\\]a-]"), "[]a-]");
        assert_eq!(e("[!^x]"), "[^x^]");
        assert_eq!(e("[^]"), "\\^");
        assert_eq!(e("a+b"), "a\\+b");
        assert_eq!(
            ere(&["*.JS".to_owned()], Matcher::Gobwas, true)
                .unwrap()
                .unwrap(),
            "^(.*\\.js)$"
        );
        assert!(translate("[*-/]", Matcher::Gobwas, Flavor::Ere).is_err());
    }

    /// One line of `tests/vectors/gobwas.jsonl`: a pattern, whether it is a `glob` (both sides lowercased) or an
    /// `exclude` (case-sensitive), whether real gobwas/glob v0.2.3 compiles it (`MustCompile` without panicking),
    /// and its verdict for each path.
    struct Vector {
        p: String,
        lower: bool,
        ok: bool,
        paths: Vec<String>,
        m: Option<Vec<bool>>,
    }

    /// The vectors were produced by the real gobwas/glob v0.2.3 (lefthook's matcher) with
    /// `tests/vectors/gobwas-oracle/` (`python3 gen.py 34 1000 | go run .`), so this is an independent reference.
    fn vectors() -> Vec<Vector> {
        include_str!("../../tests/vectors/gobwas.jsonl")
            .lines()
            .map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).unwrap();
                let strs = |k: &str| -> Vec<String> {
                    v[k].as_array()
                        .unwrap()
                        .iter()
                        .map(|x| x.as_str().unwrap().to_owned())
                        .collect()
                };
                Vector {
                    p: v["p"].as_str().unwrap().to_owned(),
                    lower: v["lower"].as_bool().unwrap(),
                    ok: v["ok"].as_bool().unwrap(),
                    paths: strs("paths"),
                    m: v["m"]
                        .as_array()
                        .map(|a| a.iter().map(|x| x.as_bool().unwrap()).collect()),
                }
            })
            .collect()
    }

    /// 05 §19.6: the `files` / `exclude` regex of a pattern ≡ gobwas; a pattern gobwas rejects is an error (the job
    /// becomes a `fail` hook, as lefthook's `MustCompile` panics).
    #[test]
    fn gobwas_vectors() {
        let all = vectors();
        assert!(all.len() >= 2000);
        let (mut compared, mut errors, mut unicode_bugs) = (0_usize, 0_usize, 0_usize);
        for v in &all {
            let pats = std::slice::from_ref(&v.p);
            let re = if v.lower {
                files_regex(pats, Matcher::Gobwas, None)
            } else {
                exclude_regex(pats, Matcher::Gobwas)
            };
            if let Err(e) = &re
                && e.0.contains("only empty alternatives")
            {
                // Refused by design whatever gobwas does with it (see `alt`).
                assert!(v.p.contains('{'), "{:?}", v.p);
                continue;
            }
            if !v.ok {
                assert!(
                    re.is_err(),
                    "gobwas rejects {:?}, the translation gives {re:?}",
                    v.p
                );
                errors += 1;
                continue;
            }
            let re = re
                .unwrap_or_else(|e| panic!("gobwas accepts {:?}: {e:?}", v.p))
                .unwrap();
            let ere = ere(pats, Matcher::Gobwas, v.lower);
            for (path, want) in v.paths.iter().zip(v.m.as_deref().unwrap()) {
                if path.is_empty() {
                    // lefthook never matches an empty path (gobwas's `?` would match it).
                    continue;
                }
                let got = matches(&re, path);
                if got && !*want && !path.is_ascii() && v.p.contains(['?', '[']) {
                    // gobwas measures some fixed-length runs in bytes, so a multi-byte character can fail a `?` or
                    // `[…]` it should match (COMPATIBILITY.md, migrate glob divergences); snp matches characters.
                    unicode_bugs += 1;
                    continue;
                }
                assert_eq!(
                    matches(&re, path),
                    *want,
                    "pattern {:?} (lower {}) regex {re:?} path {path:?}",
                    v.p,
                    v.lower
                );
                // The ERE form (the shell filter, `grep -iE` / `grep -vE`) agrees too, where it exists.
                if let Ok(Some(e)) = &ere {
                    let target = if v.lower {
                        go_lower(path)
                    } else {
                        path.clone()
                    };
                    let py = regex_syntax_like(e, v.lower);
                    assert_eq!(matches(&py, &target), *want, "ere {e:?} path {path:?}");
                }
                compared += 1;
            }
        }
        assert!(
            errors > 100 && compared > 9_000 && unicode_bugs < 20,
            "{errors} {compared} {unicode_bugs}"
        );
    }

    /// The ERE forms run by GNU grep (`LC_ALL=C`, ASCII vectors only): bracket expressions such as `[[:]` must not
    /// become POSIX class openers.
    #[test]
    fn ere_vectors_under_grep() {
        let Ok(probe) = std::process::Command::new("grep").arg("--version").output() else {
            return;
        };
        if !probe.status.success() {
            return;
        }
        let mut checked = 0_usize;
        for v in vectors().iter().filter(|v| v.ok && v.p.is_ascii()) {
            let Ok(Some(e)) = ere(std::slice::from_ref(&v.p), Matcher::Gobwas, v.lower) else {
                continue;
            };
            let paths: Vec<(&String, bool)> = v
                .paths
                .iter()
                .zip(v.m.as_deref().unwrap().iter().copied())
                .filter(|(p, _)| p.is_ascii() && !p.is_empty())
                .collect();
            if paths.is_empty() {
                continue;
            }
            let input: String = paths.iter().flat_map(|(p, _)| [p.as_str(), "\n"]).collect();
            let mut cmd = std::process::Command::new("grep");
            cmd.env("LC_ALL", "C")
                .arg(if v.lower { "-iE" } else { "-E" })
                .arg("--")
                .arg(&e);
            let out = run_with_stdin(cmd, &input);
            assert!(out.0 <= 1, "grep -E {e:?} failed: {}", out.2);
            let got: Vec<&str> = out.1.lines().collect();
            for (p, want) in &paths {
                assert_eq!(got.contains(&p.as_str()), *want, "ere {e:?} path {p:?}");
            }
            checked += 1;
        }
        assert!(checked > 300, "{checked}");
        let e = ere(&["*[[:]x".to_owned()], Matcher::Gobwas, true)
            .unwrap()
            .unwrap();
        let mut cmd = std::process::Command::new("grep");
        cmd.env("LC_ALL", "C").arg("-iE").arg("--").arg(&e);
        let out = run_with_stdin(cmd, "a:x\na[x\nax\n");
        assert_eq!((out.0, out.1.as_str()), (0, "a:x\na[x\n"), "{e}: {}", out.2);
    }

    fn run_with_stdin(mut cmd: std::process::Command, input: &str) -> (i32, String, String) {
        use std::io::Write as _;
        let mut child = cmd
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn pattern() -> impl proptest::strategy::Strategy<Value = String> {
        use proptest::prelude::*;
        let tok = prop_oneof![
            Just("a"),
            Just("b"),
            Just("B"),
            Just("."),
            Just("/"),
            Just("-"),
            Just("+"),
            Just("*"),
            Just("**"),
            Just("?"),
            Just("[ab]"),
            Just("[!a]"),
            Just("[a-c]"),
            Just("[.]"),
            Just("[ab-z]"),
            Just("[a-z0-9]"),
            Just("[a-]"),
            Just("[]"),
            Just("[[:]"),
            Just("[]-a]"),
            Just("{a,b}"),
            Just("{x*,y,}"),
            Just("{a,{b,c}}"),
            Just("{a"),
            Just("\\*"),
            Just("\\a"),
            Just("\\"),
        ];
        proptest::collection::vec(tok, 1..7).prop_map(|v| v.concat())
    }

    fn path() -> impl proptest::strategy::Strategy<Value = String> {
        use proptest::prelude::*;
        proptest::collection::vec(
            prop_oneof![
                Just('a'),
                Just('b'),
                Just('c'),
                Just('A'),
                Just('.'),
                Just('/'),
                Just('*'),
                Just('-'),
                Just('['),
                Just(':'),
                Just('x'),
                Just('y')
            ],
            0..8,
        )
        .prop_map(|v| v.into_iter().collect())
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(4000))]
        /// The ERE form (the shell filter) and the Python form agree, and both exist or fail together.
        #[test]
        fn ere_agrees_with_python(pat in pattern(), p in path()) {
            let Ok(Some(re)) = files_regex(std::slice::from_ref(&pat), Matcher::Gobwas, None) else {
                proptest::prop_assert!(translate(&pat, Matcher::Gobwas, Flavor::Ere).is_err());
                return Ok(());
            };
            if let Ok(Some(e)) = ere(std::slice::from_ref(&pat), Matcher::Gobwas, true) {
                let py = regex_syntax_like(&e, true);
                proptest::prop_assert_eq!(matches(&py, &go_lower(&p)), matches(&re, &p), "ere {:?} regex {:?}", e, re);
            }
        }
    }

    /// An ERE of [`ere`] as a Python regex: `(` → `(?:`, POSIX bracket expressions (a leading `]` and `^`-not-first
    /// are literal, a trailing `-` is literal, no escapes) re-written with Python escapes.
    fn regex_syntax_like(ere: &str, fold: bool) -> String {
        let chars: Vec<char> = ere.chars().collect();
        let mut out = String::from(if fold { "(?i:" } else { "(?:" });
        let mut i = 0;
        while i < chars.len() {
            match chars[i] {
                '\\' => {
                    out.push('\\');
                    out.push(chars[i + 1]);
                    i += 2;
                }
                '(' => {
                    out.push_str("(?:");
                    i += 1;
                }
                '[' => {
                    i += 1;
                    out.push('[');
                    if chars[i] == '^' {
                        out.push('^');
                        i += 1;
                    }
                    let mut first = true;
                    while first || chars[i] != ']' {
                        first = false;
                        let c = chars[i];
                        let range = c != '-'
                            && chars.get(i + 1) == Some(&'-')
                            && chars.get(i + 2).is_some_and(|x| *x != ']');
                        if range {
                            out.push(c);
                            out.push('-');
                            out.push(chars[i + 2]);
                            i += 3;
                        } else {
                            if c.is_ascii_punctuation() {
                                out.push('\\');
                            }
                            out.push(c);
                            i += 1;
                        }
                    }
                    out.push(']');
                    i += 1;
                }
                c => {
                    out.push(c);
                    i += 1;
                }
            }
        }
        out.push(')');
        out
    }
}
