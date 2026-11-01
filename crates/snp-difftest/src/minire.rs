//! A small, bounded backtracking matcher for the `*_matches` expectations of divergence files (Python `re.search`
//! semantics without flags: `^` = start, `$` = end or before a final newline). The harness may not link the
//! `regex` crate (deny.toml: snp-pyre only) and needs no more than this subset: literals, `.`, escapes
//! (`\d \w \s \D \W \S \. \[ …`), classes with ranges and negation, groups `(…)`/`(?:…)`, alternation and the
//! quantifiers `* + ? {n} {n,} {n,m}` (greedy; a trailing `?` is accepted and ignored). Every match attempt has a
//! step budget, so a hostile pattern fails instead of hanging.
//!
//! Owner: WP-35.

use std::cell::Cell;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    Char(char),
    Any,
    Class(Vec<ClassItem>, bool),
    Start,
    End,
    /// Alternatives; `Some(n)` = capturing group number `n` (1-based).
    Group(Vec<Vec<Self>>, Option<usize>),
    Backref(usize),
    Repeat(Box<Self>, u32, Option<u32>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ClassItem {
    Range(char, char),
    Digit(bool),
    Word(bool),
    Space(bool),
}

/// A compiled pattern.
#[derive(Debug, Clone)]
pub(crate) struct Regex {
    alts: Vec<Vec<Node>>,
    groups: usize,
    multiline: bool,
    dotall: bool,
}

const BUDGET: u64 = 2_000_000;
const MAX_REPEAT: u32 = 10_000;

/// Parser state.
struct Parser<'p> {
    p: &'p [char],
    pos: usize,
    groups: usize,
    multiline: bool,
    dotall: bool,
}

/// Matcher state shared by one search.
struct Ctx<'t> {
    t: &'t [char],
    budget: Cell<u64>,
    caps: Vec<Cell<Option<(usize, usize)>>>,
    multiline: bool,
    dotall: bool,
}

impl Regex {
    /// Compiles `pattern`.
    pub(crate) fn new(pattern: &str) -> Result<Self, String> {
        let chars: Vec<char> = pattern.chars().collect();
        let mut ps = Parser { p: &chars, pos: 0, groups: 0, multiline: false, dotall: false };
        let alts = ps.alts(0)?;
        if ps.pos != chars.len() {
            return Err(format!("unbalanced parenthesis at position {}", ps.pos));
        }
        check_backrefs(&alts, ps.groups)?;
        Ok(Self { alts, groups: ps.groups, multiline: ps.multiline, dotall: ps.dotall })
    }

    /// `re.search(pattern, text) is not None`; an exhausted step budget is `Err`.
    pub(crate) fn search(&self, text: &str) -> Result<bool, String> {
        let chars: Vec<char> = text.chars().collect();
        let mut caps = Vec::new();
        caps.resize_with(self.groups.saturating_add(1), || Cell::new(None));
        let cx = Ctx { t: &chars, budget: Cell::new(BUDGET), caps, multiline: self.multiline, dotall: self.dotall };
        for start in 0..=chars.len() {
            let mut k = |_: usize| true;
            if self.alts.iter().any(|alt| seq(alt, &cx, start, &mut k)) {
                return Ok(true);
            }
            if cx.budget.get() == 0 {
                return Err(String::from("pattern exceeded the matching budget"));
            }
        }
        Ok(false)
    }
}

fn check_backrefs(alts: &[Vec<Node>], groups: usize) -> Result<(), String> {
    alts.iter().flatten().try_for_each(|n| check_node(n, groups))
}

fn check_node(n: &Node, groups: usize) -> Result<(), String> {
    match n {
        Node::Backref(i) if *i > groups || *i == 0 => Err(format!("invalid group reference {i}")),
        Node::Group(inner, _) => check_backrefs(inner, groups),
        Node::Repeat(inner, _, _) => check_node(inner, groups),
        _ => Ok(()),
    }
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.p.get(self.pos).copied()
    }

    fn peek_at(&self, off: usize) -> Option<char> {
        self.p.get(self.pos.saturating_add(off)).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos = self.pos.saturating_add(1);
        Some(c)
    }

    fn alts(&mut self, depth: u32) -> Result<Vec<Vec<Node>>, String> {
        if depth > 64 {
            return Err(String::from("pattern nested too deeply"));
        }
        let mut alts = vec![Vec::new()];
        while let Some(c) = self.peek() {
            match c {
                ')' => break,
                '|' => {
                    self.pos = self.pos.saturating_add(1);
                    alts.push(Vec::new());
                }
                _ => {
                    let Some(atom) = self.atom(depth)? else { continue };
                    let atom = self.quant(atom)?;
                    if let Some(cur) = alts.last_mut() {
                        cur.push(atom);
                    }
                }
            }
        }
        Ok(alts)
    }

    fn quant(&mut self, atom: Node) -> Result<Node, String> {
        let (min, max) = match self.peek() {
            Some('*') => (0, None),
            Some('+') => (1, None),
            Some('?') => (0, Some(1)),
            Some('{') => {
                let rest = self.p.get(self.pos..).unwrap_or_default();
                let Some(close) = rest.iter().position(|&c| c == '}') else { return Ok(atom) };
                let body: String = rest.get(1..close).unwrap_or_default().iter().collect();
                let parsed = match body.split_once(',') {
                    Some((a, b)) => a.parse::<u32>().ok().and_then(|a| {
                        if b.is_empty() { Some((a, None)) } else { b.parse::<u32>().ok().map(|b| (a, Some(b))) }
                    }),
                    None => body.parse::<u32>().ok().map(|n| (n, Some(n))),
                };
                // Python: a `{` that does not start a valid repeat is a literal.
                let Some((a, b)) = parsed else { return Ok(atom) };
                if a > MAX_REPEAT || b.is_some_and(|b| b > MAX_REPEAT || b < a) {
                    return Err(String::from("bad repeat bounds"));
                }
                self.pos = self.pos.saturating_add(close);
                (a, b)
            }
            _ => return Ok(atom),
        };
        self.pos = self.pos.saturating_add(1);
        if self.peek() == Some('?') {
            self.pos = self.pos.saturating_add(1);
        }
        if matches!(atom, Node::Start | Node::End) {
            return Err(String::from("nothing to repeat"));
        }
        Ok(Node::Repeat(Box::new(atom), min, max))
    }

    /// One atom; `None` for an inline flag group (`(?m)`, `(?s)`), which matches nothing.
    fn atom(&mut self, depth: u32) -> Result<Option<Node>, String> {
        let c = self.bump().ok_or("unexpected end of pattern")?;
        Ok(Some(match c {
            '.' => Node::Any,
            '^' => Node::Start,
            '$' => Node::End,
            '*' | '+' | '?' => return Err(String::from("nothing to repeat")),
            '\\' => {
                let e = self.bump().ok_or("trailing backslash")?;
                if let Some(d) = e.to_digit(10).filter(|d| *d > 0) {
                    let mut n = usize::try_from(d).unwrap_or(0);
                    if let Some(d2) = self.peek().and_then(|c| c.to_digit(10)) {
                        self.pos = self.pos.saturating_add(1);
                        n = n.saturating_mul(10).saturating_add(usize::try_from(d2).unwrap_or(0));
                    }
                    Node::Backref(n)
                } else {
                    match class_escape(e) {
                        Some(item) => Node::Class(vec![item], false),
                        None => Node::Char(literal_escape(e)),
                    }
                }
            }
            '(' => {
                let mut capture = true;
                if self.peek() == Some('?') {
                    match self.peek_at(1) {
                        Some(':') => {
                            self.pos = self.pos.saturating_add(2);
                            capture = false;
                        }
                        Some(f) if "ims".contains(f) => {
                            self.pos = self.pos.saturating_add(1);
                            while let Some(f) = self.peek() {
                                match f {
                                    'm' => self.multiline = true,
                                    's' => self.dotall = true,
                                    'i' => return Err(String::from("unsupported flag (?i)")),
                                    ')' => break,
                                    _ => return Err(String::from("unknown flag")),
                                }
                                self.pos = self.pos.saturating_add(1);
                            }
                            if self.bump() != Some(')') {
                                return Err(String::from("missing ), unterminated flag group"));
                            }
                            return Ok(None);
                        }
                        _ => return Err(String::from("unsupported group extension")),
                    }
                }
                let number = if capture {
                    self.groups = self.groups.saturating_add(1);
                    Some(self.groups)
                } else {
                    None
                };
                let alts = self.alts(depth.saturating_add(1))?;
                if self.bump() != Some(')') {
                    return Err(String::from("missing ), unterminated subpattern"));
                }
                Node::Group(alts, number)
            }
            '[' => self.class()?,
            other => Node::Char(other),
        }))
    }

    fn class(&mut self) -> Result<Node, String> {
        let mut items = Vec::new();
        let negated = self.peek() == Some('^');
        if negated {
            self.pos = self.pos.saturating_add(1);
        }
        let mut first = true;
        loop {
            let c = self.bump().ok_or("unterminated character set")?;
            if c == ']' && !first {
                break;
            }
            first = false;
            let lo = if c == '\\' {
                let e = self.bump().ok_or("trailing backslash")?;
                if let Some(item) = class_escape(e) {
                    items.push(item);
                    continue;
                }
                literal_escape(e)
            } else {
                c
            };
            if self.peek() == Some('-') && self.peek_at(1).is_some_and(|n| n != ']') {
                self.pos = self.pos.saturating_add(1);
                let mut hi = self.bump().ok_or("bad range")?;
                if hi == '\\' {
                    hi = literal_escape(self.bump().ok_or("trailing backslash")?);
                }
                if hi < lo {
                    return Err(String::from("bad character range"));
                }
                items.push(ClassItem::Range(lo, hi));
            } else {
                items.push(ClassItem::Range(lo, lo));
            }
        }
        Ok(Node::Class(items, negated))
    }
}

fn class_escape(c: char) -> Option<ClassItem> {
    match c {
        'd' => Some(ClassItem::Digit(true)),
        'D' => Some(ClassItem::Digit(false)),
        'w' => Some(ClassItem::Word(true)),
        'W' => Some(ClassItem::Word(false)),
        's' => Some(ClassItem::Space(true)),
        'S' => Some(ClassItem::Space(false)),
        _ => None,
    }
}

fn literal_escape(c: char) -> char {
    match c {
        'n' => '\n',
        't' => '\t',
        'r' => '\r',
        other => other,
    }
}

fn class_hit(items: &[ClassItem], c: char) -> bool {
    items.iter().any(|it| match *it {
        ClassItem::Range(lo, hi) => (lo..=hi).contains(&c),
        ClassItem::Digit(pos) => c.is_ascii_digit() == pos,
        ClassItem::Word(pos) => (c.is_alphanumeric() || c == '_') == pos,
        ClassItem::Space(pos) => c.is_whitespace() == pos,
    })
}

fn tick(budget: &Cell<u64>) -> bool {
    let b = budget.get();
    if b == 0 {
        return false;
    }
    budget.set(b.saturating_sub(1));
    true
}

/// Matches `nodes` at `pos`, then calls `k` with the end position.
fn seq(nodes: &[Node], cx: &Ctx<'_>, pos: usize, k: &mut dyn FnMut(usize) -> bool) -> bool {
    if !tick(&cx.budget) {
        return false;
    }
    let Some((first, rest)) = nodes.split_first() else { return k(pos) };
    one(first, cx, pos, &mut |p| seq(rest, cx, p, k))
}

fn one(node: &Node, cx: &Ctx<'_>, pos: usize, k: &mut dyn FnMut(usize) -> bool) -> bool {
    if !tick(&cx.budget) {
        return false;
    }
    let t = cx.t;
    let next = pos.saturating_add(1);
    match node {
        Node::Char(c) => t.get(pos) == Some(c) && k(next),
        Node::Any => t.get(pos).is_some_and(|&c| cx.dotall || c != '\n') && k(next),
        Node::Class(items, neg) => t.get(pos).is_some_and(|&c| class_hit(items, c) != *neg) && k(next),
        Node::Start => {
            let ok = pos == 0 || (cx.multiline && pos.checked_sub(1).and_then(|p| t.get(p)) == Some(&'\n'));
            ok && k(pos)
        }
        Node::End => {
            let ok = pos == t.len()
                || (pos.saturating_add(1) == t.len() && t.get(pos) == Some(&'\n'))
                || (cx.multiline && t.get(pos) == Some(&'\n'));
            ok && k(pos)
        }
        Node::Group(alts, None) => alts.iter().any(|alt| seq(alt, cx, pos, k)),
        Node::Group(alts, Some(n)) => {
            let Some(slot) = cx.caps.get(*n) else { return false };
            let saved = slot.get();
            let hit = alts.iter().any(|alt| {
                seq(alt, cx, pos, &mut |end| {
                    let before = slot.get();
                    slot.set(Some((pos, end)));
                    if k(end) {
                        return true;
                    }
                    slot.set(before);
                    false
                })
            });
            if !hit {
                slot.set(saved);
            }
            hit
        }
        Node::Backref(n) => {
            // Python: a reference to a group that did not participate fails to match.
            let Some((s, e)) = cx.caps.get(*n).and_then(Cell::get) else { return false };
            let Some(want) = t.get(s..e) else { return false };
            let len = want.len();
            t.get(pos..).is_some_and(|rest| rest.starts_with(want)) && k(pos.saturating_add(len))
        }
        Node::Repeat(inner, min, max) => repeat(inner, (*min, *max), 0, cx, pos, k),
    }
}

fn repeat(
    inner: &Node,
    bounds: (u32, Option<u32>),
    count: u32,
    cx: &Ctx<'_>,
    pos: usize,
    k: &mut dyn FnMut(usize) -> bool,
) -> bool {
    if cx.budget.get() == 0 {
        return false;
    }
    let (min, max) = bounds;
    let can_more = max.is_none_or(|m| count < m);
    if can_more {
        let more = one(inner, cx, pos, &mut |p| {
            // An empty iteration cannot make progress: stop repeating.
            p != pos && repeat(inner, bounds, count.saturating_add(1), cx, p, k)
        });
        if more {
            return true;
        }
    }
    count >= min && k(pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(p: &str, t: &str) -> bool {
        Regex::new(p).unwrap().search(t).unwrap()
    }

    #[test]
    fn basics() {
        assert!(s("abc", "xxabcxx"));
        assert!(!s("^abc", "xabc"));
        assert!(s("^a.c$", "abc\n"));
        assert!(!s("^a.c$", "abc\n\n"));
        assert!(s(r"\d+\.\d+", "v 2.31"));
        assert!(s("a(b|c)*d", "abcbcd"));
        assert!(s("(?:ab){2}", "abab"));
        assert!(!s("^(?:ab){3}$", "abab"));
        assert!(s("[^a-c]", "abcd"));
        assert!(!s("^[a-c]+$", "abcd"));
        assert!(s(r"[\d.]+x", "1.2x"));
        assert!(s("colou?r", "color"));
        assert!(s(r"^\[WARNING\] git 2\.(19|2[0-9]|30)\.[0-9]+ is older than 2\.31\.0\n$", "[WARNING] git 2.30.1 is older than 2.31.0\n"));
        assert!(s("a{2,}", "caaat"));
        assert!(s(".*?x", "abx"));
        assert!(s("(a*)*b", "aaab"));
        assert!(s("x{,", "x{,"));
    }

    #[test]
    fn backrefs_and_flags() {
        assert!(s(r"^saved in (\S+); apply \1\n$", "saved in /p/x; apply /p/x\n"));
        assert!(!s(r"^saved in (\S+); apply \1\n$", "saved in /p/x; apply /p/y\n"));
        assert!(s(r"(a)|b\1", "a"));
        assert!(!s(r"^(?:(a)|b)\1$", "b"));
        assert!(s("(?m)^[0-9a-f]{4}$", "head\nab12\ntail\n"));
        assert!(!s("^[0-9a-f]{4}$", "head\nab12\ntail\n"));
        assert!(s("(?s)a.b", "a\nb"));
        assert!(!s("a.b", "a\nb"));
    }

    #[test]
    fn errors_and_budget() {
        for bad in ["(", "a)", "[a", "*a", "a{3,1}", "(?P<n>x)", "\\", "(a)\\2", "(?i)x"] {
            assert!(Regex::new(bad).is_err(), "{bad}");
        }
        let r = Regex::new("(a|a)*(a|a)*(a|a)*(a|a)*c").unwrap();
        let t = "a".repeat(40);
        assert!(r.search(&t).is_err());
    }
}
