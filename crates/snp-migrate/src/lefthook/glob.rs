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

/// A recursive-descent parser over the pattern's characters.
struct Parser<'a> {
    chars: &'a [char],
    pos: usize,
    matcher: Matcher,
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

    /// After `[`: `!` (and `^` for doublestar) negates; `a-z` ranges; `\x` escapes; `]` closes.
    fn class(&mut self) -> Result<Tok, GlobError> {
        let unclosed = || GlobError("unclosed '['".to_owned());
        let negated = self.peek() == Some('!')
            || (self.matcher == Matcher::Doublestar && self.peek() == Some('^'));
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
    let mut p = Parser {
        chars: &chars,
        pos: 0,
        matcher,
    };
    let seq = p.seq(0)?;
    if p.pos < chars.len() {
        // A `,` or `}` outside braces is literal text for gobwas; `seq(0)` never stops on them.
        return Err(GlobError("unbalanced '}'".to_owned()));
    }
    Ok(seq)
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
            // POSIX brackets: no escapes; `]` first, `-` last, `^` never first.
            let mut close = false;
            let mut dash = false;
            let mut caret = false;
            let mut body = String::new();
            for &(lo, hi) in items {
                if lo == hi {
                    match lo {
                        ']' => close = true,
                        '-' => dash = true,
                        '^' => caret = true,
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
            if !neg && !close && !dash && body.is_empty() && caret {
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
        let body = translate(&g.to_lowercase(), m, Flavor::Python)
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
        let src = if lower {
            g.to_lowercase()
        } else {
            (*g).clone()
        };
        out.push_str(
            &translate(&src, m, Flavor::Ere)
                .map_err(|e| GlobError(format!("glob {}: {}", quote(g), e.0)))?,
        );
    }
    out.push_str(")$");
    Ok(Some(out))
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
        assert!(translate("[", Matcher::Gobwas, Flavor::Python).is_err());
        assert!(translate("{a", Matcher::Gobwas, Flavor::Python).is_err());
        assert!(translate("a\\", Matcher::Gobwas, Flavor::Python).is_err());
        assert!(translate("[]", Matcher::Gobwas, Flavor::Python).is_err());
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

    /// A reference gobwas matcher written independently of the translator: braces are expanded first, then a
    /// backtracking wildcard match (`*` = any run, `?` = any character, `[…]` / `[!…]` classes, `\x` literal), both
    /// sides lowercased like lefthook's `glob` filter.
    mod reference {
        fn expand(p: &[char]) -> Vec<Vec<char>> {
            // The first top-level `{…}` (outside classes and escapes).
            let mut i = 0;
            let mut in_class = false;
            while i < p.len() {
                match p[i] {
                    '\\' => i += 1,
                    '[' => in_class = true,
                    ']' => in_class = false,
                    '{' if !in_class => {
                        let mut depth = 0;
                        let mut parts = Vec::new();
                        let mut start = i + 1;
                        let mut j = i;
                        while j < p.len() {
                            match p[j] {
                                '\\' => j += 1,
                                '{' => depth += 1,
                                '}' => {
                                    depth -= 1;
                                    if depth == 0 {
                                        parts.push(&p[start..j]);
                                        break;
                                    }
                                }
                                ',' if depth == 1 => {
                                    parts.push(&p[start..j]);
                                    start = j + 1;
                                }
                                _ => {}
                            }
                            j += 1;
                        }
                        let mut out = Vec::new();
                        for part in parts {
                            let mut v = p[..i].to_vec();
                            v.extend_from_slice(part);
                            v.extend_from_slice(&p[j + 1..]);
                            out.extend(expand(&v));
                        }
                        return out;
                    }
                    _ => {}
                }
                i += 1;
            }
            vec![p.to_vec()]
        }

        fn class_len_and_match(p: &[char], c: char) -> (usize, bool) {
            let mut i = 1;
            let neg = p.get(1) == Some(&'!');
            if neg {
                i += 1;
            }
            let mut hit = false;
            let mut first = true;
            while i < p.len() && (p[i] != ']' || first) {
                first = false;
                if p[i] == '\\' {
                    i += 1;
                }
                let lo = p[i];
                let mut hi = lo;
                if p.get(i + 1) == Some(&'-') && p.get(i + 2).is_some_and(|x| *x != ']') {
                    hi = p[i + 2];
                    i += 2;
                }
                if lo <= c && c <= hi {
                    hit = true;
                }
                i += 1;
            }
            (i + 1, hit != neg)
        }

        fn wild(p: &[char], s: &[char]) -> bool {
            match p.first() {
                None => s.is_empty(),
                Some('*') => (0..=s.len()).any(|k| wild(&p[1..], &s[k..])),
                Some('?') => !s.is_empty() && wild(&p[1..], &s[1..]),
                Some('[') => {
                    let Some(&c) = s.first() else { return false };
                    let (n, hit) = class_len_and_match(p, c);
                    hit && wild(&p[n..], &s[1..])
                }
                Some('\\') => s.first() == p.get(1) && !s.is_empty() && wild(&p[2..], &s[1..]),
                Some(c) => s.first() == Some(c) && wild(&p[1..], &s[1..]),
            }
        }

        pub(super) fn matches(pattern: &str, path: &str) -> bool {
            let p: Vec<char> = pattern.to_lowercase().chars().collect();
            let s: Vec<char> = path.to_lowercase().chars().collect();
            expand(&p).iter().any(|alt| wild(alt, &s))
        }
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
            Just("{a,b}"),
            Just("{x*,y,}"),
            Just("{a,{b,c}}"),
            Just("\\*"),
            Just("\\a"),
        ];
        proptest::collection::vec(tok, 0..7).prop_map(|v| v.concat())
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
                Just('x'),
                Just('y')
            ],
            0..8,
        )
        .prop_map(|v| v.into_iter().collect())
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(4000))]
        /// 05 §19.6: the translated `files` regex ≡ the reference gobwas matcher.
        #[test]
        fn gobwas_translation_matches_reference(pat in pattern(), p in path()) {
            proptest::prop_assume!(!pat.is_empty());
            let re = files_regex(std::slice::from_ref(&pat), Matcher::Gobwas, None).unwrap().unwrap();
            let want = reference::matches(&pat, &p);
            proptest::prop_assert_eq!(matches(&re, &p), want, "pattern {:?} regex {:?} path {:?}", pat, re, p);
            // The ERE form (the shell filter) agrees with the Python form on the same inputs.
            let ere = ere(std::slice::from_ref(&pat), Matcher::Gobwas, true).unwrap().unwrap();
            let rust = regex_syntax_like(&ere);
            proptest::prop_assert_eq!(matches(&rust, &p.to_lowercase()), want, "ere {:?}", ere);
        }
    }

    /// An ERE of [`ere`] as a Python regex: `(` → `(?:`, POSIX bracket expressions (a leading `]` and `^`-not-first
    /// are literal, a trailing `-` is literal, no escapes) re-written with Python escapes.
    fn regex_syntax_like(ere: &str) -> String {
        let chars: Vec<char> = ere.chars().collect();
        let mut out = String::from("(?i:");
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
