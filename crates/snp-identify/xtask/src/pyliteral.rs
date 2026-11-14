//! A deliberately tiny evaluator for the subset of Python used by identify's
//! table modules (`extensions.py`, `interpreters.py`).
//!
//! The upstream files are treated purely as *data*: nothing is imported or
//! executed. Supported constructs:
//!
//! * `from … import …` / `import …` statements (skipped),
//! * `NAME = <expr>` assignments,
//! * expressions: string literals (with implicit concatenation), set
//!   literals, dict literals with string keys, `NAME`, `NAME['key']`
//!   subscripts of previously assigned dicts, `a | b` set unions and
//!   parentheses.
//!
//! Anything else is rejected with an error that names the offending line, so
//! an upstream change that introduces new syntax makes the generator fail
//! loudly instead of silently producing wrong tables.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// A Python value produced by the evaluator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// `'text'`
    Str(String),
    /// `{'a', 'b'}`
    Set(BTreeSet<String>),
    /// `{'k': v, …}` in source order (Python dicts preserve insertion order).
    Dict(Vec<(String, Value)>),
}

impl Value {
    fn type_name(&self) -> &'static str {
        match self {
            Self::Str(_) => "str",
            Self::Set(_) => "set",
            Self::Dict(_) => "dict",
        }
    }
}

/// An evaluation error with the 1-based source line it refers to.
#[derive(Debug)]
pub struct Error {
    line: usize,
    message: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for Error {}

fn err<T>(line: usize, message: impl Into<String>) -> Result<T, Error> {
    Err(Error {
        line,
        message: message.into(),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Name(String),
    Str(String),
    Punct(char),
    Newline,
}

#[derive(Clone, Debug)]
struct Token {
    tok: Tok,
    line: usize,
}

fn tokenize(src: &str) -> Result<Vec<Token>, Error> {
    let mut out = Vec::new();
    let mut chars = src.chars().peekable();
    let mut line = 1usize;
    let mut depth = 0usize;
    while let Some(c) = chars.next() {
        match c {
            '\n' => {
                if depth == 0 && out.last().is_some_and(|t: &Token| t.tok != Tok::Newline) {
                    out.push(Token {
                        tok: Tok::Newline,
                        line,
                    });
                }
                line += 1;
            }
            ' ' | '\t' | '\r' | '\x0c' => {}
            '#' => {
                while chars.peek().is_some_and(|&c| c != '\n') {
                    chars.next();
                }
            }
            '\\' => return err(line, "explicit line continuations are not supported"),
            '{' | '[' | '(' => {
                depth += 1;
                out.push(Token {
                    tok: Tok::Punct(c),
                    line,
                });
            }
            '}' | ']' | ')' => {
                depth = match depth.checked_sub(1) {
                    Some(d) => d,
                    None => return err(line, format!("unbalanced {c:?}")),
                };
                out.push(Token {
                    tok: Tok::Punct(c),
                    line,
                });
            }
            ':' | ',' | '|' | '=' | '.' | '*' => out.push(Token {
                tok: Tok::Punct(c),
                line,
            }),
            '\'' | '"' => {
                let s = lex_string(&mut chars, c, false, &mut line)?;
                out.push(Token {
                    tok: Tok::Str(s),
                    line,
                });
            }
            c if c.is_alphabetic() || c == '_' => {
                let mut name = String::from(c);
                while let Some(&n) = chars.peek() {
                    if n.is_alphanumeric() || n == '_' {
                        name.push(n);
                        chars.next();
                    } else {
                        break;
                    }
                }
                // String prefixes: only `r` / `u` (any case) are plain text.
                let quote = chars.peek().copied().filter(|&q| {
                    (q == '\'' || q == '"')
                        && name.len() <= 2
                        && name
                            .chars()
                            .all(|p| matches!(p, 'r' | 'R' | 'u' | 'U' | 'b' | 'B' | 'f' | 'F'))
                });
                if let Some(q) = quote {
                    if name.chars().any(|p| matches!(p, 'b' | 'B' | 'f' | 'F')) {
                        return err(line, format!("unsupported string prefix {name:?}"));
                    }
                    chars.next();
                    let raw = name.chars().any(|p| matches!(p, 'r' | 'R'));
                    let s = lex_string(&mut chars, q, raw, &mut line)?;
                    out.push(Token {
                        tok: Tok::Str(s),
                        line,
                    });
                } else {
                    out.push(Token {
                        tok: Tok::Name(name),
                        line,
                    });
                }
            }
            other => return err(line, format!("unexpected character {other:?}")),
        }
    }
    if depth != 0 {
        return err(line, "unbalanced brackets at end of file");
    }
    out.push(Token {
        tok: Tok::Newline,
        line,
    });
    Ok(out)
}

#[allow(clippy::many_single_char_names)]
fn lex_string(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    quote: char,
    raw: bool,
    line: &mut usize,
) -> Result<String, Error> {
    let mut s = String::new();
    if chars.peek() == Some(&quote) {
        chars.next();
        if chars.peek() == Some(&quote) {
            return err(*line, "triple-quoted strings are not supported");
        }
        return Ok(s);
    }
    loop {
        let Some(c) = chars.next() else {
            return err(*line, "unterminated string literal");
        };
        match c {
            c if c == quote => return Ok(s),
            '\n' => return err(*line, "newline in string literal"),
            '\\' if raw => {
                // Raw strings keep the backslash; it still escapes the quote.
                s.push('\\');
                if let Some(n) = chars.next() {
                    if n == '\n' {
                        return err(*line, "line continuation in string literal");
                    }
                    s.push(n);
                }
            }
            '\\' => {
                let Some(e) = chars.next() else {
                    return err(*line, "unterminated escape");
                };
                match e {
                    '\\' => s.push('\\'),
                    '\'' => s.push('\''),
                    '"' => s.push('"'),
                    'n' => s.push('\n'),
                    't' => s.push('\t'),
                    'r' => s.push('\r'),
                    'a' => s.push('\x07'),
                    'b' => s.push('\x08'),
                    'f' => s.push('\x0c'),
                    'v' => s.push('\x0b'),
                    '0'..='7' => {
                        let mut v = e.to_digit(8).unwrap_or(0);
                        for _ in 0..2 {
                            match chars.peek().and_then(|d| d.to_digit(8)) {
                                Some(d) => {
                                    v = v * 8 + d;
                                    chars.next();
                                }
                                None => break,
                            }
                        }
                        match char::from_u32(v) {
                            Some(ch) => s.push(ch),
                            None => return err(*line, "invalid octal escape"),
                        }
                    }
                    'x' | 'u' | 'U' => {
                        let n = match e {
                            'x' => 2,
                            'u' => 4,
                            _ => 8,
                        };
                        let mut v = 0u32;
                        for _ in 0..n {
                            match chars.next().and_then(|d| d.to_digit(16)) {
                                Some(d) => v = v * 16 + d,
                                None => return err(*line, format!("truncated \\{e} escape")),
                            }
                        }
                        match char::from_u32(v) {
                            Some(ch) => s.push(ch),
                            None => return err(*line, format!("invalid \\{e} escape")),
                        }
                    }
                    '\n' => return err(*line, "line continuation in string literal"),
                    'N' => return err(*line, "\\N{…} escapes are not supported"),
                    other => {
                        // Python keeps unknown escapes verbatim.
                        s.push('\\');
                        s.push(other);
                    }
                }
            }
            c => s.push(c),
        }
    }
}

struct Parser<'a> {
    toks: &'a [Token],
    pos: usize,
    env: &'a BTreeMap<String, Value>,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|t| &t.tok)
    }

    fn line(&self) -> usize {
        self.toks
            .get(self.pos)
            .or_else(|| self.toks.last())
            .map_or(0, |t| t.line)
    }

    fn bump(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).map(|t| t.tok.clone());
        self.pos += 1;
        t
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(&Tok::Punct(c)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, c: char) -> Result<(), Error> {
        if self.eat(c) {
            Ok(())
        } else {
            err(
                self.line(),
                format!("expected {c:?}, found {:?}", self.peek()),
            )
        }
    }

    fn expr(&mut self) -> Result<Value, Error> {
        let mut left = self.atom()?;
        while self.eat('|') {
            let line = self.line();
            let right = self.atom()?;
            left = match (left, right) {
                (Value::Set(mut a), Value::Set(b)) => {
                    a.extend(b);
                    Value::Set(a)
                }
                (a, b) => {
                    return err(
                        line,
                        format!(
                            "unsupported operands for |: {} and {}",
                            a.type_name(),
                            b.type_name()
                        ),
                    );
                }
            };
        }
        Ok(left)
    }

    fn atom(&mut self) -> Result<Value, Error> {
        let line = self.line();
        match self.bump() {
            Some(Tok::Str(s)) => {
                let mut s = s;
                while let Some(Tok::Str(more)) = self.peek() {
                    s.push_str(more);
                    self.pos += 1;
                }
                Ok(Value::Str(s))
            }
            Some(Tok::Punct('(')) => {
                let v = self.expr()?;
                self.expect(')')?;
                Ok(v)
            }
            Some(Tok::Punct('{')) => self.brace(line),
            Some(Tok::Name(name)) => {
                let Some(base) = self.env.get(&name) else {
                    return err(line, format!("name {name:?} is not defined"));
                };
                if self.eat('[') {
                    let key = match self.expr()? {
                        Value::Str(k) => k,
                        other => {
                            return err(
                                line,
                                format!("subscript must be a str, got {}", other.type_name()),
                            );
                        }
                    };
                    self.expect(']')?;
                    let Value::Dict(items) = base else {
                        return err(line, format!("{name} is not a dict"));
                    };
                    match items.iter().find(|(k, _)| *k == key) {
                        Some((_, v)) => Ok(v.clone()),
                        None => err(line, format!("KeyError: {name}[{key:?}]")),
                    }
                } else if self.peek() == Some(&Tok::Punct('('))
                    || self.peek() == Some(&Tok::Punct('.'))
                {
                    err(
                        line,
                        format!("calls and attribute access are not supported ({name})"),
                    )
                } else {
                    Ok(base.clone())
                }
            }
            other => err(line, format!("unexpected token {other:?}")),
        }
    }

    fn brace(&mut self, line: usize) -> Result<Value, Error> {
        if self.eat('}') {
            return Ok(Value::Dict(Vec::new()));
        }
        if self.peek() == Some(&Tok::Punct('*')) {
            return err(line, "unpacking in literals is not supported");
        }
        let first = self.expr()?;
        if self.eat(':') {
            let mut items: Vec<(String, Value)> = Vec::new();
            let mut key = first;
            loop {
                let kline = self.line();
                let Value::Str(k) = key else {
                    return err(
                        kline,
                        format!("dict keys must be str, got {}", key.type_name()),
                    );
                };
                let v = self.expr()?;
                if let Some(slot) = items.iter_mut().find(|(ek, _)| *ek == k) {
                    eprintln!(
                        "warning: line {kline}: duplicate dict key {k:?}; last value wins (Python semantics)"
                    );
                    slot.1 = v;
                } else {
                    items.push((k, v));
                }
                if !self.eat(',') {
                    self.expect('}')?;
                    break;
                }
                if self.eat('}') {
                    break;
                }
                key = self.expr()?;
                self.expect(':')?;
            }
            Ok(Value::Dict(items))
        } else {
            let mut set = BTreeSet::new();
            let mut elem = first;
            loop {
                let eline = self.line();
                match elem {
                    Value::Str(s) => {
                        set.insert(s);
                    }
                    other => {
                        return err(
                            eline,
                            format!("set elements must be str, got {}", other.type_name()),
                        );
                    }
                }
                if !self.eat(',') {
                    self.expect('}')?;
                    break;
                }
                if self.eat('}') {
                    break;
                }
                elem = self.expr()?;
            }
            Ok(Value::Set(set))
        }
    }
}

/// Evaluate a module made only of imports and literal assignments.
///
/// Returns every assigned name with its value.
pub fn eval_module(src: &str) -> Result<BTreeMap<String, Value>, Error> {
    let toks = tokenize(src)?;
    let mut env: BTreeMap<String, Value> = BTreeMap::new();
    let mut start = 0usize;
    while start < toks.len() {
        let end = toks
            .iter()
            .skip(start)
            .position(|t| t.tok == Tok::Newline)
            .map_or(toks.len(), |p| start + p);
        let stmt = toks.get(start..end).unwrap_or_default();
        start = end + 1;
        let Some(first) = stmt.first() else { continue };
        match (&first.tok, stmt.get(1).map(|t| &t.tok)) {
            (Tok::Name(kw), _) if kw == "from" || kw == "import" => {}
            (Tok::Name(name), Some(Tok::Punct('='))) => {
                let body = stmt.get(2..).unwrap_or_default();
                let mut p = Parser {
                    toks: body,
                    pos: 0,
                    env: &env,
                };
                let value = p.expr()?;
                if p.pos != body.len() {
                    return err(p.line(), "trailing tokens after expression");
                }
                env.insert(name.clone(), value);
            }
            _ => {
                return err(
                    first.line,
                    "only imports and NAME = <literal> assignments are supported",
                );
            }
        }
    }
    Ok(env)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> Value {
        Value::Set(items.iter().map(|s| (*s).to_owned()).collect())
    }

    #[test]
    fn evaluates_identify_like_module() {
        let src = "from __future__ import annotations\n\
                   # comment\n\
                   A = {\n    'x': {'text', 'x'},  # trailing\n    \"y\": {'binary'},\n}\n\
                   B = {\n    'z': A['x'] | {'z'},\n    'w': (A['y']),\n}\n";
        let env = eval_module(src).expect("valid module");
        assert_eq!(
            env.get("B"),
            Some(&Value::Dict(vec![
                ("z".to_owned(), set(&["text", "x", "z"])),
                ("w".to_owned(), set(&["binary"])),
            ]))
        );
    }

    #[test]
    fn duplicate_keys_last_wins_first_position() {
        let env = eval_module("A = {'a': {'1'}, 'b': {'2'}, 'a': {'3'}}\n").expect("valid");
        assert_eq!(
            env.get("A"),
            Some(&Value::Dict(vec![
                ("a".to_owned(), set(&["3"])),
                ("b".to_owned(), set(&["2"]))
            ]))
        );
    }

    #[test]
    fn string_escapes_and_concatenation() {
        let env = eval_module("A = {'a\\'b' 'c', \"\\x41\\u00e9\", r'\\d'}\n").expect("valid");
        assert_eq!(env.get("A"), Some(&set(&["a'bc", "A\u{e9}", "\\d"])));
    }

    #[test]
    fn rejects_code() {
        for src in [
            "A = frozenset({'a'})\n",
            "A = {'a'}\nprint(A)\n",
            "A = {'a': 1}\n",
            "A = {'a': B['x']}\n",
            "A = {'a': {'b'}}\nC = A['missing']\n",
            "A = b'x'\n",
            "A = '''x'''\n",
            "def f(): pass\n",
            "A = {**B}\n",
        ] {
            assert!(eval_module(src).is_err(), "{src:?} should be rejected");
        }
    }
}
